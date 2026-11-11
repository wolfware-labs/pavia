//! Pavia server runtime: session tasks, registry, targeting, hooks.
//!
//! See the crate README for its place in the workspace.
#![forbid(unsafe_code)]

pub mod auth;
pub mod methods;
pub mod session;
mod session_id;
pub mod sessions;
#[cfg(test)]
mod test_support;
pub mod websocket;
