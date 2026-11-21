//! The contract fingerprint (§12.5): SHA-256 over a manifest in canonical form.
//!
//! [`fingerprint`] reads manifest JSON, from this crate's writer or from anywhere else, and accepts it
//! only in the one form §12.2 allows: no `null`, no optional field equal to its default (an empty
//! `params` included), and `services`, `server_methods`, `client_methods`, `channels` and `datagrams`
//! sorted by name in byte order. It then removes every `docs` field and the top-level `name`, which
//! do not change the contract, writes the rest with the JSON Canonicalization Scheme (RFC 8785) and
//! hashes those bytes. The fingerprint names one exact contract version and says nothing about
//! compatibility.
//!
//! ```
//! use pavia_contract::fingerprint::{canonical, fingerprint};
//!
//! let manifest = serde_json::json!({"pavia": 1, "name": "app", "services": [], "types": {}});
//! assert_eq!(canonical(&manifest).unwrap(), r#"{"pavia":1,"services":[],"types":{}}"#);
//! assert_eq!(fingerprint(&manifest).unwrap().len(), 32);
//! ```

use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// The arrays of named entries §12.2 sorts, and the key each is sorted by: top-level arrays, then the
/// method arrays of a service.
const SORTED_TOP: [(&str, &str); 3] = [("services", "name"), ("channels", "namespace"), ("datagrams", "topic")];
const SORTED_IN_SERVICE: [(&str, &str); 2] = [("server_methods", "name"), ("client_methods", "name")];

/// The SHA-256 of `manifest` in canonical form.
///
/// # Errors
///
/// [`FingerprintError`] for a manifest outside its canonical form.
pub fn fingerprint(manifest: &Value) -> Result<[u8; 32], FingerprintError> {
  Ok(Sha256::digest(canonical(manifest)?.as_bytes()).into())
}

/// `manifest` without its `docs` fields and top-level `name`, written with RFC 8785: the bytes
/// [`fingerprint`] hashes.
///
/// # Errors
///
/// [`FingerprintError`] for a manifest outside its canonical form.
pub fn canonical(manifest: &Value) -> Result<String, FingerprintError> {
  let Value::Object(top) = manifest else {
    return Err(FingerprintError::NotAnObject);
  };
  check(manifest, &mut "$".to_owned())?;
  for (array, key) in SORTED_TOP {
    sorted(top.get(array), key, &format!("$.{array}"))?;
  }
  if let Some(Value::Array(services)) = top.get("services") {
    for (index, service) in services.iter().enumerate() {
      for (array, key) in SORTED_IN_SERVICE {
        sorted(service.get(array), key, &format!("$.services[{index}].{array}"))?;
      }
    }
  }
  let mut stripped = strip_docs(manifest);
  if let Value::Object(top) = &mut stripped {
    top.remove("name");
  }
  let mut out = String::new();
  jcs(&stripped, &mut out);
  Ok(out)
}

/// Why a manifest has no fingerprint (§12.5).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum FingerprintError {
  /// The manifest is not a JSON object.
  #[error("a manifest is a JSON object")]
  NotAnObject,
  /// A `null` anywhere: §12.2 omits an absent optional field instead.
  #[error("{at} is null; an absent field is omitted (spec 12.2)")]
  Null {
    /// Where it is: `$.services[0].server_methods[1].returns`.
    at: String,
  },
  /// An optional field equal to its default (`"idempotent": false`, `"returns": "unit"`, `"params":
  /// []`), which §12.2 omits.
  #[error("{at} holds its default value and must be omitted (spec 12.2)")]
  Default {
    /// Where it is.
    at: String,
  },
  /// An array of named entries out of byte order by name, or with one name twice.
  #[error("{at} is not sorted by {key} in byte order (spec 12.2)")]
  Unsorted {
    /// The array.
    at: String,
    /// The key it is sorted by.
    key: &'static str,
  },
}

/// Refuses a `null`, and an optional field equal to its default, anywhere in `value`.
fn check(value: &Value, at: &mut String) -> Result<(), FingerprintError> {
  match value {
    Value::Null => Err(FingerprintError::Null { at: at.clone() }),
    Value::Array(items) => items
      .iter()
      .enumerate()
      .try_for_each(|(index, item)| within(at, &format!("[{index}]"), |at| check(item, at))),
    Value::Object(members) => members.iter().try_for_each(|(key, member)| {
      within(at, &format!(".{key}"), |at| {
        if is_default(key, member) {
          return Err(FingerprintError::Default { at: at.clone() });
        }
        check(member, at)
      })
    }),
    _ => Ok(()),
  }
}

fn within<T>(at: &mut String, step: &str, walk: impl FnOnce(&mut String) -> T) -> T {
  let length = at.len();
  at.push_str(step);
  let result = walk(at);
  at.truncate(length);
  result
}

/// Whether `value` is the default of the optional field `key` (§12.2).
fn is_default(key: &str, value: &Value) -> bool {
  match key {
    "idempotent" | "open" | "history" | "optional" => *value == Value::Bool(false),
    "returns" => value.as_str() == Some("unit"),
    "params" => value.as_array().is_some_and(Vec::is_empty),
    _ => false,
  }
}

/// Refuses an array of named entries that is not strictly sorted by `key` in byte order.
fn sorted(array: Option<&Value>, key: &'static str, at: &str) -> Result<(), FingerprintError> {
  let Some(Value::Array(entries)) = array else {
    return Ok(());
  };
  let names: Vec<Option<&str>> = entries.iter().map(|entry| entry.get(key).and_then(Value::as_str)).collect();
  if names.windows(2).any(|pair| match pair {
    [Some(left), Some(right)] => left.as_bytes() >= right.as_bytes(),
    _ => true,
  }) || names.iter().any(Option::is_none)
  {
    return Err(FingerprintError::Unsorted { at: at.to_owned(), key });
  }
  Ok(())
}

fn strip_docs(value: &Value) -> Value {
  match value {
    Value::Array(items) => Value::Array(items.iter().map(strip_docs).collect()),
    Value::Object(members) => Value::Object(
      members
        .iter()
        .filter(|(key, _)| *key != "docs")
        .map(|(key, member)| (key.clone(), strip_docs(member)))
        .collect::<Map<_, _>>(),
    ),
    other => other.clone(),
  }
}

/// Writes `value` as RFC 8785 serializes it: object members sorted by the UTF-16 code units of their
/// keys, no whitespace, strings and numbers as ECMAScript's `JSON.stringify` writes them.
fn jcs(value: &Value, out: &mut String) {
  match value {
    Value::Null => out.push_str("null"),
    Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
    Value::Number(number) => number_text(number, out),
    Value::String(text) => string(text, out),
    Value::Array(items) => {
      out.push('[');
      for (index, item) in items.iter().enumerate() {
        if index > 0 {
          out.push(',');
        }
        jcs(item, out);
      }
      out.push(']');
    }
    Value::Object(members) => {
      let mut sorted: Vec<(&String, &Value)> = members.iter().collect();
      sorted.sort_by(|(left, _), (right, _)| left.encode_utf16().cmp(right.encode_utf16()));
      out.push('{');
      for (index, (key, member)) in sorted.into_iter().enumerate() {
        if index > 0 {
          out.push(',');
        }
        string(key, out);
        out.push(':');
        jcs(member, out);
      }
      out.push('}');
    }
  }
}

fn number_text(number: &Number, out: &mut String) {
  match (number.as_u64(), number.as_i64(), number.as_f64()) {
    (Some(unsigned), _, _) if unsigned <= 1 << 53 => out.push_str(&unsigned.to_string()),
    (_, Some(signed), _) if signed.unsigned_abs() <= 1 << 53 => out.push_str(&signed.to_string()),
    (_, _, Some(float)) => out.push_str(ryu_js::Buffer::new().format_finite(float)),
    _ => out.push('0'),
  }
}

fn string(text: &str, out: &mut String) {
  out.push('"');
  for char in text.chars() {
    match char {
      '"' => out.push_str("\\\""),
      '\\' => out.push_str("\\\\"),
      '\u{8}' => out.push_str("\\b"),
      '\u{c}' => out.push_str("\\f"),
      '\n' => out.push_str("\\n"),
      '\r' => out.push_str("\\r"),
      '\t' => out.push_str("\\t"),
      control if u32::from(control) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(control))),
      other => out.push(other),
    }
  }
  out.push('"');
}

#[cfg(test)]
pub(crate) mod tests;
