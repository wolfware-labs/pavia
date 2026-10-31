//! Script patterns over received CBOR (`vectors/README.md`, "Patterns" and "Captures"): `*`,
//! open maps written with `...`, and `$name` for a value captured earlier.

use std::collections::HashMap;

use ciborium::Value;
use pavia_vectors::diag::Diag;

pub type Captures = HashMap<String, Value>;

/// Whether `value` matches `pattern`. Map entries match in any order; an open map may hold more
/// keys than it lists.
pub fn matches(pattern: &Diag, value: &Value, captures: &Captures) -> bool {
  match (pattern, value) {
    (Diag::Any, _) => true,
    (Diag::Ref(name), _) => captures.get(name) == Some(value),
    (Diag::Uint(n), Value::Integer(integer)) => i128::from(*integer) == i128::from(*n),
    (Diag::Nint(n), Value::Integer(integer)) => i128::from(*integer) == -1 - i128::from(*n),
    (Diag::Float(x), Value::Float(y)) => x == y,
    (Diag::Text(x), Value::Text(y)) => x == y,
    (Diag::Bytes(x), Value::Bytes(y)) => x == y,
    (Diag::Bool(x), Value::Bool(y)) => x == y,
    (Diag::Null, Value::Null) => true,
    (Diag::Tag(x, inner), Value::Tag(y, value)) => x == y && matches(inner, value, captures),
    (Diag::Array(items), Value::Array(values)) => {
      items.len() == values.len() && items.iter().zip(values).all(|(item, value)| matches(item, value, captures))
    }
    (Diag::Map { entries, open }, Value::Map(values)) => {
      (*open || entries.len() == values.len())
        && entries.iter().all(|(key, item)| {
          let Ok(key) = resolve(key, captures) else {
            return false;
          };
          values
            .iter()
            .any(|(candidate, value)| *candidate == key && matches(item, value, captures))
        })
    }
    _ => false,
  }
}

/// The value a `send` writes: `pattern` with every `$name` replaced by its capture.
pub fn resolve(pattern: &Diag, captures: &Captures) -> Result<Value, String> {
  Ok(match pattern {
    Diag::Ref(name) => captures
      .get(name)
      .cloned()
      .ok_or_else(|| format!("${name} was never captured"))?,
    Diag::Array(items) => Value::Array(items.iter().map(|item| resolve(item, captures)).collect::<Result<_, _>>()?),
    Diag::Map { entries, open: false } => Value::Map(
      entries
        .iter()
        .map(|(key, item)| Ok((resolve(key, captures)?, resolve(item, captures)?)))
        .collect::<Result<_, String>>()?,
    ),
    Diag::Tag(tag, inner) => Value::Tag(*tag, Box::new(resolve(inner, captures)?)),
    plain => plain.to_cbor()?,
  })
}

/// The value at a capture path below `value`: map keys (integers first, then text) and array
/// indexes.
pub fn at_path<'v>(mut value: &'v Value, path: &[&str]) -> Option<&'v Value> {
  for segment in path {
    value = match value {
      Value::Map(entries) => entries.iter().find(|(key, _)| key_is(key, segment)).map(|(_, value)| value)?,
      Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
      _ => return None,
    };
  }
  Some(value)
}

fn key_is(key: &Value, segment: &str) -> bool {
  match key {
    Value::Integer(integer) => segment.parse::<i128>().is_ok_and(|n| n == i128::from(*integer)),
    Value::Text(text) => text == segment,
    _ => false,
  }
}
