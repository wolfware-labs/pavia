//! Header sections: CBOR maps (RFC 8949) checked against spec §5.4.1.
//!
//! [`decode()`] validates a header and returns its entries. [`encode()`] writes entries in
//! core deterministic encoding. Both work on [`Value`]. A [`Sink`] writes items one at a time
//! instead, for encoders that already hold their fields in key order.
//!
//! # CBOR item heads
//!
//! Every CBOR item starts with a head byte. Its top 3 bits are the major type. Its low 5 bits, the
//! additional information, hold the item's argument or say where to find it (RFC 8949 §3):
//!
//! | Additional information | Argument                                                          |
//! |------------------------|-------------------------------------------------------------------|
//! | 0 to 23                | the value itself, inside the head byte                            |
//! | 24, 25, 26, 27         | in the next 1, 2, 4 or 8 bytes, most significant byte first       |
//! | 28 to 30               | reserved: not well-formed                                         |
//! | 31                     | indefinite length for major types 2 to 5, break for 7, otherwise not well-formed |
//!
//! So 24 is the smallest argument that needs a second byte: `18 18`.
//!
//! What the argument means depends on the major type:
//!
//! | Major type          | Head with info 0 | Argument                          | Example                     |
//! |---------------------|------------------|-----------------------------------|-----------------------------|
//! | 0, unsigned integer | `0x00`           | the value                         | `05` = 5, `19 01 2c` = 300  |
//! | 1, negative integer | `0x20`           | n, for the integer −1 − n         | `25` = −6                   |
//! | 2, byte string      | `0x40`           | length in bytes                   | `42 01 02`                  |
//! | 3, text string      | `0x60`           | length in UTF-8 bytes             | `62 68 69` = `"hi"`         |
//! | 4, array            | `0x80`           | number of items                   | `82 01 02` = `[1, 2]`       |
//! | 5, map              | `0xa0`           | number of key/value pairs         | `a1 01 02` = `{1: 2}`       |
//! | 6, tag              | `0xc0`           | tag number                        | `c1 00`                     |
//! | 7, simple and float | `0xe0`           | simple value; 25, 26 and 27 mean an f16, f32 or f64 follows | `f5` = true, `f9 3e 00` = 1.5 |
//!
//! Simple values 0 to 23 sit in the head (`f4` to `f7` are false, true, null and undefined). Values
//! 32 to 255 follow `f8`. `f8` followed by a byte below 32 is not well-formed (RFC 8949 §3.3).
//!
//! # Pavia's rules
//!
//! Spec §5.4.1 narrows CBOR for headers. The decoder rejects indefinite lengths, tags, duplicate
//! keys, anything other than exactly one map, and headers over the size or depth limit. It accepts
//! any valid argument width, including one longer than needed. The encoder writes core
//! deterministic encoding (RFC 8949 §4.2.1): the shortest argument, definite lengths, the shortest
//! float that keeps the exact value, and the keys of every map sorted by their encoded bytes.

mod decode;
#[cfg_attr(
  not(any(test, fuzzing)),
  expect(
    dead_code,
    reason = "typed frames write headers through Sink; this encoder serves the vectors, the fuzz target and the determinism tests"
  )
)]
mod encode;
mod write;

pub use self::write::{Measure, Out, Sink};
#[cfg_attr(
  not(fuzzing),
  expect(unused_imports, reason = "the typed frame model uses the module paths directly")
)]
pub use self::{
  decode::{DecodeError, DecodeLimits, decode},
  encode::{EncodeError, encode},
};

/// One CBOR data item in a header section (spec §5.4.1).
///
/// Text and byte strings borrow from the decoded input, so decoding copies no string data. Arrays
/// and maps own their items. Integers and floats keep only their value, not the width they were
/// encoded with: the encoder always picks the shortest form again.
#[derive(Clone, Debug, PartialEq)]
pub enum Value<'a> {
  /// An integer from 0 to 2^64 − 1 (major type 0).
  Unsigned(u64),
  /// The integer −1 − n, from −1 down to −2^64 (major type 1).
  Negative(u64),
  /// A byte string (major type 2).
  Bytes(&'a [u8]),
  /// A UTF-8 text string (major type 3).
  Text(&'a str),
  /// An array (major type 4).
  Array(Vec<Value<'a>>),
  /// A map (major type 5), with its entries in the order they were decoded.
  ///
  /// Keys can be any value. The encoder sorts entries by the encoded bytes of their keys.
  Map(Vec<(Value<'a>, Value<'a>)>),
  /// `false` or `true` (simple values 20 and 21).
  Bool(bool),
  /// `null` (simple value 22).
  Null,
  /// `undefined` (simple value 23).
  Undefined,
  /// A simple value with no other meaning in CBOR (major type 7). Values 20 to 23 are
  /// [`Value::Bool`], [`Value::Null`] and [`Value::Undefined`] instead.
  Simple(u8),
  /// A floating-point number, whether it was encoded as f16, f32 or f64.
  Float(f64),
}

#[cfg(test)]
mod tests {
  use super::*;

  fn hex_bytes(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    digits
      .chunks(2)
      .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
      .collect()
  }

  fn error_name(error: &DecodeError) -> &'static str {
    match error {
      DecodeError::TooLarge { .. } => "too_large",
      DecodeError::TooDeep => "too_deep",
      DecodeError::NotAMap => "not_a_map",
      DecodeError::IndefiniteLength => "indefinite_length",
      DecodeError::Malformed => "malformed",
      DecodeError::DuplicateKey => "duplicate_key",
      DecodeError::Tag => "tag",
      DecodeError::TrailingBytes => "trailing_bytes",
    }
  }

  #[test]
  fn matches_the_shared_vectors() {
    for file in [
      include_str!("../../../../vectors/headers/deterministic.json"),
      include_str!("../../../../vectors/headers/invalid.json"),
    ] {
      let doc: serde_json::Value = serde_json::from_str(file).unwrap();
      for case in doc["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let bytes = hex_bytes(case["hex"].as_str().unwrap());
        let defaults = DecodeLimits::default();
        let limit = |key: &str, default: usize| case[key].as_u64().map_or(default, |n| n as usize);
        let limits = DecodeLimits {
          max_size: limit("max_size", defaults.max_size),
          max_depth: limit("max_depth", defaults.max_depth),
        };
        let decoded = decode(&bytes, &limits);
        match case["error"].as_str() {
          Some(expected) => {
            let error = decoded.expect_err(id);
            assert_eq!(error_name(&error), expected, "{id}");
          }
          None => {
            let header = decoded.unwrap_or_else(|e| panic!("{id}: {e}")).expect(id);
            let mut encoded = Vec::new();
            encode(&header, &mut encoded).unwrap_or_else(|e| panic!("{id}: {e}"));
            if case["direction"] == "both" {
              assert_eq!(encoded, bytes, "{id}: encoder output");
            } else {
              assert_ne!(encoded, bytes, "{id}: a decode-only case is what the encoder writes");
            }
          }
        }
      }
    }
  }
}
