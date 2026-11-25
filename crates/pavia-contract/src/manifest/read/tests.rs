use serde_json::{Value, json};

use super::{MAX_MANIFEST_BYTES, MAX_TYPE_DEPTH, ReadError};
use crate::manifest::tests::spec_example;
use crate::manifest::{Call, Datagram, Direction, Manifest, ManifestError, Method, Mode, Service, Shape};
use crate::schema::{Primitive, Type};

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
fn reads_defaults_written_out_and_named_entries_in_any_order() {
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
  assert_eq!(read(&document).unwrap().render().unwrap(), spec_example().render().unwrap());
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
  let rendered: Value = serde_json::from_str(&read(&document).unwrap().render().unwrap()).unwrap();
  assert_eq!(
    [&rendered["docs"], &rendered["services"][0]["docs"]],
    [&json!("The example app."), &json!("Chat rooms.")]
  );
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
  assert!(read(&with_alias(field(nested(MAX_TYPE_DEPTH - 1)))).is_ok());
  assert_eq!(
    too_deep(&with_alias(field(nested(MAX_TYPE_DEPTH)))),
    format!("$.types.Deep.alias.struct.fields[0].type{}", lists(MAX_TYPE_DEPTH - 1))
  );
}

#[test]
fn counts_a_named_struct_and_a_named_enum_as_one_level() {
  let lists = |count: usize| ".list".repeat(count);
  let field = |ty: Value| json!({ "struct": { "fields": [{ "name": "f", "type": ty }] } });
  let variant = |ty: Value| json!({ "enum": { "variants": [{ "name": "V", "type": ty }] } });
  assert!(read(&with_type(field(nested(MAX_TYPE_DEPTH - 1)))).is_ok());
  assert!(read(&with_type(variant(nested(MAX_TYPE_DEPTH - 1)))).is_ok());
  assert_eq!(
    too_deep(&with_type(field(nested(MAX_TYPE_DEPTH)))),
    format!("$.types.Deep.struct.fields[0].type{}", lists(MAX_TYPE_DEPTH - 1))
  );
  assert_eq!(
    too_deep(&with_type(variant(nested(MAX_TYPE_DEPTH)))),
    format!("$.types.Deep.enum.variants[0].type{}", lists(MAX_TYPE_DEPTH - 1))
  );
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
  let cases: [(&str, Value, &str, &str); 9] = [
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
  match read(&document) {
    Err(ReadError::Invalid(errors)) => {
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
