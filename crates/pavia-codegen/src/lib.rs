//! Client code generation from a Pavia manifest (§12.4).
//!
//! A front end shared by every target validates the manifest, computes its fingerprint and works out
//! what the targets derive from it; each target is a back end, so a new language is a new module here
//! and not a second generator. [`typescript`] is the first.

mod front;
pub mod typescript;

pub use front::GenerateError;
