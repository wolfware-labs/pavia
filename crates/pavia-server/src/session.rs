//! The task that drives one [`pavia_proto::session::Session`] over a WebSocket [`Transport`].
//!
//! [`run`] feeds every received message, transport event and [`Command`] into the session and
//! carries out its outputs in order: a CLOSE frame is flushed before the transport closes (§7.7).
//! It returns the session's [`Ending`], so each session reports its end exactly once (§13.4).

use pavia_proto::session::{Command, Config, Ending, Input, Output, Session};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};

use crate::websocket::{Received, Transport};

/// Runs a server session on `transport` until it ends, taking application requests from
/// `commands`. Dropping every sender of `commands` leaves the session running.
///
/// The handshake timer starts at [`Transport::accepted`], before the upgrade. The clock is
/// tokio's, so a test that pauses time drives the session's timers too.
pub async fn run(mut transport: Transport, mut commands: mpsc::Receiver<Command>, config: &Config) -> Ending {
  let mut session = Session::server(config, transport.accepted().into_std());
  let mut commands_open = true;
  loop {
    let deadline = session.poll_timeout().map(Instant::from_std);
    let input = tokio::select! {
      () = sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => Input::Timeout,
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
    session.handle_input(input, Instant::now().into_std());
    if let Some(ending) = carry_out(&mut session, &mut transport).await {
      return ending;
    }
  }
}

async fn carry_out(session: &mut Session, transport: &mut Transport) -> Option<Ending> {
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
      Output::Ended(ending) => return Some(ending),
      _ => {}
    }
  }
  None
}

#[cfg(test)]
mod tests;
