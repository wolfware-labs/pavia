//! Session IDs (§7.2): the public name of a session, stable across resumption.
//!
//! An ID is 1 to 64 characters from `[A-Za-z0-9_-]`. This crate builds one from 16 random bytes
//! encoded as base64url without padding (RFC 4648 §5), which gives 22 characters. With the bytes
//! taken from a cryptographically secure generator, IDs are unique per endpoint across all nodes
//! without coordination: among 2^40 sessions, the chance that two of 128-bit IDs collide is about
//! 2^-49. Random IDs need no node identity and no counter that must survive restarts, which a
//! node-prefixed counter would. The bytes come from the driver because this crate does no I/O;
//! `pavia-server` reads them from the operating system.
//!
//! An ID is never a credential (§14): nothing may be authorized by it. The resume token is a
//! separate secret and is never derived from it.

use std::fmt;

use data_encoding::BASE64URL_NOPAD;
use thiserror::Error;

/// The longest session ID §7.2 allows, in characters.
pub const MAX_LEN: usize = 64;

/// A session ID in the §7.2 format. Only [`SessionId::from_random`] and [`SessionId::parse`] build
/// one, so every value is valid.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
  /// The ID for 16 bytes, which the caller takes from a cryptographically secure generator.
  ///
  /// ```
  /// use pavia_proto::session_id::SessionId;
  ///
  /// assert_eq!(SessionId::from_random([0; 16]).as_str(), "AAAAAAAAAAAAAAAAAAAAAA");
  /// ```
  #[must_use]
  pub fn from_random(bytes: [u8; 16]) -> Self {
    Self(BASE64URL_NOPAD.encode(&bytes))
  }

  /// The ID `text` names, if it has the §7.2 format.
  ///
  /// # Errors
  ///
  /// [`InvalidSessionId`] when `text` is empty, longer than [`MAX_LEN`], or has a character
  /// outside `[A-Za-z0-9_-]`.
  pub fn parse(text: &str) -> Result<Self, InvalidSessionId> {
    let allowed = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-';
    if (1..=MAX_LEN).contains(&text.len()) && text.bytes().all(allowed) {
      Ok(Self(text.to_owned()))
    } else {
      Err(InvalidSessionId)
    }
  }

  /// The ID as text, for WELCOME `session` and the HELLO `resume` map.
  #[must_use]
  pub fn as_str(&self) -> &str {
    &self.0
  }
}

impl fmt::Display for SessionId {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(&self.0)
  }
}

/// Text that is not a session ID in the §7.2 format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("session ID must be 1 to 64 characters from [A-Za-z0-9_-]")]
pub struct InvalidSessionId;

#[cfg(test)]
mod tests;
