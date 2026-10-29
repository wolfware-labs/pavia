#![no_main]

use libfuzzer_sys::fuzz_target;
use pavia_proto::data::{Codec, decode, encode};

fuzz_target!(|data: &[u8]| {
  let Ok(value) = decode(data, Codec::Cbor) else {
    return;
  };
  let mut first = Vec::new();
  encode(&value, Codec::Cbor, &mut first).expect("a decoded value encodes");
  let decoded = decode(&first, Codec::Cbor).expect("encoder output decodes");
  let mut second = Vec::new();
  encode(&decoded, Codec::Cbor, &mut second).expect("re-encoding succeeds");
  assert_eq!(first, second);
});
