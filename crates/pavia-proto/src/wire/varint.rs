use thiserror::Error;

#[derive(Debug)]
pub enum DecodeValue {
  Value { value: u64, consumed_length: usize },
  NeedMoreBytes,
}

#[derive(Debug, Error)]
pub enum EncodeError {
  #[error("the value is above 2^62 - 1")]
  OutOfRange,
}

#[derive(Debug, Error)]
pub enum DecodeError {
  #[error("too big: got {value}, max {max}")]
  AboveMax { value: u64, max: u64 },
}

pub fn encode(content: u64) -> Result<Vec<u8>, EncodeError> {
  let (first_byte, length) = match content {
    n if n <= 0x3F => (0x00u8, 1usize),
    n if n <= 0x3FFF => (0x40u8, 2usize),
    n if n <= 0x3FFF_FFFF => (0x80u8, 4usize),
    n if n <= 0x3FFF_FFFF_FFFF_FFFF => (0xC0u8, 8usize),
    _ => return Err(EncodeError::OutOfRange),
  };

  let mut result = vec![0u8; length];
  result[0] = first_byte;
  for i in 0..length {
    result[length - i - 1] |= (content >> (8 * i)) as u8; // truncation intentional
  }

  Ok(result)
}

pub fn decode(content: &[u8], max: u64) -> Result<DecodeValue, DecodeError> {
  let Some(first_byte) = content.first() else {
    return Ok(DecodeValue::NeedMoreBytes);
  };

  let value_length = 1 << (first_byte >> 6);
  if content.len() < value_length {
    return Ok(DecodeValue::NeedMoreBytes);
  }

  let mut value = u64::from(*first_byte) & 0x3F;
  for partial_value in &content[1..value_length] {
    value = (value << 8) | u64::from(*partial_value);
  }

  if value > max {
    return Err(DecodeError::AboveMax { value, max });
  }

  Ok(DecodeValue::Value {
    value,
    consumed_length: value_length,
  })
}

#[cfg(test)]
mod tests {
  use super::*;
  use proptest::prelude::*;
  use std::assert_matches;
  use std::ops::Sub;

  #[test]
  fn decodes_one_byte_values() {
    let tests = [(0x00, 0), (0x3F, 63)];

    for (input, expected_value) in tests {
      let decoded_value = decode(&[input], u64::MAX);
      let Ok(DecodeValue::Value { value, consumed_length }) = decoded_value else {
        panic!("decode returned unexpected value: {decoded_value:?}");
      };
      assert_eq!(value, expected_value);
      assert_eq!(consumed_length, 1);
    }
  }

  #[test]
  fn round_trips_length_class_boundaries() {
    let tests = [
      0u64,
      63u64,
      64u64,
      16383u64,
      16384u64,
      2u64.pow(30).sub(1),
      2u64.pow(30),
      2u64.pow(62).sub(1),
    ];

    for expected_value in tests {
      let encoded_value = encode(expected_value).unwrap();
      let decoded_result = decode(&encoded_value, u64::MAX);
      let Ok(DecodeValue::Value { value, consumed_length }) = decoded_result else {
        panic!("decode returned unexpected value: {decoded_result:?}");
      };
      assert_eq!(value, expected_value);
      assert_eq!(consumed_length, encoded_value.len());
    }
  }

  #[test]
  fn encodes_shortest_form_at_each_boundary() {
    let tests: [(u64, Vec<u8>); 8] = [
      (0u64, vec![0x00]),
      (63u64, vec![0x3F]),
      (64u64, vec![0x40, 0x40]),
      (16383u64, vec![0x7F, 0xFF]),
      (16384u64, vec![0x80, 0x00, 0x40, 0x00]),
      (2u64.pow(30).sub(1), vec![0xBF, 0xFF, 0xFF, 0xFF]),
      (2u64.pow(30), vec![0xC0, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00]),
      (2u64.pow(62).sub(1), vec![0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]),
    ];

    for (value, expected_encoded_value) in tests {
      let encoded_result = encode(value);
      let Ok(encoded_value) = encoded_result else {
        panic!("encode returned unexpected value: {encoded_result:?}");
      };
      assert_eq!(encoded_value, expected_encoded_value);
    }
  }

  #[test]
  fn decodes_rfc9000_samples() {
    let tests: [(Vec<u8>, u64, usize); 4] = [
      (vec![0xc2, 0x19, 0x7c, 0x5e, 0xff, 0x14, 0xe8, 0x8c], 151288809941952652, 8),
      (vec![0x9d, 0x7f, 0x3e, 0x7d], 494878333, 4),
      (vec![0x7b, 0xbd], 15293, 2),
      (vec![0x25], 37, 1),
    ];

    for (content, expected_value, expected_consumed_length) in tests {
      let decoded_value = decode(&content, u64::MAX);
      let Ok(DecodeValue::Value { value, consumed_length }) = decoded_value else {
        panic!("decode returned unexpected value: {decoded_value:?}");
      };
      assert_eq!(value, expected_value);
      assert_eq!(consumed_length, expected_consumed_length);
    }
  }

  #[test]
  fn reports_consumed_length() {
    let result1 = decode(&[0x25, 0xff], u64::MAX);
    let result2 = decode(&[0x40, 0x25], u64::MAX);
    assert_matches!(
      result1,
      Ok(DecodeValue::Value {
        value: 37,
        consumed_length: 1
      })
    );
    assert_matches!(
      result2,
      Ok(DecodeValue::Value {
        value: 37,
        consumed_length: 2
      })
    );
  }

  #[test]
  fn decodes_non_shortest_encoding() {
    let result = decode(&[0x40, 0x25], u64::MAX);
    assert_matches!(
      result,
      Ok(DecodeValue::Value {
        value: 37,
        consumed_length: 2
      })
    );
  }

  #[test]
  fn empty_input_needs_more_bytes() {
    let result = decode(&[], u64::MAX);
    assert_matches!(result, Ok(DecodeValue::NeedMoreBytes));
  }

  #[test]
  fn every_truncated_prefix_needs_more_bytes() {
    let tests = [
      vec![0x40],
      vec![0x80, 0x25, 0xe4],
      vec![0xC0, 0x25, 0xf1, 0xc7, 0xa2, 0xe5, 0xcc],
    ];

    for test in tests {
      for i in 1..=test.len() {
        let prefix = &test[..i];
        let result = decode(prefix, u64::MAX);
        assert_matches!(
          result,
          Ok(DecodeValue::NeedMoreBytes),
          "prefix {prefix:02x?} (length {i}) should need more bytes"
        );
      }
    }
  }

  #[test]
  fn rejects_value_above_max() {
    let result = decode(&[0x80, 0x00, 0x40, 0x00], 16383);
    assert_matches!(
      result,
      Err(DecodeError::AboveMax {
        max: 16383,
        value: 16384
      })
    );
  }

  #[test]
  fn accepts_value_equal_to_max() {
    let result = decode(&[0x40, 0xcc], 204);
    assert_matches!(
      result,
      Ok(DecodeValue::Value {
        value: 204,
        consumed_length: 2
      })
    );
  }

  #[test]
  fn encode_rejects_2_pow_62() {
    let result = encode(2u64.pow(62));
    assert_matches!(result, Err(EncodeError::OutOfRange))
  }

  #[test]
  fn encode_rejects_u64_max() {
    let result = encode(u64::MAX);
    assert_matches!(result, Err(EncodeError::OutOfRange))
  }

  #[test]
  fn round_trips_every_power_of_two() {
    for exp in 0..62 {
      let number = 2u64.pow(exp);
      let decoded_value = decode(&encode(number).unwrap(), u64::MAX);
      let Ok(DecodeValue::Value { value, .. }) = decoded_value else {
        panic!("decode returned unexpected value: {decoded_value:?}");
      };
      assert_eq!(value, number);
    }
  }

  #[test]
  fn encodes_every_power_of_two_in_shortest_form() {
    for exp in 0..62 {
      let number = 2u64.pow(exp);
      let encoded_value = encode(number).unwrap();
      let expected_length = match number {
        n if n <= 0x3F => 1usize,
        n if n <= 0x3FFF => 2usize,
        n if n <= 0x3FFF_FFFF => 4usize,
        n if n <= 0x3FFF_FFFF_FFFF_FFFF => 8usize,
        _ => unreachable!(),
      };
      assert_eq!(
        encoded_value.len(),
        expected_length,
        "The exponent {exp} didn't encode the shortest"
      );
    }
  }

  fn hex_bytes(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    digits
      .chunks(2)
      .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
      .collect()
  }

  #[test]
  fn matches_the_shared_vectors() {
    let doc: serde_json::Value = serde_json::from_str(include_str!("../../../../vectors/varints/boundaries.json")).unwrap();
    for case in doc["cases"].as_array().unwrap() {
      let id = case["id"].as_str().unwrap();
      let number = |key: &str| case[key].as_str().map(|text| text.parse::<u64>().unwrap());
      let bytes = case["hex"].as_str().map(hex_bytes);
      let max = number("max").unwrap_or(u64::MAX);
      match (bytes, number("value"), case["error"].as_str()) {
        (Some(bytes), Some(expected), None) => {
          assert_matches!(
            decode(&bytes, max),
            Ok(DecodeValue::Value { value, consumed_length }) if value == expected && consumed_length == bytes.len(),
            "{id}"
          );
          if case["direction"] == "both" {
            assert_eq!(encode(expected).unwrap(), bytes, "{id}");
          }
        }
        (Some(bytes), None, None) => assert_matches!(decode(&bytes, max), Ok(DecodeValue::NeedMoreBytes), "{id}"),
        (Some(bytes), None, Some("above_max")) => {
          assert_matches!(decode(&bytes, max), Err(DecodeError::AboveMax { .. }), "{id}")
        }
        (None, Some(value), Some("out_of_range")) => {
          assert_matches!(encode(value), Err(EncodeError::OutOfRange), "{id}")
        }
        _ => panic!("{id}: unknown case shape"),
      }
    }
  }

  const MAX_VARINT: u64 = (1 << 62) - 1;

  fn shortest_length(value: u64) -> usize {
    match value {
      0..=0x3F => 1,
      0x40..=0x3FFF => 2,
      0x4000..=0x3FFF_FFFF => 4,
      _ => 8,
    }
  }

  fn any_varint() -> impl Strategy<Value = u64> {
    prop_oneof![
      0..=0x3Fu64,
      0x40..=0x3FFFu64,
      0x4000..=0x3FFF_FFFFu64,
      0x4000_0000..=MAX_VARINT,
    ]
  }

  proptest! {
    #[test]
    fn round_trips_any_value(value in any_varint()) {
      let encoded = encode(value).expect("value is in range");
      let decoded = decode(&encoded, u64::MAX);
      prop_assert!(
        matches!(decoded, Ok(DecodeValue::Value { value: v, consumed_length: n }) if v == value && n == encoded.len()),
        "{value} encoded as {encoded:02x?} decoded to {decoded:?}"
      );
    }

    #[test]
    fn encodes_any_value_in_shortest_form(value in any_varint()) {
      let encoded = encode(value).expect("value is in range");
      prop_assert_eq!(encoded.len(), shortest_length(value), "{} encoded as {:02x?}", value, encoded);
    }

    #[test]
    fn refuses_any_value_above_range(value in (MAX_VARINT + 1)..=u64::MAX) {
      prop_assert!(matches!(encode(value), Err(EncodeError::OutOfRange)));
    }

    #[test]
    fn decode_handles_any_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..16), max in any::<u64>()) {
      if let Ok(DecodeValue::Value { value, consumed_length }) = decode(&bytes, max) {
        prop_assert!(consumed_length <= bytes.len());
        prop_assert!(value <= max);
      }
    }
  }
}
