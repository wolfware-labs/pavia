//! Rust types for the contract primitives std has no type for: `bytes`, `timestamp` and `uuid` (§12.1).
//!
//! `Vec<u8>` maps to `{"list": "u8"}`, since an impl for every `Vec<T>` leaves no room for one on
//! `Vec<u8>`; a byte string is a [`Bytes`].

/// A byte string, the contract type `bytes`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Bytes(Vec<u8>);

impl Bytes {
  /// The byte string `bytes`.
  #[must_use]
  pub const fn new(bytes: Vec<u8>) -> Self {
    Self(bytes)
  }

  /// The bytes.
  #[must_use]
  pub fn as_slice(&self) -> &[u8] {
    &self.0
  }

  /// The bytes, by value.
  #[must_use]
  pub fn into_vec(self) -> Vec<u8> {
    self.0
  }
}

impl From<Vec<u8>> for Bytes {
  fn from(bytes: Vec<u8>) -> Self {
    Self(bytes)
  }
}

/// An instant with millisecond precision, the contract type `timestamp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(i64);

impl Timestamp {
  /// The instant `millis` milliseconds after the Unix epoch (before it when negative).
  #[must_use]
  pub const fn from_unix_millis(millis: i64) -> Self {
    Self(millis)
  }

  /// Milliseconds since the Unix epoch.
  #[must_use]
  pub const fn unix_millis(self) -> i64 {
    self.0
  }
}

/// A 128-bit UUID, the contract type `uuid`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Uuid([u8; 16]);

impl Uuid {
  /// The UUID whose 16 bytes, in network order, are `bytes`.
  #[must_use]
  pub const fn from_bytes(bytes: [u8; 16]) -> Self {
    Self(bytes)
  }

  /// The 16 bytes, in network order.
  #[must_use]
  pub const fn bytes(self) -> [u8; 16] {
    self.0
  }
}

#[cfg(test)]
mod tests;
