//! A client-side codec for `WsFrame`s (§5.2, §5.4), written apart from `pavia-proto` so the
//! runner checks the server against an independent reading of the layout.

use std::fmt;

use ciborium::Value;

const MAX_HEADER_SIZE: usize = 16_384;
const MAX_HEADER_DEPTH: usize = 16;

/// One frame with its stream ID, as the runner sends or receives it.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
  pub stream: u64,
  pub frame_type: u8,
  pub flags: u8,
  pub header: Option<Value>,
  pub data: Vec<u8>,
}

impl fmt::Display for Frame {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "stream {} type {} flags {}", self.stream, self.frame_type, self.flags)?;
    match &self.header {
      Some(header) => write!(f, " header {header:?}")?,
      None => write!(f, " no header")?,
    }
    if self.data.is_empty() {
      write!(f, " no data")
    } else {
      write!(f, " data {}", hex(&self.data))
    }
  }
}

pub fn hex(bytes: &[u8]) -> String {
  bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn encode(frame: &Frame) -> Result<Vec<u8>, String> {
  let header = frame.header.as_ref().map(cbor).unwrap_or_default();
  let mut payload = Vec::new();
  put_varint(header.len() as u64, &mut payload)?;
  payload.extend_from_slice(&header);
  payload.extend_from_slice(&frame.data);

  let mut out = Vec::new();
  put_varint(frame.stream, &mut out)?;
  out.push(frame.frame_type);
  out.push(frame.flags);
  put_varint(payload.len() as u64, &mut out)?;
  out.extend_from_slice(&payload);
  Ok(out)
}

pub fn decode(mut message: &[u8]) -> Result<Vec<Frame>, String> {
  if message.is_empty() {
    return Err("an empty message holds no frame".into());
  }
  let mut frames = Vec::new();
  while !message.is_empty() {
    let stream = take_varint(&mut message)?;
    let [frame_type, flags, rest @ ..] = message else {
      return Err("message ends inside a frame prefix".into());
    };
    message = rest;
    let length = usize::try_from(take_varint(&mut message)?).map_err(|_| "frame length too large")?;
    let Some((mut payload, rest)) = message.split_at_checked(length) else {
      return Err(format!("frame length {length} runs past the message"));
    };
    message = rest;
    let header_length = usize::try_from(take_varint(&mut payload)?).map_err(|_| "header length too large")?;
    let Some((header, data)) = payload.split_at_checked(header_length) else {
      return Err(format!("header length {header_length} runs past the payload"));
    };
    let header = if header.is_empty() {
      None
    } else {
      Some(strict_header(header)?)
    };
    frames.push(Frame {
      stream,
      frame_type: *frame_type,
      flags: *flags,
      header,
      data: data.to_vec(),
    });
  }
  Ok(frames)
}

/// Decodes a header the way a strict receiver reads what a sender MUST write (§5.4.1): at most
/// 16 KiB and depth 16, one map with unsigned integer keys, no tags, no duplicate keys, and
/// exactly the bytes of its core deterministic encoding, which rules out indefinite lengths,
/// longer integer and float forms, and keys out of order.
fn strict_header(bytes: &[u8]) -> Result<Value, String> {
  if bytes.len() > MAX_HEADER_SIZE {
    return Err(format!("header of {} bytes is above {MAX_HEADER_SIZE}", bytes.len()));
  }
  let value =
    ciborium::de::from_reader_with_recursion_limit::<Value, _>(bytes, MAX_HEADER_DEPTH).map_err(|error| match error {
      ciborium::de::Error::RecursionLimitExceeded => format!("header nests deeper than {MAX_HEADER_DEPTH}"),
      error => format!("header is not CBOR: {error}"),
    })?;
  let Value::Map(entries) = &value else {
    return Err(format!("header is not a map: {value:?}"));
  };
  if let Some((key, _)) = entries
    .iter()
    .find(|(key, _)| !matches!(key, Value::Integer(integer) if i128::from(*integer) >= 0))
  {
    return Err(format!("header key {key:?} is not an unsigned integer"));
  }
  check_header_value(&value)?;
  if cbor(&deterministic(&value)) != bytes {
    return Err(format!("header is not in deterministic encoding: {}", hex(bytes)));
  }
  Ok(value)
}

fn check_header_value(value: &Value) -> Result<(), String> {
  match value {
    Value::Tag(..) => Err(format!("header holds a tag: {value:?}")),
    Value::Array(items) => items.iter().try_for_each(check_header_value),
    Value::Map(entries) => {
      for (index, (key, item)) in entries.iter().enumerate() {
        if entries[..index].iter().any(|(earlier, _)| earlier == key) {
          return Err(format!("header repeats the key {key:?}"));
        }
        check_header_value(key)?;
        check_header_value(item)?;
      }
      Ok(())
    }
    _ => Ok(()),
  }
}

fn deterministic(value: &Value) -> Value {
  match value {
    Value::Array(items) => Value::Array(items.iter().map(deterministic).collect()),
    Value::Map(entries) => {
      let mut entries: Vec<(Value, Value)> = entries
        .iter()
        .map(|(key, item)| (deterministic(key), deterministic(item)))
        .collect();
      entries.sort_by_cached_key(|(key, _)| cbor(key));
      Value::Map(entries)
    }
    other => other.clone(),
  }
}

fn cbor(value: &Value) -> Vec<u8> {
  let mut bytes = Vec::new();
  ciborium::into_writer(value, &mut bytes).expect("a CBOR value always encodes");
  bytes
}

fn put_varint(value: u64, out: &mut Vec<u8>) -> Result<(), String> {
  match value {
    0..64 => out.push(value as u8),
    64..16_384 => out.extend_from_slice(&(value as u16 | 0x4000).to_be_bytes()),
    16_384..1_073_741_824 => out.extend_from_slice(&(value as u32 | 0x8000_0000).to_be_bytes()),
    1_073_741_824..0x4000_0000_0000_0000 => out.extend_from_slice(&(value | 0xc000_0000_0000_0000).to_be_bytes()),
    _ => return Err(format!("{value} is above 2^62 - 1")),
  }
  Ok(())
}

fn take_varint(input: &mut &[u8]) -> Result<u64, String> {
  let Some(first) = input.first() else {
    return Err("message ends inside a varint".into());
  };
  let first = *first;
  let Some((bytes, rest)) = input.split_at_checked(1 << (first >> 6)) else {
    return Err("message ends inside a varint".into());
  };
  *input = rest;
  Ok(
    bytes[1..]
      .iter()
      .fold(u64::from(first & 0x3f), |value, byte| (value << 8) | u64::from(*byte)),
  )
}
