use std::collections::{BTreeMap, HashMap};

use serde_json::json;

use crate::schema::{Primitive, Type};
use crate::{Bytes, Contract, Timestamp, TypeTable, Uuid};

fn json_of<T: Contract>() -> serde_json::Value {
  let mut table = TypeTable::new();
  let ty = table.add::<T>();
  assert!(table.finish().unwrap().is_empty());
  ty.to_json()
}

#[test]
fn maps_each_primitive() {
  assert_eq!(json_of::<()>(), json!("unit"));
  assert_eq!(json_of::<bool>(), json!("bool"));
  assert_eq!(json_of::<i8>(), json!("i8"));
  assert_eq!(json_of::<i16>(), json!("i16"));
  assert_eq!(json_of::<i32>(), json!("i32"));
  assert_eq!(json_of::<i64>(), json!("i64"));
  assert_eq!(json_of::<u8>(), json!("u8"));
  assert_eq!(json_of::<u16>(), json!("u16"));
  assert_eq!(json_of::<u32>(), json!("u32"));
  assert_eq!(json_of::<u64>(), json!("u64"));
  assert_eq!(json_of::<f32>(), json!("f32"));
  assert_eq!(json_of::<f64>(), json!("f64"));
  assert_eq!(json_of::<String>(), json!("string"));
  assert_eq!(json_of::<Bytes>(), json!("bytes"));
  assert_eq!(json_of::<Timestamp>(), json!("timestamp"));
  assert_eq!(json_of::<Uuid>(), json!("uuid"));
}

#[test]
fn maps_std_collections() {
  assert_eq!(json_of::<Option<u32>>(), json!({"option": "u32"}));
  assert_eq!(json_of::<Option<Option<u32>>>(), json!({"option": {"option": "u32"}}));
  assert_eq!(json_of::<Vec<u8>>(), json!({"list": "u8"}));
  assert_eq!(json_of::<Box<String>>(), json!("string"));
  assert_eq!(json_of::<HashMap<String, bool>>(), json!({"map": "bool"}));
  assert_eq!(json_of::<BTreeMap<String, Vec<i64>>>(), json!({"map": {"list": "i64"}}));
  assert_eq!(json_of::<(u8,)>(), json!({"tuple": ["u8"]}));
  assert_eq!(
    json_of::<(u8, u16, u32, u64, i8, i16, i32, i64)>(),
    json!({"tuple": ["u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64"]}),
  );
}

#[test]
fn lets_option_wrap_every_composite() {
  assert_eq!(json_of::<Option<Vec<u8>>>(), json!({"option": {"list": "u8"}}));
  assert_eq!(json_of::<Option<Box<u8>>>(), json!({"option": "u8"}));
  assert_eq!(json_of::<Option<HashMap<String, u8>>>(), json!({"option": {"map": "u8"}}));
  assert_eq!(json_of::<Option<BTreeMap<String, u8>>>(), json!({"option": {"map": "u8"}}));
  assert_eq!(json_of::<Option<(u8, bool)>>(), json!({"option": {"tuple": ["u8", "bool"]}}));
  assert_eq!(json_of::<Option<Bytes>>(), json!({"option": "bytes"}));
}

#[test]
fn names_arguments_by_primitive_or_composite() {
  assert_eq!(<()>::name(), "unit");
  assert_eq!(u32::name(), "u32");
  assert_eq!(String::name(), "string");
  assert_eq!(Timestamp::name(), "timestamp");
  assert_eq!(<Vec<Uuid>>::name(), "List_uuid");
  assert_eq!(<Option<bool>>::name(), "Option_bool");
  assert_eq!(<Box<f64>>::name(), "f64");
  assert_eq!(<HashMap<String, Vec<u8>>>::name(), "Map_List_u8");
  assert_eq!(<BTreeMap<String, u8>>::name(), "Map_u8");
  assert_eq!(<(u8, String)>::name(), "Tuple_u8_string");
}

#[test]
fn maps_the_unit_primitive_to_unit() {
  assert_eq!(TypeTable::new().add::<()>(), Type::Primitive(Primitive::Unit));
}
