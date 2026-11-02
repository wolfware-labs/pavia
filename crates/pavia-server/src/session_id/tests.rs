use std::collections::HashSet;

use pavia_proto::session_id::MAX_LEN;

use super::generate;

#[test]
fn a_million_ids_are_unique_and_url_safe() {
  let count = 1_000_000;
  let mut seen = HashSet::with_capacity(count);
  for _ in 0..count {
    let id = generate().expect("operating system randomness");
    let text = id.as_str();
    assert!((1..=MAX_LEN).contains(&text.len()), "{text:?}");
    assert!(
      text
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'),
      "{text:?}"
    );
    assert!(seen.insert(id.to_string()), "duplicate {text:?}");
  }
  assert_eq!(seen.len(), count);
}
