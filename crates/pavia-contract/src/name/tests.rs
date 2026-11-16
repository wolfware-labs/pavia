use super::is_valid;

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
