#[cfg(not(fuzzing))]
#[cfg_attr(not(test), expect(dead_code, reason = "used by the frame codec from #10 onwards"))]
mod varint;
/// QUIC variable-length integers, public only to the fuzz targets.
#[cfg(fuzzing)]
pub mod varint;
