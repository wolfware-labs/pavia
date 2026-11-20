use super::{is_valid, is_valid_namespace};

#[test]
fn accepts_names_in_the_grammar() {
  for name in ["a", "Z", "_", "_x", "sent_at", "Page_List_Message", "u32", "x9_"] {
    assert!(is_valid(name), "{name}");
  }
}

#[test]
fn refuses_names_outside_the_grammar() {
  for name in ["", "9a", "foo-bar", "a b", "Größe", "é", "a.b", "-", "a$"] {
    assert!(!is_valid(name), "{name}");
  }
}

#[test]
fn accepts_namespaces_of_channel_name_characters() {
  let longest = "n".repeat(255);
  for namespace in ["room", "a", "Team_1", "ops-eu", "a.b", "a/b", "me@host", longest.as_str()] {
    assert!(is_valid_namespace(namespace), "{namespace}");
  }
}

#[test]
fn refuses_namespaces_with_a_colon_or_other_characters() {
  let longer = "n".repeat(256);
  for namespace in ["", "room:x", ":", "a b", "é", "a$", "a#b", longer.as_str()] {
    assert!(!is_valid_namespace(namespace), "{namespace}");
  }
}
