use pavia_proto::stream::Shape;

use super::{InvalidMethodName, Kind, Method, MethodName, MethodRegistry, RegisterError};

#[test]
fn accepts_names_that_follow_the_rule() {
  for name in ["chat/send", "_svc/_m", "A1/b2", "a/b", "Z_9/_"] {
    assert_eq!(
      MethodName::parse(name).map(|name| name.as_str().to_owned()),
      Ok(name.to_owned()),
      "{name}"
    );
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
    assert_eq!(MethodName::parse(name), Err(InvalidMethodName(name.to_owned())), "{name:?}");
  }
}

#[test]
fn shows_and_parses_a_name_as_written() {
  let name: MethodName = "chat/send".parse().unwrap();
  assert_eq!(name.to_string(), "chat/send");
  assert!("chat/".parse::<MethodName>().is_err());
}

#[test]
fn registers_and_finds_a_server_method() {
  let mut registry = MethodRegistry::new();
  registry.register_server_method("chat/send", Method::call(1)).unwrap();
  let method = registry.server_method("chat/send").unwrap();
  assert_eq!(method.handler, 1);
  assert_eq!(method.kind, Kind::Call);
  assert_eq!(method.shape, Shape::Unary);
  assert!(!method.idempotent);
}

#[test]
fn refuses_a_duplicate_on_the_same_side_and_keeps_the_first() {
  let mut registry = MethodRegistry::new();
  registry.register_server_method("chat/send", Method::call(1)).unwrap();
  assert_eq!(
    registry.register_server_method("chat/send", Method::call(2)),
    Err(RegisterError::Duplicate(MethodName::parse("chat/send").unwrap()))
  );
  assert_eq!(registry.server_method("chat/send").unwrap().handler, 1);
  registry.register_client_method("ui/alert", Method::call(3)).unwrap();
  assert!(matches!(
    registry.register_client_method("ui/alert", Method::call(4)),
    Err(RegisterError::Duplicate(_))
  ));
  assert_eq!(registry.client_method("ui/alert").unwrap().handler, 3);
}

#[test]
fn refuses_an_invalid_name_on_either_side() {
  let mut registry = MethodRegistry::new();
  assert_eq!(
    registry.register_server_method("chat/1bad", Method::call(1)),
    Err(RegisterError::InvalidName(InvalidMethodName("chat/1bad".into())))
  );
  assert!(matches!(
    registry.register_client_method("chat", Method::call(1)),
    Err(RegisterError::InvalidName(_))
  ));
  assert!(registry.server_method("chat/1bad").is_none());
}

#[test]
fn tells_names_apart_by_case() {
  let mut registry = MethodRegistry::new();
  registry.register_server_method("chat/send", Method::call(1)).unwrap();
  assert!(registry.server_method("chat/Send").is_none());
  registry.register_server_method("chat/Send", Method::call(2)).unwrap();
  assert_eq!(registry.server_method("chat/Send").unwrap().handler, 2);
  assert_eq!(registry.server_method("chat/send").unwrap().handler, 1);
}

#[test]
fn keeps_server_and_client_methods_apart() {
  let mut registry = MethodRegistry::new();
  registry.register_client_method("ui/alert", Method::call(1)).unwrap();
  assert!(registry.server_method("ui/alert").is_none());
  registry.register_server_method("ui/alert", Method::call(2)).unwrap();
  assert_eq!(registry.client_method("ui/alert").unwrap().handler, 1);
  assert_eq!(registry.server_method("ui/alert").unwrap().handler, 2);
}

#[test]
fn records_kind_shape_and_idempotence() {
  let mut registry = MethodRegistry::new();
  registry.register_server_method("chat/typing", Method::notify(1)).unwrap();
  registry
    .register_server_method("chat/history", Method::call(2).with_shape(Shape::ServerStream).idempotent())
    .unwrap();
  let typing = registry.server_method("chat/typing").unwrap();
  assert_eq!(
    (typing.kind, typing.shape, typing.idempotent),
    (Kind::Notify, Shape::Unary, false)
  );
  let history = registry.server_method("chat/history").unwrap();
  assert_eq!(
    (history.kind, history.shape, history.idempotent),
    (Kind::Call, Shape::ServerStream, true)
  );
}

#[test]
fn explains_what_went_wrong() {
  assert_eq!(
    RegisterError::Duplicate(MethodName::parse("chat/send").unwrap()).to_string(),
    "method chat/send is already registered"
  );
  assert!(InvalidMethodName("chat".into()).to_string().contains("\"chat\""));
}
