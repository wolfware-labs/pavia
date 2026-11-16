use std::collections::BTreeMap;

use super::{TypeError, TypeTable};
use crate::Contract;
use crate::schema::{Definition, Primitive, Type};

struct Named<const ID: u8>;

impl<const ID: u8> Contract for Named<ID> {
  fn name() -> String {
    if ID == 9 { "not-valid".into() } else { "Shared".into() }
  }

  fn contract_type(table: &mut TypeTable) -> Type {
    table.define::<Self>(|_| Definition::Alias(Primitive::U8.into()))
  }
}

struct Counter;

impl Contract for Counter {
  fn name() -> String {
    "Counter".into()
  }

  fn contract_type(table: &mut TypeTable) -> Type {
    table.define::<Self>(|table| Definition::Alias(Type::List(Box::new(table.add::<Self>()))))
  }
}

#[test]
fn starts_empty() {
  assert_eq!(TypeTable::new().finish(), Ok(BTreeMap::new()));
}

#[test]
fn reports_each_collision_once() {
  let mut table = TypeTable::new();
  assert_eq!(table.add::<Named<0>>(), Type::Ref("Shared".into()));
  assert_eq!(table.add::<Named<1>>(), Type::Ref("Shared".into()));
  table.add::<Named<2>>();
  table.add::<Named<0>>();

  assert_eq!(table.finish(), Err(vec![TypeError::Collision { name: "Shared".into() }]));
}

#[test]
fn reports_a_name_outside_the_grammar() {
  let mut table = TypeTable::new();
  table.add::<Named<9>>();

  let errors = table.finish().unwrap_err();
  assert_eq!(
    errors,
    [TypeError::InvalidName {
      name: "not-valid".into()
    }]
  );
  assert_eq!(
    errors[0].to_string(),
    "type name `not-valid` does not match [A-Za-z_][A-Za-z0-9_]* (spec 12.2)"
  );
}

#[test]
fn builds_a_self_referring_definition_once() {
  let mut table = TypeTable::new();
  table.add::<Counter>();

  let types = table.finish().unwrap();
  assert_eq!(
    types["Counter"],
    Definition::Alias(Type::List(Box::new(Type::Ref("Counter".into()))))
  );
}

#[test]
fn says_which_name_collides() {
  assert_eq!(
    TypeError::Collision {
      name: "Page_Message".into()
    }
    .to_string(),
    "two types are named `Page_Message` (spec 12.1)"
  );
}
