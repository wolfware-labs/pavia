use bytes::BufMut;
use minicbor::Decoder;
use minicbor::data::Type;

use super::{DecodeError, EncodeError, MAX_DEPTH, Value};
use crate::wire::header::{Out, Sink};

const NEGATIVE: u8 = 1;
const TAG: u8 = 6;
const SIMPLE: u8 = 7;
const BREAK: u8 = 0xff;

pub(super) fn decode(data: &[u8]) -> Result<Value, DecodeError> {
  let mut decoder = Decoder::new(data);
  let value = item(&mut decoder, 0)?;
  if decoder.position() < data.len() {
    return Err(DecodeError::TrailingBytes);
  }
  Ok(value)
}

pub(super) fn encode(value: &Value, out: &mut impl BufMut) -> Result<(), EncodeError> {
  check(value)?;
  write(value, &mut Out(out));
  Ok(())
}

fn nested(parent: usize) -> Result<usize, DecodeError> {
  let depth = parent + 1;
  if depth > MAX_DEPTH {
    Err(DecodeError::TooDeep)
  } else {
    Ok(depth)
  }
}

fn item(decoder: &mut Decoder<'_>, parent: usize) -> Result<Value, DecodeError> {
  let malformed = |_| DecodeError::Malformed;
  match decoder.datatype().map_err(malformed)? {
    Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::Int => {
      let integer = i128::from(decoder.int().map_err(malformed)?);
      match u64::try_from(integer) {
        Ok(n) => Ok(Value::Unsigned(n)),
        Err(_) => u64::try_from(-1 - integer)
          .map(Value::Negative)
          .map_err(|_| DecodeError::Malformed),
      }
    }
    Type::F16 => Ok(Value::Float(f64::from(decoder.f16().map_err(malformed)?))),
    Type::F32 => Ok(Value::Float(f64::from(decoder.f32().map_err(malformed)?))),
    Type::F64 => Ok(Value::Float(decoder.f64().map_err(malformed)?)),
    Type::Bool => Ok(Value::Bool(decoder.bool().map_err(malformed)?)),
    Type::Null => decoder.null().map(|()| Value::Null).map_err(malformed),
    Type::Undefined => decoder.undefined().map(|()| Value::Undefined).map_err(malformed),
    Type::Simple => simple(decoder),
    Type::Bytes => Ok(Value::Bytes(decoder.bytes().map_err(malformed)?.to_vec())),
    Type::BytesIndef => {
      let mut bytes = Vec::new();
      for chunk in decoder.bytes_iter().map_err(malformed)? {
        bytes.extend_from_slice(chunk.map_err(malformed)?);
      }
      Ok(Value::Bytes(bytes))
    }
    Type::String => Ok(Value::Text(decoder.str().map_err(malformed)?.to_owned())),
    Type::StringIndef => {
      let mut text = String::new();
      for chunk in decoder.str_iter().map_err(malformed)? {
        text.push_str(chunk.map_err(malformed)?);
      }
      Ok(Value::Text(text))
    }
    Type::Array | Type::ArrayIndef => {
      let depth = nested(parent)?;
      let length = decoder.array().map_err(malformed)?;
      let mut items = Vec::with_capacity(capacity_hint(decoder, length));
      while more(decoder, length, items.len())? {
        items.push(item(decoder, depth)?);
      }
      Ok(Value::Array(items))
    }
    Type::Map | Type::MapIndef => {
      let depth = nested(parent)?;
      let length = decoder.map().map_err(malformed)?;
      let mut entries = Vec::with_capacity(capacity_hint(decoder, length));
      while more(decoder, length, entries.len())? {
        let key = item(decoder, depth)?;
        let value = item(decoder, depth)?;
        entries.push((key, value));
      }
      Ok(Value::Map(entries))
    }
    Type::Tag => {
      let depth = nested(parent)?;
      let tag = decoder.tag().map_err(malformed)?;
      Ok(Value::Tag(tag.as_u64(), Box::new(item(decoder, depth)?)))
    }
    Type::Break | Type::Unknown(_) => Err(DecodeError::Malformed),
  }
}

fn simple(decoder: &mut Decoder<'_>) -> Result<Value, DecodeError> {
  if matches!(
    decoder.input().get(decoder.position()..),
    Some([0xf8, 0x00..0x20, ..] | [0xf8])
  ) {
    return Err(DecodeError::Malformed);
  }
  decoder.simple().map(Value::Simple).map_err(|_| DecodeError::Malformed)
}

fn more(decoder: &mut Decoder<'_>, length: Option<u64>, read: usize) -> Result<bool, DecodeError> {
  match length {
    Some(length) => Ok((read as u64) < length),
    None => match decoder.input().get(decoder.position()) {
      Some(&BREAK) => {
        decoder.set_position(decoder.position() + 1);
        Ok(false)
      }
      Some(_) => Ok(true),
      None => Err(DecodeError::Malformed),
    },
  }
}

fn capacity_hint(decoder: &Decoder<'_>, length: Option<u64>) -> usize {
  let remaining = decoder.input().len() - decoder.position();
  length.map_or(0, |length| {
    usize::try_from(length).map_or(remaining, |length| length.min(remaining))
  })
}

fn check(value: &Value) -> Result<(), EncodeError> {
  match value {
    Value::Simple(20..=31) => Err(EncodeError::ReservedSimpleValue),
    Value::Array(items) => items.iter().try_for_each(check),
    Value::Map(entries) => entries.iter().try_for_each(|(key, value)| {
      check(key)?;
      check(value)
    }),
    Value::Tag(_, item) => check(item),
    _ => Ok(()),
  }
}

fn write(value: &Value, sink: &mut impl Sink) {
  match value {
    Value::Null => sink.put(&[0xf6]),
    Value::Undefined => sink.put(&[0xf7]),
    Value::Bool(b) => sink.bool(*b),
    Value::Unsigned(n) => sink.uint(*n),
    Value::Negative(n) => sink.head(NEGATIVE, *n),
    Value::Float(x) => sink.float(*x),
    Value::Text(text) => sink.text(text),
    Value::Bytes(bytes) => sink.bytes(bytes),
    Value::Array(items) => {
      sink.array(items.len() as u64);
      for item in items {
        write(item, sink);
      }
    }
    Value::Map(entries) => {
      sink.map(entries.len() as u64);
      for (key, value) in entries {
        write(key, sink);
        write(value, sink);
      }
    }
    Value::Tag(tag, item) => {
      sink.head(TAG, *tag);
      write(item, sink);
    }
    Value::Simple(n) => sink.head(SIMPLE, u64::from(*n)),
  }
}
