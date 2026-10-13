use minicbor::Decoder;
use minicbor::data::{Int, Type};
use std::collections::HashSet;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DecodeError {
  #[error("header is {actual_size} bytes, limit is {max_size}")]
  TooLarge { actual_size: usize, max_size: usize },
  #[error("header is nested deeper than the depth limit")]
  TooDeep,
  #[error("header is not a CBOR map")]
  NotAMap,
  #[error("header contains an indefinite-length item")]
  IndefiniteLength,
  #[error("header is not well-formed CBOR")]
  Malformed,
  #[error("header has a duplicate key")]
  DuplicateKey,
  #[error("header contains a CBOR tag")]
  Tag,
  #[error("header has bytes after the map")]
  TrailingBytes,
}

pub struct DecodeLimits {
  pub max_size: usize,
  pub max_depth: usize,
}

impl Default for DecodeLimits {
  fn default() -> Self {
    Self {
      max_size: 1024 * 16,
      max_depth: 16,
    }
  }
}

#[derive(Debug, Eq, PartialEq, Hash)]
enum Key<'a> {
  Integer(Int),
  Text(&'a str),
  Bytes(&'a [u8]),
  Raw(&'a [u8]),
}

pub fn decode(content: &[u8], limits: &DecodeLimits) -> Result<(), DecodeError> {
  if content.is_empty() {
    return Ok(());
  }

  if content.len() > limits.max_size {
    return Err(DecodeError::TooLarge {
      actual_size: content.len(),
      max_size: limits.max_size,
    });
  }
  let mut decoder = Decoder::new(content);
  match decoder.datatype().map_err(|_| DecodeError::Malformed)? {
    Type::Map => {
      walk_map(&mut decoder, 1, limits.max_depth)?;
      if decoder.position() < content.len() {
        Err(DecodeError::TrailingBytes)
      } else {
        Ok(())
      }
    }
    Type::MapIndef => Err(DecodeError::IndefiniteLength),
    _ => Err(DecodeError::NotAMap),
  }
}

fn walk_map(decoder: &mut Decoder, current_depth: usize, max_depth: usize) -> Result<(), DecodeError> {
  let mut keys = HashSet::new();
  for _ in 0..decoder
    .map()
    .map_err(|_| DecodeError::Malformed)?
    .ok_or(DecodeError::IndefiniteLength)?
  {
    if !keys.insert(walk_key(decoder, current_depth, max_depth)?) {
      return Err(DecodeError::DuplicateKey);
    }

    walk_value(decoder, current_depth, max_depth)?;
  }

  Ok(())
}

fn walk_key<'a>(decoder: &mut Decoder<'a>, current_depth: usize, max_depth: usize) -> Result<Key<'a>, DecodeError> {
  match check_type(decoder)? {
    Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::Int => {
      Ok(Key::Integer(decoder.int().map_err(|_| DecodeError::Malformed)?))
    }
    Type::String => Ok(Key::Text(decoder.str().map_err(|_| DecodeError::Malformed)?)),
    Type::Bytes => Ok(Key::Bytes(decoder.bytes().map_err(|_| DecodeError::Malformed)?)),
    _ => {
      let start = decoder.position();
      walk_value(decoder, current_depth, max_depth)?;
      Ok(Key::Raw(
        decoder.input().get(start..decoder.position()).ok_or(DecodeError::Malformed)?,
      ))
    }
  }
}

fn walk_value(decoder: &mut Decoder, current_depth: usize, max_depth: usize) -> Result<(), DecodeError> {
  match check_type(decoder)? {
    Type::Bool
    | Type::Null
    | Type::Undefined
    | Type::U8
    | Type::U16
    | Type::U32
    | Type::U64
    | Type::I8
    | Type::I16
    | Type::I32
    | Type::I64
    | Type::Int
    | Type::F16
    | Type::F32
    | Type::F64
    | Type::Simple
    | Type::Bytes
    | Type::String => decoder.skip().map_err(|_| DecodeError::Malformed),
    Type::Map => walk_map(decoder, next_depth(current_depth, max_depth)?, max_depth),
    Type::Array => walk_array(decoder, next_depth(current_depth, max_depth)?, max_depth),
    _ => Err(DecodeError::Malformed),
  }
}

fn walk_array(decoder: &mut Decoder, current_depth: usize, max_depth: usize) -> Result<(), DecodeError> {
  for _ in 0..decoder
    .array()
    .map_err(|_| DecodeError::Malformed)?
    .ok_or(DecodeError::IndefiniteLength)?
  {
    walk_value(decoder, current_depth, max_depth)?;
  }

  Ok(())
}

fn check_type(decoder: &mut Decoder) -> Result<Type, DecodeError> {
  match decoder.datatype().map_err(|_| DecodeError::Malformed)? {
    Type::BytesIndef | Type::StringIndef | Type::ArrayIndef | Type::MapIndef => Err(DecodeError::IndefiniteLength),
    Type::Tag => Err(DecodeError::Tag),
    Type::Simple
      if matches!(
        decoder.input().get(decoder.position()..),
        Some([0xF8]) | Some([0xF8, 0x00..0x20, ..])
      ) =>
    {
      Err(DecodeError::Malformed)
    }
    Type::Break | Type::Unknown(_) => Err(DecodeError::Malformed),
    t => Ok(t),
  }
}

fn next_depth(current_depth: usize, max_depth: usize) -> Result<usize, DecodeError> {
  let depth = current_depth + 1;
  if depth > max_depth {
    Err(DecodeError::TooDeep)
  } else {
    Ok(depth)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::assert_matches;

  const UNKNOWN_KEY: [u8; 3] = [0x19, 0x13, 0x88];

  fn header_with_depth(depth: usize) -> Vec<u8> {
    let mut bytes = vec![0xa1, 0x01];
    bytes.extend(std::iter::repeat_n(0x81, depth - 2));
    bytes.push(0x80);
    bytes
  }

  fn header_with_depth_under_unknown_key(depth: usize) -> Vec<u8> {
    let mut bytes = vec![0xa1];
    bytes.extend(UNKNOWN_KEY);
    bytes.extend(std::iter::repeat_n(0x81, depth - 2));
    bytes.push(0x80);
    bytes
  }

  fn header_of_size(size: usize) -> Vec<u8> {
    let value_length = size - 5;
    let mut bytes = vec![0xa1, 0x01, 0x59];
    bytes.extend(u16::try_from(value_length).unwrap().to_be_bytes());
    bytes.resize(size, 0x00);
    bytes
  }

  fn header_with_unknown_key_value(value: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0xa1];
    bytes.extend(UNKNOWN_KEY);
    bytes.extend(value);
    bytes
  }

  #[test]
  fn accepts_empty_map() {
    let result = decode(&[0xA0], &DecodeLimits::default());
    assert_matches!(result, Ok(()));
  }

  #[test]
  fn accepts_empty_header() {
    let result = decode(&[], &DecodeLimits::default());
    assert_matches!(result, Ok(()));
  }

  #[test]
  fn rejects_array_at_top_level() {
    let result = decode(&[0x80], &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::NotAMap));
  }

  #[test]
  fn rejects_trailing_byte_after_map() {
    let result = decode(&[0xa0, 0x00], &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::TrailingBytes));
  }

  #[test]
  fn rejects_truncated_map() {
    let result = decode(&[0xa2, 0x01, 0x00], &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::Malformed));
  }

  #[test]
  fn rejects_indefinite_map() {
    let result = decode(&[0xbf, 0xff], &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::IndefiniteLength));
  }

  #[test]
  fn rejects_indefinite_array_inside_value() {
    let result = decode(&[0xa1, 0x01, 0x9f, 0xff], &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::IndefiniteLength));
  }

  #[test]
  fn rejects_indefinite_text_inside_value() {
    let result = decode(&[0xa1, 0x01, 0x7f, 0xff], &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::IndefiniteLength));
  }

  #[test]
  fn rejects_indefinite_item_under_unknown_key() {
    let result = decode(&header_with_unknown_key_value(&[0x9f, 0xff]), &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::IndefiniteLength));
  }

  #[test]
  fn rejects_tag_inside_value() {
    let result = decode(&[0xa1, 0x01, 0xc1, 0x00], &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::Tag));
  }

  #[test]
  fn rejects_tag_as_key() {
    let result = decode(&[0xa1, 0xc1, 0x00, 0x00], &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::Tag));
  }

  #[test]
  fn accepts_two_distinct_keys() {
    let result = decode(&[0xa2, 0x01, 0x00, 0x02, 0x00], &DecodeLimits::default());
    assert_matches!(result, Ok(()));
  }

  #[test]
  fn rejects_duplicate_key() {
    let result = decode(&[0xa2, 0x01, 0x00, 0x01, 0x00], &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::DuplicateKey));
  }

  #[test]
  fn rejects_duplicate_unknown_key() {
    let result = decode(
      &[0xa2, 0x19, 0x13, 0x88, 0x00, 0x19, 0x13, 0x88, 0x00],
      &DecodeLimits::default(),
    );
    assert_matches!(result, Err(DecodeError::DuplicateKey));
  }

  #[test]
  fn rejects_duplicate_key_with_non_shortest_encoding() {
    let result = decode(&[0xa2, 0x0e, 0x00, 0x18, 0x0e, 0x00], &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::DuplicateKey));
  }

  #[test]
  fn rejects_duplicate_key_in_nested_map() {
    let result = decode(&[0xa1, 0x01, 0xa2, 0x01, 0x00, 0x01, 0x00], &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::DuplicateKey));
  }

  #[test]
  fn rejects_duplicate_text_key() {
    let result = decode(&[0xa2, 0x61, 0x61, 0x00, 0x61, 0x61, 0x00], &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::DuplicateKey));
  }

  #[test]
  fn rejects_duplicate_raw_key() {
    let result = decode(&[0xa2, 0xf5, 0x00, 0xf5, 0x00], &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::DuplicateKey));
  }

  #[test]
  fn accepts_same_key_in_parent_and_nested_map() {
    let result = decode(&[0xa1, 0x01, 0xa1, 0x01, 0x00], &DecodeLimits::default());
    assert_matches!(result, Ok(()));
  }

  #[test]
  fn accepts_text_and_bytes_keys_with_same_content() {
    let result = decode(&[0xa2, 0x61, 0x61, 0x00, 0x41, 0x61, 0x00], &DecodeLimits::default());
    assert_matches!(result, Ok(()));
  }

  #[test]
  fn accepts_positive_and_negative_keys_with_same_magnitude() {
    let result = decode(&[0xa2, 0x01, 0x00, 0x20, 0x00], &DecodeLimits::default());
    assert_matches!(result, Ok(()));
  }

  #[test]
  fn accepts_depth_16() {
    let result = decode(&header_with_depth(16), &DecodeLimits::default());
    assert_matches!(result, Ok(()));
  }

  #[test]
  fn rejects_depth_17() {
    let result = decode(&header_with_depth(17), &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::TooDeep));
  }

  #[test]
  fn rejects_depth_17_under_unknown_key() {
    let result = decode(&header_with_depth_under_unknown_key(17), &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::TooDeep));
  }

  #[test]
  fn rejects_depth_above_custom_limit() {
    let limits = DecodeLimits {
      max_depth: 3,
      ..DecodeLimits::default()
    };
    assert_matches!(decode(&header_with_depth(3), &limits), Ok(()));
    assert_matches!(decode(&header_with_depth(4), &limits), Err(DecodeError::TooDeep));
  }

  #[test]
  fn does_not_add_depth_across_siblings() {
    let limits = DecodeLimits {
      max_depth: 3,
      ..DecodeLimits::default()
    };
    let result = decode(&[0xa1, 0x01, 0x83, 0x80, 0x80, 0x80], &limits);
    assert_matches!(result, Ok(()));
  }

  #[test]
  fn rejects_depth_17_in_key() {
    let mut bytes = vec![0xa1];
    bytes.extend(std::iter::repeat_n(0x81, 15));
    bytes.extend([0x80, 0x00]);
    let result = decode(&bytes, &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::TooDeep));
  }

  #[test]
  fn accepts_header_at_size_limit() {
    let result = decode(&header_of_size(16384), &DecodeLimits::default());
    assert_matches!(result, Ok(()));
  }

  #[test]
  fn rejects_header_above_size_limit() {
    let result = decode(&header_of_size(16385), &DecodeLimits::default());
    assert_matches!(
      result,
      Err(DecodeError::TooLarge {
        actual_size: 16385,
        max_size: 16384
      })
    );
  }

  #[test]
  fn checks_size_before_parsing() {
    let result = decode(&[0xff; 16385], &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::TooLarge { .. }));
  }

  #[test]
  fn ignores_unknown_key() {
    let result = decode(&[0xa2, 0x19, 0x13, 0x88, 0x01, 0x02, 0x61, 0x78], &DecodeLimits::default());
    assert_matches!(result, Ok(()));
  }

  #[test]
  fn accepts_keys_out_of_order() {
    let result = decode(&[0xa2, 0x02, 0x00, 0x01, 0x00], &DecodeLimits::default());
    assert_matches!(result, Ok(()));
  }

  #[test]
  fn accepts_non_shortest_key() {
    let result = decode(&[0xa2, 0x18, 0x0e, 0x00, 0x02, 0x00], &DecodeLimits::default());
    assert_matches!(result, Ok(()));
  }

  #[test]
  fn accepts_half_float_value() {
    let result = decode(&header_with_unknown_key_value(&[0xf9, 0x3c, 0x00]), &DecodeLimits::default());
    assert_matches!(result, Ok(()));
  }

  #[test]
  fn accepts_one_byte_simple_value() {
    let result = decode(&header_with_unknown_key_value(&[0xe0]), &DecodeLimits::default());
    assert_matches!(result, Ok(()));
  }

  #[test]
  fn accepts_two_byte_simple_value() {
    let result = decode(&header_with_unknown_key_value(&[0xf8, 0x20]), &DecodeLimits::default());
    assert_matches!(result, Ok(()));
  }

  #[test]
  fn accepts_simple_value_followed_by_another_entry() {
    let result = decode(&[0xa2, 0x19, 0x13, 0x88, 0xe0, 0x01, 0x00], &DecodeLimits::default());
    assert_matches!(result, Ok(()));
  }

  #[test]
  fn rejects_simple_value_with_invalid_two_byte_encoding() {
    let result = decode(&header_with_unknown_key_value(&[0xf8, 0x10]), &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::Malformed));
  }

  #[test]
  fn rejects_truncated_two_byte_simple_value() {
    let result = decode(&header_with_unknown_key_value(&[0xf8]), &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::Malformed));
  }

  #[test]
  fn rejects_reserved_additional_info() {
    let result = decode(&header_with_unknown_key_value(&[0x1c]), &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::Malformed));
  }
}
