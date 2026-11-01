#[cfg(not(fuzzing))]
pub(crate) mod envelope;
#[cfg(fuzzing)]
pub mod envelope;
#[cfg(not(fuzzing))]
pub(crate) mod frame;
#[cfg(fuzzing)]
pub mod frame;
#[cfg(not(fuzzing))]
pub(crate) mod header;
#[cfg(fuzzing)]
pub mod header;
#[cfg(not(fuzzing))]
pub(crate) mod varint;
#[cfg(fuzzing)]
pub mod varint;
