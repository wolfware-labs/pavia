//! WebSocket stream IDs (§6.1): who may open which ID, and which frames on an ID are delivered,
//! ignored or a protocol error.

use std::collections::HashSet;

use super::Side;
use crate::error::ProtocolError;

const MAX_STREAM_ID: u64 = (1 << 62) - 1;

/// What [`StreamIds::receive`] found for an incoming frame's stream ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admitted {
  /// Stream 0, the control stream.
  Control,
  /// The frame opened this stream, which the peer initiated.
  Opened,
  /// The stream was already open.
  Open,
}

/// The stream IDs of one WebSocket transport, seen from one side (§6.1).
///
/// It keeps the highest ID each side opened and the set of streams open now, nothing about
/// closed or skipped IDs, so its size follows the open streams only. A new transport, including
/// one that resumes a session, starts with a new tracker (§7.8).
#[derive(Debug, Clone)]
pub struct StreamIds {
  local: Side,
  highest_by_client: u64,
  highest_by_server: u64,
  open: HashSet<u64>,
}

impl StreamIds {
  /// A tracker for a fresh transport on the `local` side.
  #[must_use]
  pub fn new(local: Side) -> Self {
    Self {
      local,
      highest_by_client: 0,
      highest_by_server: 0,
      open: HashSet::new(),
    }
  }

  /// Opens the next stream of this side: 2, 4, 6 for the client, 1, 3, 5 for the server.
  ///
  /// Returns `None` once the next ID would be above 2^62 − 1, the largest a varint holds.
  pub fn open_next(&mut self) -> Option<u64> {
    let local = self.local;
    let highest = self.highest_mut(local);
    let next = match *highest {
      0 => first_id(local),
      id => id.checked_add(2).filter(|next| *next <= MAX_STREAM_ID)?,
    };
    *highest = next;
    self.open.insert(next);
    Some(next)
  }

  /// Checks an incoming frame on `stream_id`. `opening` is whether it is CALL or LANE_OPEN.
  ///
  /// # Errors
  ///
  /// - [`ProtocolError::StreamIdWrongParity`] for an opening frame on an ID this side initiates.
  /// - [`ProtocolError::StreamIdOutOfOrder`] for an opening frame at or below the peer's highest
  ///   opened ID, or any other frame above the initiator's highest opened ID.
  /// - [`ProtocolError::FrameOnClosedStream`] for any other frame at or below that ID on a stream
  ///   that is not open, closed or skipped. The receiver ignores it.
  pub fn receive(&mut self, stream_id: u64, opening: bool) -> Result<Admitted, ProtocolError> {
    if stream_id == 0 {
      return Ok(Admitted::Control);
    }
    let initiator = initiator(stream_id);
    if opening && initiator == self.local {
      return Err(ProtocolError::StreamIdWrongParity);
    }
    let highest = self.highest_mut(initiator);
    match (stream_id > *highest, opening) {
      (true, true) => {
        *highest = stream_id;
        self.open.insert(stream_id);
        Ok(Admitted::Opened)
      }
      (true, false) | (false, true) => Err(ProtocolError::StreamIdOutOfOrder),
      (false, false) if self.open.contains(&stream_id) => Ok(Admitted::Open),
      (false, false) => Err(ProtocolError::FrameOnClosedStream),
    }
  }

  /// Marks `stream_id` closed. Later frames on it are ignored and it cannot be opened again.
  pub fn close(&mut self, stream_id: u64) {
    self.open.remove(&stream_id);
  }

  /// Whether `stream_id` is open. Stream 0 always is.
  #[must_use]
  pub fn is_open(&self, stream_id: u64) -> bool {
    stream_id == 0 || self.open.contains(&stream_id)
  }

  fn highest_mut(&mut self, initiator: Side) -> &mut u64 {
    match initiator {
      Side::Client => &mut self.highest_by_client,
      Side::Server => &mut self.highest_by_server,
    }
  }
}

const fn first_id(side: Side) -> u64 {
  match side {
    Side::Client => 2,
    Side::Server => 1,
  }
}

const fn initiator(stream_id: u64) -> Side {
  if stream_id.is_multiple_of(2) { Side::Client } else { Side::Server }
}

#[cfg(test)]
mod tests;
