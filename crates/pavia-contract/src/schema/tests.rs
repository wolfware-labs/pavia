use serde_json::json;

use super::{Definition, Enum, Field, Primitive, Struct, Type, Variant};

#[test]
fn names_every_primitive_as_the_spec_does() {
  let names = [
    (Primitive::Unit, "unit"),
    (Primitive::Bool, "bool"),
    (Primitive::I8, "i8"),
    (Primitive::I16, "i16"),
    (Primitive::I32, "i32"),
    (Primitive::I64, "i64"),
    (Primitive::U8, "u8"),
    (Primitive::U16, "u16"),
    (Primitive::U32, "u32"),
    (Primitive::U64, "u64"),
    (Primitive::F32, "f32"),
    (Primitive::F64, "f64"),
    (Primitive::String, "string"),
    (Primitive::Bytes, "bytes"),
    (Primitive::Timestamp, "timestamp"),
    (Primitive::Uuid, "uuid"),
    (Primitive::Any, "any"),
  ];
  for (primitive, name) in names {
    assert_eq!(primitive.name(), name);
    assert_eq!(Type::from(primitive).to_json(), json!(name));
  }
}

#[test]
fn writes_composite_types_as_their_manifest_json() {
  let string = || Type::from(Primitive::String);
  assert_eq!(Type::Option(Box::new(string())).to_json(), json!({"option": "string"}));
  assert_eq!(Type::List(Box::new(string())).to_json(), json!({"list": "string"}));
  assert_eq!(
    Type::Map(Box::new(Type::Ref("Member".into()))).to_json(),
    json!({"map": {"ref": "Member"}})
  );
  assert_eq!(
    Type::Tuple(vec![string(), Primitive::U8.into()]).to_json(),
    json!({"tuple": ["string", "u8"]})
  );
  assert_eq!(
    Type::Struct(Struct::new(vec![Field::new("max", Primitive::U32.into())])).to_json(),
    json!({"struct": {"fields": [{"name": "max", "type": "u32"}]}}),
  );
}

#[test]
fn writes_optional_and_open_only_when_set() {
  let field = Field::new("lang", Type::Option(Box::new(Primitive::String.into())));
  let optional = Field::optional("lang", Type::Option(Box::new(Primitive::String.into())));
  assert!(!field.is_optional());
  assert!(optional.is_optional());
  assert_eq!(
    Definition::Struct(Struct::new(vec![field, optional])).to_json(),
    json!({"struct": {"fields": [
      {"name": "lang", "type": {"option": "string"}},
      {"name": "lang", "type": {"option": "string"}, "optional": true}
    ]}}),
  );

  let variants = || vec![Variant::unit("Online"), Variant::new("Away", Primitive::U64.into())];
  assert!(!Enum::new(variants()).is_open());
  assert!(Enum::open(variants()).is_open());
  assert_eq!(
    Definition::Enum(Enum::new(variants())).to_json(),
    json!({"enum": {"variants": [{"name": "Online"}, {"name": "Away", "type": "u64"}]}}),
  );
  assert_eq!(
    Definition::Enum(Enum::open(variants())).to_json()["enum"]["open"],
    json!(true)
  );
}

#[test]
fn exposes_what_it_was_built_from() {
  let field = Field::new("x", Primitive::F32.into());
  assert_eq!((field.name(), field.ty()), ("x", &Type::from(Primitive::F32)));
  let definition = Struct::new(vec![field.clone()]);
  assert_eq!(definition.fields(), [field]);

  let unit = Variant::unit("RoomNotFound");
  let carrying = Variant::new("TooLong", Primitive::U32.into());
  assert_eq!((unit.name(), unit.ty()), ("RoomNotFound", None));
  assert_eq!(carrying.ty(), Some(&Type::from(Primitive::U32)));
  assert_eq!(Enum::new(vec![unit.clone()]).variants(), [unit]);

  assert_eq!(Definition::Alias(Primitive::Uuid.into()).to_json(), json!({"alias": "uuid"}));
}
