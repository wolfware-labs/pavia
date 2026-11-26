//! Every manifest vector under `vectors/manifests/` (§12.2): a manifest within the limits reads and
//! renders back to the same JSON, and one past them is refused for its `error`.

use pavia_contract::manifest::{Manifest, ManifestError, ReadError};
use serde_json::Value;

fn kind(error: &ReadError) -> &'static str {
  match error {
    ReadError::TooDeep { .. } => "too_deep",
    ReadError::Invalid(errors) if errors.iter().all(|error| matches!(error, ManifestError::NameTooLong { .. })) => {
      "name_too_long"
    }
    _ => "other",
  }
}

#[test]
fn runs_every_manifest_vector() {
  let mut count = 0;
  for path in pavia_vectors::files("manifests") {
    for case in pavia_vectors::load(&path)["cases"].as_array().expect("cases") {
      let id = format!("{}#{}", path.display(), case["id"].as_str().expect("an id"));
      let text = case["manifest"].to_string();
      match case["error"].as_str() {
        Some(expected) => {
          let error = Manifest::read(&text).expect_err(&id);
          assert_eq!(kind(&error), expected, "{id}: {error}");
        }
        None => {
          let manifest = Manifest::read(&text).unwrap_or_else(|error| panic!("{id}: {error}"));
          let rendered: Value = serde_json::from_str(&manifest.render().expect(&id)).expect(&id);
          assert_eq!(rendered, case["manifest"], "{id}");
        }
      }
      count += 1;
    }
  }
  assert!(count >= 51, "only {count} manifest vectors");
}
