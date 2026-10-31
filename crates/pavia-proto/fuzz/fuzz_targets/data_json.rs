#![no_main]

use libfuzzer_sys::fuzz_target;
use pavia_proto::data::{Codec, DecodeLimits, decode, encode};

fuzz_target!(|data: &[u8]| {
  let Ok(value) = decode(data, Codec::Json, &DecodeLimits::default()) else {
    return;
  };
  let mut first = Vec::new();
  encode(&value, Codec::Json, &mut first).expect("a decoded value encodes");
  let decoded = decode(&first, Codec::Json, &DecodeLimits::default()).expect("encoder output decodes");
  let mut second = Vec::new();
  encode(&decoded, Codec::Json, &mut second).expect("re-encoding succeeds");
  assert_eq!(first, second);
});
