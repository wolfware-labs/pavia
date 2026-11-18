use std::collections::BTreeMap;

use super::{TypeError, TypeTable};
use crate::schema::{Definition, Primitive, Type};
use crate::{Contract, OptionValue};

struct Named<const ID: u8>;

impl<const ID: u8> Contract for Named<ID> {
  fn name() -> String {
    if ID == 9 { "not-valid".into() } else { "Shared".into() }
  }

  fn contract_type(table: &mut TypeTable) -> Type {
    table.define::<Self>(|_| Definition::Alias(if ID == 1 { Primitive::U16 } else { Primitive::U8 }.into()))
  }
}

struct Built;

thread_local! {
  static BUILDS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

impl Contract for Built {
  fn name() -> String {
    "Built".into()
  }

  fn contract_type(table: &mut TypeTable) -> Type {
    table.define::<Self>(|_| {
      BUILDS.set(BUILDS.get() + 1);
      Definition::Alias(Primitive::Bool.into())
    })
  }
}

impl OptionValue for Built {}

struct Counter;

impl Contract for Counter {
  fn name() -> String {
    "Counter".into()
  }

  fn contract_type(table: &mut TypeTable) -> Type {
    table.define::<Self>(|table| Definition::Alias(Type::List(Box::new(table.add::<Self>()))))
  }
}

struct Nothing;

impl Contract for Nothing {
  fn name() -> String {
    "Nothing".into()
  }

  fn contract_type(table: &mut TypeTable) -> Type {
    table.define::<Self>(|_| Definition::Alias(Primitive::Unit.into()))
  }
}

impl OptionValue for Nothing {}

struct Again;

impl Contract for Again {
  fn name() -> String {
    "Again".into()
  }

  fn contract_type(table: &mut TypeTable) -> Type {
    table.define::<Self>(|table| Definition::Alias(table.add::<Nothing>()))
  }
}

impl OptionValue for Again {}

struct Cycle;

impl Contract for Cycle {
  fn name() -> String {
    "Cycle".into()
  }

  fn contract_type(table: &mut TypeTable) -> Type {
    table.define::<Self>(|table| Definition::Alias(table.add::<Self>()))
  }
}

impl OptionValue for Cycle {}

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
fn shares_a_name_between_types_with_one_definition() {
  let mut table = TypeTable::new();
  table.add::<Named<0>>();
  table.add::<Named<2>>();

  let types = table.finish().unwrap();
  assert_eq!(types["Shared"], Definition::Alias(Primitive::U8.into()));
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
    "`not-valid` is not a contract name: names match [A-Za-z_][A-Za-z0-9_]* (spec 12.2)"
  );
}

#[test]
fn builds_each_type_once() {
  let mut table = TypeTable::new();
  table.add::<Built>();
  table.add::<Built>();
  table.add::<Option<Built>>();

  assert_eq!(BUILDS.get(), 1);
  assert_eq!(table.finish().unwrap().len(), 1);
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

#[test]
fn reports_an_option_of_a_type_that_is_unit_through_aliases() {
  let mut table = TypeTable::new();
  table.add::<Option<Again>>();
  table.add::<Option<Nothing>>();

  assert_eq!(
    table.finish(),
    Err(vec![
      TypeError::OptionOfUnit { name: "Again".into() },
      TypeError::OptionOfUnit { name: "Nothing".into() },
    ])
  );
}

#[test]
fn accepts_an_alias_of_unit_outside_an_option() {
  let mut table = TypeTable::new();
  table.add::<Again>();

  assert_eq!(table.finish().unwrap().len(), 2);
}

#[test]
fn accepts_an_option_of_an_alias_that_refers_to_itself() {
  let mut table = TypeTable::new();
  table.add::<Option<Cycle>>();

  let types = table.finish().unwrap();
  assert_eq!(types["Cycle"], Definition::Alias(Type::Ref("Cycle".into())));
}
