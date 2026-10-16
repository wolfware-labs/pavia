use super::Value;
use bytes::BufMut;
use half::f16;
use thiserror::Error;

const UNSIGNED: u8 = 0;
const NEGATIVE: u8 = 1;
const BYTES: u8 = 2;
const TEXT: u8 = 3;
const ARRAY: u8 = 4;
const MAP: u8 = 5;
const SIMPLE: u8 = 7;

/// Error returned by [`encode`].
#[derive(Debug, Error)]
pub enum EncodeError {
  /// A [`Value::Simple`] from 24 to 31. CBOR reserves these, and no well-formed encoding exists for
  /// them.
  #[error("simple value {0} is reserved and cannot be encoded")]
  ReservedSimpleValue(u8),
  /// Two keys of the same map encode to the same bytes.
  #[error("header has a duplicate key")]
  DuplicateKey,
}

/// Writes `header` to `buffer` as a CBOR map in core deterministic encoding (spec §5.4.1, RFC 8949
/// §4.2.1): definite lengths, shortest integer and float forms, and the keys of every map sorted by
/// their encoded bytes.
///
/// An empty slice writes `a0`, an empty map. A frame without a header has HeaderLength 0 and does
/// not call `encode` at all.
///
/// # Errors
///
/// Returns [`EncodeError::DuplicateKey`] if two keys of the same map encode to the same bytes, and
/// [`EncodeError::ReservedSimpleValue`] for a simple value from 24 to 31. `buffer` may already hold
/// part of the header when an error is returned.
pub fn encode(header: &[(Value<'_>, Value<'_>)], buffer: &mut impl BufMut) -> Result<(), EncodeError> {
  write_map(header, buffer)
}

fn write_value(value: &Value<'_>, buffer: &mut impl BufMut) -> Result<(), EncodeError> {
  match *value {
    Value::Unsigned(n) => write_head(UNSIGNED, n, buffer),
    Value::Negative(n) => write_head(NEGATIVE, n, buffer),
    Value::Bytes(bytes) => {
      write_head(BYTES, bytes.len() as u64, buffer);
      buffer.put_slice(bytes);
    }
    Value::Text(text) => {
      write_head(TEXT, text.len() as u64, buffer);
      buffer.put_slice(text.as_bytes());
    }
    Value::Array(ref items) => write_array(items, buffer)?,
    Value::Map(ref entries) => write_map(entries, buffer)?,
    Value::Bool(false) => buffer.put_u8(0xf4),
    Value::Bool(true) => buffer.put_u8(0xf5),
    Value::Null => buffer.put_u8(0xf6),
    Value::Undefined => buffer.put_u8(0xf7),
    Value::Simple(n @ 24..32) => return Err(EncodeError::ReservedSimpleValue(n)),
    Value::Simple(n) => write_head(SIMPLE, u64::from(n), buffer),
    Value::Float(x) => write_float(x, buffer),
  }

  Ok(())
}

fn write_head(major_type: u8, argument: u64, buffer: &mut impl BufMut) {
  let major_type = major_type << 5;
  match argument {
    0..24 => buffer.put_u8(major_type | argument as u8),
    24..0x100 => {
      buffer.put_u8(major_type | 24);
      buffer.put_u8(argument as u8);
    }
    0x100..0x1_0000 => {
      buffer.put_u8(major_type | 25);
      buffer.put_u16(argument as u16);
    }
    0x1_0000..0x1_0000_0000 => {
      buffer.put_u8(major_type | 26);
      buffer.put_u32(argument as u32);
    }
    _ => {
      buffer.put_u8(major_type | 27);
      buffer.put_u64(argument);
    }
  }
}

fn write_float(x: f64, buffer: &mut impl BufMut) {
  if x.is_nan() {
    buffer.put_slice(&[0xf9, 0x7e, 0x00]);
    return;
  }

  let half = f16::from_f64(x);
  if half.to_f64().to_bits() == x.to_bits() {
    buffer.put_u8(0xf9);
    buffer.put_u16(half.to_bits());
    return;
  }

  let single = x as f32;
  if f64::from(single).to_bits() == x.to_bits() {
    buffer.put_u8(0xfa);
    buffer.put_u32(single.to_bits());
    return;
  }

  buffer.put_u8(0xfb);
  buffer.put_u64(x.to_bits());
}

fn write_array(items: &[Value<'_>], buffer: &mut impl BufMut) -> Result<(), EncodeError> {
  write_head(ARRAY, items.len() as u64, buffer);
  for item in items {
    write_value(item, buffer)?;
  }
  Ok(())
}

fn write_map(entries: &[(Value<'_>, Value<'_>)], buffer: &mut impl BufMut) -> Result<(), EncodeError> {
  let mut encoded = Vec::with_capacity(entries.len());
  for (key, value) in entries {
    let mut encoded_key = vec![];
    write_value(key, &mut encoded_key)?;
    encoded.push((encoded_key, value));
  }

  encoded.sort_unstable_by(|(k1, _), (k2, _)| k1.cmp(k2));

  if encoded.array_windows().any(|[(k1, _), (k2, _)]| k1 == k2) {
    return Err(EncodeError::DuplicateKey);
  }

  write_head(MAP, entries.len() as u64, buffer);

  for (key, value) in encoded {
    buffer.put_slice(&key);
    write_value(value, buffer)?;
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::wire::header::{DecodeLimits, decode};
  use proptest::prelude::*;
  use std::assert_matches;

  #[test]
  fn encodes_same_header_to_identical_bytes() {
    let header = [
      (Value::Text("id"), Value::Unsigned(7)),
      (Value::Unsigned(1), Value::Bool(true)),
      (Value::Negative(0), Value::Null),
    ];
    let reordered = [header[2].clone(), header[0].clone(), header[1].clone()];
    assert_eq!(encoded(&header), encoded(&reordered));
  }

  #[test]
  fn rejects_duplicate_keys() {
    let header = [
      (Value::Unsigned(2), Value::Null),
      (Value::Unsigned(1), Value::Null),
      (Value::Unsigned(0), Value::Null),
      (Value::Unsigned(1), Value::Null),
    ];
    assert_matches!(encode(&header, &mut Vec::new()), Err(EncodeError::DuplicateKey));
  }

  #[test]
  fn rejects_duplicate_keys_in_nested_maps() {
    let nested = Value::Map(vec![(Value::Text("a"), Value::Null), (Value::Text("a"), Value::Null)]);
    let header = [(Value::Unsigned(0), nested)];
    assert_matches!(encode(&header, &mut Vec::new()), Err(EncodeError::DuplicateKey));
  }

  #[test]
  fn encodes_integer_keys_in_sorted_order() {
    let header = [
      (Value::Unsigned(10), Value::Unsigned(0)),
      (Value::Unsigned(1), Value::Unsigned(1)),
      (Value::Negative(0), Value::Unsigned(2)),
      (Value::Unsigned(100), Value::Unsigned(3)),
    ];
    assert_eq!(encoded(&header), [0xa4, 0x01, 0x01, 0x0a, 0x00, 0x18, 0x64, 0x03, 0x20, 0x02]);
  }

  #[test]
  fn encodes_text_keys_by_encoded_bytes() {
    let header = [
      (Value::Text("aa"), Value::Unsigned(2)),
      (Value::Text("b"), Value::Unsigned(1)),
    ];
    assert_eq!(encoded(&header), [0xa2, 0x61, 0x62, 0x01, 0x62, 0x61, 0x61, 0x02]);
  }

  #[test]
  fn encodes_shortest_integer_forms() {
    let cases: [(Value<'_>, &[u8]); 10] = [
      (Value::Unsigned(0), &[0x00]),
      (Value::Unsigned(23), &[0x17]),
      (Value::Unsigned(24), &[0x18, 0x18]),
      (Value::Unsigned(0xff), &[0x18, 0xff]),
      (Value::Unsigned(0x100), &[0x19, 0x01, 0x00]),
      (Value::Unsigned(0xffff), &[0x19, 0xff, 0xff]),
      (Value::Unsigned(0x1_0000), &[0x1a, 0x00, 0x01, 0x00, 0x00]),
      (
        Value::Unsigned(0x1_0000_0000),
        &[0x1b, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00],
      ),
      (Value::Negative(0), &[0x20]),
      (
        Value::Negative(u64::MAX),
        &[0x3b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
      ),
    ];
    for (value, expected) in cases {
      assert_eq!(written(&value), expected, "{value:?}");
    }
  }

  #[test]
  fn encodes_shortest_exact_float_forms() {
    let cases: [(f64, &[u8]); 8] = [
      (1.5, &[0xf9, 0x3e, 0x00]),
      (0.0, &[0xf9, 0x00, 0x00]),
      (-0.0, &[0xf9, 0x80, 0x00]),
      (f64::INFINITY, &[0xf9, 0x7c, 0x00]),
      (f64::NAN, &[0xf9, 0x7e, 0x00]),
      (100_000.0, &[0xfa, 0x47, 0xc3, 0x50, 0x00]),
      (0.1, &[0xfb, 0x3f, 0xb9, 0x99, 0x99, 0x99, 0x99, 0x99, 0x9a]),
      (65_504.0, &[0xf9, 0x7b, 0xff]),
    ];
    for (x, expected) in cases {
      assert_eq!(written(&Value::Float(x)), expected, "{x}");
    }
  }

  #[test]
  fn encodes_strings_and_simple_values() {
    let cases: [(Value<'_>, &[u8]); 8] = [
      (Value::Text("hi"), &[0x62, 0x68, 0x69]),
      (Value::Bytes(&[0x01]), &[0x41, 0x01]),
      (Value::Bool(false), &[0xf4]),
      (Value::Bool(true), &[0xf5]),
      (Value::Null, &[0xf6]),
      (Value::Undefined, &[0xf7]),
      (Value::Simple(16), &[0xf0]),
      (Value::Simple(32), &[0xf8, 0x20]),
    ];
    for (value, expected) in cases {
      assert_eq!(written(&value), expected, "{value:?}");
    }
  }

  #[test]
  fn rejects_reserved_simple_values() {
    for n in 24..32 {
      let mut buffer = Vec::new();
      let result = write_value(&Value::Simple(n), &mut buffer);
      assert_matches!(result, Err(EncodeError::ReservedSimpleValue(value)) if value == n);
    }
  }

  #[test]
  fn writes_long_text_with_a_two_byte_length() {
    let text = "a".repeat(300);
    let written = written(&Value::Text(&text));
    assert_eq!(written[..3], [0x79, 0x01, 0x2c]);
    assert_eq!(written.len(), 303);
  }

  proptest! {
    #[test]
    fn round_trips_any_header(header in proptest::collection::vec((any_value(), any_value()), 0..6)) {
      let mut first = Vec::new();
      prop_assume!(encode(&header, &mut first).is_ok());
      let decoded = decode(&first, &DecodeLimits::default()).unwrap().unwrap();
      prop_assert_eq!(encoded(&decoded), first);
    }
  }

  fn any_value() -> impl Strategy<Value = Value<'static>> {
    let leaf = prop_oneof![
      any::<u64>().prop_map(Value::Unsigned),
      any::<u64>().prop_map(Value::Negative),
      proptest::collection::vec(any::<u8>(), 0..8).prop_map(|bytes| Value::Bytes(bytes.leak())),
      ".{0,8}".prop_map(|text| Value::Text(text.leak())),
      any::<bool>().prop_map(Value::Bool),
      Just(Value::Null),
      Just(Value::Undefined),
      prop_oneof![0..20_u8, 32..=255_u8].prop_map(Value::Simple),
      any::<f64>().prop_map(Value::Float),
    ];
    leaf.prop_recursive(3, 32, 4, |inner| {
      prop_oneof![
        proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
        proptest::collection::vec((inner.clone(), inner), 0..4).prop_map(Value::Map),
      ]
    })
  }

  fn written(value: &Value<'_>) -> Vec<u8> {
    let mut buffer = Vec::new();
    write_value(value, &mut buffer).unwrap();
    buffer
  }

  fn encoded(header: &[(Value<'_>, Value<'_>)]) -> Vec<u8> {
    let mut buffer = Vec::new();
    encode(header, &mut buffer).unwrap();
    buffer
  }
}
