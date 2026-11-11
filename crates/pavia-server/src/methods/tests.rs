use pavia_proto::stream::Shape;

use super::{InvalidMethodName, Kind, MethodEntry, MethodName, MethodRegistry, RegisterError};

#[test]
fn accepts_names_that_follow_the_rule() {
  let mut registry = MethodRegistry::new();
  for name in ["chat/send", "_svc/_m", "A1/b2", "a/b", "Z_9/_"] {
    assert_eq!(
      MethodName::parse(name).map(|name| name.as_str().to_owned()),
      Ok(name.to_owned()),
      "{name}"
    );
    assert_eq!(registry.register_server_method(name, MethodEntry::call(())), Ok(()), "{name}");
    assert_eq!(registry.register_client_method(name, MethodEntry::call(())), Ok(()), "{name}");
  }
}

#[test]
fn rejects_names_that_break_the_rule() {
  for name in [
    "chat/1bad",
    "chat",
    "chat/",
    "/send",
    "chat/send/extra",
    "chat/se-nd",
    "chät/send",
    "",
    "1chat/send",
    "chat /send",
    "/",
  ] {
    let invalid = InvalidMethodName(name.to_owned());
    assert_eq!(MethodName::parse(name), Err(invalid.clone()), "{name:?}");
    let mut registry = MethodRegistry::new();
    assert_eq!(
      registry.register_server_method(name, MethodEntry::call(())),
      Err(RegisterError::InvalidName(invalid.clone())),
      "{name:?}"
    );
    assert_eq!(
      registry.register_client_method(name, MethodEntry::call(())),
      Err(RegisterError::InvalidName(invalid)),
      "{name:?}"
    );
  }
}

#[test]
fn shows_and_parses_a_name_as_written() {
  let name: MethodName = "chat/send".parse().unwrap();
  assert_eq!(name.to_string(), "chat/send");
  assert!("chat/".parse::<MethodName>().is_err());
}

#[test]
fn splits_a_name_into_service_and_method() {
  let name = MethodName::parse("chat/send").unwrap();
  assert_eq!((name.service(), name.method()), ("chat", "send"));
}

#[test]
fn registers_and_finds_a_server_method() {
  let mut registry = MethodRegistry::new();
  registry.register_server_method("chat/send", MethodEntry::call(1)).unwrap();
  let method = registry.server_method("chat/send").unwrap();
  assert_eq!(method.handler, 1);
  assert_eq!(
    method.kind,
    Kind::Call {
      shape: Shape::Unary,
      idempotent: false
    }
  );
}

#[test]
fn refuses_a_duplicate_on_the_same_side_and_keeps_the_first() {
  let mut registry = MethodRegistry::new();
  registry.register_server_method("chat/send", MethodEntry::call(1)).unwrap();
  assert_eq!(
    registry.register_server_method("chat/send", MethodEntry::call(2)),
    Err(RegisterError::Duplicate(MethodName::parse("chat/send").unwrap()))
  );
  assert_eq!(registry.server_method("chat/send").unwrap().handler, 1);
  registry.register_client_method("ui/alert", MethodEntry::call(3)).unwrap();
  assert!(matches!(
    registry.register_client_method("ui/alert", MethodEntry::call(4)),
    Err(RegisterError::Duplicate(_))
  ));
  assert_eq!(registry.client_method("ui/alert").unwrap().handler, 3);
}

#[test]
fn tells_names_apart_by_case() {
  let mut registry = MethodRegistry::new();
  registry.register_server_method("chat/send", MethodEntry::call(1)).unwrap();
  assert!(registry.server_method("chat/Send").is_none());
  registry.register_server_method("chat/Send", MethodEntry::call(2)).unwrap();
  assert_eq!(registry.server_method("chat/Send").unwrap().handler, 2);
  assert_eq!(registry.server_method("chat/send").unwrap().handler, 1);
}

#[test]
fn keeps_server_and_client_methods_apart() {
  let mut registry = MethodRegistry::new();
  registry.register_client_method("ui/alert", MethodEntry::call(1)).unwrap();
  assert!(registry.server_method("ui/alert").is_none());
  registry.register_server_method("ui/alert", MethodEntry::call(2)).unwrap();
  assert_eq!(registry.client_method("ui/alert").unwrap().handler, 1);
  assert_eq!(registry.server_method("ui/alert").unwrap().handler, 2);
}

#[test]
fn records_kind_shape_and_idempotence() {
  let mut registry = MethodRegistry::new();
  registry
    .register_server_method("chat/typing", MethodEntry::notify(1))
    .unwrap();
  registry
    .register_server_method(
      "chat/history",
      MethodEntry::call(2).with_shape(Shape::ServerStream).idempotent(),
    )
    .unwrap();
  assert_eq!(registry.server_method("chat/typing").unwrap().kind, Kind::Notify);
  assert_eq!(
    registry.server_method("chat/history").unwrap().kind,
    Kind::Call {
      shape: Shape::ServerStream,
      idempotent: true
    }
  );
  assert_eq!(
    MethodEntry::call(()).idempotent().with_shape(Shape::Bidirectional).kind,
    Kind::Call {
      shape: Shape::Bidirectional,
      idempotent: true
    }
  );
}

#[test]
fn gives_a_notify_method_no_shape_and_no_retries() {
  let method = MethodEntry::notify(1).with_shape(Shape::ServerStream).idempotent();
  assert_eq!(method.kind, Kind::Notify);
}

#[test]
fn lists_the_methods_of_each_side() {
  let mut registry = MethodRegistry::new();
  registry.register_server_method("chat/send", MethodEntry::call(1)).unwrap();
  registry
    .register_server_method("chat/typing", MethodEntry::notify(2))
    .unwrap();
  registry.register_client_method("ui/alert", MethodEntry::call(3)).unwrap();
  let mut server: Vec<_> = registry
    .server_methods()
    .map(|(name, method)| (name.as_str(), method.handler))
    .collect();
  server.sort_unstable();
  assert_eq!(server, [("chat/send", 1), ("chat/typing", 2)]);
  let client: Vec<_> = registry
    .client_methods()
    .map(|(name, method)| (name.as_str(), method.handler))
    .collect();
  assert_eq!(client, [("ui/alert", 3)]);
}

#[test]
fn explains_what_went_wrong() {
  assert_eq!(
    RegisterError::Duplicate(MethodName::parse("chat/send").unwrap()).to_string(),
    "method chat/send is already registered"
  );
  assert!(InvalidMethodName("chat".into()).to_string().contains("\"chat\""));
}
