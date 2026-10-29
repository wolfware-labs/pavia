use proptest::prelude::*;

use super::{Codec, DecodeError, DecodeLimits, EncodeError, Value, encode};
use crate::error::{DataSection, ErrorStatus, Outcome, ProtocolError};
use crate::registry::StatusCode;

const MAX_DEPTH: usize = 16;

fn decode(data: &[u8], codec: Codec) -> Result<Value, DecodeError> {
  super::decode(data, codec, &DecodeLimits::default())
}

fn hex(text: &str) -> Vec<u8> {
  text
    .split_whitespace()
    .map(|byte| u8::from_str_radix(byte, 16).unwrap())
    .collect()
}

fn text(value: impl Into<String>) -> Value {
  Value::Text(value.into())
}

fn encoded(value: &Value, codec: Codec) -> Vec<u8> {
  let mut out = Vec::new();
  encode(value, codec, &mut out).unwrap();
  out
}

fn assert_cbor(value: &Value, expected: &str) {
  assert_eq!(encoded(value, Codec::Cbor), hex(expected), "{value:?}");
  assert_eq!(decode(&hex(expected), Codec::Cbor).as_ref(), Ok(value), "{expected}");
}

fn assert_json(value: &Value, expected: &str) {
  assert_eq!(String::from_utf8(encoded(value, Codec::Json)).unwrap(), expected, "{value:?}");
  assert_eq!(decode(expected.as_bytes(), Codec::Json).as_ref(), Ok(value), "{expected}");
}

#[test]
fn round_trips_the_call_arguments_of_appendix_a() {
  let arguments = Value::Array(vec![text("general"), text("hello")]);
  assert_cbor(&arguments, "82 67 67 65 6e 65 72 61 6c 65 68 65 6c 6c 6f");
  assert_json(&arguments, r#"["general","hello"]"#);
}

#[test]
fn matches_the_primitive_vectors_in_both_codecs() {
  let cases = [
    (Value::Null, "f6", "null"),
    (Value::Bool(true), "f5", "true"),
    (Value::Bool(false), "f4", "false"),
    (Value::Unsigned(0), "00", "0"),
    (Value::Negative(0), "20", "-1"),
    (Value::Unsigned(255), "18 ff", "255"),
    (Value::Float(1.5), "f9 3e 00", "1.5"),
    (text("text"), "64 74 65 78 74", r#""text""#),
    (
      Value::Array(vec![Value::Unsigned(1), Value::Unsigned(2)]),
      "82 01 02",
      "[1,2]",
    ),
    (Value::Map(vec![(text("a"), Value::Unsigned(1))]), "a1 61 61 01", r#"{"a":1}"#),
  ];
  for (value, cbor, json) in cases {
    assert_cbor(&value, cbor);
    assert_json(&value, json);
  }
  assert_cbor(&Value::Bytes(vec![1, 2]), "42 01 02");
}

#[test]
fn writes_the_shortest_cbor_forms() {
  assert_cbor(&Value::Float(1.5), "f9 3e 00");
  assert_cbor(&Value::Float(100_000.0), "fa 47 c3 50 00");
  assert_cbor(&Value::Float(1.1), "fb 3f f1 99 99 99 99 99 9a");
  assert_cbor(&Value::Unsigned(24), "18 18");
  assert_cbor(&Value::Unsigned(23), "17");
  let long = "a".repeat(24);
  let mut expected = vec![0x78, 0x18];
  expected.extend(long.as_bytes());
  assert_eq!(encoded(&text(long), Codec::Cbor), expected);
}

#[test]
fn writes_numbers_and_strings_as_rfc_8785_does() {
  let cases = [
    (Value::Float(1e21), "1e+21"),
    (Value::Float(-0.0), "0"),
    (Value::Float(0.000_001), "0.000001"),
    (text("\u{1f}"), r#""\u001f""#),
    (text("/é"), "\"/é\""),
    (text("\"\\\u{08}\u{0c}\n\r\t"), r#""\"\\\b\f\n\r\t""#),
    (Value::Negative(u64::MAX), "-18446744073709551616"),
  ];
  for (value, expected) in cases {
    assert_eq!(
      String::from_utf8(encoded(&value, Codec::Json)).unwrap(),
      expected,
      "{value:?}"
    );
  }
}

#[test]
fn keeps_map_entries_in_value_order() {
  let map = Value::Map(vec![(text("b"), Value::Unsigned(1)), (text("a"), Value::Unsigned(2))]);
  assert_cbor(&map, "a2 61 62 01 61 61 02");
  assert_json(&map, r#"{"b":1,"a":2}"#);
}

#[test]
fn rejects_invalid_json() {
  let cases: [(&[u8], DecodeError); 5] = [
    (b"[\"\xff\"]", DecodeError::InvalidUtf8),
    (b"[NaN]", DecodeError::Malformed),
    (b"[Infinity]", DecodeError::Malformed),
    (br#"{"a": 1, "a": 2}"#, DecodeError::DuplicateKey),
    (b"[1] [2]", DecodeError::TrailingBytes),
  ];
  for (input, expected) in cases {
    assert_eq!(
      decode(input, Codec::Json),
      Err(expected),
      "{}",
      String::from_utf8_lossy(input)
    );
  }
}

#[test]
fn reports_undecodable_call_arguments_as_invalid_argument() {
  assert!(decode(b"[NaN]", Codec::Json).is_err());
  assert_eq!(
    ProtocolError::UndecodableData(DataSection::CallArguments).outcome(),
    Outcome::PerCall {
      status: ErrorStatus::try_from(StatusCode::InvalidArgument).unwrap(),
      nx: true
    }
  );
}

#[test]
fn reads_every_cbor_float_width() {
  for input in ["f9 3e 00", "fa 3f c0 00 00", "fb 3f f8 00 00 00 00 00 00"] {
    assert_eq!(decode(&hex(input), Codec::Cbor), Ok(Value::Float(1.5)), "{input}");
  }
}

#[test]
fn reads_any_valid_cbor_encoding() {
  let cases = [
    ("18 01", Value::Unsigned(1)),
    ("1b ff ff ff ff ff ff ff ff", Value::Unsigned(u64::MAX)),
    ("3b ff ff ff ff ff ff ff ff", Value::Negative(u64::MAX)),
    ("9f 01 02 ff", Value::Array(vec![Value::Unsigned(1), Value::Unsigned(2)])),
    ("bf 61 61 01 ff", Value::Map(vec![(text("a"), Value::Unsigned(1))])),
    ("7f 61 61 61 62 ff", text("ab")),
    ("5f 41 01 41 02 ff", Value::Bytes(vec![1, 2])),
    ("c1 1a 68 d0 a4 80", Value::Tag(1, Box::new(Value::Unsigned(1_758_504_064)))),
    ("f7", Value::Undefined),
    ("f8 20", Value::Simple(32)),
  ];
  for (input, expected) in cases {
    assert_eq!(decode(&hex(input), Codec::Cbor), Ok(expected), "{input}");
  }
}

#[test]
fn rejects_cbor_that_is_not_exactly_one_item() {
  let cases = [
    ("", DecodeError::Malformed),
    ("01 02", DecodeError::TrailingBytes),
    ("82 01", DecodeError::Malformed),
    ("ff", DecodeError::Malformed),
    ("9f 01", DecodeError::Malformed),
    ("f8 18", DecodeError::Malformed),
    ("1c", DecodeError::Malformed),
    ("62 ff fe", DecodeError::Malformed),
  ];
  for (input, expected) in cases {
    assert_eq!(decode(&hex(input), Codec::Cbor), Err(expected), "{input}");
  }
}

#[test]
fn refuses_values_a_codec_cannot_carry() {
  for value in [
    Value::Bytes(vec![1]),
    Value::Tag(1, Box::new(Value::Unsigned(0))),
    Value::Undefined,
    Value::Simple(32),
    Value::Float(f64::NAN),
    Value::Float(f64::INFINITY),
    Value::Map(vec![(Value::Unsigned(1), Value::Null)]),
  ] {
    let mut out = Vec::new();
    assert_eq!(encode(&value, Codec::Json, &mut out), Err(EncodeError::NotJson), "{value:?}");
    assert!(out.is_empty());
  }
  for simple in 20..=31 {
    let mut out = Vec::new();
    let nested = Value::Array(vec![Value::Simple(simple)]);
    assert_eq!(encode(&nested, Codec::Cbor, &mut out), Err(EncodeError::ReservedSimpleValue));
    assert!(out.is_empty());
  }
}

fn nested_cbor(depth: usize) -> Vec<u8> {
  let mut bytes = vec![0x81; depth - 1];
  bytes.push(0x80);
  bytes
}

fn nested_json(depth: usize) -> String {
  format!("{}{}", "[".repeat(depth), "]".repeat(depth))
}

#[test]
fn limits_nesting_depth_in_both_codecs() {
  assert!(decode(&nested_cbor(MAX_DEPTH), Codec::Cbor).is_ok());
  assert_eq!(decode(&nested_cbor(MAX_DEPTH + 1), Codec::Cbor), Err(DecodeError::TooDeep));
  assert!(decode(nested_json(MAX_DEPTH).as_bytes(), Codec::Json).is_ok());
  assert_eq!(
    decode(nested_json(MAX_DEPTH + 1).as_bytes(), Codec::Json),
    Err(DecodeError::TooDeep)
  );
  let mut tags = vec![0xc1; MAX_DEPTH + 1];
  tags.push(0x00);
  assert_eq!(decode(&tags, Codec::Cbor), Err(DecodeError::TooDeep));
  assert_eq!(decode(&vec![0x81; 100_000], Codec::Cbor), Err(DecodeError::TooDeep));
  assert_eq!(decode("[".repeat(100_000).as_bytes(), Codec::Json), Err(DecodeError::TooDeep));
}

#[test]
fn takes_the_depth_limit_from_the_caller() {
  let limits = DecodeLimits { max_depth: 2 };
  assert!(super::decode(&nested_cbor(2), Codec::Cbor, &limits).is_ok());
  assert_eq!(
    super::decode(&nested_cbor(3), Codec::Cbor, &limits),
    Err(DecodeError::TooDeep)
  );
  assert!(super::decode(nested_json(2).as_bytes(), Codec::Json, &limits).is_ok());
  assert_eq!(
    super::decode(nested_json(3).as_bytes(), Codec::Json, &limits),
    Err(DecodeError::TooDeep)
  );
  assert_eq!(DecodeLimits::default().max_depth, MAX_DEPTH);
}

#[test]
fn names_the_codecs_as_the_handshake_does() {
  for codec in [Codec::Cbor, Codec::Json] {
    assert_eq!(Codec::from_name(codec.name()), Some(codec));
  }
  assert_eq!(Codec::from_name("msgpack"), None);
}

fn any_json_value() -> impl Strategy<Value = Value> {
  let leaf = prop_oneof![
    Just(Value::Null),
    any::<bool>().prop_map(Value::Bool),
    any::<u64>().prop_map(Value::Unsigned),
    (0..=i64::MAX as u64).prop_map(Value::Negative),
    any::<f64>()
      .prop_filter("finite, not integral", |x| x.is_finite() && x.fract() != 0.0)
      .prop_map(Value::Float),
    ".*".prop_map(Value::Text),
  ];
  leaf.prop_recursive(4, 32, 4, |inner| {
    prop_oneof![
      prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
      prop::collection::btree_map(".*", inner, 0..4)
        .prop_map(|entries| Value::Map(entries.into_iter().map(|(key, value)| (Value::Text(key), value)).collect())),
    ]
  })
}

fn any_cbor_value() -> impl Strategy<Value = Value> {
  let leaf = prop_oneof![
    Just(Value::Null),
    Just(Value::Undefined),
    any::<bool>().prop_map(Value::Bool),
    any::<u64>().prop_map(Value::Unsigned),
    any::<u64>().prop_map(Value::Negative),
    any::<f64>().prop_filter("not NaN", |x| !x.is_nan()).prop_map(Value::Float),
    ".*".prop_map(Value::Text),
    prop::collection::vec(any::<u8>(), 0..8).prop_map(Value::Bytes),
    prop_oneof![0_u8..20, 32_u8..=255].prop_map(Value::Simple),
  ];
  leaf.prop_recursive(4, 32, 4, |inner| {
    prop_oneof![
      prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
      prop::collection::vec((inner.clone(), inner.clone()), 0..4).prop_map(Value::Map),
      (any::<u64>(), inner).prop_map(|(tag, item)| Value::Tag(tag, Box::new(item))),
    ]
  })
}

proptest! {
  #[test]
  fn decodes_what_it_encodes_in_cbor(value in any_cbor_value()) {
    let bytes = encoded(&value, Codec::Cbor);
    prop_assert_eq!(decode(&bytes, Codec::Cbor), Ok(value));
  }

  #[test]
  fn decodes_what_it_encodes_in_json(value in any_json_value()) {
    let bytes = encoded(&value, Codec::Json);
    prop_assert_eq!(decode(&bytes, Codec::Json), Ok(value));
  }

  #[test]
  fn never_panics_on_arbitrary_input(bytes in prop::collection::vec(any::<u8>(), 0..64)) {
    let _ = decode(&bytes, Codec::Cbor);
    let _ = decode(&bytes, Codec::Json);
  }
}
