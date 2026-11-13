//! The task that drives one [`pavia_proto::session::Session`] over a WebSocket [`Transport`].
//!
//! [`run`] feeds every received message, transport event, [`Command`] sent through the session's
//! [`Handle`] and answer of a method handler into the session and carries out its outputs in
//! order: a CLOSE frame is flushed before the transport closes (§7.7). It returns the session's
//! [`Ending`], so each session reports its end exactly once (§13.4), and leaves the connection
//! to [`Transport::linger`] on a task of its own, so the peer reads the close before the
//! connection drops.

use std::collections::HashMap;
use std::error::Error;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use futures_util::FutureExt;
use pavia_proto::caps::Capability;
use pavia_proto::data::{self, Codec, DecodeLimits, Value};
use pavia_proto::frame::{Auth, Call, MetaValue};
use pavia_proto::registry::{CloseCode, StatusCode};
use pavia_proto::session::{Command, Config as SessionConfig, Ending, Input, Output, Session};
use pavia_proto::session_id::SessionId;
use pavia_proto::stream::Shape;
use tokio::sync::mpsc;
use tokio::task::{self, AbortHandle, JoinError, JoinSet};
use tokio::time::{Instant, sleep_until};

use crate::auth::{AuthFailure, Authenticator, Authorizer, Principal};
use crate::methods::{Handler, HandlerError, Kind, MethodName, MethodRegistry};
use crate::session_id;
use crate::sessions::{Handle, Registration, Registry};
use crate::websocket::{Received, Transport, TransportError};
use lane::Lane;

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
/// Before the handler runs, the callee checks of §8.2 run in order and the first that fails
/// answers ERROR with `nx`: more than `max_calls` calls open (RESOURCE_EXHAUSTED, checked by the
/// session), an unknown method (UNIMPLEMENTED), a NOTIFY method or a shape other than the method's
/// (INVALID_ARGUMENT), arguments that are not an array in the session codec, a wrong argument
/// count or, with capability `idempotency`, a `pavia-idempotency-key` above 128 bytes
/// (INVALID_ARGUMENT), and last `authorizer`, on the call's task (UNAUTHENTICATED for an anonymous
/// session, PERMISSION_DENIED otherwise). A streaming method is answered UNIMPLEMENTED until
/// streams arrive in M5. A handler's [`StatusError`](crate::methods::StatusError) is answered as
/// given, with its detail and `meta`. Any other failure (an unexpected handler error, a panic, a
/// value the codec cannot encode) is answered INTERNAL with the configured generic message, or its
/// own text cut to `MAX_INTERNAL_ERROR_MESSAGE` bytes in development mode, and logged in full with
/// `tracing` either way (§8.4, §14).
/// Handlers still running when the session ends are dropped with it. A CANCEL from the client on a
/// call not answered yet drops that call's handler, and once it is dropped the call is answered
/// ERROR CANCELLED in place of whatever the handler returned; a failure it had already hit is
/// still logged (§8.5).
/// A NOTIFY on the control stream runs its `notify` method and is never answered: an unknown
/// method, a `call` method, arguments that do not fit, a refusal by the authorizer or a failing
/// handler is logged with the session ID and the method and dropped. Notification handlers run one
/// at a time, in delivery order, each after the previous one has completed (§9.1, §9.2).
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
pub async fn run<A: Authenticator, Z: Authorizer + 'static>(
  mut transport: Transport,
  registry: &Registry,
  methods: &MethodRegistry<Handler>,
  authenticator: &A,
  authorizer: &Arc<Z>,
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
    authorizer: Arc::clone(authorizer),
    principal: Arc::new(Principal::anonymous()),
    disclosure: Disclosure {
      generic: transport.session().internal_error_message().to_owned(),
      disclose: transport.session().disclose_internal_errors(),
    },
    running: JoinSet::new(),
    streams: HashMap::new(),
    tasks: HashMap::new(),
    lane: Lane::default(),
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
          Ok(principal) => {
            let input = authenticated(&principal, session_id::generate());
            calls.principal = Arc::new(principal);
            input
          }
          Err(failure) => Input::AuthFailed(failure),
        }
      },
      received = transport.recv() => received_input(received),
      Some(command) = commands.recv() => Input::Command(command),
      Some(joined) = calls.running.join_next_with_id(), if !calls.running.is_empty() => {
        calls.answered(joined, codec(&session))
      }
      Some(joined) = calls.lane.join(), if calls.lane.is_running() => {
        calls.lane.finished(joined);
        continue;
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

struct Calls<'a, Z> {
  methods: &'a MethodRegistry<Handler>,
  authorizer: Arc<Z>,
  principal: Arc<Principal>,
  disclosure: Disclosure,
  running: JoinSet<Result<Value, Failed>>,
  streams: HashMap<task::Id, RunningCall>,
  tasks: HashMap<u64, AbortHandle>,
  lane: Lane,
}

struct RunningCall {
  stream: u64,
  method: MethodName,
  cancelled: bool,
}

enum Failed {
  Refused(Refusal),
  Handler(HandlerError),
}

struct Disclosure {
  generic: String,
  disclose: bool,
}

impl Disclosure {
  fn internal(&self, stream: u64, method: &MethodName, error: &(dyn Error + 'static)) -> Command {
    tracing::error!(stream, method = method.as_str(), error, "call failed unexpectedly");
    let msg = if self.disclose {
      let mut text = error.to_string();
      text.truncate(text.floor_char_boundary(SessionConfig::MAX_INTERNAL_ERROR_MESSAGE));
      text
    } else {
      self.generic.clone()
    };
    Command::Error {
      stream,
      code: StatusCode::Internal,
      msg,
      nx: false,
      meta: None,
      detail: Bytes::new(),
    }
  }
}

#[derive(Debug, PartialEq, Eq)]
struct Refusal {
  code: StatusCode,
  msg: String,
}

impl Refusal {
  fn new(code: StatusCode, msg: impl Into<String>) -> Self {
    Self { code, msg: msg.into() }
  }

  fn denied(principal: &Principal) -> Self {
    let code = if principal.is_anonymous() {
      StatusCode::Unauthenticated
    } else {
      StatusCode::PermissionDenied
    };
    Self::new(code, "not allowed to call this method")
  }

  fn answering(self, stream: u64) -> Command {
    Command::Error {
      stream,
      code: self.code,
      msg: self.msg,
      nx: true,
      meta: None,
      detail: Bytes::new(),
    }
  }
}

const IDEMPOTENCY_KEY: &str = "pavia-idempotency-key";
const MAX_IDEMPOTENCY_KEY: usize = 128;

fn validate_call<'m>(
  call: &Call,
  methods: &'m MethodRegistry<Handler>,
  codec: Codec,
  idempotency: bool,
) -> Result<(&'m MethodName, &'m Handler, Vec<Value>), Refusal> {
  let Some((name, method)) = methods.server_entry(&call.method) else {
    return Err(Refusal::new(StatusCode::Unimplemented, "unknown method"));
  };
  let Kind::Call { shape, .. } = method.kind else {
    return Err(Refusal::new(StatusCode::InvalidArgument, "method takes NOTIFY, not CALL"));
  };
  if Shape::try_from(call.shape) != Ok(shape) {
    return Err(Refusal::new(StatusCode::InvalidArgument, "shape does not match the method"));
  }
  if shape != Shape::Unary {
    return Err(Refusal::new(StatusCode::Unimplemented, "streaming calls are not served yet"));
  }
  let arguments = arguments(&call.data, codec, &method.handler)?;
  if idempotency && idempotency_key_length(call).is_some_and(|length| length > MAX_IDEMPOTENCY_KEY) {
    return Err(Refusal::new(StatusCode::InvalidArgument, "idempotency key above 128 bytes"));
  }
  Ok((name, &method.handler, arguments))
}

fn arguments(data: &Bytes, codec: Codec, handler: &Handler) -> Result<Vec<Value>, Refusal> {
  let arguments = match data::decode(data, codec, &DecodeLimits::default()) {
    Ok(Value::Array(arguments)) => arguments,
    Ok(_) => return Err(Refusal::new(StatusCode::InvalidArgument, "arguments are not an array")),
    Err(_) => return Err(Refusal::new(StatusCode::InvalidArgument, "arguments do not decode")),
  };
  if arguments.len() != handler.params() {
    let msg = format!("expected {} arguments, got {}", handler.params(), arguments.len());
    return Err(Refusal::new(StatusCode::InvalidArgument, msg));
  }
  Ok(arguments)
}

fn idempotency_key_length(call: &Call) -> Option<usize> {
  let (_, value) = call
    .meta
    .as_ref()?
    .iter()
    .find(|(name, _)| name.as_str() == IDEMPOTENCY_KEY)?;
  Some(match value {
    MetaValue::Text(text) => text.len(),
    MetaValue::Bytes(bytes) => bytes.len(),
  })
}

impl<Z: Authorizer + 'static> Calls<'_, Z> {
  fn dispatch(&mut self, stream: u64, call: &Call, session: &Session) -> Option<Command> {
    let idempotency = session
      .negotiated()
      .is_some_and(|negotiated| negotiated.caps.contains(Capability::Idempotency));
    let (name, handler, arguments) = match validate_call(call, self.methods, codec(session), idempotency) {
      Ok(approved) => approved,
      Err(refusal) => return Some(refusal.answering(stream)),
    };
    let (method, handler) = (name.clone(), handler.clone());
    let (authorizer, principal) = (Arc::clone(&self.authorizer), Arc::clone(&self.principal));
    let task = self.running.spawn(async move {
      if authorizer.authorize(&principal, &method).await {
        handler.run(arguments).await.map_err(Failed::Handler)
      } else {
        Err(Failed::Refused(Refusal::denied(&principal)))
      }
    });
    let running = RunningCall {
      stream,
      method: name.clone(),
      cancelled: false,
    };
    self.streams.insert(task.id(), running);
    self.tasks.insert(stream, task);
    None
  }

  fn cancel(&mut self, stream: u64) {
    if let Some(task) = self.tasks.get(&stream) {
      task.abort();
      self
        .streams
        .get_mut(&task.id())
        .expect("every running call has a stream")
        .cancelled = true;
    }
  }

  fn answered(&mut self, joined: Result<(task::Id, Result<Value, Failed>), JoinError>, codec: Codec) -> Input {
    let (id, answer) = match joined {
      Ok((id, answer)) => (id, Ok(answer)),
      Err(error) => (error.id(), Err(error)),
    };
    let RunningCall {
      stream,
      method,
      cancelled,
    } = self.streams.remove(&id).expect("every running call has a stream");
    self.tasks.remove(&stream);
    let command = match answer {
      Err(error) if error.is_cancelled() => cancelled_answer(stream),
      Ok(Ok(value)) => match encode(&value, codec) {
        Ok(data) => Command::Result { stream, data },
        Err(error) => self.disclosure.internal(stream, &method, &error),
      },
      Ok(Err(Failed::Refused(refusal))) => refusal.answering(stream),
      Ok(Err(Failed::Handler(HandlerError::Status(status)))) => {
        let (code, msg, detail, meta) = status.into_parts();
        match detail.map_or(Ok(Bytes::new()), |detail| encode(&detail, codec)) {
          Ok(detail) => Command::Error {
            stream,
            code: code.into(),
            msg,
            nx: false,
            meta,
            detail,
          },
          Err(error) => self.disclosure.internal(stream, &method, &error),
        }
      }
      Ok(Err(Failed::Handler(HandlerError::Unexpected(error)))) => self.disclosure.internal(stream, &method, &*error),
      Err(error) => self.disclosure.internal(stream, &method, &error),
    };
    Input::Command(if cancelled { cancelled_answer(stream) } else { command })
  }
}

fn cancelled_answer(stream: u64) -> Command {
  Command::Error {
    stream,
    code: StatusCode::Cancelled,
    msg: "cancelled".to_owned(),
    nx: false,
    meta: None,
    detail: Bytes::new(),
  }
}

fn encode(value: &Value, codec: Codec) -> Result<Bytes, data::EncodeError> {
  let mut encoded = BytesMut::new();
  data::encode(value, codec, &mut encoded)?;
  Ok(encoded.freeze())
}

fn codec(session: &Session) -> Codec {
  session.negotiated().map_or(Codec::Cbor, |negotiated| negotiated.codec)
}

enum Step {
  Continue,
  Authenticate(Option<Auth>),
  Ended(Ending),
}

async fn carry_out<Z: Authorizer + 'static>(
  session: &mut Session,
  transport: &mut Transport,
  membership: &mut Membership<'_>,
  calls: &mut Calls<'_, Z>,
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
      Output::Established { session } => {
        calls.lane.established(session.clone());
        membership.welcoming = Some(session);
      }
      Output::Notify { notify } => {
        let codec = codec(session);
        calls
          .lane
          .receive(&notify, calls.methods, codec, &calls.authorizer, &calls.principal);
      }
      Output::Call { stream, call } => {
        if let Some(refusal) = calls.dispatch(stream, &call, session) {
          session.handle_input(Input::Command(refusal), Instant::now().into_std());
        }
      }
      Output::Cancelled { stream } => calls.cancel(stream),
      Output::Ended(ending) => return Step::Ended(ending),
      _ => {}
    }
  }
  step
}

mod lane;

#[cfg(test)]
mod tests;
