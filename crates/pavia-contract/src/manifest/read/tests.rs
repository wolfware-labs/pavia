use std::collections::BTreeMap;

use serde::de;
use serde_json::{Value, json};

use super::{MAX_MANIFEST_BYTES, MAX_TYPE_DEPTH, ReadError, StrictVisitor};
use crate::manifest::tests::spec_example;
use crate::manifest::{Call, Channel, Datagram, Direction, Manifest, ManifestError, Method, Mode, Param, Service, Shape};
use crate::schema::{Definition, Enum, Field, Primitive, Struct, Type, Variant};

fn written() -> Value {
  serde_json::from_str(&spec_example().render().unwrap()).unwrap()
}

fn read(document: &Value) -> Result<Manifest, ReadError> {
  Manifest::read(&document.to_string())
}

fn form(document: &Value) -> (String, String) {
  match read(document) {
    Err(ReadError::Form { at, problem }) => (at, problem),
    other => panic!("expected a form error, got {other:?}"),
  }
}

fn nested(depth: usize) -> Value {
  let mut ty = json!("u8");
  for _ in 1..depth {
    ty = json!({ "list": ty });
  }
  ty
}

fn with_type(definition: Value) -> Value {
  let mut document = written();
  document["types"]["Deep"] = definition;
  document
}

fn with_alias(ty: Value) -> Value {
  with_type(json!({ "alias": ty }))
}

fn too_deep(document: &Value) -> String {
  match read(document) {
    Err(ReadError::TooDeep { at, limit }) => {
      assert_eq!(limit, MAX_TYPE_DEPTH);
      at
    }
    other => panic!("expected a depth error, got {other:?}"),
  }
}

#[test]
fn reads_what_the_writer_writes_and_writes_it_back_the_same() {
  let text = spec_example().render().unwrap();
  let manifest = Manifest::read(&text).unwrap();
  assert_eq!(manifest.render().unwrap(), text);
  assert_eq!(manifest.fingerprint().unwrap(), spec_example().fingerprint().unwrap());
}

#[test]
fn reads_defaults_written_out_and_named_entries_in_any_order_to_the_same_bytes_and_fingerprint() {
  let mut document = written();
  let methods = document["services"][0]["server_methods"].as_array_mut().unwrap();
  methods.reverse();
  for method in methods.iter_mut().filter(|method| method["kind"] == "call") {
    method["idempotent"] = json!(false);
    if method.get("params").is_none() {
      method["params"] = json!([]);
    }
  }
  document["types"]["SendError"]["enum"]["open"] = json!(false);
  document["types"]["Message"]["struct"]["fields"][0]["optional"] = json!(false);
  document["services"].as_array_mut().unwrap().push(json!({ "name": "aaa" }));
  document["services"].as_array_mut().unwrap().reverse();
  let read = read(&document).unwrap();
  let expected = spec_example().service(Service::new("aaa"));
  assert_eq!(read.render().unwrap(), expected.render().unwrap());
  assert_eq!(read.fingerprint().unwrap(), expected.fingerprint().unwrap());
}

#[test]
fn takes_text_of_exactly_the_size_bound_both_ways() {
  let bare = Manifest::new("app").docs("").render().unwrap().len();
  let manifest = Manifest::new("app").docs("d".repeat(MAX_MANIFEST_BYTES - bare));
  let text = manifest.render().unwrap();
  assert_eq!(text.len(), MAX_MANIFEST_BYTES);
  assert_eq!(Manifest::read(&text).unwrap(), manifest);
}

#[test]
fn reads_docs_on_any_object_and_keeps_them_where_the_model_does() {
  let mut document = written();
  document["docs"] = json!("The example app.");
  document["services"][0]["docs"] = json!("Chat rooms.");
  document["types"]["Message"]["docs"] = json!("One message.");
  document["types"]["Message"]["struct"]["docs"] = json!("Its fields.");
  document["types"]["Message"]["struct"]["fields"][0]["docs"] = json!("The ID.");
  document["types"]["SendError"]["enum"]["variants"][0]["docs"] = json!("No such room.");
  let kept = [
    "/docs",
    "/services/0/docs",
    "/services/0/server_methods/1/docs",
    "/services/0/server_methods/1/params/0/docs",
    "/channels/0/docs",
    "/datagrams/0/docs",
  ];
  for pointer in kept {
    let (parent, key) = pointer.rsplit_once('/').unwrap();
    document.pointer_mut(parent).unwrap()[key] = json!(pointer);
  }
  let rendered: Value = serde_json::from_str(&read(&document).unwrap().render().unwrap()).unwrap();
  for pointer in kept {
    assert_eq!(rendered.pointer(pointer), Some(&json!(pointer)), "{pointer}");
  }
  assert_eq!(rendered["types"]["Message"], written()["types"]["Message"]);
}

#[test]
fn refuses_null_for_an_optional_field() {
  let mut document = written();
  document["services"][0]["server_methods"][1]["error"] = Value::Null;
  assert_eq!(
    form(&document),
    (
      "$.services[0].server_methods[1].error".into(),
      "is `null`, which is not allowed: leave the key out (spec 12.2)".into()
    )
  );
}

#[test]
fn refuses_a_key_the_spec_does_not_define_and_names_its_place() {
  let mut document = written();
  document["services"][0]["server_methods"][0]["returns"] = json!("u8");
  assert_eq!(
    form(&document),
    (
      "$.services[0].server_methods[0].returns".into(),
      "is not a key of a `server_stream` call".into()
    )
  );
  let mut document = written();
  document["services"][0]["server_methods"][2]["shape"] = json!("unary");
  assert_eq!(
    form(&document),
    (
      "$.services[0].server_methods[2].shape".into(),
      "is not a key of this object (spec 12.2)".into()
    )
  );
  let mut document = written();
  document["extra"] = json!(1);
  assert_eq!(form(&document).0, "$.extra");
}

#[test]
fn refuses_a_key_written_twice_in_one_object() {
  let text = spec_example()
    .render()
    .unwrap()
    .replacen("\"name\": \"example-app\",", "\"name\": \"a\", \"name\": \"b\",", 1);
  match Manifest::read(&text) {
    Err(ReadError::Json(message)) => assert!(message.contains("`name` twice"), "{message}"),
    other => panic!("expected a JSON error, got {other:?}"),
  }
}

#[test]
fn refuses_text_after_the_document_and_text_that_is_not_json() {
  let text = format!("{} {{}}", spec_example().render().unwrap());
  assert!(matches!(Manifest::read(&text), Err(ReadError::Json(_))));
  assert!(matches!(Manifest::read("{"), Err(ReadError::Json(_))));
  assert_eq!(form(&json!([])), ("$".into(), "is not an object".into()));
}

#[test]
fn refuses_another_format_version() {
  let mut document = written();
  document["pavia"] = json!(2);
  assert_eq!(read(&document).unwrap_err(), ReadError::Version { found: "2".into() });
  document["pavia"] = json!("1");
  assert_eq!(read(&document).unwrap_err(), ReadError::Version { found: "\"1\"".into() });
  document["pavia"] = json!(2);
  document["added_in_2"] = json!({ "docs": 7 });
  assert_eq!(read(&document).unwrap_err(), ReadError::Version { found: "2".into() });
  for found in ["-1", "1.5"] {
    let text = format!(r#"{{"pavia": {found}, "name": "app"}}"#);
    assert_eq!(
      Manifest::read(&text).unwrap_err(),
      ReadError::Version { found: found.into() },
      "{found}"
    );
  }
}

#[test]
fn refuses_text_over_the_size_bound_before_parsing_it() {
  let text = " ".repeat(MAX_MANIFEST_BYTES + 1);
  assert_eq!(
    Manifest::read(&text).unwrap_err(),
    ReadError::TooLarge {
      size: MAX_MANIFEST_BYTES + 1,
      limit: MAX_MANIFEST_BYTES
    }
  );
}

#[test]
fn reads_types_nested_to_the_depth_bound_and_refuses_one_deeper() {
  let lists = |count: usize| ".list".repeat(count);
  assert!(read(&with_alias(nested(MAX_TYPE_DEPTH))).is_ok());
  assert_eq!(
    too_deep(&with_alias(nested(MAX_TYPE_DEPTH + 1))),
    format!("$.types.Deep.alias{}", lists(MAX_TYPE_DEPTH))
  );
  let field = |ty: Value| json!({ "struct": { "fields": [{ "name": "f", "type": ty }] } });
  assert!(read(&with_alias(field(nested(MAX_TYPE_DEPTH - 4)))).is_ok());
  assert_eq!(
    too_deep(&with_alias(field(nested(MAX_TYPE_DEPTH - 3)))),
    format!("$.types.Deep.alias.struct.fields[0].type{}", lists(MAX_TYPE_DEPTH - 4))
  );
  let tuple = |ty: Value| json!({ "tuple": ["u8", ty] });
  assert!(read(&with_alias(tuple(nested(MAX_TYPE_DEPTH - 2)))).is_ok());
  assert_eq!(
    too_deep(&with_alias(tuple(nested(MAX_TYPE_DEPTH - 1)))),
    format!("$.types.Deep.alias.tuple[1]{}", lists(MAX_TYPE_DEPTH - 2))
  );
}

#[test]
fn counts_the_types_of_a_named_struct_and_a_named_enum_from_one() {
  let lists = |count: usize| ".list".repeat(count);
  let field = |ty: Value| json!({ "struct": { "fields": [{ "name": "f", "type": ty }] } });
  let variant = |ty: Value| json!({ "enum": { "variants": [{ "name": "V", "type": ty }] } });
  assert!(read(&with_type(field(nested(MAX_TYPE_DEPTH)))).is_ok());
  assert!(read(&with_type(variant(nested(MAX_TYPE_DEPTH)))).is_ok());
  assert_eq!(
    too_deep(&with_type(field(nested(MAX_TYPE_DEPTH + 1)))),
    format!("$.types.Deep.struct.fields[0].type{}", lists(MAX_TYPE_DEPTH))
  );
  assert_eq!(
    too_deep(&with_type(variant(nested(MAX_TYPE_DEPTH + 1)))),
    format!("$.types.Deep.enum.variants[0].type{}", lists(MAX_TYPE_DEPTH))
  );
}

fn structs(count: usize) -> Type {
  let mut ty = Type::from(Primitive::U8);
  for _ in 0..count {
    ty = Type::Struct(Struct::new(vec![Field::new("f", ty)]));
  }
  ty
}

fn around_u8(count: usize, wrap: fn(Type) -> Type) -> Type {
  let mut ty = Type::from(Primitive::U8);
  for _ in 0..count {
    ty = wrap(ty);
  }
  ty
}

#[test]
fn reads_back_every_type_the_writer_writes_and_both_refuse_one_deeper() {
  let returns = "`returns` of server method `s/m`";
  let inner: [fn(Type) -> Type; 3] = [
    |ty| Type::List(Box::new(ty)),
    |ty| Type::Map(Box::new(ty)),
    |ty| Type::Option(Box::new(ty)),
  ];
  let mut cases: Vec<(Type, Type, String)> = inner
    .into_iter()
    .map(|wrap| {
      (
        around_u8(MAX_TYPE_DEPTH - 1, wrap),
        around_u8(MAX_TYPE_DEPTH, wrap),
        returns.to_owned(),
      )
    })
    .collect();
  let tuple: fn(Type) -> Type = |ty| Type::Tuple(vec![Type::from(Primitive::U8), ty]);
  cases.push((around_u8(15, tuple), around_u8(16, tuple), returns.to_owned()));
  cases.push((structs(7), structs(8), format!("{}{returns}", "field `f` of ".repeat(8))));
  for (fits, deeper, at) in cases {
    let manifest = |ty: Type| Manifest::new("app").service(Service::new("s").server_method(Method::call("m", Call::unary(ty))));
    let text = manifest(fits).render().unwrap();
    assert_eq!(Manifest::read(&text).unwrap().render().unwrap(), text);
    assert_eq!(
      manifest(deeper).render().unwrap_err(),
      [ManifestError::TooDeep {
        at,
        limit: MAX_TYPE_DEPTH
      }]
    );
  }
}

/// A manifest with `ty` in each place that names a type, and where that place is.
fn every_place(ty: &Type) -> [(Manifest, &'static str); 7] {
  let named = |definition: Definition| Manifest::new("app").types(BTreeMap::from([("Deep".into(), definition)]));
  let method = |method: Method| Manifest::new("app").service(Service::new("s").server_method(method));
  let unary = || Call::unary(Primitive::Unit.into());
  [
    (named(Definition::Alias(ty.clone())), "type `Deep`"),
    (
      named(Definition::Struct(Struct::new(vec![Field::new("f", ty.clone())]))),
      "field `f` of type `Deep`",
    ),
    (
      named(Definition::Enum(Enum::new(vec![Variant::new("V", ty.clone())]))),
      "variant `V` of type `Deep`",
    ),
    (
      method(Method::call("m", unary()).param(Param::new("p", ty.clone()))),
      "param `p` of server method `s/m`",
    ),
    (
      method(Method::call("m", unary().error(ty.clone()))),
      "`error` of server method `s/m`",
    ),
    (
      Manifest::new("app").channel(Channel::new("c", ty.clone())),
      "`publication` of channel namespace `c`",
    ),
    (
      Manifest::new("app").datagram(Datagram::new("d", Direction::Both, Mode::All, ty.clone())),
      "datagram topic `d`",
    ),
  ]
}

#[test]
fn both_count_every_type_a_manifest_names_from_one() {
  let lists = |depth: usize| around_u8(depth - 1, |ty| Type::List(Box::new(ty)));
  let deeper = every_place(&lists(MAX_TYPE_DEPTH + 1));
  for ((fits, at), (deeper, _)) in every_place(&lists(MAX_TYPE_DEPTH)).into_iter().zip(deeper) {
    let text = fits.render().unwrap();
    assert_eq!(Manifest::read(&text).unwrap().render().unwrap(), text, "{at}");
    assert_eq!(
      deeper.render().unwrap_err(),
      [ManifestError::TooDeep {
        at: at.to_owned(),
        limit: MAX_TYPE_DEPTH
      }],
      "{at}"
    );
  }
}

#[test]
fn refuses_to_render_text_over_the_size_bound() {
  let manifest = Manifest::new("app").docs("d".repeat(MAX_MANIFEST_BYTES));
  assert_eq!(manifest.fingerprint(), Manifest::new("app").fingerprint());
  match manifest.render().unwrap_err().as_slice() {
    [ManifestError::TooLarge { size, limit }] => {
      assert!(*size > MAX_MANIFEST_BYTES);
      assert_eq!(*limit, MAX_MANIFEST_BYTES);
    }
    other => panic!("expected a size error, got {other:?}"),
  }
}

#[test]
fn reads_and_fingerprints_compact_text_whose_pretty_form_is_past_the_size_bound() {
  let fields: Vec<Value> = (0..80_000)
    .map(|index| json!({ "name": format!("f{index:05}"), "type": "u8" }))
    .collect();
  let document = json!({ "pavia": 1, "name": "app", "types": { "Wide": { "struct": { "fields": fields } } } });
  let compact = document.to_string();
  assert!(compact.len() < MAX_MANIFEST_BYTES);
  let manifest = Manifest::read(&compact).unwrap();
  assert!(manifest.fingerprint().is_ok());
  assert!(matches!(
    manifest.render().unwrap_err().as_slice(),
    [ManifestError::TooLarge { .. }]
  ));
}

#[test]
fn reads_a_long_chain_of_aliases_in_one_walk() {
  let count = 50_000;
  let mut types = serde_json::Map::new();
  for index in 0..count {
    let target = if index + 1 == count {
      json!("u8")
    } else {
      json!({ "ref": format!("A{:06}", index + 1) })
    };
    types.insert(format!("A{index:06}"), json!({ "alias": target }));
  }
  let document = json!({ "pavia": 1, "name": "app", "types": types });
  assert_eq!(read(&document).unwrap().types.len(), count);
}

#[test]
fn reads_a_manifest_with_no_types_and_every_type_form() {
  assert!(read(&json!({ "pavia": 1, "name": "app" })).unwrap().types.is_empty());
  let forms = json!({ "map": { "tuple": ["u8", { "option": "string" }] } });
  let manifest = read(&with_alias(forms.clone())).unwrap();
  let rendered: Value = serde_json::from_str(&manifest.render().unwrap()).unwrap();
  assert_eq!(rendered["types"]["Deep"]["alias"], forms);
}

#[test]
fn a_location_moves_only_a_problem_that_has_one() {
  assert_eq!(ReadError::Json("x".into()).under(".a"), ReadError::Json("x".into()));
  let expected = <serde_json::Error as de::Error>::invalid_type(de::Unexpected::Unit, &StrictVisitor);
  assert_eq!(expected.to_string(), "invalid type: null, expected a JSON value");
}

#[test]
fn refuses_json_nested_past_the_parser_limit_anywhere() {
  let text = format!(
    r#"{{"pavia": 1, "name": "app", "docs": {}{}}}"#,
    "[".repeat(200),
    "]".repeat(200)
  );
  match Manifest::read(&text) {
    Err(ReadError::Json(message)) => assert!(message.starts_with("recursion limit exceeded"), "{message}"),
    other => panic!("expected a JSON error, got {other:?}"),
  }
}

#[test]
fn reads_every_shape_direction_and_mode() {
  let byte = || Type::from(Primitive::U8);
  let mut service = Service::new("s");
  for shape in Shape::ALL {
    let call = match shape {
      Shape::Unary => Call::unary(byte()),
      Shape::ServerStream => Call::server_stream(byte()),
      Shape::ClientStream => Call::client_stream(byte(), byte()),
      Shape::Bidi => Call::bidi(byte(), byte()),
    };
    service = service.server_method(Method::call(shape.name(), call));
  }
  let mut manifest = Manifest::new("app").service(service);
  for direction in Direction::ALL {
    for mode in Mode::ALL {
      manifest = manifest.datagram(Datagram::new(
        format!("{}_{}", direction.name(), mode.name()),
        direction,
        mode,
        byte(),
      ));
    }
  }
  let text = manifest.render().unwrap();
  assert_eq!(Manifest::read(&text).unwrap().render().unwrap(), text);
}

#[test]
fn refuses_docs_that_are_not_a_string_where_they_are_skipped() {
  let cases: [(&str, &str); 4] = [
    ("/types/Message/docs", "$.types.Message.docs"),
    ("/types/Message/struct/docs", "$.types.Message.struct.docs"),
    ("/types/Message/struct/fields/0/docs", "$.types.Message.struct.fields[0].docs"),
    (
      "/types/SendError/enum/variants/0/docs",
      "$.types.SendError.enum.variants[0].docs",
    ),
  ];
  for (pointer, at) in cases {
    let mut document = written();
    let (parent, key) = pointer.rsplit_once('/').unwrap();
    document.pointer_mut(parent).unwrap()[key] = json!(7);
    assert_eq!(form(&document), (at.into(), "is not a string".into()), "{pointer}");
  }
}

#[test]
fn refuses_a_part_out_of_its_form() {
  let cases: [(&str, Value, &str, &str); 12] = [
    (
      "/types/RoomId/alias",
      json!({ "tuple": "u8" }),
      "$.types.RoomId.alias.tuple",
      "is not an array",
    ),
    (
      "/types/RoomId/alias",
      json!({ "ref": 7 }),
      "$.types.RoomId.alias.ref",
      "is not a string",
    ),
    (
      "/channels/0/history",
      json!("yes"),
      "$.channels[0].history",
      "is not `true` or `false`",
    ),
    (
      "/services/0/server_methods/0/kind",
      json!("stream"),
      "$.services[0].server_methods[0].kind",
      "is not one of `call`, `notify`",
    ),
    (
      "/services/0/server_methods/0/shape",
      json!("duplex"),
      "$.services[0].server_methods[0].shape",
      "is not one of `unary`, `server_stream`, `client_stream`, `bidi`",
    ),
    (
      "/datagrams/0/direction",
      json!("up"),
      "$.datagrams[0].direction",
      "is not one of `to_server`, `to_client`, `both`",
    ),
    (
      "/datagrams/0/mode",
      json!("first"),
      "$.datagrams[0].mode",
      "is not one of `latest`, `all`",
    ),
    (
      "/services/0/server_methods/1/params/0/type",
      json!("u128"),
      "$.services[0].server_methods[1].params[0].type",
      "`u128` is not a primitive type",
    ),
    (
      "/types/RoomId",
      json!({ "alias": "string", "struct": { "fields": [] } }),
      "$.types.RoomId",
      "does not have exactly one of `struct`, `enum`, `alias`",
    ),
    (
      "/types/RoomId",
      json!({ "union": "string" }),
      "$.types.RoomId.union",
      "is not one of `struct`, `enum`, `alias`",
    ),
    ("/datagrams", json!({}), "$.datagrams", "is not an array"),
    ("/services/0/name", json!(7), "$.services[0].name", "is not a string"),
  ];
  for (pointer, value, at, problem) in cases {
    let mut document = written();
    *document.pointer_mut(pointer).unwrap() = value;
    assert_eq!(form(&document), (at.into(), problem.into()), "{pointer}");
  }
  let mut document = written();
  document["services"][0]["server_methods"].as_array_mut().unwrap()[0]
    .as_object_mut()
    .unwrap()
    .remove("yields");
  assert_eq!(
    form(&document),
    ("$.services[0].server_methods[0].yields".into(), "is missing".into())
  );
}

#[test]
fn refuses_a_document_validation_refuses_and_reports_every_error() {
  let mut document = written();
  document["services"][0]["server_methods"][1]["name"] = json!("a(){}; evil");
  document["types"]["Message"]["struct"]["fields"][0]["name"] = json!("foo-bar");
  let error = read(&document).unwrap_err();
  assert_eq!(
    error.to_string(),
    "the manifest is not valid: server method of service `chat` name `a(){}; evil` is not a contract name: \
     names match [A-Za-z_][A-Za-z0-9_]* (spec 12.2); field of type `Message` name `foo-bar` is not a contract \
     name: names match [A-Za-z_][A-Za-z0-9_]* (spec 12.2)"
  );
  match error {
    ReadError::Invalid(errors) => {
      let names: Vec<&str> = errors
        .iter()
        .filter_map(|error| match error {
          ManifestError::InvalidName { name, .. } => Some(name.as_str()),
          _ => None,
        })
        .collect();
      assert_eq!(names, ["a(){}; evil", "foo-bar"]);
    }
    other => panic!("expected validation errors, got {other:?}"),
  }
}
