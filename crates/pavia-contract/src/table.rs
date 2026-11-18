//! The type table: the named types a contract uses, collected from the Rust types that map to them.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet};

use crate::Contract;
use crate::name;
use crate::schema::{Definition, Primitive, Type};

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
  seen: BTreeSet<TypeId>,
  names: BTreeSet<String>,
  definitions: BTreeMap<String, Definition>,
  shared: Vec<(String, Definition)>,
  wrapped: BTreeSet<String>,
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
  /// reference instead of recursing. Several Rust types can share a name when they define the same type,
  /// as `Page<Box<Message>>` and `Page<Message>` do; [`TypeTable::finish`] reports a name whose types
  /// differ.
  pub fn define<T: Contract>(&mut self, build: impl FnOnce(&mut Self) -> Definition) -> Type {
    let name = T::name();
    if !self.seen.insert(TypeId::of::<T>()) {
      return Type::Ref(name);
    }
    if self.names.insert(name.clone()) {
      if !name::is_valid(&name) {
        self.report(TypeError::InvalidName { name: name.clone() });
      }
      let definition = build(self);
      self.definitions.insert(name.clone(), definition);
    } else {
      let definition = build(self);
      self.shared.push((name.clone(), definition));
    }
    Type::Ref(name)
  }

  /// Records that an `Option` wraps `value`, so that [`TypeTable::finish`] can check that it is not `unit`
  /// through aliases.
  pub(crate) fn record_wrapped(&mut self, value: &Type) {
    if let Type::Ref(name) = value {
      self.wrapped.insert(name.clone());
    }
  }

  /// The named types by name, or every error found while collecting them.
  ///
  /// # Errors
  ///
  /// [`TypeError::Collision`] when two Rust types with the same name define different types,
  /// [`TypeError::InvalidName`] for a name outside the grammar of §12.2, and [`TypeError::OptionOfUnit`]
  /// for an `Option` of a named type that is `unit` through aliases.
  pub fn finish(mut self) -> Result<BTreeMap<String, Definition>, Vec<TypeError>> {
    for (name, definition) in std::mem::take(&mut self.shared) {
      if self.definitions.get(&name) != Some(&definition) {
        self.report(TypeError::Collision { name });
      }
    }
    for name in std::mem::take(&mut self.wrapped) {
      if self.aliases_unit(&name) {
        self.report(TypeError::OptionOfUnit { name });
      }
    }
    if self.errors.is_empty() {
      Ok(self.definitions)
    } else {
      Err(self.errors)
    }
  }

  fn aliases_unit(&self, name: &str) -> bool {
    let mut visited = BTreeSet::new();
    let mut name = name;
    while visited.insert(name) {
      match self.definitions.get(name) {
        Some(Definition::Alias(Type::Primitive(Primitive::Unit))) => return true,
        Some(Definition::Alias(Type::Ref(target))) => name = target,
        _ => return false,
      }
    }
    false
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
  /// Two Rust types have the same name and define different types, such as a user type `Page_Message`
  /// and the instantiation `Page<Message>` (§12.1).
  #[error("two types are named `{name}` (spec 12.1)")]
  Collision {
    /// The name both types have.
    name: String,
  },
  /// An `Option` wraps a named type that is `unit` through aliases, such as `struct Nothing(())`, which
  /// would be `{"option": "unit"}` (§12.1).
  #[error(
    "`{name}` is `unit` through an alias, and an `Option` wraps it: `{{\"option\": \"unit\"}}` is not allowed (spec 12.1)"
  )]
  OptionOfUnit {
    /// The name of the alias.
    name: String,
  },
  /// A name outside `[A-Za-z_][A-Za-z0-9_]*` (§12.2).
  #[error("`{name}` is not a contract name: names match [A-Za-z_][A-Za-z0-9_]* (spec 12.2)")]
  InvalidName {
    /// The name.
    name: String,
  },
}

#[cfg(test)]
mod tests;
