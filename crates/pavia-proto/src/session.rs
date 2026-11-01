//! The session state machine (§7), driven as sans-IO: the driver feeds [`Input`]s with
//! [`Session::handle_input`] and then drains [`Session::poll_output`] until it returns `None`.
//!
//! So far it is the server side up to a validated HELLO (§6.2, §7.1) and every way a session ends
//! (§7.7, §13.4). Time is an input: the session is built with the accept time, every input
//! carries `now`, and [`Session::poll_timeout`] gives the handshake deadline.
//!
//! ```text
//!  AwaitHello ──HELLO──▶ Handshaking ──(WELCOME, #31)──▶ …
//!      │                     │
//!      ├ other frame first ──┤ second HELLO ─────▶ CLOSE PROTOCOL_ERROR
//!      ├ deadline ───────────┤ deadline ─────────▶ CLOSE HANDSHAKE_TIMEOUT
//!      └ TransportLost ──────┴ CLOSE received ───▶ Ended
//! ```
//!
//! Every ending yields exactly one [`Output::Ended`], and after it the session ignores all input.
//! A text message is a PROTOCOL_ERROR in every state (§4.3). Frames the client pipelines after
//! HELLO are ignored until WELCOME exists (#31, #40). Only the WebSocket binding exists so far:
//! one [`Output::Transmit`] is one binary message.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};

use crate::error::{Outcome, ProtocolError};
use crate::frame::{Close, Frame, Hello};
use crate::registry::CloseCode;
use crate::stream::Side;
use crate::wire::envelope::{self, MAX_FRAME_FLOOR};
use crate::wire::header::DecodeLimits;
use crate::wire::varint;

const CONTROL_STREAM: u64 = 0;

/// What the driver tells the session.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Input {
  /// One whole binary WebSocket message.
  Message(Bytes),
  /// A text WebSocket message, which closes the session with PROTOCOL_ERROR at any point (§4.3).
  TextMessage,
  /// The transport closed or failed without CLOSE.
  TransportLost,
  /// The time from [`Session::poll_timeout`] has come.
  Timeout,
  /// A request from the application.
  Command(Command),
}

/// A request from the application.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Command {
  /// End the session: send CLOSE with `code` and `msg`, then close the transport (§7.7).
  Close {
    /// The close code.
    code: CloseCode,
    /// The CLOSE `msg`, also the transport close reason.
    msg: Option<String>,
  },
}

/// What the session asks the driver to do, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Output {
  /// Send one binary WebSocket message, already in the `WsFrame` envelope (§5.4).
  Transmit(Bytes),
  /// Close the transport. On WebSocket the close code is `4000 + code` and the reason is cut
  /// to 123 bytes on a character boundary (§4.3, §7.7).
  CloseTransport {
    /// The session close code.
    code: CloseCode,
    /// The close reason.
    reason: String,
  },
  /// The session is over. Emitted once, as the last output.
  Ended(Ending),
}

/// How a session ended (§13.4).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Ending {
  /// CLOSE was sent or received.
  Closed {
    /// The close code. A received code outside §15.1 is [`CloseCode::Internal`] (§15).
    code: CloseCode,
    /// The CLOSE `msg`.
    msg: Option<String>,
    /// The side that sent CLOSE.
    by: Side,
  },
  /// The transport closed or failed without CLOSE. Without resumption this ends the session.
  TransportLost,
}

/// Session settings chosen by the application.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Config {
  /// How long the handshake may take, from transport accept until WELCOME or REJECT is sent
  /// (§7.1). Default 10 s.
  pub handshake_timeout: Duration,
}

impl Default for Config {
  fn default() -> Self {
    Self {
      handshake_timeout: Duration::from_secs(10),
    }
  }
}

#[derive(Debug)]
enum State {
  AwaitHello { deadline: Instant },
  Handshaking { deadline: Instant, hello: Box<Hello> },
  Ended,
}

/// One server session.
#[derive(Debug)]
pub struct Session {
  max_frame: u64,
  limits: DecodeLimits,
  state: State,
  outputs: VecDeque<Output>,
}

impl Session {
  /// A server session on a WebSocket transport accepted at `now`. The handshake timer starts
  /// here, not at the first byte (§7.1).
  #[must_use]
  pub fn server(config: &Config, now: Instant) -> Self {
    Self {
      max_frame: MAX_FRAME_FLOOR,
      limits: DecodeLimits::default(),
      state: State::AwaitHello {
        deadline: now + config.handshake_timeout,
      },
      outputs: VecDeque::new(),
    }
  }

  /// Whether the session has ended. Every later input is ignored.
  #[must_use]
  pub const fn is_ended(&self) -> bool {
    matches!(self.state, State::Ended)
  }

  /// The client's HELLO, once it has arrived and passed validation.
  #[must_use]
  pub fn hello(&self) -> Option<&Hello> {
    match &self.state {
      State::Handshaking { hello, .. } => Some(hello),
      State::AwaitHello { .. } | State::Ended => None,
    }
  }

  /// When the driver must feed [`Input::Timeout`], or `None` when no timer is running.
  #[must_use]
  pub const fn poll_timeout(&self) -> Option<Instant> {
    match self.state {
      State::AwaitHello { deadline } | State::Handshaking { deadline, .. } => Some(deadline),
      State::Ended => None,
    }
  }

  /// Applies one input that arrived at `now`. Drain [`poll_output`](Self::poll_output) and read
  /// [`poll_timeout`](Self::poll_timeout) afterwards.
  pub fn handle_input(&mut self, input: Input, now: Instant) {
    if self.is_ended() {
      return;
    }
    let expired = self.poll_timeout().is_some_and(|deadline| now >= deadline);
    match input {
      Input::TransportLost => self.end(Ending::TransportLost),
      Input::TextMessage => self.fail(&ProtocolError::TextMessage),
      Input::Command(Command::Close { code, msg }) => self.close(code, msg),
      Input::Message(_) | Input::Timeout if expired => self.close(CloseCode::HandshakeTimeout, None),
      Input::Message(message) => self.receive(message),
      Input::Timeout => {}
    }
  }

  /// The next output, or `None` when there is nothing to do until the next input.
  pub fn poll_output(&mut self) -> Option<Output> {
    self.outputs.pop_front()
  }

  fn receive(&mut self, message: Bytes) {
    let frames = match envelope::decode_message(message, self.max_frame) {
      Ok(frames) => frames,
      Err(error) => return self.fail(&error),
    };
    for ws_frame in frames {
      let frame = match Frame::from_raw(&ws_frame.frame, &self.limits) {
        Ok(frame) => frame,
        Err(error) => return self.fail(&error.into()),
      };
      let on_control = ws_frame.stream_id == CONTROL_STREAM;
      match (&self.state, frame) {
        (State::AwaitHello { deadline }, Frame::Hello(hello)) if on_control => {
          if hello.versions.is_empty() || hello.codecs.is_empty() {
            return self.fail(&ProtocolError::InvalidHeaderField);
          }
          self.state = State::Handshaking {
            deadline: *deadline,
            hello: Box::new(hello),
          };
        }
        (State::Handshaking { .. }, Frame::Close(close)) if on_control => return self.peer_closed(close),
        (State::AwaitHello { .. }, _) | (State::Handshaking { .. }, Frame::Hello(_) | Frame::Close(_)) => {
          return self.fail(&ProtocolError::FrameTypeNotAllowed);
        }
        (State::Handshaking { .. }, _) => {}
        (State::Ended, _) => return,
      }
    }
  }

  fn fail(&mut self, error: &ProtocolError) {
    if let Outcome::SessionFatal(code) = error.outcome() {
      self.close(code, None);
    }
  }

  fn peer_closed(&mut self, close: Close) {
    let code = CloseCode::from_u16_lossy(u16::try_from(close.code).unwrap_or(u16::MAX));
    self.outputs.push_back(Output::CloseTransport {
      code,
      reason: close.msg.clone().unwrap_or_default(),
    });
    self.end(Ending::Closed {
      code,
      msg: close.msg,
      by: Side::Client,
    });
  }

  fn close(&mut self, code: CloseCode, msg: Option<String>) {
    let close = Frame::Close(Close {
      code: u64::from(u16::from(code)),
      msg: msg.clone(),
    });
    let mut message = BytesMut::new();
    varint::encode(CONTROL_STREAM, &mut message).expect("stream 0 fits a varint");
    if close.encode(&mut message).is_ok() {
      self.outputs.push_back(Output::Transmit(message.freeze()));
    }
    self.outputs.push_back(Output::CloseTransport {
      code,
      reason: msg.clone().unwrap_or_default(),
    });
    self.end(Ending::Closed {
      code,
      msg,
      by: Side::Server,
    });
  }

  fn end(&mut self, ending: Ending) {
    self.state = State::Ended;
    self.outputs.push_back(Output::Ended(ending));
  }
}

#[cfg(test)]
mod tests;
