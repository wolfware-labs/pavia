//! Feeds arbitrary bytes to the varint decoder: it must never panic, and a decoded value must
//! stay within the input and within `max`.
#![no_main]

use libfuzzer_sys::fuzz_target;
use pavia_proto::wire::varint::{DecodeValue, decode};

fuzz_target!(|data: &[u8]| {
  // The first 8 bytes, when present, choose `max`, so the limit check is fuzzed too.
  let (max, bytes) = match data.split_first_chunk::<8>() {
    Some((head, rest)) => (u64::from_be_bytes(*head), rest),
    None => (u64::MAX, data),
  };
  if let Ok(DecodeValue::Value { value, consumed_length }) = decode(bytes, max) {
    assert!(consumed_length <= bytes.len());
    assert!(value <= max);
  }
});
