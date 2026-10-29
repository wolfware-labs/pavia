//! The call stream state machine (§6.3, §8.3 to §8.5): which frames a call accepts given its shape
//! and what it has seen, as pure state with no IO and no timers.

use super::Role;
use crate::error::ProtocolError;
use crate::frame::FrameType;

/// Who streams in a call (§8.1). "Server" and "client" mean callee and caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
  /// One request, one RESULT (0).
  Unary,
  /// One request, then ITEMs and END from the callee (1).
  ServerStream,
  /// ITEMs and END from the caller, then one RESULT (2).
  ClientStream,
  /// ITEMs and END in both directions (3).
  Bidirectional,
}

impl Shape {
  /// The caller streams ITEMs and closes the request half with END.
  const fn caller_streams(self) -> bool {
    matches!(self, Self::ClientStream | Self::Bidirectional)
  }

  /// The callee streams ITEMs and closes the response half with END instead of RESULT.
  const fn callee_streams(self) -> bool {
    matches!(self, Self::ServerStream | Self::Bidirectional)
  }
}

impl TryFrom<u64> for Shape {
  type Error = u64;

  fn try_from(value: u64) -> Result<Self, u64> {
    match value {
      0 => Ok(Self::Unary),
      1 => Ok(Self::ServerStream),
      2 => Ok(Self::ClientStream),
      3 => Ok(Self::Bidirectional),
      other => Err(other),
    }
  }
}

/// What a frame did to the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
  /// The frame is legal and was applied.
  Accepted,
  /// The frame is dropped: a second CANCEL, or anything but the terminal frame after this side
  /// cancelled as the caller (§8.5).
  Ignored,
}

/// A WebTransport event on the peer's side of the call stream (§6.3, §8.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerEvent {
  /// RESET_STREAM on the peer's send side.
  Reset,
  /// FIN on the peer's send side, at a frame boundary.
  Fin,
  /// STOP_SENDING for this side's send side.
  StopSending,
}

/// What a [`PeerEvent`] means for the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
  /// Nothing to do: the event follows a terminal frame, or the call is complete.
  None,
  /// The call is cancelled. A callee stops work and answers ERROR CANCELLED while its response
  /// half is open; a caller completes the call with CANCELLED.
  Cancelled,
  /// The callee went away: the caller completes the call with ERROR UNAVAILABLE.
  Unavailable,
}

#[derive(Debug, Clone, Copy)]
enum Response {
  Item,
  End,
  Result,
  Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Request {
  Open,
  Ended,
  Cancelled,
}

/// One call stream as one side sees it, from CALL on.
///
/// Feed it every frame on the stream, sent and received, with the role of the side that sent it.
/// Once [`is_complete`](Self::is_complete) holds, the stream ID rules of §6.1 take over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallState {
  shape: Shape,
  local: Role,
  request: Request,
  response_open: bool,
}

impl CallState {
  /// The state right after CALL, for the `local` side of a call of `shape`. CALL closes the
  /// request half of unary and server-stream calls.
  #[must_use]
  pub const fn new(shape: Shape, local: Role) -> Self {
    let request = if shape.caller_streams() {
      Request::Open
    } else {
      Request::Ended
    };
    Self {
      shape,
      local,
      request,
      response_open: true,
    }
  }

  /// Whether the call is complete: both halves closed, or RESULT or ERROR closed the response
  /// half while the request half was open (§6.3).
  #[must_use]
  pub const fn is_complete(&self) -> bool {
    !self.response_open && !matches!(self.request, Request::Open)
  }

  /// Whether the callee may still send on the response half.
  #[must_use]
  pub const fn response_open(&self) -> bool {
    self.response_open
  }

  /// Applies one frame on this stream, sent by `sender`.
  ///
  /// CANCEL is accepted until the call is complete. On a bidirectional call the caller may cancel
  /// after the callee's END, which completes the call (§6.3); the callee then has nothing to
  /// answer, since [`response_open`](Self::response_open) is false (§8.5).
  ///
  /// # Errors
  ///
  /// - [`ProtocolError::FrameAfterCallCompleted`] once the call is complete; the frame is dropped.
  /// - [`ProtocolError::FrameAfterTerminalFrame`] for a frame on a half after its terminal frame.
  /// - [`ProtocolError::FrameNotAllowedInShape`] for ITEM, END or RESULT the shape does not use on
  ///   the response half.
  /// - [`ProtocolError::FrameTypeNotAllowed`] for a type that `sender` never sends on a call
  ///   stream, including a second CALL.
  pub fn on_frame(&mut self, sender: Role, frame_type: FrameType) -> Result<Step, ProtocolError> {
    use FrameType as T;

    if self.is_complete() {
      return Err(ProtocolError::FrameAfterCallCompleted);
    }
    match (sender, frame_type) {
      (Role::Callee, T::Credit) if self.cancelled_as_caller() => Ok(Step::Ignored),
      (_, T::Credit) => Ok(Step::Accepted),
      (Role::Caller, T::Item) => self.request_frame(false),
      (Role::Caller, T::End) => self.request_frame(true),
      (Role::Caller, T::Cancel) => Ok(self.cancel()),
      (Role::Callee, T::Item) => self.response_frame(Response::Item),
      (Role::Callee, T::End) => self.response_frame(Response::End),
      (Role::Callee, T::Result) => self.response_frame(Response::Result),
      (Role::Callee, T::Error) => self.response_frame(Response::Error),
      (Role::Caller, T::Call | T::Result | T::Error)
      | (Role::Callee, T::Call | T::Cancel)
      | (
        _,
        T::Hello
        | T::Welcome
        | T::Reject
        | T::Ping
        | T::Pong
        | T::Goaway
        | T::Close
        | T::AuthRefresh
        | T::AuthResult
        | T::Ack
        | T::AuthExpiring
        | T::RequestError
        | T::Notify
        | T::LaneOpen
        | T::Subscribe
        | T::Subscribed
        | T::Unsubscribe
        | T::Unsubscribed
        | T::Publication
        | T::Presence
        | T::Datagram
        | T::DgramBind,
      ) => Err(ProtocolError::FrameTypeNotAllowed),
    }
  }

  /// Applies a WebTransport event from the peer (§6.3, §8.5). A reset, or a FIN before the half's
  /// terminal frame, counts as CANCEL from a caller and as ERROR UNAVAILABLE from a callee.
  /// STOP_SENDING from a caller counts as CANCEL, and the callee resets its send side instead of
  /// answering. Anything after the call is complete is ignored.
  pub fn on_peer_event(&mut self, event: PeerEvent) -> Effect {
    if self.is_complete() {
      return Effect::None;
    }
    match (self.local, event) {
      (Role::Callee, PeerEvent::Reset) => self.peer_cancelled(),
      (Role::Callee, PeerEvent::Fin) if matches!(self.request, Request::Open) => self.peer_cancelled(),
      (Role::Callee, PeerEvent::StopSending) => {
        self.response_open = false;
        self.peer_cancelled()
      }
      (Role::Caller, PeerEvent::Reset | PeerEvent::Fin) if self.response_open => {
        self.response_open = false;
        if matches!(self.request, Request::Cancelled) {
          Effect::Cancelled
        } else {
          self.request = Request::Ended;
          Effect::Unavailable
        }
      }
      (Role::Callee, PeerEvent::Fin) | (Role::Caller, _) => Effect::None,
    }
  }

  fn request_frame(&mut self, end: bool) -> Result<Step, ProtocolError> {
    match self.request {
      Request::Open => {
        if end {
          self.request = Request::Ended;
        }
        Ok(Step::Accepted)
      }
      Request::Ended | Request::Cancelled => Err(ProtocolError::FrameAfterTerminalFrame),
    }
  }

  const fn cancelled_as_caller(&self) -> bool {
    matches!((self.local, self.request), (Role::Caller, Request::Cancelled))
  }

  fn cancel(&mut self) -> Step {
    if matches!(self.request, Request::Cancelled) {
      Step::Ignored
    } else {
      self.request = Request::Cancelled;
      Step::Accepted
    }
  }

  fn peer_cancelled(&mut self) -> Effect {
    match self.cancel() {
      Step::Accepted => Effect::Cancelled,
      Step::Ignored => Effect::None,
    }
  }

  fn response_frame(&mut self, frame: Response) -> Result<Step, ProtocolError> {
    let terminal = match frame {
      Response::Item => false,
      Response::End => self.shape.callee_streams(),
      Response::Result => !self.shape.callee_streams(),
      Response::Error => true,
    };
    let in_shape = match frame {
      Response::Item => self.shape.callee_streams(),
      Response::End | Response::Result => terminal,
      Response::Error => true,
    };

    if self.cancelled_as_caller() && !terminal {
      return Ok(Step::Ignored);
    }
    if !self.response_open {
      return Err(ProtocolError::FrameAfterTerminalFrame);
    }
    if !in_shape {
      return Err(ProtocolError::FrameNotAllowedInShape);
    }
    if terminal {
      self.response_open = false;
      if matches!(frame, Response::Result | Response::Error) && matches!(self.request, Request::Open) {
        self.request = Request::Ended;
      }
    }
    Ok(Step::Accepted)
  }
}

#[cfg(test)]
mod tests;
