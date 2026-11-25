//! The type table: the named types a contract uses, collected from the Rust types that map to them.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet};

use crate::Contract;
use crate::name;
use crate::schema::{Definition, Primitive, Type, Types};

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
  kinds: BTreeMap<String, NameKind>,
  definitions: Types,
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
  /// such as two structs with the same fields renamed to one name; [`TypeTable::finish`] reports a name
  /// whose types differ.
  pub fn define<T: Contract>(&mut self, build: impl FnOnce(&mut Self) -> Definition) -> Type {
    self.define_as::<T>(NameKind::UserDefined, build)
  }

  /// As [`TypeTable::define`], for an instantiation of a generic type, whose name is monomorphized
  /// (§12.1): `Page<Message>` is `Page_Message`.
  ///
  /// Instantiations share a name when they define the same type, as `Page<Box<Message>>` and
  /// `Page<Message>` do. A generated name that a type defined with [`TypeTable::define`] also takes is a
  /// collision, even when both define the same type.
  pub fn define_instance<T: Contract>(&mut self, build: impl FnOnce(&mut Self) -> Definition) -> Type {
    self.define_as::<T>(NameKind::Generated, build)
  }

  fn define_as<T: Contract>(&mut self, kind: NameKind, build: impl FnOnce(&mut Self) -> Definition) -> Type {
    let name = T::name();
    if !self.seen.insert(TypeId::of::<T>()) {
      return Type::Ref(name);
    }
    let first = self.kinds.get(&name).copied();
    if first.is_none() {
      self.kinds.insert(name.clone(), kind);
      if !name::is_valid(&name) {
        self.report(TypeError::InvalidName { name: name.clone() });
      }
    }
    let definition = build(self);
    match first {
      None => {
        self.definitions.insert(name.clone(), definition);
      }
      Some(first) if first == kind => self.shared.push((name.clone(), definition)),
      Some(_) => self.report(TypeError::Collision { name: name.clone() }),
    }
    Type::Ref(name)
  }

  /// As [`TypeTable::add`], for the value an `Option` wraps. A `unit` value is reported at once, for a
  /// hand-written [`Contract`] impl that also implements [`OptionValue`](crate::OptionValue); a named one
  /// is checked by [`TypeTable::finish`], once every alias is defined.
  pub(crate) fn add_wrapped<T: Contract>(&mut self) -> Type {
    let value = self.add::<T>();
    match &value {
      Type::Primitive(Primitive::Unit) => self.report(TypeError::OptionOfUnit { name: T::name() }),
      Type::Ref(name) => {
        self.wrapped.insert(name.clone());
      }
      _ => {}
    }
    value
  }

  /// The named types by name, or every error found while collecting them.
  ///
  /// # Errors
  ///
  /// [`TypeError::Collision`] when two Rust types with the same name define different types, or a user
  /// type takes a generated name, [`TypeError::InvalidName`] for a name outside the grammar of §12.2,
  /// [`TypeError::OptionOfUnit`] for an `Option` of a type that is `unit`, directly or through aliases, and
  /// [`TypeError::AliasCycle`] for an alias that leads back to itself through aliases only. The last two
  /// are not checked through a name with a collision, whose types the table cannot tell apart; they show
  /// once the collision is fixed.
  pub fn finish(mut self) -> Result<Types, Vec<TypeError>> {
    for (name, definition) in std::mem::take(&mut self.shared) {
      if self.definitions.get(&name) != Some(&definition) {
        self.report(TypeError::Collision { name });
      }
    }
    let collided: BTreeSet<String> = self
      .errors
      .iter()
      .filter_map(|error| match error {
        TypeError::Collision { name } => Some(name.clone()),
        _ => None,
      })
      .collect();
    let chains = Chains::new(&self.definitions, &collided);
    let wrapped: Vec<String> = std::mem::take(&mut self.wrapped)
      .into_iter()
      .filter(|name| chains.from(name).ends_in_unit())
      .collect();
    let cycles: Vec<String> = self
      .definitions
      .keys()
      .filter(|name| matches!(chains.from(name), Chain::Loops))
      .cloned()
      .collect();
    for name in wrapped {
      self.report(TypeError::OptionOfUnit { name });
    }
    for name in cycles {
      self.report(TypeError::AliasCycle { name });
    }
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

/// Where a name in the table comes from (§12.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameKind {
  /// The name of a type the user defined, after any rename.
  UserDefined,
  /// The monomorphized name of a generic instantiation.
  Generated,
}

/// Where the chain of aliases from each name of a `types` table leads, every name walked once, so a table
/// of n names costs O(n log n) however its chains run.
pub(crate) struct Chains<'a> {
  ends: BTreeMap<&'a str, Chain<'a>>,
}

impl<'a> Chains<'a> {
  /// The chains through `definitions`. A chain through a name in `collided` ends there unknown: that name
  /// has several definitions, and the table keeps only the first.
  pub(crate) fn new(definitions: &'a Types, collided: &BTreeSet<String>) -> Self {
    let mut ends = BTreeMap::new();
    for start in definitions.keys() {
      let mut path = Vec::new();
      let mut on_path = BTreeSet::new();
      let mut name = start.as_str();
      let end = loop {
        if let Some(end) = ends.get(name) {
          break *end;
        }
        if !on_path.insert(name) {
          break Chain::Loops;
        }
        path.push(name);
        if collided.contains(name) {
          break Chain::Ends(None);
        }
        match definitions.get(name) {
          Some(Definition::Alias(Type::Ref(target))) => name = target,
          definition => break Chain::Ends(definition),
        }
      };
      for name in path {
        ends.insert(name, end);
      }
    }
    Self { ends }
  }

  /// Where the chain from `name` leads; a name the table does not have ends at once, unknown.
  pub(crate) fn from(&self, name: &str) -> Chain<'a> {
    self.ends.get(name).copied().unwrap_or(Chain::Ends(None))
  }
}

/// Where a chain of `Alias(Ref)` definitions leads.
#[derive(Clone, Copy)]
pub(crate) enum Chain<'a> {
  /// To a definition that is not an alias of a reference, or to a name whose definition is not known.
  Ends(Option<&'a Definition>),
  /// Back to a name it passed, never reaching a type.
  Loops,
}

impl Chain<'_> {
  /// Whether the chain ends at an alias of `unit`.
  pub(crate) fn ends_in_unit(&self) -> bool {
    matches!(self, Chain::Ends(Some(Definition::Alias(Type::Primitive(Primitive::Unit)))))
  }
}

/// What keeps the collected types from being a valid `types` table.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TypeError {
  /// Two Rust types have the same name and define different types, or a user type such as `Page_Message`
  /// takes the generated name of an instantiation such as `Page<Message>` (§12.1).
  #[error("two types are named `{name}` (spec 12.1)")]
  Collision {
    /// The name both types have.
    name: String,
  },
  /// An `Option` wraps a type that is `unit`: through aliases, such as `struct Nothing(())`, or directly,
  /// from a hand-written [`Contract`] impl. It would be `{"option": "unit"}` (§12.1).
  #[error("`{name}` is `unit`, and an `Option` wraps it: `{{\"option\": \"unit\"}}` is not allowed (spec 12.1)")]
  OptionOfUnit {
    /// The name of the type.
    name: String,
  },
  /// An alias that leads back to itself through aliases only, such as `struct Left(Box<Right>)` with
  /// `struct Right(Box<Left>)`: the chain never reaches a type, so a generated `type Left = Right` cannot
  /// compile.
  #[error("`{name}` leads back to itself through aliases only, so it never reaches a type")]
  AliasCycle {
    /// The name of an alias on the cycle, or leading to it.
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
