use super::{camel, pascal};

#[test]
fn camel_cases_a_name_keeping_leading_and_trailing_underscores() {
  for (name, expected) in [
    ("sent_at", "sentAt"),
    ("sentAt", "sentAt"),
    ("text", "text"),
    ("_x", "_x"),
    ("__x_y", "__xY"),
    ("x_1", "x1"),
    ("a__b", "aB"),
    ("a_b", "aB"),
    ("HTTP_status", "HTTPStatus"),
    ("Room_id", "RoomId"),
    ("x_", "x_"),
    ("a_b__", "aB__"),
    ("_", "_"),
    ("__", "__"),
    ("user_id2", "userId2"),
    ("to_string", "toString"),
  ] {
    assert_eq!(camel(name), expected, "{name}");
  }
}

#[test]
fn pascal_cases_a_name_dropping_every_underscore() {
  for (name, expected) in [
    ("system_notice", "SystemNotice"),
    ("_2fa", "2fa"),
    ("a__b_", "AB"),
    ("chat", "Chat"),
  ] {
    assert_eq!(pascal(name), expected, "{name}");
  }
}
