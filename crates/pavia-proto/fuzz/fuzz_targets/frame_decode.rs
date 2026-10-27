#![no_main]

use libfuzzer_sys::fuzz_target;
use pavia_proto::wire::frame::FrameDecoder;

fuzz_target!(|data: &[u8]| {
  let Some((&chunk_length, bytes)) = data.split_first() else {
    return;
  };
  let mut decoder = FrameDecoder::new(4096);
  let mut taken = 0;
  for chunk in bytes.chunks(usize::from(chunk_length).max(1)) {
    if decoder.feed(chunk).is_err() {
      return;
    }
    loop {
      match decoder.next_frame() {
        Ok(Some(frame)) => taken += frame.header.len() + frame.data.len(),
        Ok(None) => break,
        Err(_) => return,
      }
    }
  }
  assert!(taken <= bytes.len());
  let _ = decoder.finish();
});
