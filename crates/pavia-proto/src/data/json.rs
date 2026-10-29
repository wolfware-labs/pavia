use std::collections::HashSet;
use std::fmt;

use bytes::BufMut;
use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};

use super::{DecodeError, EncodeError, MAX_DEPTH, Value};

const DUPLICATE_KEY: &str = "duplicate object key";

pub(super) fn decode(data: &[u8]) -> Result<Value, DecodeError> {
  let text = std::str::from_utf8(data).map_err(|_| DecodeError::InvalidUtf8)?;
  let mut deserializer = serde_json::Deserializer::from_str(text);
  let Json(value) = Json::deserialize(&mut deserializer).map_err(|error| {
    if error.to_string().starts_with(DUPLICATE_KEY) {
      DecodeError::DuplicateKey
    } else {
      DecodeError::Malformed
    }
  })?;
  deserializer.end().map_err(|_| DecodeError::TrailingBytes)?;
  if depth(&value) > MAX_DEPTH {
    return Err(DecodeError::TooDeep);
  }
  Ok(value)
}

pub(super) fn encode(value: &Value, out: &mut impl BufMut) -> Result<(), EncodeError> {
  let mut text = Vec::new();
  write(value, &mut text)?;
  out.put_slice(&text);
  Ok(())
}

fn depth(value: &Value) -> usize {
  match value {
    Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
    Value::Map(entries) => 1 + entries.iter().map(|(_, value)| depth(value)).max().unwrap_or(0),
    _ => 0,
  }
}

struct Json(Value);

impl<'de> Deserialize<'de> for Json {
  fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    deserializer.deserialize_any(JsonVisitor).map(Json)
  }
}

struct JsonVisitor;

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
    let mut items = Vec::new();
    while let Some(Json(item)) = seq.next_element()? {
      items.push(item);
    }
    Ok(Value::Array(items))
  }

  fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
    let mut keys = HashSet::new();
    let mut entries = Vec::new();
    while let Some(key) = map.next_key::<String>()? {
      if !keys.insert(key.clone()) {
        return Err(de::Error::custom(DUPLICATE_KEY));
      }
      let Json(value) = map.next_value()?;
      entries.push((Value::Text(key), value));
    }
    Ok(Value::Map(entries))
  }
}

fn write(value: &Value, out: &mut Vec<u8>) -> Result<(), EncodeError> {
  match value {
    Value::Null => out.extend_from_slice(b"null"),
    Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
    Value::Unsigned(n) => out.extend_from_slice(n.to_string().as_bytes()),
    Value::Negative(n) => out.extend_from_slice(format!("-{}", u128::from(*n) + 1).as_bytes()),
    Value::Float(x) if x.is_finite() => out.extend_from_slice(ryu_js::Buffer::new().format_finite(*x).as_bytes()),
    Value::Text(text) => write_string(text, out),
    Value::Array(items) => {
      out.push(b'[');
      for (index, item) in items.iter().enumerate() {
        if index > 0 {
          out.push(b',');
        }
        write(item, out)?;
      }
      out.push(b']');
    }
    Value::Map(entries) => {
      out.push(b'{');
      for (index, (key, value)) in entries.iter().enumerate() {
        let Value::Text(key) = key else {
          return Err(EncodeError::NotJson);
        };
        if index > 0 {
          out.push(b',');
        }
        write_string(key, out);
        out.push(b':');
        write(value, out)?;
      }
      out.push(b'}');
    }
    Value::Float(_) | Value::Undefined | Value::Bytes(_) | Value::Tag(..) | Value::Simple(_) => {
      return Err(EncodeError::NotJson);
    }
  }
  Ok(())
}

fn write_string(text: &str, out: &mut Vec<u8>) {
  out.push(b'"');
  for c in text.chars() {
    match c {
      '"' => out.extend_from_slice(b"\\\""),
      '\\' => out.extend_from_slice(b"\\\\"),
      '\u{08}' => out.extend_from_slice(b"\\b"),
      '\u{09}' => out.extend_from_slice(b"\\t"),
      '\u{0a}' => out.extend_from_slice(b"\\n"),
      '\u{0c}' => out.extend_from_slice(b"\\f"),
      '\u{0d}' => out.extend_from_slice(b"\\r"),
      '\u{00}'..='\u{1f}' => out.extend_from_slice(format!("\\u{:04x}", u32::from(c)).as_bytes()),
      _ => {
        let mut buffer = [0; 4];
        out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
      }
    }
  }
  out.push(b'"');
}
