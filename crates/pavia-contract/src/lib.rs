//! Pavia contract layer: type system, manifest, fingerprint.
//!
//! It holds the type system (§12.1), the manifest (§12.2) and its fingerprint (§12.5): [`Contract`] maps a
//! Rust type to its contract type, a [`TypeTable`] collects the named types the manifest's `types` table
//! lists, [`manifest::Manifest`] checks and writes the whole document, and [`fingerprint`] hashes it.
//!
//! ```
//! use pavia_contract::{Contract, Timestamp, TypeTable};
//!
//! #[derive(Contract)]
//! struct Message {
//!   author: String,
//!   sent_at: Timestamp,
//! }
//!
//! let mut table = TypeTable::new();
//! assert_eq!(table.add::<Vec<Message>>().to_json(), serde_json::json!({"list": {"ref": "Message"}}));
//! let types = table.finish().unwrap();
//! assert_eq!(
//!   types["Message"].to_json(),
//!   serde_json::json!({"struct": {"fields": [
//!     {"name": "author", "type": "string"},
//!     {"name": "sent_at", "type": "timestamp"},
//!   ]}}),
//! );
//! ```
//!
//! See the crate README for its place in the workspace.
//!
//! [`schema`] is public because a hand-written [`Contract`] impl builds its contract type from it, and the
//! manifest and the typed codecs of `pavia-codec` read it. [`name`] is the name grammar of §12.2, which
//! the server's method names follow too. [`fingerprint`] reads manifest JSON from any source, not only
//! from [`manifest::Manifest`], so a tool can fingerprint a manifest file.
#![forbid(unsafe_code)]

pub mod fingerprint;
pub mod manifest;
mod mapping;
pub mod name;
mod scalar;
pub mod schema;
mod table;

pub use mapping::{Contract, MapKey, OptionField, OptionValue};
pub use pavia_contract_derive::Contract;
pub use scalar::{Bytes, Timestamp, Uuid};
pub use table::{TypeError, TypeTable};
