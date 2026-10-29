//! Data sections (§5.5): one value in the session codec, `cbor` or `json`, without a contract type.
//!
//! [`decode`] reads any valid encoding and keeps map entries in the order received. [`encode`]
//! writes the one encoding of §12.3: definite lengths and the shortest integer, length and float
//! forms in `cbor`, compact RFC 8785 strings and numbers in `json`, and map entries in the order the
//! [`Value`] holds them. Putting struct fields and map keys in order is the typed layer's job.
//!
//! A decode failure is never session-fatal. The receive path wraps it in
//! [`ProtocolError::UndecodableData`](crate::error::ProtocolError::UndecodableData) for the data
//! section at hand, which decides the outcome (§8.10).

use bytes::BufMut;
use thiserror::Error;

mod cbor;
mod json;

/// Limits a data-section decoder enforces on input from the peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeLimits {
  /// How deep a value may nest (§12.3, the same limit as headers in §5.4.1). The outermost array,
  /// map or tag has depth 1 and every one inside it adds one; other items add nothing. Tags count
  /// so that a chain of them cannot recurse without bound.
  pub max_depth: usize,
}

impl Default for DecodeLimits {
  /// A depth of 16, the protocol default.
  fn default() -> Self {
    Self { max_depth: 16 }
  }
}

/// The session codec, chosen in the handshake (§5.5, §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
  /// CBOR (RFC 8949), the default every implementation supports.
  Cbor,
  /// JSON (RFC 8259) in UTF-8, meant for debugging.
  Json,
}

impl Codec {
  /// The name used in HELLO and WELCOME.
  #[must_use]
  pub const fn name(self) -> &'static str {
    match self {
      Self::Cbor => "cbor",
      Self::Json => "json",
    }
  }

  /// The codec with this name, if Pavia defines one.
  #[must_use]
  pub fn from_name(name: &str) -> Option<Self> {
    match name {
      "cbor" => Some(Self::Cbor),
      "json" => Some(Self::Json),
      _ => None,
    }
  }
}

/// One data-section value without a contract type.
///
/// It covers every CBOR data item. A `json` value uses `Null`, `Bool`, the number variants, `Text`,
/// `Array` and maps with text keys. JSON has one number type: integers decode to `Unsigned` or
/// `Negative` when they fit, anything else to `Float`.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
  /// `null`.
  Null,
  /// CBOR `undefined`.
  Undefined,
  /// `false` or `true`.
  Bool(bool),
  /// An integer from 0 to 2^64 − 1.
  Unsigned(u64),
  /// The integer −1 − n, from −1 down to −2^64.
  Negative(u64),
  /// A floating-point number.
  Float(f64),
  /// A text string.
  Text(String),
  /// A byte string (CBOR only).
  Bytes(Vec<u8>),
  /// An array.
  Array(Vec<Value>),
  /// A map, with its entries in order.
  Map(Vec<(Value, Value)>),
  /// A tagged item (CBOR only).
  Tag(u64, Box<Value>),
  /// Any other CBOR simple value: 0 to 19 or 32 to 255. The encoder refuses 20 to 31, which are
  /// `false`, `true`, `null`, `undefined` or reserved.
  Simple(u8),
}

/// A data section that does not decode in the session codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum DecodeError {
  /// The bytes are not one well-formed CBOR item or JSON value.
  #[error("data section is not well-formed")]
  Malformed,
  /// Bytes follow the single item.
  #[error("bytes after the data section's value")]
  TrailingBytes,
  /// The value nests deeper than [`DecodeLimits::max_depth`].
  #[error("data section nests deeper than the depth limit")]
  TooDeep,
  /// A `json` data section that is not valid UTF-8 (§12.3).
  #[error("json data section is not valid UTF-8")]
  InvalidUtf8,
  /// A `json` object with the same key twice (§12.3).
  #[error("json object has a duplicate key")]
  DuplicateKey,
}

/// A [`Value`] the codec cannot carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum EncodeError {
  /// A [`Value::Simple`] from 20 to 31.
  #[error("simple value 20 to 31 has another form or none")]
  ReservedSimpleValue,
  /// `json` has no form for byte strings, tags, `undefined`, simple values, map keys that are not
  /// text, NaN or infinities.
  #[error("value has no json form")]
  NotJson,
}

/// Decodes one data section in `codec`, within `limits`.
///
/// # Errors
///
/// Returns a [`DecodeError`] for input that is not exactly one value of `codec` within `limits`,
/// or `json` that breaks the rules of §12.3.
pub fn decode(data: &[u8], codec: Codec, limits: &DecodeLimits) -> Result<Value, DecodeError> {
  match codec {
    Codec::Cbor => cbor::decode(data, limits.max_depth),
    Codec::Json => json::decode(data, limits.max_depth),
  }
}

/// Encodes `value` as a data section in `codec`.
///
/// # Errors
///
/// Returns [`EncodeError::ReservedSimpleValue`] for a simple value from 20 to 31, and
/// [`EncodeError::NotJson`] for a `json` value with no JSON form. Nothing is written to `out` in
/// either case.
pub fn encode(value: &Value, codec: Codec, out: &mut impl BufMut) -> Result<(), EncodeError> {
  match codec {
    Codec::Cbor => cbor::encode(value, out),
    Codec::Json => json::encode(value, out),
  }
}

#[cfg(test)]
mod tests;
