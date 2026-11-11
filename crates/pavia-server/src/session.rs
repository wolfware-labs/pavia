//! The task that drives one [`pavia_proto::session::Session`] over a WebSocket [`Transport`].
//!
//! [`run`] feeds every received message, transport event and [`Command`] sent through the
//! session's [`Handle`] into the session and carries out its outputs in order: a CLOSE frame is
//! flushed before the transport closes (§7.7). It returns the session's [`Ending`], so each
//! session reports its end exactly once (§13.4), and leaves the connection to
//! [`Transport::linger`] on a task of its own, so the peer reads the close before the connection
//! drops.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::pin::Pin;

use bytes::BytesMut;
use futures_util::FutureExt;
use pavia_proto::data::{self, Codec, DecodeLimits, Value};
use pavia_proto::frame::{Auth, Call};
use pavia_proto::registry::{CloseCode, StatusCode};
use pavia_proto::session::{Command, Ending, Input, Output, Session};
use pavia_proto::session_id::SessionId;
use tokio::sync::mpsc;
use tokio::task::{self, JoinError, JoinSet};
use tokio::time::{Instant, sleep_until};

use crate::auth::{AuthFailure, Authenticator, Principal};
use crate::methods::{Handler, Kind, MethodRegistry};
use crate::session_id;
use crate::sessions::{Handle, Registration, Registry};
use crate::websocket::{Received, Transport, TransportError};

const COMMAND_QUEUE: usize = 32;

type Authenticating<'a> = Pin<Box<dyn Future<Output = Result<Principal, AuthFailure>> + Send + 'a>>;

/// Runs a server session on `transport` until it ends.
///
/// The session joins `registry` once its WELCOME is written, and leaves it when this returns,
/// however it ended (§13.4). The application reaches it through [`Registry::get`] in between;
/// before WELCOME nothing can reach it (§14), and a session whose WELCOME cannot be written never
/// joins.
///
/// Each CALL from the client runs the server method of that name from `methods` on a task of its
/// own, so a slow handler holds up no other call, and its value goes back as RESULT (§8.2, §8.4).
/// A call this task can refuse without running anything is answered ERROR with `nx`: an unknown
/// method (UNIMPLEMENTED), a NOTIFY method, arguments that are not an array in the session codec
/// or a wrong argument count (INVALID_ARGUMENT). A handler that panics is answered INTERNAL.
/// Handlers still running when the session ends are dropped with it.
///
/// `authenticator` runs once, after HELLO (§7.5). The transport keeps being read while it runs,
/// so a CLOSE, a transport loss or the handshake deadline still ends the session. When it
/// accepts, the session gets a new ID and sends WELCOME; if the operating system cannot supply
/// the ID's randomness, the session closes with INTERNAL instead.
///
/// When a deadline has passed and a whole message is already readable, the message goes in
/// first: a late poll of this task is not the peer's silence. Past the idle deadline, a waiting
/// message moves it instead of ending the session with IDLE_TIMEOUT, and a waiting CLOSE is
/// mirrored. Past the handshake deadline the session still closes with HANDSHAKE_TIMEOUT
/// (§7.1). Time this task spends blocked on a write still counts as the peer's silence, and
/// nothing bounds a write to a peer that stops reading.
///
/// The session is built from the configuration of the transport's endpoint
/// ([`Config::session`](crate::websocket::Config::session)), so its limits and the transport's
/// message cap come from one value. The handshake timer starts at [`Transport::accepted`], before
/// the upgrade. The clock is tokio's, so a test that pauses time drives the session's timers too.
pub async fn run<A: Authenticator>(
  mut transport: Transport,
  registry: &Registry,
  methods: &MethodRegistry<Handler>,
  authenticator: &A,
) -> Ending {
  let (sender, mut commands) = mpsc::channel(COMMAND_QUEUE);
  let mut membership = Membership {
    registry,
    handle: Handle::new(sender),
    welcoming: None,
    registration: None,
  };
  let mut calls = Calls {
    methods,
    running: JoinSet::new(),
    streams: HashMap::new(),
  };
  let mut session = Session::server(transport.session(), transport.accepted().into_std());
  let context = transport.context().clone();
  let mut authenticating: Option<Authenticating<'_>> = None;
  loop {
    let mut answered = false;
    let deadline = session.poll_timeout().map(Instant::from_std);
    let input = tokio::select! {
      () = sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => {
        transport.recv().now_or_never().map_or(Input::Timeout, received_input)
      }
      answer = async { authenticating.as_mut().expect("guarded by the branch condition").await },
        if authenticating.is_some() => {
        answered = true;
        match answer {
          Ok(principal) => authenticated(&principal, session_id::generate()),
          Err(failure) => Input::AuthFailed(failure),
        }
      },
      received = transport.recv() => received_input(received),
      Some(command) = commands.recv() => Input::Command(command),
      Some(joined) = calls.running.join_next_with_id(), if !calls.running.is_empty() => {
        calls.answered(joined, codec(&session))
      }
    };
    if answered {
      authenticating = None;
    }
    session.handle_input(input, Instant::now().into_std());
    match carry_out(&mut session, &mut transport, &mut membership, &mut calls).await {
      Step::Continue => {}
      Step::Authenticate(auth) => {
        let context = &context;
        authenticating = Some(Box::pin(
          async move { authenticator.authenticate(auth.as_ref(), context).await },
        ));
      }
      Step::Ended(ending) => {
        drop(membership);
        tokio::spawn(transport.linger());
        return ending;
      }
    }
  }
}

fn received_input(received: Option<Result<Received, TransportError>>) -> Input {
  match received {
    Some(Ok(Received::Binary(message))) => Input::Message(message),
    Some(Ok(Received::Text)) => Input::TextMessage,
    Some(Ok(Received::TooLarge)) => Input::MessageTooLarge,
    Some(Err(_)) | None => Input::TransportLost,
  }
}

fn authenticated(principal: &Principal, id: io::Result<SessionId>) -> Input {
  match id {
    Ok(session) => Input::Authenticated {
      session,
      user: principal.user().map(str::to_owned),
    },
    Err(_) => Input::Command(Command::Close {
      code: CloseCode::Internal,
      msg: None,
    }),
  }
}

struct Membership<'a> {
  registry: &'a Registry,
  handle: Handle,
  welcoming: Option<SessionId>,
  registration: Option<Registration>,
}

impl Membership<'_> {
  fn join(&mut self, id: SessionId) {
    self.registration = Some(self.registry.register(id, self.handle.clone()));
  }
}

struct Calls<'a> {
  methods: &'a MethodRegistry<Handler>,
  running: JoinSet<Value>,
  streams: HashMap<task::Id, u64>,
}

impl Calls<'_> {
  fn dispatch(&mut self, stream: u64, call: &Call, codec: Codec) -> Option<Command> {
    let refuse = |code, msg: &str| {
      Some(Command::Error {
        stream,
        code,
        msg: msg.to_owned(),
        not_executed: true,
      })
    };
    let Some(method) = self.methods.server_method(&call.method) else {
      return refuse(StatusCode::Unimplemented, "unknown method");
    };
    if !matches!(method.kind, Kind::Call { .. }) {
      return refuse(StatusCode::InvalidArgument, "method takes NOTIFY, not CALL");
    }
    let arguments = match data::decode(&call.data, codec, &DecodeLimits::default()) {
      Ok(Value::Array(arguments)) => arguments,
      Ok(_) => return refuse(StatusCode::InvalidArgument, "arguments are not an array"),
      Err(_) => return refuse(StatusCode::InvalidArgument, "arguments do not decode"),
    };
    if arguments.len() != method.handler.params() {
      let msg = format!("expected {} arguments, got {}", method.handler.params(), arguments.len());
      return refuse(StatusCode::InvalidArgument, &msg);
    }
    let task = self.running.spawn(method.handler.run(arguments));
    self.streams.insert(task.id(), stream);
    None
  }

  fn answered(&mut self, joined: Result<(task::Id, Value), JoinError>, codec: Codec) -> Input {
    let (id, value) = match joined {
      Ok((id, value)) => (id, Ok(value)),
      Err(error) => (error.id(), Err(error)),
    };
    let stream = self.streams.remove(&id).expect("every running call has a stream");
    let mut encoded = BytesMut::new();
    let command = match value.map(|value| data::encode(&value, codec, &mut encoded)) {
      Ok(Ok(())) => Command::Result {
        stream,
        data: encoded.freeze(),
      },
      Ok(Err(_)) => internal(stream, "result does not encode"),
      Err(_) => internal(stream, "handler failed"),
    };
    Input::Command(command)
  }
}

fn internal(stream: u64, msg: &str) -> Command {
  Command::Error {
    stream,
    code: StatusCode::Internal,
    msg: msg.to_owned(),
    not_executed: false,
  }
}

fn codec(session: &Session) -> Codec {
  session.negotiated().map_or(Codec::Cbor, |negotiated| negotiated.codec)
}

enum Step {
  Continue,
  Authenticate(Option<Auth>),
  Ended(Ending),
}

async fn carry_out(
  session: &mut Session,
  transport: &mut Transport,
  membership: &mut Membership<'_>,
  calls: &mut Calls<'_>,
) -> Step {
  let mut step = Step::Continue;
  while let Some(output) = session.poll_output() {
    let welcoming = membership.welcoming.take();
    match output {
      Output::Transmit(message) => {
        if transport.send(message).await.is_ok() {
          if let Some(id) = welcoming {
            membership.join(id);
          }
        } else {
          session.handle_input(Input::TransportLost, Instant::now().into_std());
        }
      }
      Output::CloseTransport { code, reason } => {
        let _ = transport.close(code, &reason).await;
      }
      Output::Authenticate { auth } => step = Step::Authenticate(auth),
      Output::Established { session } => membership.welcoming = Some(session),
      Output::Call { stream, call } => {
        if let Some(refusal) = calls.dispatch(stream, &call, codec(session)) {
          session.handle_input(Input::Command(refusal), Instant::now().into_std());
        }
      }
      Output::Ended(ending) => return Step::Ended(ending),
      _ => {}
    }
  }
  step
}

#[cfg(test)]
mod tests;
