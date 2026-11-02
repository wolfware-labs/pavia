//! The task that drives one [`pavia_proto::session::Session`] over a WebSocket [`Transport`].
//!
//! [`run`] feeds every received message, transport event and [`Command`] into the session and
//! carries out its outputs in order: a CLOSE frame is flushed before the transport closes (§7.7).
//! It returns the session's [`Ending`], so each session reports its end exactly once (§13.4).

use std::future::Future;
use std::pin::Pin;

use pavia_proto::frame::Auth;
use pavia_proto::session::{Command, Config, Ending, Input, Output, Session};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};

use crate::auth::{AuthFailure, Authenticator, Principal};
use crate::websocket::{Received, Transport};

type Authenticating<'a> = Pin<Box<dyn Future<Output = Result<Principal, AuthFailure>> + Send + 'a>>;

/// Runs a server session on `transport` until it ends, taking application requests from
/// `commands`. Dropping every sender of `commands` leaves the session running.
///
/// `authenticator` runs once, after HELLO (§7.5). The transport keeps being read while it runs,
/// so a CLOSE, a transport loss or the handshake deadline still ends the session.
///
/// The handshake timer starts at [`Transport::accepted`], before the upgrade. The clock is
/// tokio's, so a test that pauses time drives the session's timers too.
pub async fn run<A: Authenticator>(
  mut transport: Transport,
  mut commands: mpsc::Receiver<Command>,
  config: &Config,
  authenticator: &A,
) -> Ending {
  let mut session = Session::server(config, transport.accepted().into_std());
  let context = transport.context().clone();
  let mut authenticating: Option<Authenticating<'_>> = None;
  let mut commands_open = true;
  loop {
    let deadline = session.poll_timeout().map(Instant::from_std);
    let input = tokio::select! {
      () = sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => Input::Timeout,
      answer = async { authenticating.as_mut().expect("guarded by the branch condition").await },
        if authenticating.is_some() => match answer {
        Ok(principal) => Input::Authenticated {
          user: principal.user().map(str::to_owned),
        },
        Err(failure) => Input::AuthFailed(failure),
      },
      received = transport.recv() => match received {
        Some(Ok(Received::Binary(message))) => Input::Message(message),
        Some(Ok(Received::Text)) => Input::TextMessage,
        Some(Err(_)) | None => Input::TransportLost,
      },
      command = commands.recv(), if commands_open => match command {
        Some(command) => Input::Command(command),
        None => {
          commands_open = false;
          continue;
        }
      },
    };
    if matches!(input, Input::Authenticated { .. } | Input::AuthFailed(_)) {
      authenticating = None;
    }
    session.handle_input(input, Instant::now().into_std());
    match carry_out(&mut session, &mut transport).await {
      Step::Continue => {}
      Step::Authenticate(auth) => {
        let context = &context;
        authenticating = Some(Box::pin(
          async move { authenticator.authenticate(auth.as_ref(), context).await },
        ));
      }
      Step::Ended(ending) => return ending,
    }
  }
}

enum Step {
  Continue,
  Authenticate(Option<Auth>),
  Ended(Ending),
}

async fn carry_out(session: &mut Session, transport: &mut Transport) -> Step {
  let mut step = Step::Continue;
  while let Some(output) = session.poll_output() {
    match output {
      Output::Transmit(message) => {
        if transport.send(message).await.is_err() {
          session.handle_input(Input::TransportLost, Instant::now().into_std());
        }
      }
      Output::CloseTransport { code, reason } => {
        let _ = transport.close(code, &reason).await;
      }
      Output::Authenticate { auth } => step = Step::Authenticate(auth),
      Output::Ended(ending) => return Step::Ended(ending),
      _ => {}
    }
  }
  step
}

#[cfg(test)]
mod tests;
