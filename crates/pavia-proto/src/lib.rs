//! Sans-IO core of the Pavia protocol: framing, streams, session state, sequencing.
//!
//! See the crate README for its place in the workspace.
//!
//! [`error`] and [`registry`] are public because the session drivers in other crates match on
//! [`error::Outcome`] and convert the registry codes; the wire layer stays private.
#![forbid(unsafe_code)]

pub mod error;
pub mod registry;

#[cfg(not(fuzzing))]
mod wire;
#[cfg(fuzzing)]
pub mod wire;
