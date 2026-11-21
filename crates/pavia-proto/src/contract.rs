//! The contract fingerprint (§12.5) and what a server does when the client's differs (§7.2).
//!
//! The fingerprint is the SHA-256 of the contract manifest in canonical form. HELLO and WELCOME
//! carry it in `contract` (key 24). It names one exact contract version and says nothing about
//! compatibility, so a mismatch under [`ContractPolicy::Warn`] is only reported.

use std::fmt;

use data_encoding::HEXLOWER;

/// A contract fingerprint: 32 bytes, shown as 64 lowercase hex digits.
///
/// ```
/// use pavia_proto::contract::Fingerprint;
///
/// assert_eq!(Fingerprint::new([0xab; 32]).to_string(), "ab".repeat(32));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
  /// The fingerprint `bytes`.
  #[must_use]
  pub const fn new(bytes: [u8; 32]) -> Self {
    Self(bytes)
  }

  /// The 32 bytes.
  #[must_use]
  pub const fn bytes(&self) -> [u8; 32] {
    self.0
  }
}

impl fmt::Display for Fingerprint {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str(&HEXLOWER.encode(&self.0))
  }
}

impl fmt::Debug for Fingerprint {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(formatter, "Fingerprint({self})")
  }
}

/// What a server with a fingerprint does when a HELLO carries a different one, or none (§7.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContractPolicy {
  /// Report the mismatch and go on with the session. The default.
  #[default]
  Warn,
  /// Refuse a HELLO whose fingerprint differs, or that carries none, with REJECT CONTRACT_MISMATCH,
  /// before the session is authenticated. A server without a fingerprint never refuses on it.
  ///
  /// This is a compatibility check, not access control: the fingerprint is public (it ships in every
  /// generated client, in WELCOME and at the well-known contract URL), so any peer can send the right
  /// one. The REJECT carries no fingerprint and no detail beyond its code.
  Strict,
}

#[cfg(test)]
mod tests;
