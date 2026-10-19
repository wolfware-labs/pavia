#![no_main]

use libfuzzer_sys::fuzz_target;
use pavia_proto::wire::header::{DecodeLimits, EncodeError, decode, encode};

fuzz_target!(|data: &[u8]| {
  let limits = DecodeLimits::default();
  let Ok(Some(header)) = decode(data, &limits) else {
    return;
  };
  let mut first = Vec::new();
  match encode(&header, &mut first) {
    Ok(()) => {}
    Err(EncodeError::DuplicateKey) => return,
    Err(error) => panic!("decoded header does not encode: {error}"),
  }
  let decoded = decode(&first, &limits)
    .expect("encoder output decodes")
    .expect("encoder output is not empty");
  let mut second = Vec::new();
  encode(&decoded, &mut second).expect("re-encoding succeeds");
  assert_eq!(first, second);
});
