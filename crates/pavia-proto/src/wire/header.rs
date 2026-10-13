mod decode;
mod encode;

#[expect(
  unused_imports,
  reason = "used by the frame codec and the typed header model from #10 and #13"
)]
pub use decode::{DecodeError, DecodeLimits, decode};
