//! Session ID generation. The format and the uniqueness argument are in
//! [`pavia_proto::session_id`]; this module supplies the random bytes from the operating system's
//! cryptographically secure generator.

use std::io;

use pavia_proto::session_id::SessionId;

/// A new session ID from 16 bytes of operating system randomness.
///
/// # Errors
///
/// The operating system's error when it cannot supply random bytes. The caller then refuses the
/// session instead of falling back to a weaker source.
#[cfg_attr(not(test), expect(dead_code))]
pub(crate) fn generate() -> io::Result<SessionId> {
  let mut bytes = [0; 16];
  getrandom::fill(&mut bytes)?;
  Ok(SessionId::from_random(bytes))
}

#[cfg(test)]
mod tests;
