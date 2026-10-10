//! Sans-IO core of the Pavia protocol: framing, streams, session state, sequencing.
//!
//! See the crate README for its place in the workspace.
#![forbid(unsafe_code)]

#[cfg(not(fuzzing))]
mod wire;
#[cfg(fuzzing)]
pub mod wire;
