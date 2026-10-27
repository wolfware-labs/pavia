#[cfg(not(fuzzing))]
#[cfg_attr(not(test), expect(dead_code, reason = "used by the session from #24 onwards"))]
mod frame;
#[cfg(fuzzing)]
pub mod frame;
#[cfg(not(fuzzing))]
#[cfg_attr(not(test), expect(dead_code, reason = "used by the typed header model from #13 onwards"))]
mod header;
#[cfg(fuzzing)]
pub mod header;
#[cfg(not(fuzzing))]
#[cfg_attr(not(test), expect(dead_code, reason = "used by the frame codec from #10 onwards"))]
mod varint;
#[cfg(fuzzing)]
pub mod varint;
