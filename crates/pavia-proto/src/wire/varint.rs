use thiserror::Error;

#[derive(Debug)]
pub enum DecodeValue {
  Value { value: u64, consumed_length: usize },
  NeedMoreBytes,
}

#[derive(Debug, Error)]
pub enum VarIntError {
  #[error("too big: got {value}, max {max}")]
  AboveMax { value: u64, max: u64 },
  #[error("the value is above 2^62 - 1")]
  OutOfRange,
}

pub fn encode(content: u64) -> Result<Vec<u8>, VarIntError> {
  todo!();
}

pub fn decode(content: &[u8], max: u64) -> Result<DecodeValue, VarIntError> {
  todo!();
}

#[cfg(test)]
mod tests {
  use super::*;

  // Round trips and shortest form (#9 criterion 1, spec 5.1).

  /// 0 and 63 are the smallest and largest values that fit in one byte.
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

  /// 0, 63, 64, 16383, 16384, 2^30-1, 2^30, 2^62-1 survive encode then decode.
  #[test]
  fn round_trips_length_class_boundaries() {
    todo!()
  }

  /// Each boundary value encodes to the shortest length: 63 -> 1 byte, 64 -> 2 bytes, ...
  #[test]
  fn encodes_shortest_form_at_each_boundary() {
    todo!()
  }

  /// RFC 9000 appendix A.1 samples, e.g. `c2 19 7c 5e ff 14 e8 8c` -> 151288809941952652.
  #[test]
  fn decodes_rfc9000_samples() {
    todo!()
  }

  // Consumed length.

  /// `[0x25]` consumes 1 byte, `[0x40, 0x25]` consumes 2; trailing bytes are not consumed.
  #[test]
  fn reports_consumed_length() {
    todo!()
  }

  // Non-shortest input (#9 criterion 4).

  /// `40 25` is a valid, non-shortest encoding of 37 and must decode.
  #[test]
  fn decodes_non_shortest_encoding() {
    todo!()
  }

  // Truncated input (#9 criterion 3).

  /// An empty slice needs more bytes.
  #[test]
  fn empty_input_needs_more_bytes() {
    todo!()
  }

  /// Every proper prefix of a multi-byte varint needs more bytes and is not an error.
  #[test]
  fn every_truncated_prefix_needs_more_bytes() {
    todo!()
  }

  // Limit on decode (#9 criterion 2).

  /// `80 00 40 00` (16384) with max 16383 is refused with `AboveMax`.
  #[test]
  fn rejects_value_above_max() {
    todo!()
  }

  /// A value equal to max is accepted: the limit is inclusive.
  #[test]
  fn accepts_value_equal_to_max() {
    todo!()
  }

  // Encode range (#9 criterion 5).

  /// 2^62 has no varint form and is refused with `OutOfRange`.
  #[test]
  fn encode_rejects_2_pow_62() {
    todo!()
  }

  /// u64::MAX is refused too.
  #[test]
  fn encode_rejects_u64_max() {
    todo!()
  }

  // Property tests (#9 tests section). These need a property-testing crate such as `proptest`.

  /// For any v < 2^62, decode(encode(v)) == v.
  #[test]
  fn property_round_trip_any_value() {
    todo!()
  }

  /// For any v < 2^62, encode(v) has the shortest length for v.
  #[test]
  fn property_encode_is_shortest() {
    todo!()
  }
}
