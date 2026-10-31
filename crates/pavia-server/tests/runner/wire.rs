//! A client-side codec for `WsFrame`s (§5.2, §5.4), written apart from `pavia-proto` so the
//! runner checks the server against an independent reading of the layout.

use std::fmt;

use ciborium::Value;

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

pub fn encode(frame: &Frame) -> Vec<u8> {
  let mut header = Vec::new();
  if let Some(value) = &frame.header {
    ciborium::into_writer(value, &mut header).expect("a CBOR value always encodes");
  }
  let mut payload = Vec::new();
  put_varint(header.len() as u64, &mut payload);
  payload.extend_from_slice(&header);
  payload.extend_from_slice(&frame.data);

  let mut out = Vec::new();
  put_varint(frame.stream, &mut out);
  out.push(frame.frame_type);
  out.push(frame.flags);
  put_varint(payload.len() as u64, &mut out);
  out.extend_from_slice(&payload);
  out
}

pub fn decode(mut message: &[u8]) -> Result<Vec<Frame>, String> {
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
      Some(ciborium::from_reader::<Value, _>(header).map_err(|error| format!("header is not CBOR: {error}"))?)
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

fn put_varint(value: u64, out: &mut Vec<u8>) {
  match value {
    0..64 => out.push(value as u8),
    64..16_384 => out.extend_from_slice(&(value as u16 | 0x4000).to_be_bytes()),
    16_384..1_073_741_824 => out.extend_from_slice(&(value as u32 | 0x8000_0000).to_be_bytes()),
    _ => out.extend_from_slice(&(value | 0xc000_0000_0000_0000).to_be_bytes()),
  }
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
