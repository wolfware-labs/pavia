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
    (Diag::Ref(name), _) => captures.get(name).is_some_and(|captured| same(captured, value)),
    (Diag::Uint(n), Value::Integer(integer)) => i128::from(*integer) == i128::from(*n),
    (Diag::Nint(n), Value::Integer(integer)) => i128::from(*integer) == -1 - i128::from(*n),
    (Diag::Float(x), Value::Float(y)) => same_float(*x, *y),
    (Diag::Text(x), Value::Text(y)) => x == y,
    (Diag::Bytes(x), Value::Bytes(y)) => x == y,
    (Diag::Bool(x), Value::Bool(y)) => x == y,
    (Diag::Null, Value::Null) => true,
    (Diag::Tag(x, inner), Value::Tag(y, value)) => x == y && matches(inner, value, captures),
    (Diag::Array(items), Value::Array(values)) => {
      items.len() == values.len() && items.iter().zip(values).all(|(item, value)| matches(item, value, captures))
    }
    (Diag::Map { entries, open }, Value::Map(values)) => {
      (*open || entries.len() == values.len()) && assign(entries, values, &mut vec![false; values.len()], captures)
    }
    _ => false,
  }
}

/// Value equality that holds every NaN equal, tells 0.0 from -0.0, as CBOR encodes them, and
/// compares map entries in any order, each entry used once.
pub fn same(a: &Value, b: &Value) -> bool {
  match (a, b) {
    (Value::Float(x), Value::Float(y)) => same_float(*x, *y),
    (Value::Tag(x, a), Value::Tag(y, b)) => x == y && same(a, b),
    (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b)),
    (Value::Map(a), Value::Map(b)) => {
      let mut used = vec![false; b.len()];
      a.len() == b.len()
        && a.iter().all(|(ka, va)| {
          let found = (0..b.len()).find(|&index| !used[index] && same(ka, &b[index].0) && same(va, &b[index].1));
          found.inspect(|&index| used[index] = true).is_some()
        })
    }
    _ => a == b,
  }
}

fn same_float(x: f64, y: f64) -> bool {
  x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan())
}

/// Whether every pattern entry matches a received entry of its own, trying each free candidate
/// in turn so that a `*` key never takes the entry a later literal key needs.
fn assign(entries: &[(Diag, Diag)], values: &[(Value, Value)], used: &mut [bool], captures: &Captures) -> bool {
  let Some(((key, item), rest)) = entries.split_first() else {
    return true;
  };
  for (index, (candidate, value)) in values.iter().enumerate() {
    if !used[index] && matches(key, candidate, captures) && matches(item, value, captures) {
      used[index] = true;
      if assign(rest, values, used, captures) {
        return true;
      }
      used[index] = false;
    }
  }
  false
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
