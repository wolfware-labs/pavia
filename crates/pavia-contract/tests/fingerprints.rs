//! Every fingerprint vector under `vectors/fingerprints/` (§12.5): a manifest gives exactly its
//! `canonical` text and `fingerprint`, or is refused for its `error`.

use pavia_contract::fingerprint::{FingerprintError, canonical, fingerprint};

fn kind(error: &FingerprintError) -> &'static str {
  match error {
    FingerprintError::NotAnObject { .. } => "not_an_object",
    FingerprintError::Null { .. } => "null",
    FingerprintError::Default { .. } => "default",
    FingerprintError::NotAnArray { .. } => "not_an_array",
    FingerprintError::Unsorted { .. } => "unsorted",
    FingerprintError::NotADouble { .. } => "not_a_double",
    _ => "other",
  }
}

#[test]
fn runs_every_fingerprint_vector() {
  let mut count = 0;
  for path in pavia_vectors::files("fingerprints") {
    for case in pavia_vectors::load(&path)["cases"].as_array().expect("cases") {
      let id = format!("{}#{}", path.display(), case["id"].as_str().expect("an id"));
      let manifest = &case["manifest"];
      match case["error"].as_str() {
        Some(expected) => {
          let error = fingerprint(manifest).expect_err(&id);
          assert_eq!(kind(&error), expected, "{id}: {error}");
        }
        None => {
          assert_eq!(
            canonical(manifest).as_deref(),
            Ok(case["canonical"].as_str().expect("canonical")),
            "{id}"
          );
          let hex: String = fingerprint(manifest)
            .unwrap()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
          assert_eq!(hex, case["fingerprint"].as_str().expect("fingerprint"), "{id}");
        }
      }
      count += 1;
    }
  }
  assert!(count >= 10, "only {count} fingerprint vectors");
}
