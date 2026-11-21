use pavia_contract::schema::{Definition, Enum, Field, Primitive, Struct, Type, Types, Variant};
use pavia_proto::data::{Codec, DecodeError, DecodeLimits, EncodeError, Value};
use proptest::prelude::*;

use super::{Problem, cbor_order, decode, encode, scalar};

fn primitive(primitive: Primitive) -> Type {
  Type::from(primitive)
}

fn reference(name: &str) -> Type {
  Type::Ref(name.into())
}

fn text(text: &str) -> Value {
  Value::Text(text.into())
}

fn map(entries: &[(&str, Value)]) -> Value {
  Value::Map(entries.iter().map(|(key, value)| (text(key), value.clone())).collect())
}

fn types() -> Types {
  Types::from([
    (
      "Message".into(),
      Definition::Struct(Struct::new(vec![
        Field::new("id", primitive(Primitive::U32)),
        Field::new("tags", Type::List(Box::new(primitive(Primitive::String)))),
        Field::new("note", Type::Option(Box::new(primitive(Primitive::String)))).optional(),
      ])),
    ),
    (
      "Status".into(),
      Definition::Enum(Enum::new(vec![
        Variant::unit("Idle"),
        Variant::new("Busy", primitive(Primitive::U8)),
      ])),
    ),
    ("Loop".into(), Definition::Alias(reference("Again"))),
    ("Again".into(), Definition::Alias(reference("Loop"))),
    (
      "Nested".into(),
      Definition::Alias(Type::Option(Box::new(reference("Nested")))),
    ),
  ])
}

fn problem_of(ty: &Type, value: &Value, codec: Codec) -> (String, Problem) {
  let error = encode(ty, &types(), value, codec, &DecodeLimits::default()).unwrap_err();
  (error.at().to_owned(), error.problem().clone())
}

fn decode_problem(ty: &Type, data: &[u8], codec: Codec) -> (String, Problem) {
  let error = decode(ty, &types(), data, codec, &DecodeLimits::default()).unwrap_err();
  (error.at().to_owned(), error.problem().clone())
}

#[test]
fn names_where_an_encoded_value_does_not_fit() {
  let message = reference("Message");
  let cases = [
    (
      map(&[
        ("id", Value::Unsigned(1)),
        ("tags", Value::Array(vec![text("a"), Value::Unsigned(2)])),
      ]),
      ("$.tags[1]", Problem::Expected("string")),
    ),
    (
      map(&[("id", Value::Unsigned(1))]),
      ("$", Problem::MissingField("tags".into())),
    ),
    (
      map(&[
        ("id", Value::Unsigned(1)),
        ("tags", Value::Array(vec![])),
        ("extra", Value::Null),
      ]),
      ("$", Problem::UnknownField("extra".into())),
    ),
    (
      map(&[("id", Value::Unsigned(1)), ("id", Value::Unsigned(2))]),
      ("$", Problem::DuplicateKey("id".into())),
    ),
    (
      map(&[("id", Value::Negative(0)), ("tags", Value::Array(vec![]))]),
      ("$.id", Problem::OutOfRange("u32")),
    ),
    (text("message"), ("$", Problem::Expected("a map"))),
    (
      Value::Map(vec![(Value::Unsigned(1), Value::Null)]),
      ("$", Problem::Expected("text keys")),
    ),
  ];
  for (value, (at, problem)) in cases {
    assert_eq!(problem_of(&message, &value, Codec::Cbor), (at.to_owned(), problem));
  }
}

#[test]
fn refuses_variants_tuples_and_types_that_do_not_fit() {
  let status = reference("Status");
  assert_eq!(
    problem_of(&status, &text("Gone"), Codec::Json),
    ("$".into(), Problem::UnknownVariant("Gone".into()))
  );
  assert_eq!(
    problem_of(&status, &map(&[("Idle", Value::Null)]), Codec::Json),
    ("$".into(), Problem::UnknownVariant("Idle".into()))
  );
  assert_eq!(
    problem_of(&status, &text("Busy"), Codec::Json),
    ("$".into(), Problem::UnknownVariant("Busy".into()))
  );
  assert_eq!(
    problem_of(&status, &map(&[("Busy", Value::Unsigned(300))]), Codec::Json),
    ("$.Busy".into(), Problem::OutOfRange("u8"))
  );
  assert_eq!(
    problem_of(&status, &Value::Unsigned(1), Codec::Json),
    ("$".into(), Problem::Expected("a variant name or a one-entry map"))
  );
  let pair = Type::Tuple(vec![primitive(Primitive::U8), primitive(Primitive::Bool)]);
  assert_eq!(
    problem_of(&pair, &Value::Array(vec![Value::Unsigned(1)]), Codec::Cbor),
    ("$".into(), Problem::TupleLength { expected: 2, found: 1 })
  );
  assert_eq!(
    problem_of(&pair, &Value::Array(vec![Value::Unsigned(1), Value::Null]), Codec::Cbor),
    ("$[1]".into(), Problem::Expected("bool"))
  );
  assert_eq!(
    problem_of(&reference("Missing"), &Value::Null, Codec::Cbor),
    ("$".into(), Problem::UnknownType("Missing".into()))
  );
  assert_eq!(
    problem_of(&reference("Loop"), &Value::Null, Codec::Cbor),
    ("$".into(), Problem::UnknownType("Loop".into()))
  );
}

#[test]
fn refuses_scalars_outside_their_type() {
  let cases = [
    (Primitive::F32, Value::Float(1e40), Problem::OutOfRange("f32")),
    (Primitive::F32, Value::Float(0.1), Problem::OutOfRange("f32")),
    (Primitive::F64, Value::Unsigned(1), Problem::Expected("f64")),
    (
      Primitive::Timestamp,
      Value::Tag(1, Box::new(Value::Float(f64::NAN))),
      Problem::OutOfRange("timestamp"),
    ),
    (
      Primitive::Timestamp,
      Value::Tag(0, Box::new(text("2025"))),
      Problem::Expected("timestamp"),
    ),
    (
      Primitive::Uuid,
      Value::Tag(37, Box::new(Value::Bytes(vec![0; 15]))),
      Problem::Expected("16 bytes in tag 37"),
    ),
    (
      Primitive::Uuid,
      Value::Tag(37, Box::new(text("x"))),
      Problem::Expected("16 bytes in tag 37"),
    ),
    (Primitive::Bytes, text("AA"), Problem::Expected("bytes")),
    (Primitive::Unit, Value::Bool(false), Problem::Expected("unit")),
    (Primitive::I64, Value::Unsigned(u64::MAX), Problem::OutOfRange("i64")),
  ];
  for (primitive, value, expected) in cases {
    for codec in [Codec::Cbor, Codec::Json] {
      assert_eq!(
        problem_of(&Type::from(primitive), &value, codec).1,
        expected,
        "{primitive:?} {value:?}"
      );
    }
  }
  assert_eq!(
    problem_of(&Type::from(Primitive::F32), &Value::Float(f64::NAN), Codec::Json).1,
    Problem::Encode(EncodeError::NotJson)
  );
}

#[test]
fn passes_any_through_and_refuses_it_where_the_codec_has_no_form() {
  let any = Type::from(Primitive::Any);
  let tagged = Value::Map(vec![(Value::Unsigned(1), Value::Tag(2, Box::new(Value::Bytes(vec![1]))))]);
  let cbor = encode(&any, &types(), &tagged, Codec::Cbor, &DecodeLimits::default()).unwrap();
  assert_eq!(
    decode(&any, &types(), &cbor, Codec::Cbor, &DecodeLimits::default()),
    Ok(tagged.clone())
  );
  assert_eq!(
    problem_of(&any, &tagged, Codec::Json),
    ("$".into(), Problem::Encode(EncodeError::NotJson))
  );
}

#[test]
fn reports_where_a_decoded_value_does_not_fit() {
  let list = Type::List(Box::new(reference("Status")));
  assert_eq!(
    decode_problem(&list, br#"["Idle",{"Busy":"7"}]"#, Codec::Json),
    ("$[1].Busy".into(), Problem::Expected("u8"))
  );
  assert_eq!(
    decode_problem(&list, br#"["Idle",{"Busy":1,"Idle":2}]"#, Codec::Json),
    ("$[1]".into(), Problem::Expected("a variant name or a one-entry map"))
  );
  assert_eq!(
    decode_problem(&list, &[0x81, 0xa1, 0x01, 0x01], Codec::Cbor),
    ("$[0]".into(), Problem::Expected("a variant name or a one-entry map"))
  );
  assert_eq!(
    decode_problem(&reference("Message"), br#"{"id":1,"tags":[],"x":1}"#, Codec::Json),
    ("$".into(), Problem::UnknownField("x".into()))
  );
  assert_eq!(
    decode_problem(&primitive(Primitive::U8), b"[", Codec::Json),
    ("$".into(), Problem::Decode(DecodeError::Malformed))
  );
  assert_eq!(
    decode_problem(
      &primitive(Primitive::F32),
      &[0xfb, 0x48, 0x3d, 0x6e, 0x3d, 0x00, 0, 0, 0],
      Codec::Cbor
    ),
    ("$".into(), Problem::OutOfRange("f32"))
  );
  assert_eq!(
    decode_problem(&primitive(Primitive::Bool), &[0xc5, 0xf5], Codec::Cbor),
    ("$".into(), Problem::Tag(5))
  );
  assert_eq!(
    decode_problem(&primitive(Primitive::Bytes), b"\"AA==\"", Codec::Json),
    ("$".into(), Problem::Expected("unpadded base64url"))
  );
  assert_eq!(
    decode_problem(
      &Type::Map(Box::new(primitive(Primitive::U8))),
      &[0xa1, 0x01, 0x01],
      Codec::Cbor
    ),
    ("$".into(), Problem::Expected("text keys"))
  );
  assert_eq!(
    decode_problem(&Type::Tuple(vec![primitive(Primitive::U8)]), b"{}", Codec::Json),
    ("$".into(), Problem::Expected("an array"))
  );
}

#[test]
fn refuses_a_type_that_loops_through_options_without_reading_the_value() {
  let nested = reference("Nested");
  let refused = ("$".to_owned(), Problem::UnknownType("Nested".into()));
  for codec in [Codec::Cbor, Codec::Json] {
    assert_eq!(problem_of(&nested, &Value::Unsigned(1), codec), refused, "{codec:?}");
    assert_eq!(
      encode(&nested, &types(), &Value::Null, codec, &DecodeLimits::default()).map(|_| ()),
      Ok(()),
      "{codec:?}"
    );
  }
  assert_eq!(decode_problem(&nested, &[0x01], Codec::Cbor), refused);
  assert_eq!(decode_problem(&nested, b"1", Codec::Json), refused);
  assert_eq!(
    decode(&nested, &types(), &[0xf6], Codec::Cbor, &DecodeLimits::default()),
    Ok(Value::Null)
  );
  let doubled = Type::Option(Box::new(Type::Option(Box::new(primitive(Primitive::U8)))));
  assert_eq!(
    decode(&doubled, &types(), b"7", Codec::Json, &DecodeLimits::default()),
    Ok(Value::Unsigned(7))
  );
}

#[test]
fn reads_a_json_integer_as_a_float_and_rounds_f32() {
  let limits = DecodeLimits::default();
  assert_eq!(
    decode(&primitive(Primitive::F64), &types(), b"-3", Codec::Json, &limits),
    Ok(Value::Float(-3.0))
  );
  assert_eq!(
    decode(&primitive(Primitive::F32), &types(), b"0.1", Codec::Json, &limits),
    Ok(Value::Float(f64::from(0.1_f32)))
  );
  assert_eq!(
    decode(&primitive(Primitive::F64), &types(), &[0x01], Codec::Cbor, &limits).map_err(|error| error.to_string()),
    Err("$: expected f64".to_owned())
  );
}

fn rich() -> (Type, Types) {
  let field = |name: &str, primitive: Primitive| Field::new(name, Type::from(primitive));
  let mut fields: Vec<Field> = [
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
  ]
  .into_iter()
  .map(|primitive| field(primitive.name(), primitive))
  .collect();
  fields.push(Field::new("maybe", Type::Option(Box::new(primitive(Primitive::U8)))).optional());
  fields.push(Field::new("list", Type::List(Box::new(primitive(Primitive::I16)))));
  fields.push(Field::new("map", Type::Map(Box::new(primitive(Primitive::Bool)))));
  fields.push(Field::new(
    "pair",
    Type::Tuple(vec![primitive(Primitive::U8), primitive(Primitive::String)]),
  ));
  fields.push(Field::new("status", reference("Status")));
  let types = Types::from([
    ("Rich".into(), Definition::Struct(Struct::new(fields))),
    (
      "Status".into(),
      Definition::Enum(Enum::new(vec![
        Variant::unit("Idle"),
        Variant::new("Busy", primitive(Primitive::U8)),
      ])),
    ),
  ]);
  (reference("Rich"), types)
}

fn integer_in(low: i128, high: i128) -> impl Strategy<Value = Value> {
  (low..=high).prop_map(scalar::integer)
}

fn rich_value() -> impl Strategy<Value = Value> {
  let scalars = (
    any::<bool>(),
    integer_in(i128::from(i8::MIN), i128::from(i8::MAX)),
    integer_in(i128::from(i16::MIN), i128::from(i16::MAX)),
    integer_in(i128::from(i32::MIN), i128::from(i32::MAX)),
    integer_in(i128::from(i64::MIN), i128::from(i64::MAX)),
    integer_in(0, i128::from(u8::MAX)),
    integer_in(0, i128::from(u16::MAX)),
    integer_in(0, i128::from(u32::MAX)),
    integer_in(0, i128::from(u64::MAX)),
  );
  let others = (
    any::<f32>().prop_filter("finite", |float| float.is_finite()),
    any::<f64>().prop_filter("finite", |float| float.is_finite()),
    ".*",
    proptest::collection::vec(any::<u8>(), 0..8),
    scalar::MIN_MILLIS..=scalar::MAX_MILLIS,
    any::<[u8; 16]>(),
    proptest::option::of(any::<u8>()),
    proptest::collection::vec(any::<i16>(), 0..4),
    proptest::collection::btree_map(".{0,3}", any::<bool>(), 0..4),
    (any::<u8>(), ".{0,3}"),
    proptest::option::of(any::<u8>()),
  );
  (scalars, others).prop_map(
    |(
      (flag, i8, i16, i32, i64, u8, u16, u32, u64),
      (f32, f64, string, bytes, millis, uuid, maybe, list, entries, (first, second), busy),
    )| {
      let mut entries: Vec<(String, bool)> = entries.into_iter().collect();
      entries.sort_by(|(left, _), (right, _)| cbor_order(left, right));
      let mut fields = vec![
        ("unit", Value::Null),
        ("bool", Value::Bool(flag)),
        ("i8", i8),
        ("i16", i16),
        ("i32", i32),
        ("i64", i64),
        ("u8", u8),
        ("u16", u16),
        ("u32", u32),
        ("u64", u64),
        ("f32", Value::Float(f64::from(f32))),
        ("f64", Value::Float(f64)),
        ("string", Value::Text(string)),
        ("bytes", Value::Bytes(bytes)),
        ("timestamp", Value::Tag(1, Box::new(scalar::epoch_seconds(millis)))),
        ("uuid", Value::Tag(37, Box::new(Value::Bytes(uuid.to_vec())))),
      ];
      if let Some(maybe) = maybe {
        fields.push(("maybe", Value::Unsigned(u64::from(maybe))));
      }
      fields.push((
        "list",
        Value::Array(list.into_iter().map(|item| scalar::integer(i128::from(item))).collect()),
      ));
      fields.push((
        "map",
        Value::Map(
          entries
            .into_iter()
            .map(|(key, flag)| (Value::Text(key), Value::Bool(flag)))
            .collect(),
        ),
      ));
      fields.push((
        "pair",
        Value::Array(vec![Value::Unsigned(u64::from(first)), Value::Text(second)]),
      ));
      fields.push((
        "status",
        busy.map_or_else(|| text("Idle"), |busy| map(&[("Busy", Value::Unsigned(u64::from(busy)))])),
      ));
      map(&fields)
    },
  )
}

proptest! {
  #[test]
  fn every_type_round_trips_in_both_codecs(value in rich_value()) {
    let (ty, types) = rich();
    for codec in [Codec::Cbor, Codec::Json] {
      let encoded = encode(&ty, &types, &value, codec, &DecodeLimits::default()).unwrap();
      let decoded = decode(&ty, &types, &encoded, codec, &DecodeLimits::default()).unwrap();
      prop_assert_eq!(&decoded, &value);
      prop_assert_eq!(encode(&ty, &types, &decoded, codec, &DecodeLimits::default()).unwrap(), encoded);
    }
  }
}

#[test]
fn refuses_to_encode_a_value_deeper_than_the_receiver_takes() {
  let tree = Types::from([(
    "Tree".into(),
    Definition::Struct(Struct::new(vec![Field::new("kids", Type::List(Box::new(reference("Tree"))))])),
  )]);
  let mut value = map(&[("kids", Value::Array(vec![]))]);
  for _ in 0..7 {
    value = map(&[("kids", Value::Array(vec![value]))]);
  }
  let limits = DecodeLimits::default();
  for codec in [Codec::Cbor, Codec::Json] {
    let encoded = encode(&reference("Tree"), &tree, &value, codec, &limits).unwrap();
    assert_eq!(decode(&reference("Tree"), &tree, &encoded, codec, &limits), Ok(value.clone()));
    let deeper = map(&[("kids", Value::Array(vec![value.clone()]))]);
    let error = encode(&reference("Tree"), &tree, &deeper, codec, &limits).unwrap_err();
    assert_eq!((error.at(), error.problem()), ("$", &Problem::TooDeep));
    let tagged = Value::Array(vec![Value::Tag(1, Box::new(Value::Unsigned(0)))]);
    let list = Type::List(Box::new(primitive(Primitive::Timestamp)));
    assert!(encode(&list, &tree, &tagged, codec, &DecodeLimits { max_depth: 1 }).is_ok());
  }
  let tags = (0..=limits.max_depth).fold(Value::Unsigned(0), |item, _| Value::Tag(2, Box::new(item)));
  let error = encode(&primitive(Primitive::Any), &tree, &tags, Codec::Cbor, &limits).unwrap_err();
  assert_eq!((error.at(), error.problem()), ("$", &Problem::TooDeep));
}

#[test]
fn names_what_the_container_needs_in_both_directions() {
  let list = Type::List(Box::new(primitive(Primitive::U8)));
  let pair = Type::Tuple(vec![primitive(Primitive::U8)]);
  let by_name = Type::Map(Box::new(primitive(Primitive::U8)));
  assert_eq!(
    problem_of(&list, &Value::Null, Codec::Cbor),
    ("$".into(), Problem::Expected("an array"))
  );
  assert_eq!(
    problem_of(&pair, &Value::Null, Codec::Cbor),
    ("$".into(), Problem::Expected("an array"))
  );
  assert_eq!(
    problem_of(&by_name, &map(&[("a", text("x"))]), Codec::Json),
    ("$[\"a\"]".into(), Problem::Expected("u8"))
  );
  assert_eq!(
    decode_problem(&list, b"{}", Codec::Json),
    ("$".into(), Problem::Expected("an array"))
  );
  assert_eq!(
    decode_problem(&list, b"[1,\"x\"]", Codec::Json),
    ("$[1]".into(), Problem::Expected("u8"))
  );
  assert_eq!(
    decode_problem(&by_name, br#"{"a":"x"}"#, Codec::Json),
    ("$[\"a\"]".into(), Problem::Expected("u8"))
  );
  assert_eq!(
    decode_problem(&pair, b"[\"x\"]", Codec::Json),
    ("$[0]".into(), Problem::Expected("u8"))
  );
  assert_eq!(
    decode_problem(&reference("Message"), b"[]", Codec::Json),
    ("$".into(), Problem::Expected("a map"))
  );
  assert_eq!(
    decode_problem(&reference("Status"), &[0x01], Codec::Cbor),
    ("$".into(), Problem::Expected("a variant name or a one-entry map"))
  );
  assert_eq!(
    decode_problem(&primitive(Primitive::Bool), &[0x01], Codec::Cbor),
    ("$".into(), Problem::Expected("bool"))
  );
  assert_eq!(
    decode_problem(&primitive(Primitive::Bytes), b"1", Codec::Json),
    ("$".into(), Problem::Expected("bytes"))
  );
}

#[test]
fn refuses_a_variant_map_whose_key_is_not_a_name() {
  let entry = Value::Map(vec![(Value::Unsigned(1), Value::Unsigned(1))]);
  for codec in [Codec::Cbor, Codec::Json] {
    assert_eq!(
      problem_of(&reference("Status"), &entry, codec),
      ("$".into(), Problem::Expected("a variant name or a one-entry map"))
    );
  }
}
