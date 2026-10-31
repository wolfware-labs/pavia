use std::collections::HashSet;
use std::fmt;

use bytes::BufMut;
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};

use super::{DecodeError, EncodeError, Value};

const DUPLICATE_KEY: &str = "duplicate object key";
const TOO_DEEP: &str = "nested deeper than the depth limit";

pub(super) fn decode(data: &[u8], max_depth: usize) -> Result<Value, DecodeError> {
  let text = std::str::from_utf8(data).map_err(|_| DecodeError::InvalidUtf8)?;
  let mut deserializer = serde_json::Deserializer::from_str(text);
  let seed = Seed { parent: 0, max_depth };
  let value = seed.deserialize(&mut deserializer).map_err(|error| {
    let message = error.to_string();
    if message.starts_with(DUPLICATE_KEY) {
      DecodeError::DuplicateKey
    } else if message.starts_with(TOO_DEEP) {
      DecodeError::TooDeep
    } else {
      DecodeError::Malformed
    }
  })?;
  deserializer.end().map_err(|_| DecodeError::TrailingBytes)?;
  Ok(value)
}

pub(super) fn encode(value: &Value, out: &mut impl BufMut) -> Result<(), EncodeError> {
  write(value, &mut Discard)?;
  write(value, &mut Out(out))
}

trait Text {
  fn put(&mut self, bytes: &[u8]);
}

struct Discard;

impl Text for Discard {
  fn put(&mut self, _: &[u8]) {}
}

struct Out<'b, B: BufMut>(&'b mut B);

impl<B: BufMut> Text for Out<'_, B> {
  fn put(&mut self, bytes: &[u8]) {
    self.0.put_slice(bytes);
  }
}

#[derive(Clone, Copy)]
struct Seed {
  parent: usize,
  max_depth: usize,
}

impl Seed {
  fn nested<E: de::Error>(self) -> Result<Self, E> {
    let depth = self.parent + 1;
    if depth > self.max_depth {
      Err(E::custom(TOO_DEEP))
    } else {
      Ok(Self {
        parent: depth,
        max_depth: self.max_depth,
      })
    }
  }
}

impl<'de> DeserializeSeed<'de> for Seed {
  type Value = Value;

  fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
    deserializer.deserialize_any(JsonVisitor(self))
  }
}

struct JsonVisitor(Seed);

impl<'de> Visitor<'de> for JsonVisitor {
  type Value = Value;

  fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str("a JSON value")
  }

  fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
    Ok(Value::Null)
  }

  fn visit_bool<E: de::Error>(self, b: bool) -> Result<Value, E> {
    Ok(Value::Bool(b))
  }

  fn visit_u64<E: de::Error>(self, n: u64) -> Result<Value, E> {
    Ok(Value::Unsigned(n))
  }

  fn visit_i64<E: de::Error>(self, n: i64) -> Result<Value, E> {
    Ok(match u64::try_from(n) {
      Ok(n) => Value::Unsigned(n),
      Err(_) => Value::Negative(!n as u64),
    })
  }

  fn visit_f64<E: de::Error>(self, x: f64) -> Result<Value, E> {
    Ok(Value::Float(x))
  }

  fn visit_str<E: de::Error>(self, text: &str) -> Result<Value, E> {
    Ok(Value::Text(text.to_owned()))
  }

  fn visit_string<E: de::Error>(self, text: String) -> Result<Value, E> {
    Ok(Value::Text(text))
  }

  fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
    let seed = self.0.nested()?;
    let mut items = Vec::new();
    while let Some(item) = seq.next_element_seed(seed)? {
      items.push(item);
    }
    Ok(Value::Array(items))
  }

  fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
    let seed = self.0.nested()?;
    let mut keys = HashSet::new();
    let mut entries = Vec::new();
    while let Some(key) = map.next_key::<String>()? {
      if !keys.insert(key.clone()) {
        return Err(de::Error::custom(DUPLICATE_KEY));
      }
      let value = map.next_value_seed(seed)?;
      entries.push((Value::Text(key), value));
    }
    Ok(Value::Map(entries))
  }
}

fn write(value: &Value, out: &mut impl Text) -> Result<(), EncodeError> {
  match value {
    Value::Null => out.put(b"null"),
    Value::Bool(b) => out.put(if *b { b"true" } else { b"false" }),
    Value::Unsigned(n) => out.put(n.to_string().as_bytes()),
    Value::Negative(n) => out.put(format!("-{}", u128::from(*n) + 1).as_bytes()),
    Value::Float(x) if x.is_finite() => out.put(ryu_js::Buffer::new().format_finite(*x).as_bytes()),
    Value::Text(text) => write_string(text, out),
    Value::Array(items) => {
      out.put(b"[");
      for (index, item) in items.iter().enumerate() {
        if index > 0 {
          out.put(b",");
        }
        write(item, out)?;
      }
      out.put(b"]");
    }
    Value::Map(entries) => {
      let mut keys = HashSet::new();
      out.put(b"{");
      for (index, (key, value)) in entries.iter().enumerate() {
        let Value::Text(key) = key else {
          return Err(EncodeError::NotJson);
        };
        if !keys.insert(key.as_str()) {
          return Err(EncodeError::NotJson);
        }
        if index > 0 {
          out.put(b",");
        }
        write_string(key, out);
        out.put(b":");
        write(value, out)?;
      }
      out.put(b"}");
    }
    Value::Float(_) | Value::Undefined | Value::Bytes(_) | Value::Tag(..) | Value::Simple(_) => {
      return Err(EncodeError::NotJson);
    }
  }
  Ok(())
}

fn write_string(text: &str, out: &mut impl Text) {
  out.put(b"\"");
  for c in text.chars() {
    match c {
      '"' => out.put(b"\\\""),
      '\\' => out.put(b"\\\\"),
      '\u{08}' => out.put(b"\\b"),
      '\u{09}' => out.put(b"\\t"),
      '\u{0a}' => out.put(b"\\n"),
      '\u{0c}' => out.put(b"\\f"),
      '\u{0d}' => out.put(b"\\r"),
      '\u{00}'..='\u{1f}' => out.put(format!("\\u{:04x}", u32::from(c)).as_bytes()),
      _ => {
        let mut buffer = [0; 4];
        out.put(c.encode_utf8(&mut buffer).as_bytes());
      }
    }
  }
  out.put(b"\"");
}

#[cfg(test)]
mod tests {
  use serde::de::{Error as _, Visitor};

  use super::{JsonVisitor, Seed, Value};

  const SEED: Seed = Seed {
    parent: 0,
    max_depth: 16,
  };

  #[test]
  fn handles_the_calls_serde_json_does_not_make() {
    assert_eq!(
      JsonVisitor(SEED).visit_i64::<serde_json::Error>(5).ok(),
      Some(Value::Unsigned(5))
    );
    assert_eq!(
      JsonVisitor(SEED).visit_string::<serde_json::Error>("a".to_owned()).ok(),
      Some(Value::Text("a".to_owned()))
    );
    let error = JsonVisitor(SEED).visit_bytes::<serde_json::Error>(b"a").unwrap_err();
    assert_eq!(
      error.to_string(),
      serde_json::Error::invalid_type(serde::de::Unexpected::Bytes(b"a"), &JsonVisitor(SEED)).to_string()
    );
    assert!(error.to_string().contains("a JSON value"));
  }
}
