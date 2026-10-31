//! Which frame types may arrive on which kind of stream, and from which end (§5.6).
//!
//! [`allowed`] answers whether a type may ever appear on a stream. Whether it may appear now, such
//! as ITEM after END, is the call state machine's question; capabilities and the handshake are
//! session checks.

use crate::error::ProtocolError;
use crate::frame::FrameType;

#[cfg_attr(not(test), expect(dead_code, reason = "used by the WebSocket session from #24 onwards"))]
pub(crate) mod ids;

#[cfg_attr(not(test), expect(dead_code, reason = "used by the session from #24 onwards"))]
pub(crate) mod call;

/// The transport a session runs on (§4.2, §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Binding {
  /// WebTransport over HTTP/3.
  WebTransport,
  /// WebSocket.
  WebSocket,
}

/// Which end of the session sent a frame on the control stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
  /// The client.
  Client,
  /// The server.
  Server,
}

/// Which end of a call sent a frame on its call stream. The caller is the side that sent CALL,
/// which is the server for a server-initiated call (§8.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
  /// The side that opened the call.
  Caller,
  /// The side running the method.
  Callee,
}

/// Where a frame arrived: the kind of stream, with the sender where the registry restricts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
  /// Stream 0, the control stream (§6.2).
  Control(Side),
  /// A call stream (§8).
  Call(Role),
  /// A lane stream, for lanes 1 to 16 (§9.2).
  Lane,
  /// A WebTransport datagram (§11.3).
  Datagram,
}

/// Whether the registry allows `frame_type` from `origin` on `binding` (§5.6).
#[must_use]
pub const fn allowed(frame_type: FrameType, origin: Origin, binding: Binding) -> bool {
  use FrameType as T;
  use Origin::{Call, Control, Datagram, Lane};

  match frame_type {
    T::Hello | T::AuthRefresh | T::Subscribe | T::Unsubscribe => matches!(origin, Control(Side::Client)),
    T::Welcome
    | T::Reject
    | T::Goaway
    | T::AuthResult
    | T::AuthExpiring
    | T::RequestError
    | T::Subscribed
    | T::Unsubscribed
    | T::Publication
    | T::Presence => matches!(origin, Control(Side::Server)),
    T::Ping | T::Pong | T::Close | T::Ack | T::DgramBind => matches!(origin, Control(_)),
    T::Call | T::Item | T::End => matches!(origin, Call(_)),
    T::Result | T::Error => matches!(origin, Call(Role::Callee)),
    T::Cancel => matches!(origin, Call(Role::Caller)),
    T::Credit => matches!((origin, binding), (Call(_), Binding::WebSocket)),
    T::Notify => matches!(origin, Control(_) | Lane),
    T::LaneOpen => matches!(origin, Lane),
    T::Datagram => matches!(
      (origin, binding),
      (Control(_), Binding::WebSocket) | (Datagram, Binding::WebTransport)
    ),
  }
}

/// [`allowed`] as a check for the receive path.
///
/// # Errors
///
/// Returns [`ProtocolError::FrameTypeNotAllowed`] when the frame arrived on the wrong kind of
/// stream or from the wrong end (§5.6).
pub const fn check(frame_type: FrameType, origin: Origin, binding: Binding) -> Result<(), ProtocolError> {
  if allowed(frame_type, origin, binding) {
    Ok(())
  } else {
    Err(ProtocolError::FrameTypeNotAllowed)
  }
}

#[cfg(test)]
mod tests;
