use serde_json::{Value, json};

use super::{MAX_MANIFEST_BYTES, MAX_TYPE_DEPTH, ReadError};
use crate::manifest::tests::spec_example;
use crate::manifest::{Manifest, ManifestError};

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

fn with_alias(ty: Value) -> Value {
  let mut document = written();
  document["types"]["Deep"] = json!({ "alias": ty });
  document
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
  let (at, problem) = form(&document);
  assert_eq!(at, "services[0].server_methods[1].error");
  assert!(problem.contains("`null`"), "{problem}");
}

#[test]
fn refuses_a_key_the_spec_does_not_define_and_names_its_place() {
  let mut document = written();
  document["services"][0]["server_methods"][0]["returns"] = json!("u8");
  assert_eq!(form(&document).0, "services[0].server_methods[0].returns");
  let mut document = written();
  document["services"][0]["server_methods"][2]["shape"] = json!("unary");
  assert_eq!(form(&document).0, "services[0].server_methods[2].shape");
  let mut document = written();
  document["extra"] = json!(1);
  assert_eq!(form(&document).0, "extra");
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
  assert_eq!(form(&json!([])).0, "the manifest");
}

#[test]
fn refuses_another_format_version() {
  let mut document = written();
  document["pavia"] = json!(2);
  assert_eq!(read(&document).unwrap_err(), ReadError::Version { found: "2".into() });
  document["pavia"] = json!("1");
  assert_eq!(read(&document).unwrap_err(), ReadError::Version { found: "\"1\"".into() });
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
  assert!(read(&with_alias(nested(MAX_TYPE_DEPTH))).is_ok());
  match read(&with_alias(nested(MAX_TYPE_DEPTH + 1))) {
    Err(ReadError::TooDeep { limit, .. }) => assert_eq!(limit, MAX_TYPE_DEPTH),
    other => panic!("expected a depth error, got {other:?}"),
  }
  let field = |ty: Value| json!({ "struct": { "fields": [{ "name": "f", "type": ty }] } });
  assert!(read(&with_alias(field(nested(MAX_TYPE_DEPTH - 1)))).is_ok());
  let outcome = read(&with_alias(field(nested(MAX_TYPE_DEPTH))));
  assert!(matches!(outcome, Err(ReadError::TooDeep { .. })), "{outcome:?}");
}

#[test]
fn refuses_a_part_out_of_its_form() {
  let cases: [(&str, Value, &str); 7] = [
    ("/services/0/server_methods/0/kind", json!("stream"), "kind"),
    ("/services/0/server_methods/0/shape", json!("duplex"), "shape"),
    ("/services/0/server_methods/1/params/0/type", json!("u128"), "type"),
    (
      "/types/RoomId",
      json!({ "alias": "string", "struct": { "fields": [] } }),
      "RoomId",
    ),
    ("/types/RoomId", json!({ "union": "string" }), "union"),
    ("/datagrams", json!({}), "datagrams"),
    ("/services/0/name", json!(7), "name"),
  ];
  for (pointer, value, key) in cases {
    let mut document = written();
    *document.pointer_mut(pointer).unwrap() = value;
    let (at, _) = form(&document);
    assert!(at.ends_with(key), "{pointer}: {at}");
  }
  let mut document = written();
  document["services"][0]["server_methods"].as_array_mut().unwrap()[0]
    .as_object_mut()
    .unwrap()
    .remove("yields");
  assert_eq!(
    form(&document),
    ("services[0].server_methods[0].yields".into(), "is missing".into())
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
