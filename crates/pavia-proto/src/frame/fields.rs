//! Reading typed fields out of a decoded header map, and writing them back.
//!
//! Readers return `None` for a value of the wrong CBOR type; [`Fields`] turns that into a
//! [`Bad`] that names the key. A nested map with a bad entry is a bad value of its outer key.

use std::collections::BTreeMap;

use bytes::Bytes;

use super::{AckMap, Auth, ClientInfo, Limits, Meta, MetaValue, Resume, ServerLimits, Since, Token, key};
use crate::wire::header::{self, Value};

/// What is wrong with a header field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
  /// A field the frame must carry is absent.
  Missing,
  /// The field is present but has the wrong CBOR type or shape.
  WrongType,
  /// The field is present but the frame's form forbids it.
  Unexpected,
}

/// A field problem at `key`, before the frame type and `rid` are attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bad {
  pub key: u64,
  pub problem: Problem,
}

/// The entries of one header map, looked up by integer key. Keys the frame does not read are
/// ignored (§5.4.1).
pub struct Fields<'h, 'a> {
  entries: &'h [(Value<'a>, Value<'a>)],
}

impl<'h, 'a> Fields<'h, 'a> {
  /// `None` when a key is not an unsigned integer, which only `meta` allows (§5.4.1).
  pub fn new(entries: &'h [(Value<'a>, Value<'a>)]) -> Option<Self> {
    entries
      .iter()
      .all(|(key, _)| matches!(key, Value::Unsigned(_)))
      .then_some(Self { entries })
  }

  fn get(&self, key: u64) -> Option<&'h Value<'a>> {
    self
      .entries
      .iter()
      .find_map(|(entry_key, value)| matches!(entry_key, Value::Unsigned(n) if *n == key).then_some(value))
  }

  pub fn optional<T>(&self, key: u64, read: impl Fn(&Value<'a>) -> Option<T>) -> Result<Option<T>, Bad> {
    self
      .get(key)
      .map(|value| {
        read(value).ok_or(Bad {
          key,
          problem: Problem::WrongType,
        })
      })
      .transpose()
  }

  pub fn required<T>(&self, key: u64, read: impl Fn(&Value<'a>) -> Option<T>) -> Result<T, Bad> {
    self.optional(key, read)?.ok_or(Bad {
      key,
      problem: Problem::Missing,
    })
  }
}

pub fn uint(value: &Value<'_>) -> Option<u64> {
  match value {
    Value::Unsigned(n) => Some(*n),
    _ => None,
  }
}

pub fn text(value: &Value<'_>) -> Option<String> {
  match value {
    Value::Text(text) => Some((*text).to_owned()),
    _ => None,
  }
}

pub fn boolean(value: &Value<'_>) -> Option<bool> {
  match value {
    Value::Bool(b) => Some(*b),
    _ => None,
  }
}

pub fn token(value: &Value<'_>) -> Option<Token> {
  match value {
    Value::Bytes(bytes) => Some(Token::Bytes(Bytes::copy_from_slice(bytes))),
    Value::Text(text) => Some(Token::Text((*text).to_owned())),
    _ => None,
  }
}

pub fn fingerprint(value: &Value<'_>) -> Option<[u8; 32]> {
  match value {
    Value::Bytes(bytes) => (*bytes).try_into().ok(),
    _ => None,
  }
}

fn array<T>(value: &Value<'_>, read: impl Fn(&Value<'_>) -> Option<T>) -> Option<Vec<T>> {
  match value {
    Value::Array(items) => items.iter().map(read).collect(),
    _ => None,
  }
}

pub fn uint_array(value: &Value<'_>) -> Option<Vec<u64>> {
  array(value, uint)
}

pub fn text_array(value: &Value<'_>) -> Option<Vec<String>> {
  array(value, text)
}

fn map<'h, 'a>(value: &'h Value<'a>) -> Option<&'h [(Value<'a>, Value<'a>)]> {
  match value {
    Value::Map(entries) => Some(entries),
    _ => None,
  }
}

pub fn ack_map(value: &Value<'_>) -> Option<AckMap> {
  map(value)?
    .iter()
    .map(|(lane, seq)| Some((uint(lane)?, uint(seq)?)))
    .collect()
}

pub fn meta(value: &Value<'_>) -> Option<Meta> {
  map(value)?
    .iter()
    .map(|(name, value)| {
      let value = match value {
        Value::Text(text) => MetaValue::Text((*text).to_owned()),
        Value::Bytes(bytes) => MetaValue::Bytes(Bytes::copy_from_slice(bytes)),
        _ => return None,
      };
      Some((meta_name(name)?, value))
    })
    .collect()
}

/// A metadata name: non-empty lowercase ASCII `[a-z0-9-]` (§8.7).
fn meta_name(value: &Value<'_>) -> Option<String> {
  text(value).filter(|name| {
    !name.is_empty()
      && name
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
  })
}

pub fn limits(value: &Value<'_>) -> Option<Limits> {
  let fields = Fields::new(map(value)?)?;
  Some(Limits {
    max_calls: fields.optional(key::MAX_CALLS, uint).ok()?,
    max_frame: fields.optional(key::MAX_FRAME, uint).ok()?,
    max_subs: fields.optional(key::MAX_SUBS, uint).ok()?,
    max_dgram: fields.optional(key::MAX_DGRAM, uint).ok()?,
    window: fields.optional(key::WINDOW, uint).ok()?,
  })
}

pub fn server_limits(value: &Value<'_>) -> Option<ServerLimits> {
  let fields = Fields::new(map(value)?)?;
  Some(ServerLimits {
    max_calls: fields.required(key::MAX_CALLS, uint).ok()?,
    max_frame: fields.required(key::MAX_FRAME, uint).ok()?,
    max_subs: fields.required(key::MAX_SUBS, uint).ok()?,
    max_dgram: fields.required(key::MAX_DGRAM, uint).ok()?,
    window: fields.optional(key::WINDOW, uint).ok()?,
  })
}

pub fn auth(value: &Value<'_>) -> Option<Auth> {
  let fields = Fields::new(map(value)?)?;
  Some(Auth {
    scheme: fields.required(key::SCHEME, text).ok()?,
    token: fields.required(key::TOKEN, token).ok()?,
  })
}

pub fn resume(value: &Value<'_>) -> Option<Resume> {
  let fields = Fields::new(map(value)?)?;
  Some(Resume {
    token: fields.required(key::TOKEN, token).ok()?,
    session: fields.required(key::SESSION, text).ok()?,
    ack: fields.required(key::ACK, ack_map).ok()?,
    lost: fields.optional(key::LOST, uint_array).ok()?,
  })
}

pub fn client(value: &Value<'_>) -> Option<ClientInfo> {
  let fields = Fields::new(map(value)?)?;
  Some(ClientInfo {
    name: fields.optional(key::NAME, text).ok()?,
    version: fields.optional(key::CLIENT_VERSION, text).ok()?,
  })
}

pub fn since(value: &Value<'_>) -> Option<Since> {
  let fields = Fields::new(map(value)?)?;
  Some(Since {
    epoch: fields.required(key::EPOCH, text).ok()?,
    offset: fields.required(key::OFFSET, uint).ok()?,
  })
}

/// Collects header entries and writes them as a header section.
#[derive(Default)]
pub struct Writer<'a> {
  entries: Vec<(Value<'a>, Value<'a>)>,
}

impl<'a> Writer<'a> {
  pub fn put(&mut self, key: u64, value: Value<'a>) {
    self.entries.push((Value::Unsigned(key), value));
  }

  pub fn put_some<T>(&mut self, key: u64, value: Option<T>, write: impl Fn(T) -> Value<'a>) {
    if let Some(value) = value {
      self.put(key, write(value));
    }
  }

  /// The header bytes: nothing for an empty header, which is HeaderLength 0 (§5.2).
  pub fn finish(self) -> Bytes {
    if self.entries.is_empty() {
      return Bytes::new();
    }
    let mut out = Vec::new();
    header::encode(&self.entries, &mut out).expect("typed headers have unique keys and no simple values");
    Bytes::from(out)
  }
}

pub fn uint_value<'a>(n: &u64) -> Value<'a> {
  Value::Unsigned(*n)
}

pub fn text_value(text: &str) -> Value<'_> {
  Value::Text(text)
}

pub fn bool_value<'a>(b: &bool) -> Value<'a> {
  Value::Bool(*b)
}

pub fn token_value(token: &Token) -> Value<'_> {
  match token {
    Token::Bytes(bytes) => Value::Bytes(bytes),
    Token::Text(text) => Value::Text(text),
  }
}

pub fn fingerprint_value(fingerprint: &[u8; 32]) -> Value<'_> {
  Value::Bytes(fingerprint)
}

pub fn uint_array_value<'a>(items: &[u64]) -> Value<'a> {
  Value::Array(items.iter().map(uint_value).collect())
}

pub fn text_array_value(items: &[String]) -> Value<'_> {
  Value::Array(items.iter().map(|item| Value::Text(item)).collect())
}

pub fn ack_map_value<'a>(ack: &AckMap) -> Value<'a> {
  Value::Map(
    ack
      .iter()
      .map(|(lane, seq)| (Value::Unsigned(*lane), Value::Unsigned(*seq)))
      .collect(),
  )
}

pub fn meta_value(meta: &Meta) -> Value<'_> {
  Value::Map(
    meta
      .iter()
      .map(|(name, value)| {
        let value = match value {
          MetaValue::Text(text) => Value::Text(text),
          MetaValue::Bytes(bytes) => Value::Bytes(bytes),
        };
        (Value::Text(name), value)
      })
      .collect(),
  )
}

fn nested<'a>(write: impl FnOnce(&mut Writer<'a>)) -> Value<'a> {
  let mut writer = Writer::default();
  write(&mut writer);
  Value::Map(writer.entries)
}

pub fn limits_value<'a>(limits: &Limits) -> Value<'a> {
  let entries = [
    (key::MAX_CALLS, limits.max_calls),
    (key::MAX_FRAME, limits.max_frame),
    (key::MAX_SUBS, limits.max_subs),
    (key::MAX_DGRAM, limits.max_dgram),
    (key::WINDOW, limits.window),
  ];
  let map: BTreeMap<u64, u64> = entries.into_iter().filter_map(|(key, value)| Some((key, value?))).collect();
  ack_map_value(&map)
}

pub fn server_limits_value<'a>(limits: &ServerLimits) -> Value<'a> {
  limits_value(&Limits {
    max_calls: Some(limits.max_calls),
    max_frame: Some(limits.max_frame),
    max_subs: Some(limits.max_subs),
    max_dgram: Some(limits.max_dgram),
    window: limits.window,
  })
}

pub fn auth_value(auth: &Auth) -> Value<'_> {
  nested(|writer| {
    writer.put(key::SCHEME, Value::Text(&auth.scheme));
    writer.put(key::TOKEN, token_value(&auth.token));
  })
}

pub fn resume_value(resume: &Resume) -> Value<'_> {
  nested(|writer| {
    writer.put(key::TOKEN, token_value(&resume.token));
    writer.put(key::SESSION, Value::Text(&resume.session));
    writer.put(key::ACK, ack_map_value(&resume.ack));
    writer.put_some(key::LOST, resume.lost.as_deref(), uint_array_value);
  })
}

pub fn client_value(client: &ClientInfo) -> Value<'_> {
  nested(|writer| {
    writer.put_some(key::NAME, client.name.as_deref(), text_value);
    writer.put_some(key::CLIENT_VERSION, client.version.as_deref(), text_value);
  })
}

pub fn since_value(since: &Since) -> Value<'_> {
  nested(|writer| {
    writer.put(key::EPOCH, Value::Text(&since.epoch));
    writer.put(key::OFFSET, Value::Unsigned(since.offset));
  })
}
