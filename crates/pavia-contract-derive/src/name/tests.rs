use proc_macro2::Span;
use syn::Ident;

use super::{is_valid, of};
use crate::attr::Rename;

#[test]
fn checks_the_grammar() {
  for name in ["a", "_", "Z9_", "sent_at"] {
    assert!(is_valid(name), "{name}");
  }
  for name in ["", "9a", "foo-bar", "Größe", "a b"] {
    assert!(!is_valid(name), "{name}");
  }
}

#[test]
fn names_by_the_rename_when_there_is_one() {
  let ident = Ident::new("display_name", Span::call_site());
  assert_eq!(of(&ident, None).unwrap(), "display_name");
  let rename = Rename {
    value: "displayName".into(),
    span: Span::call_site(),
  };
  assert_eq!(of(&ident, Some(&rename)).unwrap(), "displayName");
}

#[test]
fn drops_the_raw_prefix() {
  assert_eq!(of(&Ident::new_raw("type", Span::call_site()), None).unwrap(), "type");
}

#[test]
fn refuses_a_rename_outside_the_grammar() {
  let ident = Ident::new("value", Span::call_site());
  let rename = Rename {
    value: "foo-bar".into(),
    span: Span::call_site(),
  };
  assert_eq!(
    of(&ident, Some(&rename)).unwrap_err().to_string(),
    "`foo-bar` is not a contract name: names match [A-Za-z_][A-Za-z0-9_]* (spec 12.2)",
  );
}
