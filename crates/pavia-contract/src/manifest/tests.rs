use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::{Call, Channel, Datagram, Direction, Manifest, ManifestError, Method, Mode, Param, Service, Shape};
use crate::fingerprint::tests::{SPEC_EXAMPLE, hex};
use crate::schema::{Definition, Enum, Field, Primitive, Struct, Type, Types, Variant};

fn reference(name: &str) -> Type {
  Type::Ref(name.into())
}

fn string() -> Type {
  Primitive::String.into()
}

fn option(inner: Type) -> Type {
  Type::Option(Box::new(inner))
}

fn spec_types() -> Types {
  let field = |name: &str, ty: Type| Field::new(name, ty);
  BTreeMap::from([
    ("RoomId".into(), Definition::Alias(string())),
    ("MessageId".into(), Definition::Alias(Primitive::Uuid.into())),
    (
      "Message".into(),
      Definition::Struct(Struct::new(vec![
        field("id", reference("MessageId")),
        field("author", string()),
        field("text", string()),
        field("sent_at", Primitive::Timestamp.into()),
      ])),
    ),
    (
      "SendError".into(),
      Definition::Enum(Enum::new(vec![
        Variant::unit("RoomNotFound"),
        Variant::new(
          "TooLong",
          Type::Struct(Struct::new(vec![field("max", Primitive::U32.into())])),
        ),
      ])),
    ),
    (
      "Member".into(),
      Definition::Struct(Struct::new(vec![field("display_name", string())])),
    ),
    (
      "RoomOptions".into(),
      Definition::Struct(Struct::new(vec![field("lang", option(string())).optional()])),
    ),
    (
      "CursorPos".into(),
      Definition::Struct(Struct::new(vec![
        field("x", Primitive::F32.into()),
        field("y", Primitive::F32.into()),
      ])),
    ),
  ])
}

fn chat() -> Service {
  let room = || Param::new("room", reference("RoomId"));
  Service::new("chat")
    .server_method(Method::call(
      "upload",
      Call::client_stream(Primitive::Bytes.into(), Primitive::U64.into()),
    ))
    .server_method(
      Method::call("send", Call::unary(reference("MessageId")).error(reference("SendError")))
        .param(room())
        .param(Param::new("text", string())),
    )
    .server_method(Method::notify("typing").param(room()))
    .server_method(Method::call("history", Call::server_stream(reference("Message"))).param(room()))
    .client_method(Method::notify("systemNotice").param(Param::new("text", string())))
    .client_method(Method::call("confirm", Call::unary(Primitive::Bool.into())).param(Param::new("prompt", string())))
}

fn spec_example() -> Manifest {
  Manifest::new("example-app")
    .service(chat())
    .channel(
      Channel::new("room", reference("Message"))
        .presence_info(reference("Member"))
        .subscribe(reference("RoomOptions"))
        .history(),
    )
    .datagram(Datagram::new("cursor", Direction::Both, Mode::Latest, reference("CursorPos")))
    .types(spec_types())
}

fn rendered(manifest: &Manifest) -> Value {
  serde_json::from_str(&manifest.render().unwrap()).unwrap()
}

fn errors(manifest: &Manifest) -> Vec<ManifestError> {
  manifest.render().unwrap_err()
}

#[test]
fn writes_the_example_of_spec_12_2() {
  let expected = json!({
    "pavia": 1,
    "name": "example-app",
    "services": [{
      "name": "chat",
      "server_methods": [
        {
          "name": "history", "kind": "call", "shape": "server_stream",
          "params": [{"name": "room", "type": {"ref": "RoomId"}}],
          "yields": {"ref": "Message"}
        },
        {
          "name": "send", "kind": "call", "shape": "unary",
          "params": [{"name": "room", "type": {"ref": "RoomId"}}, {"name": "text", "type": "string"}],
          "returns": {"ref": "MessageId"},
          "error": {"ref": "SendError"}
        },
        {"name": "typing", "kind": "notify", "params": [{"name": "room", "type": {"ref": "RoomId"}}]},
        {"name": "upload", "kind": "call", "shape": "client_stream", "accepts": "bytes", "returns": "u64"}
      ],
      "client_methods": [
        {
          "name": "confirm", "kind": "call", "shape": "unary",
          "params": [{"name": "prompt", "type": "string"}],
          "returns": "bool"
        },
        {"name": "systemNotice", "kind": "notify", "params": [{"name": "text", "type": "string"}]}
      ]
    }],
    "channels": [{
      "namespace": "room",
      "publication": {"ref": "Message"},
      "presence_info": {"ref": "Member"},
      "subscribe": {"ref": "RoomOptions"},
      "history": true
    }],
    "datagrams": [{"topic": "cursor", "direction": "both", "mode": "latest", "payload": {"ref": "CursorPos"}}],
    "types": {
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
      "RoomOptions": {"struct": {"fields": [{"name": "lang", "type": {"option": "string"}, "optional": true}]}},
      "CursorPos": {"struct": {"fields": [{"name": "x", "type": "f32"}, {"name": "y", "type": "f32"}]}}
    }
  });
  assert_eq!(rendered(&spec_example()), expected);
}

#[test]
fn writes_the_same_bytes_whatever_the_declaration_order() {
  let reversed = Manifest::new("example-app")
    .datagram(Datagram::new("cursor", Direction::Both, Mode::Latest, reference("CursorPos")))
    .channel(
      Channel::new("room", reference("Message"))
        .history()
        .subscribe(reference("RoomOptions"))
        .presence_info(reference("Member")),
    )
    .service(Service::new("auth"))
    .service(chat())
    .types(spec_types());
  let forward = spec_example().service(Service::new("auth"));
  let text = forward.render().unwrap();
  assert_eq!(text, reversed.render().unwrap());
  assert_eq!(text, forward.render().unwrap());
  assert!(text.ends_with("}\n"));
  assert!(text.starts_with("{\n  \"channels\""));
}

#[test]
fn sorts_named_entries_by_name_in_byte_order() {
  let manifest = Manifest::new("app")
    .service(
      Service::new("b")
        .server_method(Method::notify("b"))
        .server_method(Method::notify("B")),
    )
    .service(Service::new("a"))
    .service(Service::new("Z"))
    .channel(Channel::new("room", string()))
    .channel(Channel::new("feed", string()))
    .datagram(Datagram::new("pos", Direction::ToServer, Mode::All, string()))
    .datagram(Datagram::new("cursor", Direction::ToClient, Mode::Latest, string()));
  let json = rendered(&manifest);
  let names = |list: &Value, key: &str| -> Vec<String> {
    list
      .as_array()
      .unwrap()
      .iter()
      .map(|entry| entry[key].as_str().unwrap().to_owned())
      .collect()
  };
  assert_eq!(names(&json["services"], "name"), ["Z", "a", "b"]);
  assert_eq!(names(&json["services"][2]["server_methods"], "name"), ["B", "b"]);
  assert_eq!(names(&json["channels"], "namespace"), ["feed", "room"]);
  assert_eq!(names(&json["datagrams"], "topic"), ["cursor", "pos"]);
  assert_eq!(json["datagrams"][0]["direction"], "to_client");
  assert_eq!(json["datagrams"][1]["direction"], "to_server");
  assert_eq!(json["datagrams"][1]["mode"], "all");
}

#[test]
fn keeps_params_in_declaration_order() {
  let method = |first: &str, second: &str| {
    Manifest::new("app").service(
      Service::new("chat").server_method(
        Method::call("send", Call::unary(Primitive::Unit.into()))
          .param(Param::new(first, string()))
          .param(Param::new(second, Primitive::U32.into())),
      ),
    )
  };
  let params = rendered(&method("text", "count"))["services"][0]["server_methods"][0]["params"].clone();
  assert_eq!(
    params,
    json!([{"name": "text", "type": "string"}, {"name": "count", "type": "u32"}])
  );
  let swapped = rendered(&method("count", "text"))["services"][0]["server_methods"][0]["params"].clone();
  assert_eq!(swapped[0]["name"], "count");
  assert_ne!(
    method("text", "count").render().unwrap(),
    method("count", "text").render().unwrap()
  );
}

#[test]
fn omits_absent_and_default_fields() {
  let manifest = Manifest::new("app").service(
    Service::new("chat")
      .server_method(Method::call("ping", Call::unary(Primitive::Unit.into())))
      .server_method(Method::call("drain", Call::client_stream(string(), Primitive::Unit.into())))
      .server_method(Method::notify("poke")),
  );
  let text = manifest.render().unwrap();
  for forbidden in [
    "null",
    "\"idempotent\"",
    "\"returns\"",
    "\"params\"",
    "\"docs\"",
    "\"history\"",
  ] {
    assert!(!text.contains(forbidden), "{forbidden} in {text}");
  }
  let json = rendered(&manifest);
  assert_eq!(
    json["services"][0]["server_methods"],
    json!([
      {"name": "drain", "kind": "call", "shape": "client_stream", "accepts": "string"},
      {"name": "ping", "kind": "call", "shape": "unary"},
      {"name": "poke", "kind": "notify"}
    ])
  );
  assert_eq!(json["channels"], json!([]));
  assert_eq!(json["datagrams"], json!([]));
  assert_eq!(json["types"], json!({}));
  assert_eq!(json["services"][0]["client_methods"], json!([]));
}

#[test]
fn writes_every_shape_with_its_types() {
  let manifest = Manifest::new("app").service(
    Service::new("s")
      .server_method(Method::call("unary", Call::unary(string()).idempotent()))
      .server_method(Method::call("server", Call::server_stream(string())))
      .server_method(Method::call("client", Call::client_stream(Primitive::U8.into(), string())))
      .server_method(Method::call(
        "bidi",
        Call::bidi(Primitive::U8.into(), string()).error(string()),
      )),
  );
  assert_eq!(
    rendered(&manifest)["services"][0]["server_methods"],
    json!([
      {"name": "bidi", "kind": "call", "shape": "bidi", "accepts": "u8", "yields": "string", "error": "string"},
      {"name": "client", "kind": "call", "shape": "client_stream", "accepts": "u8", "returns": "string"},
      {"name": "server", "kind": "call", "shape": "server_stream", "yields": "string"},
      {"name": "unary", "kind": "call", "shape": "unary", "returns": "string", "idempotent": true}
    ])
  );
  assert_eq!(Call::bidi(string(), string()).shape(), Shape::Bidi);
  for (shape, name) in [
    (Shape::Unary, "unary"),
    (Shape::ServerStream, "server_stream"),
    (Shape::ClientStream, "client_stream"),
    (Shape::Bidi, "bidi"),
  ] {
    assert_eq!(shape.name(), name);
  }
}

#[test]
fn writes_docs_where_they_are_given() {
  let manifest = Manifest::new("app")
    .docs("The app.")
    .service(
      Service::new("chat").docs("Chat.").server_method(
        Method::notify("typing")
          .docs("Typing.")
          .param(Param::new("room", string()).docs("The room.")),
      ),
    )
    .channel(Channel::new("room", string()).snapshot(Primitive::U32.into()).docs("Rooms."))
    .datagram(Datagram::new("cursor", Direction::Both, Mode::Latest, string()).docs("Cursors."));
  let json = rendered(&manifest);
  assert_eq!(json["docs"], "The app.");
  assert_eq!(json["services"][0]["docs"], "Chat.");
  assert_eq!(json["services"][0]["server_methods"][0]["docs"], "Typing.");
  assert_eq!(json["services"][0]["server_methods"][0]["params"][0]["docs"], "The room.");
  assert_eq!(
    json["channels"][0],
    json!({"namespace": "room", "docs": "Rooms.", "publication": "string", "snapshot": "u32"})
  );
  assert_eq!(json["datagrams"][0]["docs"], "Cursors.");
}

#[test]
fn refuses_names_outside_the_grammar() {
  let manifest = Manifest::new("not-checked")
    .service(
      Service::new("chat-room")
        .server_method(Method::notify("1bad"))
        .client_method(Method::notify("ok")),
    )
    .datagram(Datagram::new("cur sor", Direction::Both, Mode::All, string()))
    .types(BTreeMap::from([
      (
        "Bad-Type".into(),
        Definition::Struct(Struct::new(vec![Field::new("foo-bar", string())])),
      ),
      ("Choice".into(), Definition::Enum(Enum::new(vec![Variant::unit("not.ok")]))),
    ]));
  let names: Vec<(String, String)> = errors(&manifest)
    .into_iter()
    .map(|error| match error {
      ManifestError::InvalidName { what, name } => (what, name),
      other => panic!("{other}"),
    })
    .collect();
  assert_eq!(
    names,
    [
      ("service", "chat-room"),
      ("server method of service `chat-room`", "1bad"),
      ("datagram topic", "cur sor"),
      ("type", "Bad-Type"),
      ("field of type `Bad-Type`", "foo-bar"),
      ("variant of type `Choice`", "not.ok"),
    ]
    .map(|(what, name)| (what.to_owned(), name.to_owned()))
  );
}

#[test]
fn refuses_a_channel_namespace_with_a_colon() {
  let manifest = Manifest::new("app").channel(Channel::new("room:x", string()));
  assert_eq!(
    errors(&manifest),
    [ManifestError::InvalidNamespace {
      namespace: "room:x".into()
    }]
  );
  assert!(errors(&manifest)[0].to_string().contains("no `:`"));
}

#[test]
fn refuses_a_reference_to_no_named_type() {
  let manifest = Manifest::new("app")
    .service(
      Service::new("chat").server_method(
        Method::call("send", Call::unary(reference("Nope")).error(reference("Gone")))
          .param(Param::new("room", reference("RoomId"))),
      ),
    )
    .channel(Channel::new("room", reference("Lost")))
    .types(BTreeMap::from([(
      "Wrapper".into(),
      Definition::Alias(Type::List(Box::new(reference("Missing")))),
    )]));
  let errors = errors(&manifest);
  assert_eq!(
    errors,
    [
      ManifestError::UnknownType {
        name: "RoomId".into(),
        at: "param `room` of server method `chat/send`".into()
      },
      ManifestError::UnknownType {
        name: "Nope".into(),
        at: "`returns` of server method `chat/send`".into()
      },
      ManifestError::UnknownType {
        name: "Gone".into(),
        at: "`error` of server method `chat/send`".into()
      },
      ManifestError::UnknownType {
        name: "Lost".into(),
        at: "`publication` of channel namespace `room`".into()
      },
      ManifestError::UnknownType {
        name: "Missing".into(),
        at: "type `Wrapper`".into()
      },
    ]
  );
  assert_eq!(
    errors[0].to_string(),
    "param `room` of server method `chat/send` refers to `RoomId`, which is not in `types`"
  );
}

#[test]
fn refuses_an_option_of_unit_built_by_hand() {
  let unit = || option(Primitive::Unit.into());
  let manifest = Manifest::new("app")
    .service(Service::new("s").client_method(Method::notify("n").param(Param::new("p", Type::Tuple(vec![unit()])))))
    .datagram(Datagram::new("d", Direction::Both, Mode::All, Type::Map(Box::new(unit()))))
    .types(BTreeMap::from([(
      "Holder".into(),
      Definition::Enum(Enum::new(vec![Variant::new(
        "Some",
        Type::Struct(Struct::new(vec![Field::new("value", option(unit()))])),
      )])),
    )]));
  assert_eq!(
    errors(&manifest),
    [
      ManifestError::OptionOfUnit {
        at: "param `p` of client method `s/n`".into()
      },
      ManifestError::OptionOfUnit {
        at: "datagram topic `d`".into()
      },
      ManifestError::OptionOfUnit {
        at: "field `value` of variant `Some` of type `Holder`".into()
      },
    ]
  );
}

#[test]
fn refuses_two_entries_with_one_name() {
  let manifest = Manifest::new("app")
    .service(
      Service::new("chat")
        .server_method(Method::notify("send"))
        .server_method(Method::notify("send"))
        .server_method(Method::notify("send"))
        .client_method(Method::notify("send")),
    )
    .service(Service::new("chat"))
    .channel(Channel::new("room", string()))
    .channel(Channel::new("room", string()))
    .datagram(Datagram::new("cursor", Direction::Both, Mode::All, string()))
    .datagram(Datagram::new("cursor", Direction::Both, Mode::All, string()));
  let duplicates: Vec<String> = errors(&manifest)
    .into_iter()
    .map(|error| match error {
      ManifestError::Duplicate { what, name } => format!("{what}: {name}"),
      other => panic!("{other}"),
    })
    .collect();
  assert_eq!(
    duplicates,
    [
      "service: chat",
      "server method of service `chat`: send",
      "channel namespace: room",
      "datagram topic: cursor"
    ]
  );
}

#[test]
fn refuses_params_fields_and_variants_with_one_name_or_outside_the_grammar() {
  let manifest = Manifest::new("app")
    .service(
      Service::new("s").server_method(
        Method::notify("n")
          .param(Param::new("a", string()))
          .param(Param::new("a", string()))
          .param(Param::new("foo-bar", string())),
      ),
    )
    .types(BTreeMap::from([
      (
        "Pair".into(),
        Definition::Struct(Struct::new(vec![Field::new("x", string()), Field::new("x", string())])),
      ),
      (
        "Choice".into(),
        Definition::Enum(Enum::new(vec![
          Variant::unit("V"),
          Variant::unit("V"),
          Variant::new(
            "W",
            Type::Struct(Struct::new(vec![Field::new("y", string()), Field::new("y", string())])),
          ),
        ])),
      ),
    ]));
  let found: Vec<String> = errors(&manifest).iter().map(ToString::to_string).collect();
  assert_eq!(
    found,
    [
      "more than one param of server method `s/n` is named `a`",
      "param of server method `s/n` name `foo-bar` is not a contract name: names match [A-Za-z_][A-Za-z0-9_]* (spec 12.2)",
      "more than one variant of type `Choice` is named `V`",
      "more than one field of variant `W` of type `Choice` is named `y`",
      "more than one field of type `Pair` is named `x`",
    ]
  );
}

#[test]
fn refuses_an_optional_field_whose_type_is_not_an_option() {
  let manifest = Manifest::new("app").types(BTreeMap::from([(
    "Options".into(),
    Definition::Struct(Struct::new(vec![
      Field::new("lang", string()).optional(),
      Field::new("tz", option(string())).optional(),
    ])),
  )]));
  assert_eq!(
    errors(&manifest),
    [ManifestError::OptionalNotOption {
      at: "field `lang` of type `Options`".into()
    }]
  );
}

#[test]
fn refuses_an_option_of_an_alias_of_unit_and_an_alias_cycle() {
  let manifest = Manifest::new("app").types(BTreeMap::from([
    ("Nothing".into(), Definition::Alias(Primitive::Unit.into())),
    ("Again".into(), Definition::Alias(reference("Nothing"))),
    ("Maybe".into(), Definition::Alias(option(reference("Again")))),
    ("Fine".into(), Definition::Alias(option(reference("Nested")))),
    ("Nested".into(), Definition::Alias(option(string()))),
    ("Left".into(), Definition::Alias(reference("Right"))),
    ("Right".into(), Definition::Alias(reference("Left"))),
  ]));
  assert_eq!(
    errors(&manifest),
    [
      ManifestError::OptionOfUnit {
        at: "type `Maybe`".into()
      },
      ManifestError::AliasCycle { name: "Left".into() },
      ManifestError::AliasCycle { name: "Right".into() },
    ]
  );
}

#[test]
fn fingerprints_only_a_valid_manifest_and_leaves_out_its_name_and_docs() {
  let example = spec_example().fingerprint().unwrap();
  assert_eq!(hex(example), SPEC_EXAMPLE);
  let renamed = Manifest::new("another-name").docs("Docs.");
  let plain = Manifest::new("app");
  assert_eq!(renamed.fingerprint(), plain.fingerprint());
  assert!(
    Manifest::new("app")
      .channel(Channel::new("room:x", string()))
      .fingerprint()
      .is_err()
  );
}
