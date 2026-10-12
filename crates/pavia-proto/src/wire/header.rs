use minicbor::Decoder;
use minicbor::data::Type;
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
  #[error("header has duplicate key {0}")]
  DuplicateKey(u64),
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
    Type::MapIndef => Err(DecodeError::IndefiniteLength),
    Type::Map => {
      walk_header(&mut decoder, limits.max_depth)?;
      if decoder.position() < content.len() {
        Err(DecodeError::TrailingBytes)
      } else {
        Ok(())
      }
    }
    _ => Err(DecodeError::NotAMap),
  }
}

fn walk_header(decoder: &mut Decoder, max_depth: usize) -> Result<(), DecodeError> {
  let map_size = decoder
    .map()
    .map_err(|_| DecodeError::Malformed)?
    .ok_or(DecodeError::IndefiniteLength)?;

  let depth = 1;
  let mut keys = HashSet::new();
  for _ in 0..map_size {
    let key = decoder.u64().map_err(|_| DecodeError::Malformed)?;
    if !keys.insert(key) {
      return Err(DecodeError::DuplicateKey(key));
    }

    walk_item(decoder, depth, max_depth)?
  }

  Ok(())
}

fn walk_item(decoder: &mut Decoder, mut current_depth: usize, max_depth: usize) -> Result<(), DecodeError> {
  match decoder.datatype().map_err(|_| DecodeError::Malformed)? {
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
    | Type::String => decoder.skip().map_err(|_| DecodeError::Malformed)?,
    Type::Array => {
      current_depth += 1;
      if current_depth > max_depth {
        return Err(DecodeError::TooDeep);
      }
      for _ in 0..decoder
        .array()
        .map_err(|_| DecodeError::Malformed)?
        .ok_or(DecodeError::IndefiniteLength)?
      {
        walk_item(decoder, current_depth, max_depth)?;
      }
    }
    Type::Map => {
      current_depth += 1;
      if current_depth > max_depth {
        return Err(DecodeError::TooDeep);
      }
      for _ in 0..decoder
        .map()
        .map_err(|_| DecodeError::Malformed)?
        .ok_or(DecodeError::IndefiniteLength)?
      {
        walk_item(decoder, current_depth, max_depth)?;
        walk_item(decoder, current_depth, max_depth)?;
      }
    }
    Type::BytesIndef | Type::StringIndef | Type::ArrayIndef | Type::MapIndef => {
      return Err(DecodeError::IndefiniteLength);
    }
    Type::Tag => {
      return Err(DecodeError::Tag);
    }
    Type::Break | Type::Unknown(_) => {
      return Err(DecodeError::Malformed);
    }
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::assert_matches;

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
    todo!()
  }

  #[test]
  fn rejects_truncated_map() {
    todo!()
  }

  #[test]
  fn rejects_indefinite_map() {
    let result = decode(&[0xbf, 0xff], &DecodeLimits::default());
    assert_matches!(result, Err(DecodeError::IndefiniteLength));
  }

  #[test]
  fn rejects_indefinite_array_inside_value() {
    todo!()
  }

  #[test]
  fn rejects_indefinite_text_inside_value() {
    todo!()
  }

  #[test]
  fn rejects_indefinite_item_under_unknown_key() {
    todo!()
  }

  #[test]
  fn rejects_duplicate_key() {
    todo!()
  }

  #[test]
  fn rejects_duplicate_unknown_key() {
    todo!()
  }

  #[test]
  fn accepts_depth_16() {
    todo!()
  }

  #[test]
  fn rejects_depth_17() {
    todo!()
  }

  #[test]
  fn rejects_depth_17_under_unknown_key() {
    todo!()
  }

  #[test]
  fn rejects_depth_above_custom_limit() {
    todo!()
  }

  #[test]
  fn accepts_header_at_size_limit() {
    todo!()
  }

  #[test]
  fn rejects_header_above_size_limit() {
    todo!()
  }

  #[test]
  fn checks_size_before_parsing() {
    todo!()
  }

  #[test]
  fn ignores_unknown_key() {
    todo!()
  }

  #[test]
  fn accepts_keys_out_of_order() {
    todo!()
  }

  #[test]
  fn accepts_non_shortest_key() {
    todo!()
  }

  #[test]
  fn encodes_same_header_to_identical_bytes() {
    todo!()
  }

  #[test]
  fn encodes_integer_keys_in_sorted_order() {
    todo!()
  }

  #[test]
  fn encodes_text_keys_by_encoded_bytes() {
    todo!()
  }

  #[test]
  fn encodes_shortest_integer_forms() {
    todo!()
  }

  #[test]
  fn round_trips_any_header() {
    todo!()
  }
}
