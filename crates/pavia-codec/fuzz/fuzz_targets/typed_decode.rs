#![no_main]

use std::collections::BTreeMap;

use libfuzzer_sys::fuzz_target;
use pavia_codec::{Types, decode, encode};
use pavia_contract::schema::{Definition, Enum, Field, Primitive, Struct, Type, Variant};
use pavia_proto::data::{Codec, DecodeLimits};

const PRIMITIVES: [Primitive; 17] = [
  Primitive::Unit,
  Primitive::Bool,
  Primitive::I8,
  Primitive::I16,
  Primitive::I32,
  Primitive::I64,
  Primitive::U8,
  Primitive::U16,
  Primitive::U32,
  Primitive::U64,
  Primitive::F32,
  Primitive::F64,
  Primitive::String,
  Primitive::Bytes,
  Primitive::Timestamp,
  Primitive::Uuid,
  Primitive::Any,
];

fn schema() -> (Type, Types) {
  let mut fields: Vec<Field> = PRIMITIVES
    .into_iter()
    .map(|primitive| Field::new(primitive.name(), Type::from(primitive)).optional())
    .map(|field| Field::new(field.name(), Type::Option(Box::new(field.ty().clone()))).optional())
    .filter(|field| field.name() != "unit")
    .collect();
  let item = Type::Ref("Item".into());
  fields.push(Field::new("items", Type::List(Box::new(item.clone()))).optional());
  fields.push(Field::new("by_name", Type::Map(Box::new(item.clone()))).optional());
  fields.push(Field::new("pair", Type::Tuple(vec![Type::from(Primitive::U8), item])).optional());
  let types = BTreeMap::from([
    ("Rich".into(), Definition::Struct(Struct::new(fields))),
    (
      "Item".into(),
      Definition::Enum(Enum::new(vec![
        Variant::unit("Empty"),
        Variant::new("Rich", Type::Ref("Rich".into())),
        Variant::new("Unit", Type::from(Primitive::Unit)),
      ])),
    ),
  ]);
  (Type::List(Box::new(Type::Ref("Rich".into()))), types)
}

fuzz_target!(|data: &[u8]| {
  let Some((&selector, data)) = data.split_first() else {
    return;
  };
  let codec = if selector % 2 == 0 { Codec::Cbor } else { Codec::Json };
  let (ty, types) = schema();
  let limits = DecodeLimits::default();
  let Ok(value) = decode(&ty, &types, data, codec, &limits) else {
    return;
  };
  let first = encode(&ty, &types, &value, codec).expect("a decoded value encodes");
  let again = decode(&ty, &types, &first, codec, &limits).expect("encoder output decodes");
  assert_eq!(encode(&ty, &types, &again, codec).expect("re-encoding succeeds"), first);
});
