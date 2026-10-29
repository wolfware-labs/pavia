//! Sans-IO core of the Pavia protocol: framing, streams, session state, sequencing.
//!
//! See the crate README for its place in the workspace.
//!
//! [`error`] and [`registry`] are public because the session drivers in other crates match on
//! [`error::Outcome`] and convert the registry codes. [`data`] is public because handlers receive
//! and return data-section values. [`stream`] is public so a driver can reject a frame on the
//! wrong kind of stream as soon as it decodes one; the wire layer stays private.
#![forbid(unsafe_code)]

pub mod data;
pub mod error;
pub mod frame;
pub mod registry;
pub mod stream;

#[cfg(not(fuzzing))]
mod wire;
#[cfg(fuzzing)]
pub mod wire;
