//! The task that drives one [`pavia_proto::session::Session`] over a WebSocket [`Transport`].
//!
//! [`run`] feeds every received message, transport event and [`Command`] sent through the
//! session's [`Handle`] into the session and carries out its outputs in order: a CLOSE frame is
//! flushed before the transport closes (§7.7). It returns the session's [`Ending`], so each
//! session reports its end exactly once (§13.4), and leaves the connection to
//! [`Transport::linger`] on a task of its own, so the peer reads the close before the connection
//! drops.

use std::future::Future;
use std::io;
use std::pin::Pin;

use futures_util::FutureExt;
use pavia_proto::frame::Auth;
use pavia_proto::registry::CloseCode;
use pavia_proto::session::{Command, Ending, Input, Output, Session};
use pavia_proto::session_id::SessionId;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};

use crate::auth::{AuthFailure, Authenticator, Principal};
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
pub async fn run<A: Authenticator>(mut transport: Transport, registry: &Registry, authenticator: &A) -> Ending {
  let (sender, mut commands) = mpsc::channel(COMMAND_QUEUE);
  let mut membership = Membership {
    registry,
    handle: Handle::new(sender),
    welcoming: None,
    registration: None,
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
    };
    if answered {
      authenticating = None;
    }
    session.handle_input(input, Instant::now().into_std());
    match carry_out(&mut session, &mut transport, &mut membership).await {
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

enum Step {
  Continue,
  Authenticate(Option<Auth>),
  Ended(Ending),
}

async fn carry_out(session: &mut Session, transport: &mut Transport, membership: &mut Membership<'_>) -> Step {
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
      Output::Ended(ending) => return Step::Ended(ending),
      _ => {}
    }
  }
  step
}

#[cfg(test)]
mod tests;
