//! Which contract type a Rust type is (§12.1): the [`Contract`] trait and its impls for std types.
//!
//! A Rust type with no contract equivalent has no impl, so using it fails to compile. The marker traits
//! [`MapKey`] and [`OptionValue`] carry the message for the two shapes the spec rules out by name: a map
//! whose keys are not strings, and `{"option": "unit"}`. An `Option` of a named type that is `unit` through
//! aliases passes the marker trait, and [`TypeTable::finish`] reports it.

use std::collections::{BTreeMap, HashMap};

use crate::scalar::{Bytes, Timestamp, Uuid};
use crate::schema::{Primitive, Type};
use crate::table::TypeTable;

/// A Rust type that maps to a contract type (§12.1).
///
/// Primitives and std collections have impls here; user structs and enums derive it with
/// `#[derive(Contract)]`.
///
/// ```
/// use pavia_contract::{Contract, TypeTable};
///
/// assert_eq!(<Vec<u32>>::name(), "List_u32");
/// assert_eq!(TypeTable::new().add::<Option<String>>().to_json(), serde_json::json!({"option": "string"}));
/// ```
#[diagnostic::on_unimplemented(
  message = "`{Self}` has no contract type (spec 12.1)",
  label = "not a contract type",
  note = "derive `pavia_contract::Contract` for it, or use a type of spec 12.1"
)]
pub trait Contract: 'static {
  /// The name this type gives a generic instantiation it is an argument of: the primitive's name, or the
  /// name in the type table (`Page<Vec<Message>>` is `Page_List_Message`, §12.1).
  fn name() -> String;

  /// The contract type, after adding every named type it uses to `table`.
  fn contract_type(table: &mut TypeTable) -> Type;
}

/// A contract type that `Option` may wrap: every one but `unit`, since `{"option": "unit"}` is not allowed
/// (§12.1).
///
/// It also bounds the type parameters of a derived generic type, whose fields may wrap them in an
/// `Option`. `#[derive(Contract)]` implements it too; a hand-written [`Contract`] impl adds it the same way.
/// A named type that is `unit` through aliases, such as `struct Nothing(())`, implements it as well, and
/// [`TypeTable::finish`](crate::TypeTable::finish) reports an `Option` of it.
#[diagnostic::on_unimplemented(
  message = "`{Self}` cannot be an `Option` value or a type argument of a derived contract type: \
             `{{\"option\": \"unit\"}}` is not allowed (spec 12.1)",
  label = "`unit` here"
)]
pub trait OptionValue: Contract {}

/// A Rust type a contract map may be keyed by: `String` only, since map keys are strings (§12.1).
#[diagnostic::on_unimplemented(
  message = "map keys must be strings: `{Self}` cannot key a contract map (spec 12.1)",
  label = "not a string key",
  note = "key the map by `String`"
)]
pub trait MapKey: 'static + sealed::Sealed {}

impl MapKey for String {}

/// A field type `#[pavia(optional)]` accepts: an `Option`, since only an option may be missing on the
/// wire (§12.1).
#[diagnostic::on_unimplemented(
  message = "`#[pavia(optional)]` needs an `Option` field, and `{Self}` is not one (spec 12.1)",
  label = "not an `Option`"
)]
pub trait OptionField {}

impl<T> OptionField for Option<T> {}

mod sealed {
  pub trait Sealed {}

  impl Sealed for String {}
}

macro_rules! primitives {
  ($($rust:ty => $primitive:ident),* $(,)?) => {
    $(
      impl Contract for $rust {
        fn name() -> String {
          Primitive::$primitive.name().to_owned()
        }

        fn contract_type(_: &mut TypeTable) -> Type {
          Type::Primitive(Primitive::$primitive)
        }
      }

      impl OptionValue for $rust {}
    )*
  };
}

primitives! {
  bool => Bool,
  i8 => I8,
  i16 => I16,
  i32 => I32,
  i64 => I64,
  u8 => U8,
  u16 => U16,
  u32 => U32,
  u64 => U64,
  f32 => F32,
  f64 => F64,
  String => String,
  Bytes => Bytes,
  Timestamp => Timestamp,
  Uuid => Uuid,
}

impl Contract for () {
  fn name() -> String {
    Primitive::Unit.name().to_owned()
  }

  fn contract_type(_: &mut TypeTable) -> Type {
    Type::Primitive(Primitive::Unit)
  }
}

impl<T: OptionValue> Contract for Option<T> {
  fn name() -> String {
    format!("Option_{}", T::name())
  }

  fn contract_type(table: &mut TypeTable) -> Type {
    let value = table.add::<T>();
    table.record_wrapped(&value);
    Type::Option(Box::new(value))
  }
}

impl<T: OptionValue> OptionValue for Option<T> {}

impl<T: Contract> Contract for Vec<T> {
  fn name() -> String {
    format!("List_{}", T::name())
  }

  fn contract_type(table: &mut TypeTable) -> Type {
    Type::List(Box::new(table.add::<T>()))
  }
}

impl<T: Contract> OptionValue for Vec<T> {}

impl<T: Contract> Contract for Box<T> {
  fn name() -> String {
    T::name()
  }

  fn contract_type(table: &mut TypeTable) -> Type {
    table.add::<T>()
  }
}

impl<T: OptionValue> OptionValue for Box<T> {}

impl<K: MapKey, V: Contract, S: 'static> Contract for HashMap<K, V, S> {
  fn name() -> String {
    format!("Map_{}", V::name())
  }

  fn contract_type(table: &mut TypeTable) -> Type {
    Type::Map(Box::new(table.add::<V>()))
  }
}

impl<K: MapKey, V: Contract, S: 'static> OptionValue for HashMap<K, V, S> {}

impl<K: MapKey, V: Contract> Contract for BTreeMap<K, V> {
  fn name() -> String {
    format!("Map_{}", V::name())
  }

  fn contract_type(table: &mut TypeTable) -> Type {
    Type::Map(Box::new(table.add::<V>()))
  }
}

impl<K: MapKey, V: Contract> OptionValue for BTreeMap<K, V> {}

macro_rules! tuples {
  ($(($($item:ident),+)),* $(,)?) => {
    $(
      impl<$($item: Contract),+> Contract for ($($item,)+) {
        fn name() -> String {
          let mut name = String::from("Tuple");
          $(
            name.push('_');
            name.push_str(&$item::name());
          )+
          name
        }

        fn contract_type(table: &mut TypeTable) -> Type {
          Type::Tuple(vec![$(table.add::<$item>()),+])
        }
      }

      impl<$($item: Contract),+> OptionValue for ($($item,)+) {}
    )*
  };
}

tuples! {
  (A),
  (A, B),
  (A, B, C),
  (A, B, C, D),
  (A, B, C, D, E),
  (A, B, C, D, E, F),
  (A, B, C, D, E, F, G),
  (A, B, C, D, E, F, G, H),
}

#[cfg(test)]
mod tests;
