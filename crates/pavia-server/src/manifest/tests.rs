use std::fs;
use std::io;

use pavia_contract::schema::Type;
use pavia_contract::{Contract, TypeError, TypeTable};
use pavia_proto::data::Value;
use pavia_proto::stream::Shape;
use serde_json::json;

use super::{ExportError, Problem, export, fingerprint, manifest, render, write};
use crate::methods::{Handler, MethodEntry, MethodRegistry};

fn handler(params: usize) -> Handler {
  Handler::new(params, |_| async { Value::Null })
}

fn server(entries: Vec<(&str, MethodEntry<Handler>)>) -> MethodRegistry<Handler> {
  let mut methods = MethodRegistry::new();
  for (name, entry) in entries {
    methods.register_server_method(name, entry).unwrap();
  }
  methods
}

fn exported(methods: &MethodRegistry<Handler>) -> serde_json::Value {
  serde_json::from_str(&render("app", methods).unwrap()).unwrap()
}

fn problems(methods: &MethodRegistry<Handler>) -> Vec<String> {
  match manifest("app", methods) {
    Err(ExportError::Signatures(errors)) => errors.iter().map(ToString::to_string).collect(),
    other => panic!("{other:?}"),
  }
}

fn call(shape: Shape) -> MethodEntry<Handler> {
  MethodEntry::call(handler(0)).with_shape(shape)
}

#[test]
fn writes_each_shape_with_the_types_it_carries() {
  let methods = server(vec![
    ("s/unit", call(Shape::Unary)),
    ("s/get", call(Shape::Unary).returns::<String>().error::<u32>().idempotent()),
    ("s/feed", call(Shape::ServerStream).yields::<u8>()),
    ("s/drain", call(Shape::ClientStream).accepts::<u8>()),
    ("s/sum", call(Shape::ClientStream).accepts::<u8>().returns::<u64>()),
    ("s/chat", call(Shape::Bidirectional).accepts::<u8>().yields::<String>()),
    ("s/poke", MethodEntry::notify(handler(0)).docs("Pokes.")),
  ]);
  assert_eq!(
    exported(&methods)["services"][0]["server_methods"],
    json!([
      {"name": "chat", "kind": "call", "shape": "bidi", "accepts": "u8", "yields": "string"},
      {"name": "drain", "kind": "call", "shape": "client_stream", "accepts": "u8"},
      {"name": "feed", "kind": "call", "shape": "server_stream", "yields": "u8"},
      {"name": "get", "kind": "call", "shape": "unary", "returns": "string", "error": "u32", "idempotent": true},
      {"name": "poke", "kind": "notify", "docs": "Pokes."},
      {"name": "sum", "kind": "call", "shape": "client_stream", "accepts": "u8", "returns": "u64"},
      {"name": "unit", "kind": "call", "shape": "unary"}
    ])
  );
}

#[test]
fn groups_methods_by_service_on_both_sides() {
  let mut methods = server(vec![("b/one", call(Shape::Unary)), ("a/two", call(Shape::Unary))]);
  methods
    .register_client_method("b/alert", MethodEntry::notify(handler(1)).param::<String>("text"))
    .unwrap();
  methods.register_client_method("c/ask", call(Shape::Unary)).unwrap();
  let services = exported(&methods)["services"].clone();
  assert_eq!(
    services,
    json!([
      {"name": "a", "server_methods": [{"name": "two", "kind": "call", "shape": "unary"}], "client_methods": []},
      {
        "name": "b",
        "server_methods": [{"name": "one", "kind": "call", "shape": "unary"}],
        "client_methods": [{"name": "alert", "kind": "notify", "params": [{"name": "text", "type": "string"}]}]
      },
      {"name": "c", "server_methods": [], "client_methods": [{"name": "ask", "kind": "call", "shape": "unary"}]}
    ])
  );
}

#[test]
fn refuses_types_a_kind_or_shape_does_not_carry() {
  let methods = server(vec![
    (
      "s/notify",
      MethodEntry::notify(handler(0))
        .returns::<u8>()
        .yields::<u8>()
        .accepts::<u8>()
        .error::<u8>(),
    ),
    ("s/unary", call(Shape::Unary).yields::<u8>().accepts::<u8>()),
    ("s/server", call(Shape::ServerStream).yields::<u8>().returns::<u8>()),
    ("s/client", call(Shape::ClientStream).accepts::<u8>().yields::<u8>()),
    (
      "s/bidi",
      call(Shape::Bidirectional).accepts::<u8>().yields::<u8>().returns::<u8>(),
    ),
  ]);
  assert_eq!(
    problems(&methods),
    [
      "server method s/bidi: a bidi call has no `returns`",
      "server method s/client: a client_stream call has no `yields`",
      "server method s/notify: a notification has no `returns`",
      "server method s/notify: a notification has no `yields`",
      "server method s/notify: a notification has no `accepts`",
      "server method s/notify: a notification has no `error`",
      "server method s/server: a server_stream call has no `returns`",
      "server method s/unary: a unary call has no `yields`",
      "server method s/unary: a unary call has no `accepts`",
    ]
  );
}

#[test]
fn refuses_a_streaming_call_without_its_item_types() {
  let mut methods = server(vec![
    ("s/server", call(Shape::ServerStream)),
    ("s/client", call(Shape::ClientStream).returns::<u8>()),
    ("s/bidi", call(Shape::Bidirectional)),
  ]);
  methods
    .register_client_method("s/half", call(Shape::Bidirectional).accepts::<u8>())
    .unwrap();
  assert_eq!(
    problems(&methods),
    [
      "server method s/bidi: a bidi call needs `accepts`",
      "server method s/bidi: a bidi call needs `yields`",
      "server method s/client: a client_stream call needs `accepts`",
      "server method s/server: a server_stream call needs `yields`",
      "client method s/half: a bidi call needs `yields`",
    ]
  );
}

#[test]
fn refuses_a_signature_whose_params_the_handler_does_not_take() {
  let methods = server(vec![
    ("s/fewer", MethodEntry::call(handler(2)).param::<u8>("a")),
    ("s/more", MethodEntry::notify(handler(0)).param::<u8>("a")),
    ("s/fits", MethodEntry::call(handler(1)).param::<u8>("a")),
  ]);
  let Err(ExportError::Signatures(errors)) = manifest("app", &methods) else {
    panic!("expected signature errors");
  };
  assert_eq!(errors.len(), 2);
  assert_eq!(errors[0].method().as_str(), "s/fewer");
  assert_eq!(errors[0].problem(), &Problem::ParamCount { declared: 1, handler: 2 });
  assert_eq!(
    errors[1].to_string(),
    "server method s/more: the signature declares 1 params, the handler takes 0"
  );
}

#[derive(pavia_contract::Contract)]
struct Nothing(());

#[derive(pavia_contract::Contract)]
#[expect(dead_code, reason = "the type exists for its derived impl, which never reads the field")]
struct Holder(Option<Nothing>);

#[test]
fn reports_types_that_do_not_make_a_types_table() {
  let methods = server(vec![("s/get", call(Shape::Unary).returns::<Holder>())]);
  let error = manifest("app", &methods).unwrap_err();
  let ExportError::Types(errors) = &error else {
    panic!("{error:?}");
  };
  assert_eq!(errors, &[TypeError::OptionOfUnit { name: "Nothing".into() }]);
  assert_eq!(error.to_string(), errors[0].to_string());
}

struct Dangling;

impl Contract for Dangling {
  fn name() -> String {
    "Dangling".into()
  }

  fn contract_type(_: &mut TypeTable) -> Type {
    Type::List(Box::new(Type::Ref("Nowhere".into())))
  }
}

#[test]
fn reports_what_the_manifest_validation_finds() {
  let methods = server(vec![("s/get", call(Shape::Unary).returns::<Dangling>())]);
  let built = manifest("app", &methods).unwrap();
  assert_eq!(built.render().unwrap_err().len(), 1);
  let error = render("app", &methods).unwrap_err();
  let ExportError::Manifest(errors) = &error else {
    panic!("{error:?}");
  };
  assert_eq!(errors.len(), 1);
  assert_eq!(
    error.to_string(),
    "`returns` of server method `s/get` refers to `Nowhere`, which is not in `types`"
  );
}

#[test]
fn has_no_fingerprint_for_a_manifest_that_does_not_validate() {
  let dangling = server(vec![("s/get", call(Shape::Unary).returns::<Dangling>())]);
  assert!(matches!(fingerprint("app", &dangling), Err(ExportError::Manifest(_))));
  let unfinished = server(vec![("s/feed", call(Shape::ServerStream))]);
  assert!(matches!(fingerprint("app", &unfinished), Err(ExportError::Signatures(_))));
  let fine = server(vec![("s/get", call(Shape::Unary))]);
  assert_eq!(fingerprint("app", &fine).unwrap(), fingerprint("other", &fine).unwrap());
}

#[test]
fn joins_several_errors_one_per_line() {
  let methods = server(vec![("s/a", call(Shape::ServerStream)), ("s/b", call(Shape::ServerStream))]);
  assert_eq!(
    manifest("app", &methods).unwrap_err().to_string(),
    "server method s/a: a server_stream call needs `yields`\nserver method s/b: a server_stream call needs `yields`"
  );
}

#[test]
fn writes_the_rendered_manifest_to_a_writer_and_a_file() {
  let methods = server(vec![("s/get", call(Shape::Unary).returns::<u8>())]);
  let text = render("app", &methods).unwrap();
  let mut written = Vec::new();
  write("app", &methods, &mut written).unwrap();
  assert_eq!(written, text.as_bytes());

  let folder = std::env::temp_dir().join(format!("pavia-manifest-{}", std::process::id()));
  fs::create_dir_all(&folder).unwrap();
  let path = folder.join("manifest.json");
  export("app", &methods, &path).unwrap();
  assert_eq!(fs::read_to_string(&path).unwrap(), text);
  fs::remove_dir_all(&folder).unwrap();
}

#[test]
fn writes_nothing_for_an_invalid_manifest() {
  let methods = server(vec![("s/feed", call(Shape::ServerStream))]);
  let mut written = Vec::new();
  assert!(matches!(
    write("app", &methods, &mut written),
    Err(ExportError::Signatures(_))
  ));
  assert!(written.is_empty());
  let path = std::env::temp_dir().join(format!("pavia-manifest-none-{}.json", std::process::id()));
  assert!(matches!(export("app", &methods, &path), Err(ExportError::Signatures(_))));
  assert!(!path.exists());
}

#[test]
fn reports_a_write_that_fails() {
  struct Broken;

  impl io::Write for Broken {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
      Err(io::Error::other("disk full"))
    }

    fn flush(&mut self) -> io::Result<()> {
      Ok(())
    }
  }

  let methods = server(vec![("s/get", call(Shape::Unary))]);
  let error = write("app", &methods, &mut Broken).unwrap_err();
  assert_eq!(error.to_string(), "cannot write the manifest: disk full");
  let missing = std::env::temp_dir().join("pavia-no-such-folder").join("manifest.json");
  assert!(matches!(export("app", &methods, missing), Err(ExportError::Io(_))));
}
