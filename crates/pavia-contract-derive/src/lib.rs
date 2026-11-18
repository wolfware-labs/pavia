//! Derive macros for Pavia contract types.
//!
//! `#[derive(Contract)]` implements `pavia_contract::Contract` for a struct or an enum (§12.1):
//!
//! - a struct with named fields is a `struct`, a tuple struct with one field an `alias` of that field's
//!   type, one with several an `alias` of a `tuple`, and a unit struct a `struct` without fields;
//! - an enum is an `enum`: a unit variant has no type, a one-field variant that field's type, a tuple
//!   variant a `tuple`, and a variant with named fields a `struct` written in place.
//!
//! Attributes, under `#[pavia(...)]`, each given at most once:
//!
//! - `rename = "name"` on the type, a field or a variant: the name in the manifest;
//! - `optional` on a named field of type `Option<T>`: the key may be missing on the wire;
//! - `open` on an enum: decoders accept unknown variants (§12.1);
//! - `crate = "path"` on the type: the path the generated code reaches `pavia_contract` by, for a crate
//!   that renames the dependency or uses the derive through a re-export.
//!
//! Every name, after any rename, must match `[A-Za-z_][A-Za-z0-9_]*` (§12.2). A generic type's name is
//! monomorphized: `Page<Message>` is `Page_Message` (§12.1). The generated code refers to the
//! `pavia_contract` crate as `::pavia_contract` unless `crate` says otherwise.
//!
//! See the crate README for its place in the workspace.
#![forbid(unsafe_code)]

mod attr;
mod expand;
mod name;

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

/// Implements `pavia_contract::Contract` (and `pavia_contract::OptionValue`) for a struct or an enum.
#[proc_macro_derive(Contract, attributes(pavia))]
pub fn derive_contract(input: TokenStream) -> TokenStream {
  let input = parse_macro_input!(input as DeriveInput);
  expand::contract(&input).unwrap_or_else(syn::Error::into_compile_error).into()
}
