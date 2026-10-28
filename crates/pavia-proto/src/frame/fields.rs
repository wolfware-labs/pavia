//! Reading typed fields out of a decoded header map, and writing them back.
//!
//! Readers return `None` for a value of the wrong CBOR type; [`Fields`] turns that into a
//! [`Bad`] that names the key. A nested map with a bad entry is a bad value of its outer key.

use bytes::Bytes;

use super::{AckMap, Auth, ClientInfo, Limits, Meta, MetaName, MetaValue, Resume, ServerLimits, Since, Token, key};
use crate::wire::header::{Measure, Sink, Value};

/// What is wrong with a header field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
  /// A field the frame must carry is absent.
  Missing,
  /// The field is present but has the wrong CBOR type or shape.
  WrongType,
  /// The field is present but the frame's form forbids it.
  Unexpected,
  /// The field has the right type but a value the spec forbids.
  OutOfRange,
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
      let Value::Text(name) = name else {
        return None;
      };
      Some((MetaName::try_from(*name).ok()?, value))
    })
    .collect()
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

/// A field value that writes itself as one CBOR item.
pub trait Encode {
  fn encode(&self, sink: &mut impl Sink);
}

/// A map whose entries are written in ascending key order, which for unsigned keys is the
/// deterministic order of §5.4.1.
pub trait MapBody {
  fn entries<S: Sink>(&self, entries: &mut Entries<'_, S>);
}

/// Receives the entries of a [`MapBody`]: counts them, or writes them.
pub struct Entries<'s, S: Sink> {
  sink: Option<&'s mut S>,
  count: u64,
  last: Option<u64>,
}

impl<S: Sink> Entries<'_, S> {
  pub fn put<T: Encode + ?Sized>(&mut self, key: u64, value: &T) {
    debug_assert!(self.last.is_none_or(|last| last < key), "key {key} out of order");
    self.last = Some(key);
    self.count += 1;
    if let Some(sink) = self.sink.as_deref_mut() {
      sink.uint(key);
      value.encode(sink);
    }
  }

  pub fn put_some<T: Encode + ?Sized>(&mut self, key: u64, value: Option<&T>) {
    if let Some(value) = value {
      self.put(key, value);
    }
  }
}

/// How many entries `body` writes.
pub fn entry_count(body: &impl MapBody) -> u64 {
  let mut entries = Entries::<Measure> {
    sink: None,
    count: 0,
    last: None,
  };
  body.entries(&mut entries);
  entries.count
}

/// Writes `body` as a map: its size, then its entries.
pub fn encode_map(body: &impl MapBody, sink: &mut impl Sink) {
  sink.map(entry_count(body));
  body.entries(&mut Entries {
    sink: Some(sink),
    count: 0,
    last: None,
  });
}

impl Encode for u64 {
  fn encode(&self, sink: &mut impl Sink) {
    sink.uint(*self);
  }
}

impl Encode for bool {
  fn encode(&self, sink: &mut impl Sink) {
    sink.bool(*self);
  }
}

impl Encode for str {
  fn encode(&self, sink: &mut impl Sink) {
    sink.text(self);
  }
}

impl Encode for String {
  fn encode(&self, sink: &mut impl Sink) {
    sink.text(self);
  }
}

impl Encode for [u8; 32] {
  fn encode(&self, sink: &mut impl Sink) {
    sink.bytes(self);
  }
}

impl<T: Encode> Encode for [T] {
  fn encode(&self, sink: &mut impl Sink) {
    sink.array(self.len() as u64);
    for item in self {
      item.encode(sink);
    }
  }
}

impl<T: Encode> Encode for Vec<T> {
  fn encode(&self, sink: &mut impl Sink) {
    self.as_slice().encode(sink);
  }
}

impl Encode for Token {
  fn encode(&self, sink: &mut impl Sink) {
    match self {
      Token::Bytes(bytes) => sink.bytes(bytes),
      Token::Text(text) => sink.text(text),
    }
  }
}

impl Encode for AckMap {
  fn encode(&self, sink: &mut impl Sink) {
    sink.map(self.len() as u64);
    for (lane, seq) in self {
      sink.uint(*lane);
      sink.uint(*seq);
    }
  }
}

impl Encode for Meta {
  fn encode(&self, sink: &mut impl Sink) {
    sink.map(self.len() as u64);
    for (name, value) in self {
      sink.text(name.as_str());
      match value {
        MetaValue::Text(text) => sink.text(text),
        MetaValue::Bytes(bytes) => sink.bytes(bytes),
      }
    }
  }
}

impl MapBody for Limits {
  fn entries<S: Sink>(&self, entries: &mut Entries<'_, S>) {
    entries.put_some(key::WINDOW, self.window.as_ref());
    entries.put_some(key::MAX_CALLS, self.max_calls.as_ref());
    entries.put_some(key::MAX_FRAME, self.max_frame.as_ref());
    entries.put_some(key::MAX_SUBS, self.max_subs.as_ref());
    entries.put_some(key::MAX_DGRAM, self.max_dgram.as_ref());
  }
}

impl MapBody for ServerLimits {
  fn entries<S: Sink>(&self, entries: &mut Entries<'_, S>) {
    entries.put_some(key::WINDOW, self.window.as_ref());
    entries.put(key::MAX_CALLS, &self.max_calls);
    entries.put(key::MAX_FRAME, &self.max_frame);
    entries.put(key::MAX_SUBS, &self.max_subs);
    entries.put(key::MAX_DGRAM, &self.max_dgram);
  }
}

impl MapBody for Auth {
  fn entries<S: Sink>(&self, entries: &mut Entries<'_, S>) {
    entries.put(key::TOKEN, &self.token);
    entries.put(key::SCHEME, &self.scheme);
  }
}

impl MapBody for Resume {
  fn entries<S: Sink>(&self, entries: &mut Entries<'_, S>) {
    entries.put(key::TOKEN, &self.token);
    entries.put(key::SESSION, &self.session);
    entries.put(key::ACK, &self.ack);
    entries.put_some(key::LOST, self.lost.as_ref());
  }
}

impl MapBody for ClientInfo {
  fn entries<S: Sink>(&self, entries: &mut Entries<'_, S>) {
    entries.put_some(key::NAME, self.name.as_ref());
    entries.put_some(key::CLIENT_VERSION, self.version.as_ref());
  }
}

impl MapBody for Since {
  fn entries<S: Sink>(&self, entries: &mut Entries<'_, S>) {
    entries.put(key::EPOCH, &self.epoch);
    entries.put(key::OFFSET, &self.offset);
  }
}

macro_rules! encode_as_map {
  ($($type:ty),*) => {
    $(impl Encode for $type {
      fn encode(&self, sink: &mut impl Sink) {
        encode_map(self, sink);
      }
    })*
  };
}

encode_as_map!(Limits, ServerLimits, Auth, Resume, ClientInfo, Since);
