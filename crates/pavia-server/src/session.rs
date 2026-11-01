//! The task that drives one [`pavia_proto::session::Session`] over a WebSocket [`Transport`].
//!
//! [`run`] feeds every received message, transport event and [`Command`] into the session and
//! carries out its outputs in order: a CLOSE frame is flushed before the transport closes (§7.7).
//! It returns the session's [`Ending`], so each session reports its end exactly once (§13.4).

use pavia_proto::session::{Command, Ending, Input, Output, Session};
use pavia_proto::stream::Side;
use tokio::sync::mpsc;

use crate::websocket::{Received, Transport};

/// Runs a server session on `transport` until it ends, taking application requests from
/// `commands`. Dropping every sender of `commands` leaves the session running.
pub async fn run(transport: Transport, mut commands: mpsc::Receiver<Command>) -> Ending {
  let mut session = Session::new(Side::Server);
  let mut transport = Some(transport);
  let mut commands_open = true;
  loop {
    let Some(open) = transport.as_mut() else {
      return Ending::TransportLost;
    };
    let input = {
      tokio::select! {
        received = open.recv() => match received {
          Some(Ok(Received::Binary(message))) => Input::Message(message),
          Some(Ok(Received::Text)) => continue,
          Some(Err(_)) | None => Input::TransportLost,
        },
        command = commands.recv(), if commands_open => match command {
          Some(command) => Input::Command(command),
          None => {
            commands_open = false;
            continue;
          }
        },
      }
    };
    session.handle_input(input);
    if let Some(ending) = carry_out(&mut session, &mut transport).await {
      return ending;
    }
  }
}

async fn carry_out(session: &mut Session, transport: &mut Option<Transport>) -> Option<Ending> {
  while let Some(output) = session.poll_output() {
    match output {
      Output::Transmit(message) => {
        let sent = match transport.as_mut() {
          Some(open) => open.send(message).await.is_ok(),
          None => false,
        };
        if !sent {
          *transport = None;
          session.handle_input(Input::TransportLost);
        }
      }
      Output::CloseTransport { code, reason } => {
        if let Some(open) = transport.take() {
          let _ = open.close(code, &reason).await;
        }
      }
      Output::Ended(ending) => return Some(ending),
      _ => {}
    }
  }
  None
}

#[cfg(test)]
mod tests;
