//! The session state machine (§7), driven as sans-IO: the driver feeds [`Input`]s with
//! [`Session::handle_input`] and then drains [`Session::poll_output`] until it returns `None`.
//!
//! This first part covers how a session ends (§7.7, §13.4): CLOSE in either direction, a
//! session-fatal error, and transport loss. Every ending yields exactly one [`Output::Ended`], and
//! after it the session ignores all input. The handshake and the frames it unlocks come next
//! (#27 onwards); until then the session ignores every frame except CLOSE.
//!
//! ```text
//!  Command::Close ──▶ Transmit(CLOSE) ─▶ CloseTransport(code) ─▶ Ended(Closed, by us)
//!  CLOSE received ──────────────────────▶ CloseTransport(code) ─▶ Ended(Closed, by peer)
//!  fatal error ─────▶ Transmit(CLOSE) ─▶ CloseTransport(code) ─▶ Ended(Closed, by us)
//!  TransportLost ──────────────────────────────────────────────▶ Ended(TransportLost)
//! ```
//!
//! Only the WebSocket binding exists so far: one [`Output::Transmit`] is one binary message.

use std::collections::VecDeque;

use bytes::{Bytes, BytesMut};

use crate::error::{Outcome, ProtocolError};
use crate::frame::{Close, Frame};
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
  /// The transport closed or failed without CLOSE.
  TransportLost,
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

/// One session, as one side sees it.
#[derive(Debug)]
pub struct Session {
  local: Side,
  max_frame: u64,
  limits: DecodeLimits,
  ended: bool,
  outputs: VecDeque<Output>,
}

impl Session {
  /// A WebSocket session for the `local` side.
  #[must_use]
  pub fn new(local: Side) -> Self {
    Self {
      local,
      max_frame: MAX_FRAME_FLOOR,
      limits: DecodeLimits::default(),
      ended: false,
      outputs: VecDeque::new(),
    }
  }

  /// Whether the session has ended. Every later input is ignored.
  #[must_use]
  pub const fn is_ended(&self) -> bool {
    self.ended
  }

  /// Applies one input. Drain [`poll_output`](Self::poll_output) afterwards.
  pub fn handle_input(&mut self, input: Input) {
    if self.ended {
      return;
    }
    match input {
      Input::Message(message) => self.receive(message),
      Input::TransportLost => self.end(Ending::TransportLost),
      Input::Command(Command::Close { code, msg }) => self.close(code, msg),
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
      if let Frame::Close(close) = frame {
        if ws_frame.stream_id != CONTROL_STREAM {
          return self.fail(&ProtocolError::FrameTypeNotAllowed);
        }
        return self.peer_closed(close);
      }
    }
  }

  fn fail(&mut self, error: &ProtocolError) {
    if let Outcome::SessionFatal(code) = error.outcome() {
      self.close(code, Some(error.to_string()));
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
      by: self.local.peer(),
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
      by: self.local,
    });
  }

  fn end(&mut self, ending: Ending) {
    self.ended = true;
    self.outputs.push_back(Output::Ended(ending));
  }
}

#[cfg(test)]
mod tests;
