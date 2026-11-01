//! The WebSocket envelope (spec §4.3, §5.4): every frame is prefixed with its stream ID, and one
//! binary message carries one or more of these `WsFrame`s.
//!
//! A WebSocket message arrives whole, so [`decode_message`] works on complete messages and
//! anything left at the end is an error. [`encode_messages`] batches frames from any streams into
//! as few messages as fit under the peer's `2 × max_frame` limit, never splitting a frame.
//!
//! ```text
//!  00 04 00 01 00 │ 02 15 00 01 00
//!  │  └ PING      │ │  └ CANCEL
//!  └ stream 0     │ └ stream 2
//! ```

use bytes::{Bytes, BytesMut};

use super::frame::{self, Frame};
use super::varint::{self, DecodeValue};
use crate::error::ProtocolError;

/// The smallest `max_frame` a peer may advertise (§4.1). Before WELCOME a sender does not know the
/// peer's value, so it assumes this one.
pub const MAX_FRAME_FLOOR: u64 = 65_536;

/// One frame with the stream it belongs to (§5.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsFrame {
  /// The logical stream: 0 is the control stream (§6.1).
  pub stream_id: u64,
  /// The frame itself.
  pub frame: Frame,
}

/// Error returned by [`encode_messages`].
#[cfg_attr(
  not(any(test, fuzzing)),
  expect(
    dead_code,
    reason = "the session batches frames into messages once it sends more than CLOSE (#33)"
  )
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
  /// A frame's payload is above the peer's `max_frame`, so it cannot be sent at all.
  #[error("frame payload above the peer's max_frame")]
  FrameTooLarge,
  /// A stream ID above 2^62 − 1, which no varint can hold.
  #[error("stream ID above 2^62 - 1")]
  StreamIdOutOfRange,
}

/// The largest message a receiver that advertised `max_frame` accepts: `2 × max_frame` (§4.3).
pub fn message_limit(max_frame: u64) -> u64 {
  max_frame.saturating_mul(2)
}

/// Splits one WebSocket binary message into its `WsFrame`s, in order.
///
/// `max_frame` is the limit this side advertised. The size check runs first, so an oversized
/// message is rejected before any of its frames is decoded.
///
/// # Errors
///
/// - [`ProtocolError::MessageTooLarge`] for a message above `2 × max_frame` (§4.3).
/// - [`ProtocolError::MalformedMessage`] for an empty message, which holds no `WsFrame`.
/// - [`ProtocolError::LeftoverBytes`] for a trailing partial `WsFrame` (§5.4).
/// - [`ProtocolError::FrameTooLarge`] and [`ProtocolError::MalformedHeader`] from each frame.
pub fn decode_message(mut message: Bytes, max_frame: u64) -> Result<Vec<WsFrame>, ProtocolError> {
  if u64::try_from(message.len()).map_or(true, |length| length > message_limit(max_frame)) {
    return Err(ProtocolError::MessageTooLarge);
  }
  if message.is_empty() {
    return Err(ProtocolError::MalformedMessage);
  }
  let mut frames = Vec::new();
  while !message.is_empty() {
    let (stream_id, consumed_length) = match varint::decode(&message, u64::MAX) {
      Ok(DecodeValue::Value { value, consumed_length }) => (value, consumed_length),
      Ok(DecodeValue::NeedMoreBytes) | Err(varint::DecodeError::AboveMax { .. }) => {
        return Err(ProtocolError::LeftoverBytes);
      }
    };
    let mut rest = message.slice(consumed_length..);
    let Some(frame) = frame::take_frame(&mut rest, max_frame)? else {
      return Err(ProtocolError::LeftoverBytes);
    };
    frames.push(WsFrame { stream_id, frame });
    message = rest;
  }
  Ok(frames)
}

/// Packs `frames` into WebSocket messages, in order, each at most `2 × max_frame` of the peer.
///
/// `peer_max_frame` is the value from the peer's WELCOME or HELLO, or `None` before it is known;
/// then [`MAX_FRAME_FLOOR`] applies. A new message starts only at a frame boundary.
///
/// # Errors
///
/// - [`EncodeError::FrameTooLarge`] if a frame's payload is above the peer's `max_frame`.
/// - [`EncodeError::StreamIdOutOfRange`] for a stream ID above 2^62 − 1.
#[cfg_attr(
  not(any(test, fuzzing)),
  expect(
    dead_code,
    reason = "the session batches frames into messages once it sends more than CLOSE (#33)"
  )
)]
pub fn encode_messages(frames: &[WsFrame], peer_max_frame: Option<u64>) -> Result<Vec<Bytes>, EncodeError> {
  let max_frame = peer_max_frame.unwrap_or(MAX_FRAME_FLOOR);
  let limit = usize::try_from(message_limit(max_frame)).unwrap_or(usize::MAX);
  let mut messages = Vec::new();
  let mut current = BytesMut::new();
  for ws_frame in frames {
    if ws_frame.frame.payload_length().map_or(true, |length| length > max_frame) {
      return Err(EncodeError::FrameTooLarge);
    }
    let start = current.len();
    varint::encode(ws_frame.stream_id, &mut current).map_err(|_| EncodeError::StreamIdOutOfRange)?;
    ws_frame.frame.encode(&mut current).map_err(|_| EncodeError::FrameTooLarge)?;
    if current.len() > limit && start > 0 {
      let next = current.split_off(start);
      messages.push(current.freeze());
      current = next;
    }
    if current.len() > limit {
      return Err(EncodeError::FrameTooLarge);
    }
  }
  if !current.is_empty() {
    messages.push(current.freeze());
  }
  Ok(messages)
}

#[cfg(test)]
mod tests {
  use super::*;
  use proptest::prelude::*;

  fn hex_bytes(hex: &str) -> Bytes {
    let digits: Vec<u8> = hex.bytes().filter(|byte| !byte.is_ascii_whitespace()).collect();
    digits
      .chunks(2)
      .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
      .collect()
  }

  fn ws_frame(stream_id: u64, frame_type: u8, header: &[u8], data: &[u8]) -> WsFrame {
    WsFrame {
      stream_id,
      frame: Frame {
        frame_type,
        flags: 0,
        header: Bytes::copy_from_slice(header),
        data: Bytes::copy_from_slice(data),
      },
    }
  }

  #[test]
  fn decodes_a_single_frame_message() {
    let frames = decode_message(hex_bytes("00 04 00 01 00"), MAX_FRAME_FLOOR).unwrap();
    assert_eq!(frames, [ws_frame(0, 0x04, &[], &[])]);
  }

  #[test]
  fn decodes_frames_of_different_streams_in_order() {
    let frames = decode_message(hex_bytes("00 04 00 01 00 02 15 00 01 00"), MAX_FRAME_FLOOR).unwrap();
    assert_eq!(frames, [ws_frame(0, 0x04, &[], &[]), ws_frame(2, 0x15, &[], &[])]);
  }

  #[test]
  fn rejects_a_trailing_partial_ws_frame() {
    for trailing in ["04", "02 15", "02 15 00 02 00"] {
      let message = hex_bytes(&format!("00 04 00 01 00 {trailing}"));
      assert!(
        matches!(decode_message(message, MAX_FRAME_FLOOR), Err(ProtocolError::LeftoverBytes)),
        "trailing {trailing}"
      );
    }
  }

  #[test]
  fn rejects_an_empty_message() {
    assert!(matches!(
      decode_message(Bytes::new(), MAX_FRAME_FLOOR),
      Err(ProtocolError::MalformedMessage)
    ));
  }

  #[test]
  fn accepts_a_message_of_exactly_twice_max_frame() {
    let frames = [
      ws_frame(2, 0x11, &[], &vec![0; 65_535]),
      ws_frame(4, 0x11, &[], &vec![0; 65_521]),
    ];
    let mut message = BytesMut::new();
    for ws_frame in &frames {
      varint::encode(ws_frame.stream_id, &mut message).unwrap();
      ws_frame.frame.encode(&mut message).unwrap();
    }
    assert_eq!(message.len(), 131_072);
    assert_eq!(decode_message(message.freeze(), MAX_FRAME_FLOOR).unwrap(), frames);
  }

  #[test]
  fn rejects_a_message_above_twice_max_frame_before_decoding() {
    let message = Bytes::from(vec![0xff; 131_073]);
    assert!(matches!(
      decode_message(message, MAX_FRAME_FLOOR),
      Err(ProtocolError::MessageTooLarge)
    ));
  }

  #[test]
  fn checks_each_frame_against_max_frame() {
    let message = hex_bytes("00 10 00 40 65");
    assert!(matches!(decode_message(message, 100), Err(ProtocolError::FrameTooLarge)));
  }

  #[test]
  fn batches_frames_of_different_streams_into_one_message() {
    let frames = [ws_frame(0, 0x04, &[], &[]), ws_frame(2, 0x15, &[], &[])];
    let messages = encode_messages(&frames, None).unwrap();
    assert_eq!(messages, [hex_bytes("00 04 00 01 00 02 15 00 01 00")]);
  }

  #[test]
  fn splits_a_batch_above_128_kib_before_welcome() {
    let frames: Vec<WsFrame> = (0..5).map(|i| ws_frame(2 * i, 0x11, &[], &vec![0; 60_000])).collect();
    let messages = encode_messages(&frames, None).unwrap();
    assert!(messages.len() > 1);
    let mut decoded = Vec::new();
    for message in messages {
      assert!(message.len() <= 131_072, "{} bytes", message.len());
      decoded.extend(decode_message(message, MAX_FRAME_FLOOR).unwrap());
    }
    assert_eq!(decoded, frames);
  }

  #[test]
  fn uses_the_peer_max_frame_once_known() {
    let frames: Vec<WsFrame> = (0..5).map(|i| ws_frame(2 * i, 0x11, &[], &vec![0; 60_000])).collect();
    let messages = encode_messages(&frames, Some(1 << 20)).unwrap();
    assert_eq!(messages.len(), 1);
  }

  #[test]
  fn refuses_a_frame_above_the_peer_max_frame() {
    let frames = [ws_frame(2, 0x11, &[], &[0; 100])];
    assert_eq!(encode_messages(&frames, Some(100)), Err(EncodeError::FrameTooLarge));
  }

  #[test]
  fn refuses_a_stream_id_no_varint_holds() {
    let frames = [ws_frame(1 << 62, 0x04, &[], &[])];
    assert_eq!(encode_messages(&frames, None), Err(EncodeError::StreamIdOutOfRange));
  }

  #[test]
  fn encodes_nothing_for_no_frames() {
    assert!(encode_messages(&[], None).unwrap().is_empty());
  }

  #[test]
  fn matches_the_websocket_frame_vectors() {
    for file in [
      include_str!("../../../../vectors/frames/handshake.json"),
      include_str!("../../../../vectors/frames/invalid.json"),
    ] {
      let doc: serde_json::Value = serde_json::from_str(file).unwrap();
      for case in doc["cases"].as_array().unwrap() {
        if case["binding"] != "ws" {
          continue;
        }
        let id = case["id"].as_str().unwrap();
        let bytes = hex_bytes(case["hex"].as_str().unwrap());
        let decoded = decode_message(bytes.clone(), MAX_FRAME_FLOOR);
        let Some(expected) = case["frames"].as_array().filter(|_| case["error"].is_null()) else {
          assert!(decoded.is_err(), "{id}: {decoded:?}");
          continue;
        };
        let frames = decoded.unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(frames.len(), expected.len(), "{id}");
        for (ws_frame, want) in frames.iter().zip(expected) {
          assert_eq!(ws_frame.stream_id, want["stream"].as_u64().unwrap(), "{id}");
          assert_eq!(u64::from(ws_frame.frame.frame_type), want["type"].as_u64().unwrap(), "{id}");
          assert_eq!(ws_frame.frame.header.is_empty(), want["header"].is_null(), "{id}");
          assert_eq!(ws_frame.frame.data.is_empty(), want["data"].is_null(), "{id}");
        }
        if case["direction"] == "both" {
          assert_eq!(encode_messages(&frames, None).unwrap(), [bytes], "{id}: encoder output");
        }
      }
    }
  }

  fn any_ws_frame() -> impl Strategy<Value = WsFrame> {
    (
      0u64..1 << 20,
      any::<u8>(),
      prop::collection::vec(any::<u8>(), 0..64),
      prop::collection::vec(any::<u8>(), 0..3_000),
    )
      .prop_map(|(stream_id, frame_type, header, data)| ws_frame(stream_id, frame_type, &header, &data))
  }

  proptest! {
    #[test]
    fn encoded_messages_decode_to_the_same_frames_under_the_limit(
      frames in prop::collection::vec(any_ws_frame(), 0..12),
      max_frame in 3_100u64..20_000,
    ) {
      let messages = encode_messages(&frames, Some(max_frame)).unwrap();
      let mut decoded = Vec::new();
      for message in messages {
        prop_assert!(message.len() as u64 <= message_limit(max_frame));
        decoded.extend(decode_message(message, max_frame).unwrap());
      }
      prop_assert_eq!(decoded, frames);
    }
  }
}
