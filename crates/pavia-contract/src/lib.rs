//! Pavia contract layer: type system, manifest, fingerprint, typed codecs.
//!
//! Today it holds the type system (§12.1) and the manifest (§12.2): [`Contract`] maps a Rust type to its
//! contract type, a [`TypeTable`] collects the named types the manifest's `types` table lists, and
//! [`manifest::Manifest`] checks and writes the whole document.
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
//! manifest and the typed codecs (#66) read it. [`name`] is the name grammar of §12.2, which
//! the server's method names follow too.
#![forbid(unsafe_code)]

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
