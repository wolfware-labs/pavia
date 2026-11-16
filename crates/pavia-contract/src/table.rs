//! The type table: the named types a contract uses, collected from the Rust types that map to them.

use std::any::TypeId;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use crate::Contract;
use crate::name;
use crate::schema::{Definition, Type};

/// The named types collected so far, keyed by their manifest name (§12.2 `types`).
///
/// [`TypeTable::add`] maps a Rust type to its contract type and collects every named type it uses.
/// [`TypeTable::finish`] returns the table, or what keeps it from being a valid `types` table.
///
/// ```
/// use pavia_contract::TypeTable;
///
/// let mut table = TypeTable::new();
/// assert_eq!(table.add::<Vec<String>>().to_json(), serde_json::json!({"list": "string"}));
/// assert!(table.finish().unwrap().is_empty());
/// ```
#[derive(Debug, Default)]
pub struct TypeTable {
  owners: BTreeMap<String, TypeId>,
  definitions: BTreeMap<String, Definition>,
  errors: Vec<TypeError>,
}

impl TypeTable {
  /// An empty table.
  #[must_use]
  pub fn new() -> Self {
    Self::default()
  }

  /// The contract type of `T`, after collecting every named type it uses.
  pub fn add<T: Contract>(&mut self) -> Type {
    T::contract_type(self)
  }

  /// A reference to `T`'s named type, defined by `build` the first time `T` is added.
  ///
  /// `build` runs once per type, after the name is taken, so a type that refers to itself gets a
  /// reference instead of recursing. A second Rust type with the same name is reported by
  /// [`TypeTable::finish`].
  pub fn define<T: Contract>(&mut self, build: impl FnOnce(&mut Self) -> Definition) -> Type {
    let name = T::name();
    match self.owners.entry(name.clone()) {
      Entry::Occupied(owner) => {
        if *owner.get() != TypeId::of::<T>() {
          self.report(TypeError::Collision { name: name.clone() });
        }
      }
      Entry::Vacant(owner) => {
        owner.insert(TypeId::of::<T>());
        if !name::is_valid(&name) {
          self.report(TypeError::InvalidName { name: name.clone() });
        }
        let definition = build(self);
        self.definitions.insert(name.clone(), definition);
      }
    }
    Type::Ref(name)
  }

  /// The named types by name, or every error found while collecting them.
  ///
  /// # Errors
  ///
  /// [`TypeError::Collision`] when two Rust types have the same name, and [`TypeError::InvalidName`] for a
  /// name outside the grammar of §12.2.
  pub fn finish(self) -> Result<BTreeMap<String, Definition>, Vec<TypeError>> {
    if self.errors.is_empty() {
      Ok(self.definitions)
    } else {
      Err(self.errors)
    }
  }

  fn report(&mut self, error: TypeError) {
    if !self.errors.contains(&error) {
      self.errors.push(error);
    }
  }
}

/// What keeps the collected types from being a valid `types` table.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TypeError {
  /// Two Rust types have the same name, such as a user type `Page_Message` and the instantiation
  /// `Page<Message>` (§12.1).
  #[error("two types are named `{name}` (spec 12.1)")]
  Collision {
    /// The name both types have.
    name: String,
  },
  /// A name outside `[A-Za-z_][A-Za-z0-9_]*` (§12.2).
  #[error("type name `{name}` does not match [A-Za-z_][A-Za-z0-9_]* (spec 12.2)")]
  InvalidName {
    /// The name.
    name: String,
  },
}

#[cfg(test)]
mod tests;
