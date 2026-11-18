#![expect(dead_code, reason = "the types exist for their derived impls, which never read a field")]

use std::collections::{BTreeMap, HashMap};

use pavia_contract::schema::{Definition, Type};
use pavia_contract::{Bytes, Contract, Timestamp, TypeError, TypeTable, Uuid};
use serde_json::{Value, json};

#[derive(Contract)]
struct RoomId(String);

#[derive(Contract)]
struct MessageId(Uuid);

#[derive(Contract)]
struct Message {
  id: MessageId,
  author: String,
  text: String,
  sent_at: Timestamp,
}

#[derive(Contract)]
enum SendError {
  RoomNotFound,
  TooLong { max: u32 },
}

#[derive(Contract)]
struct Member {
  display_name: String,
}

#[derive(Contract)]
struct RoomOptions {
  #[pavia(optional)]
  lang: Option<String>,
}

#[derive(Contract)]
struct CursorPos {
  x: f32,
  y: f32,
}

#[derive(Contract)]
struct Page<T> {
  items: Vec<T>,
  next: Option<String>,
}

#[derive(Contract)]
#[expect(non_camel_case_types, reason = "the name `Page<Message>` is given, to collide with it")]
struct Page_Message {
  count: u32,
}

#[derive(Contract)]
#[pavia(open)]
enum Presence {
  Online,
  Away,
}

#[derive(Contract)]
struct Node {
  label: String,
  children: Vec<Node>,
  parent: Option<Box<Node>>,
}

#[derive(Contract)]
#[pavia(rename = "Upload")]
struct Renamed {
  #[pavia(rename = "chunk")]
  data: Bytes,
  r#type: u8,
}

#[derive(Contract)]
enum Shape {
  #[pavia(rename = "Point")]
  Dot,
  Circle(f64),
  Line(f64, f64),
}

#[derive(Contract)]
struct Pair(u32, String);

#[derive(Contract)]
struct UnitFirst((), u32);

#[derive(Contract)]
struct Nothing(());

#[derive(Contract)]
struct Again(Box<Nothing>);

#[derive(Contract)]
struct Chain(Option<Box<Chain>>);

#[derive(Contract)]
struct Doc(HashMap<String, Option<Box<Doc>>>);

#[derive(Contract)]
struct Ping(Option<Box<Pong>>);

#[derive(Contract)]
struct Pong(Vec<Option<Box<Ping>>>);

#[derive(Contract)]
struct Left(Box<Right>);

#[derive(Contract)]
struct Right(Box<Left>);

#[derive(Contract)]
struct Empty;

#[derive(Contract)]
struct Envelope<K, V> {
  key: K,
  values: Vec<V>,
}

fn types(table: TypeTable) -> Value {
  let definitions = table.finish().expect("valid types");
  Value::Object(
    definitions
      .into_iter()
      .map(|(name, definition)| (name, definition.to_json()))
      .collect(),
  )
}

#[test]
fn produces_the_types_table_of_the_spec_example() {
  let mut table = TypeTable::new();
  assert_eq!(table.add::<RoomId>(), Type::Ref("RoomId".into()));
  table.add::<Message>();
  table.add::<SendError>();
  table.add::<Member>();
  table.add::<RoomOptions>();
  table.add::<CursorPos>();

  assert_eq!(
    types(table),
    json!({
      "RoomId": {"alias": "string"},
      "MessageId": {"alias": "uuid"},
      "Message": {"struct": {"fields": [
        {"name": "id", "type": {"ref": "MessageId"}},
        {"name": "author", "type": "string"},
        {"name": "text", "type": "string"},
        {"name": "sent_at", "type": "timestamp"}
      ]}},
      "SendError": {"enum": {"variants": [
        {"name": "RoomNotFound"},
        {"name": "TooLong", "type": {"struct": {"fields": [{"name": "max", "type": "u32"}]}}}
      ]}},
      "Member": {"struct": {"fields": [{"name": "display_name", "type": "string"}]}},
      "RoomOptions": {"struct": {"fields": [
        {"name": "lang", "type": {"option": "string"}, "optional": true}
      ]}},
      "CursorPos": {"struct": {"fields": [
        {"name": "x", "type": "f32"},
        {"name": "y", "type": "f32"}
      ]}}
    }),
  );
}

#[test]
fn names_generic_instantiations_after_their_arguments() {
  assert_eq!(Page::<Message>::name(), "Page_Message");
  assert_eq!(Page::<u32>::name(), "Page_u32");
  assert_eq!(Page::<Vec<Message>>::name(), "Page_List_Message");
  assert_eq!(Envelope::<String, Option<u64>>::name(), "Envelope_string_Option_u64");
}

#[test]
fn defines_each_generic_instantiation_as_its_own_type() {
  let mut table = TypeTable::new();
  assert_eq!(table.add::<Page<Message>>(), Type::Ref("Page_Message".into()));
  table.add::<Page<Vec<Message>>>();
  let types = types(table);

  assert_eq!(
    types["Page_Message"],
    json!({"struct": {"fields": [
      {"name": "items", "type": {"list": {"ref": "Message"}}},
      {"name": "next", "type": {"option": "string"}}
    ]}}),
  );
  assert_eq!(
    types["Page_List_Message"]["struct"]["fields"][0]["type"],
    json!({"list": {"list": {"ref": "Message"}}})
  );
}

#[test]
fn shares_a_name_between_rust_types_with_one_contract_type() {
  let mut table = TypeTable::new();
  table.add::<Page<Box<Message>>>();
  table.add::<Page<Message>>();
  table.add::<Page<HashMap<String, u32>>>();
  table.add::<Page<BTreeMap<String, u32>>>();
  let types = types(table);

  assert_eq!(
    types["Page_Message"]["struct"]["fields"][0]["type"],
    json!({"list": {"ref": "Message"}})
  );
  assert_eq!(
    types["Page_Map_u32"]["struct"]["fields"][0]["type"],
    json!({"list": {"map": "u32"}})
  );
}

#[test]
fn reports_a_generated_name_taken_by_a_user_type() {
  let mut table = TypeTable::new();
  table.add::<Page<Message>>();
  table.add::<Page_Message>();
  table.add::<Page_Message>();

  assert_eq!(
    table.finish().unwrap_err(),
    vec![TypeError::Collision {
      name: "Page_Message".into()
    }]
  );
}

#[test]
fn defines_a_type_used_twice_once() {
  let mut table = TypeTable::new();
  table.add::<Message>();
  table.add::<Vec<Message>>();
  table.add::<Option<MessageId>>();

  assert_eq!(table.finish().unwrap().keys().collect::<Vec<_>>(), ["Message", "MessageId"]);
}

#[test]
fn marks_an_open_enum_open() {
  let mut table = TypeTable::new();
  table.add::<Presence>();

  assert_eq!(
    types(table)["Presence"],
    json!({"enum": {"variants": [{"name": "Online"}, {"name": "Away"}], "open": true}}),
  );
}

#[test]
fn refers_to_a_recursive_type_by_name() {
  let mut table = TypeTable::new();
  table.add::<Node>();

  assert_eq!(
    types(table),
    json!({"Node": {"struct": {"fields": [
      {"name": "label", "type": "string"},
      {"name": "children", "type": {"list": {"ref": "Node"}}},
      {"name": "parent", "type": {"option": {"ref": "Node"}}}
    ]}}}),
  );
}

#[test]
fn uses_renames_and_raw_identifiers_without_their_prefix() {
  let mut table = TypeTable::new();
  assert_eq!(table.add::<Renamed>(), Type::Ref("Upload".into()));

  assert_eq!(
    types(table),
    json!({"Upload": {"struct": {"fields": [
      {"name": "chunk", "type": "bytes"},
      {"name": "type", "type": "u8"}
    ]}}}),
  );
}

#[test]
fn maps_unit_newtype_and_tuple_variants() {
  let mut table = TypeTable::new();
  table.add::<Shape>();

  assert_eq!(
    types(table)["Shape"],
    json!({"enum": {"variants": [
      {"name": "Point"},
      {"name": "Circle", "type": "f64"},
      {"name": "Line", "type": {"tuple": ["f64", "f64"]}}
    ]}}),
  );
}

#[test]
fn maps_a_tuple_struct_to_an_alias_of_a_tuple_and_a_unit_struct_to_a_struct_without_fields() {
  let mut table = TypeTable::new();
  table.add::<Pair>();
  table.add::<Empty>();
  let types = table.finish().unwrap();

  assert_eq!(types["Pair"].to_json(), json!({"alias": {"tuple": ["u32", "string"]}}));
  assert!(matches!(&types["Empty"], Definition::Struct(definition) if definition.fields().is_empty()));
  assert_eq!(types["Empty"].to_json(), json!({"struct": {"fields": []}}));
}

#[test]
fn lets_option_wrap_a_tuple_struct_with_a_unit_field() {
  let mut table = TypeTable::new();
  assert_eq!(
    table.add::<Option<UnitFirst>>().to_json(),
    json!({"option": {"ref": "UnitFirst"}})
  );
  let types = types(table);

  assert_eq!(types["UnitFirst"], json!({"alias": {"tuple": ["unit", "u32"]}}));
}

#[test]
fn refuses_an_option_of_an_alias_of_unit() {
  let mut table = TypeTable::new();
  table.add::<Nothing>();
  table.add::<Option<Again>>();
  table.add::<Option<Box<Nothing>>>();

  let errors = table.finish().unwrap_err();
  assert_eq!(
    errors,
    [
      TypeError::OptionOfUnit { name: "Again".into() },
      TypeError::OptionOfUnit { name: "Nothing".into() },
    ]
  );
  assert_eq!(
    errors[0].to_string(),
    r#"`Again` is `unit` through an alias, and an `Option` wraps it: `{"option": "unit"}` is not allowed (spec 12.1)"#
  );
}

#[test]
fn maps_aliases_that_recurse_through_an_option() {
  let mut table = TypeTable::new();
  table.add::<Chain>();
  table.add::<Doc>();
  table.add::<Ping>();
  table.add::<Option<Left>>();
  let types = types(table);

  assert_eq!(types["Chain"], json!({"alias": {"option": {"ref": "Chain"}}}));
  assert_eq!(types["Doc"], json!({"alias": {"map": {"option": {"ref": "Doc"}}}}));
  assert_eq!(types["Ping"], json!({"alias": {"option": {"ref": "Pong"}}}));
  assert_eq!(types["Pong"], json!({"alias": {"list": {"option": {"ref": "Ping"}}}}));
  assert_eq!(types["Left"], json!({"alias": {"ref": "Right"}}));
}
