//! Frames (spec §5.2, §5.3): the Type, Flags and Length prefix, then a payload of HeaderLength,
//! header bytes and data bytes.
//!
//! [`FrameDecoder`] is incremental: the caller feeds byte chunks of any size and takes out each
//! frame once all of its bytes have arrived. A Length above `max_frame` is rejected as soon as the
//! Length varint is complete, before the payload is waited for. Decoded frames share the decoder's
//! buffer through [`Bytes`], so no payload byte is copied.

use bytes::{Buf, BufMut, Bytes, BytesMut};

use super::varint::{self, DecodeValue};
use crate::error::ProtocolError;

/// The largest value a varint holds, 2^62 − 1 (§5.1).
const MAX_VARINT: u64 = (1 << 62) - 1;

/// One frame. The header is raw CBOR bytes; parsing it is the header layer's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
  /// The frame type (§5.6).
  pub frame_type: u8,
  /// The flags byte, passed through as received: receivers ignore unknown bits (§5.2).
  pub flags: u8,
  /// The header section, empty when HeaderLength is 0.
  pub header: Bytes,
  /// The data section, possibly empty.
  pub data: Bytes,
}

/// Error returned by [`Frame::encode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
  /// The payload needs more than 2^62 − 1 bytes, so no Length varint can hold it.
  #[error("frame payload is above 2^62 - 1 bytes")]
  PayloadTooLarge,
}

impl Frame {
  /// Writes the frame to `out`: Type, Flags, Length, HeaderLength, header, data.
  ///
  /// The caller checks the payload against the peer's `max_frame` first; this only fails when
  /// the payload cannot be framed at all.
  ///
  /// # Errors
  ///
  /// Returns [`EncodeError::PayloadTooLarge`] if the payload is above 2^62 − 1 bytes. Nothing is
  /// written to `out` in that case.
  pub fn encode(&self, out: &mut impl BufMut) -> Result<(), EncodeError> {
    let header_length = u64::try_from(self.header.len()).map_err(|_| EncodeError::PayloadTooLarge)?;
    let data_length = u64::try_from(self.data.len()).map_err(|_| EncodeError::PayloadTooLarge)?;
    let payload_length = varint_length(header_length)
      .checked_add(header_length)
      .and_then(|length| length.checked_add(data_length))
      .filter(|length| *length <= MAX_VARINT)
      .ok_or(EncodeError::PayloadTooLarge)?;

    out.put_u8(self.frame_type);
    out.put_u8(self.flags);
    varint::encode(payload_length, out).map_err(|_| EncodeError::PayloadTooLarge)?;
    varint::encode(header_length, out).map_err(|_| EncodeError::PayloadTooLarge)?;
    out.put_slice(&self.header);
    out.put_slice(&self.data);
    Ok(())
  }
}

/// How many bytes the shortest varint for `value` takes (§5.1).
fn varint_length(value: u64) -> u64 {
  match value {
    0..=0x3F => 1,
    0x40..=0x3FFF => 2,
    0x4000..=0x3FFF_FFFF => 4,
    _ => 8,
  }
}

/// Cuts a byte stream into frames, one chunk at a time (§5.3).
///
/// ```text
/// feed(chunk) ─► checks each Length ─► buffer ─► next_frame() ─► Ok(Some(frame)) │ Ok(None)
///                                       finish() at end of stream ─► Err if a frame is cut off
/// ```
#[derive(Debug)]
pub struct FrameDecoder {
  buffer: BytesMut,
  max_frame: u64,
  checked: usize,
}

impl FrameDecoder {
  /// A decoder that rejects any frame whose Length is above `max_frame`.
  pub fn new(max_frame: u64) -> Self {
    Self {
      buffer: BytesMut::new(),
      max_frame,
      checked: 0,
    }
  }

  /// Appends bytes read from the stream, checking each frame's Length as soon as it is complete.
  ///
  /// Payload bytes are copied only once their frame's Length has passed the check, so an
  /// oversized frame never gets its payload buffered, even when it arrives in the same chunk as
  /// the prefix (§5.2).
  ///
  /// # Errors
  ///
  /// Returns [`ProtocolError::FrameTooLarge`] for a Length above `max_frame`. This ends the
  /// session, so the decoder is not meant to be used after an error.
  pub fn feed(&mut self, mut chunk: &[u8]) -> Result<(), ProtocolError> {
    while !chunk.is_empty() {
      if self.checked > self.buffer.len() {
        let length = (self.checked - self.buffer.len()).min(chunk.len());
        let (payload, rest) = chunk.split_at(length);
        self.buffer.extend_from_slice(payload);
        chunk = rest;
      } else {
        let (byte, rest) = chunk.split_at(1);
        self.buffer.extend_from_slice(byte);
        chunk = rest;
        if let Some((_, frame_length)) = read_prefix(&self.buffer[self.checked..], self.max_frame)? {
          self.checked = self.checked.checked_add(frame_length).ok_or(ProtocolError::FrameTooLarge)?;
        }
      }
    }
    Ok(())
  }

  /// Takes the next complete frame out of the buffer, or `Ok(None)` if it has not fully arrived.
  ///
  /// One [`feed`](Self::feed) can complete several frames, so call this until it returns
  /// `Ok(None)`.
  ///
  /// # Errors
  ///
  /// Returns [`ProtocolError::MalformedHeader`] if HeaderLength does not fit in the payload
  /// (§5.4.1). This ends the session, so the decoder is not meant to be used after an error.
  pub fn next_frame(&mut self) -> Result<Option<Frame>, ProtocolError> {
    let Some((prefix_length, frame_length)) = read_prefix(&self.buffer, self.max_frame)? else {
      return Ok(None);
    };
    if self.buffer.len() < frame_length {
      return Ok(None);
    }
    let mut bytes = self.buffer.split_to(frame_length).freeze();
    self.checked -= frame_length;
    let frame_type = bytes.get_u8();
    let flags = bytes.get_u8();
    bytes.advance(prefix_length - 2);
    split_payload(frame_type, flags, bytes).map(Some)
  }

  /// Checks the end of the stream: it is an error if a frame was cut off. Complete frames that
  /// are still buffered are fine and can still be taken with [`next_frame`](Self::next_frame).
  ///
  /// # Errors
  ///
  /// Returns [`ProtocolError::StreamEndedMidFrame`] if the last buffered frame is incomplete
  /// (§5.3).
  pub fn finish(&self) -> Result<(), ProtocolError> {
    if self.buffer.len() == self.checked {
      Ok(())
    } else {
      Err(ProtocolError::StreamEndedMidFrame)
    }
  }
}

/// Reads Type, Flags and Length. Returns the prefix length and the whole frame's length, or `None`
/// if the prefix is incomplete.
fn read_prefix(bytes: &[u8], max_frame: u64) -> Result<Option<(usize, usize)>, ProtocolError> {
  let Some(length_bytes) = bytes.get(2..) else {
    return Ok(None);
  };
  match varint::decode(length_bytes, max_frame) {
    Ok(DecodeValue::NeedMoreBytes) => Ok(None),
    Ok(DecodeValue::Value { value, consumed_length }) => {
      let prefix_length = 2 + consumed_length;
      Ok(Some((prefix_length, frame_length(prefix_length, value)?)))
    }
    Err(varint::DecodeError::AboveMax { .. }) => Err(ProtocolError::FrameTooLarge),
  }
}

/// The prefix plus `payload_length`, or [`ProtocolError::FrameTooLarge`] if that does not fit in
/// a `usize`, which a Length below `max_frame` can still do on a 32-bit target.
fn frame_length(prefix_length: usize, payload_length: u64) -> Result<usize, ProtocolError> {
  usize::try_from(payload_length)
    .ok()
    .and_then(|length| length.checked_add(prefix_length))
    .ok_or(ProtocolError::FrameTooLarge)
}

/// Splits a payload into header and data at HeaderLength.
fn split_payload(frame_type: u8, flags: u8, mut payload: Bytes) -> Result<Frame, ProtocolError> {
  let (header_length, consumed_length) = match varint::decode(&payload, u64::MAX) {
    Ok(DecodeValue::Value { value, consumed_length }) => (value, consumed_length),
    Ok(DecodeValue::NeedMoreBytes) | Err(varint::DecodeError::AboveMax { .. }) => {
      return Err(ProtocolError::MalformedHeader);
    }
  };
  payload.advance(consumed_length);
  let header_length = usize::try_from(header_length)
    .ok()
    .filter(|length| *length <= payload.len())
    .ok_or(ProtocolError::MalformedHeader)?;
  let header = payload.split_to(header_length);
  Ok(Frame {
    frame_type,
    flags,
    header,
    data: payload,
  })
}

#[cfg(test)]
mod tests {
  use super::*;
  use proptest::prelude::*;

  const MAX_FRAME: u64 = 65_536;

  fn hex_bytes(hex: &str) -> Vec<u8> {
    let digits: Vec<u8> = hex.bytes().filter(|byte| !byte.is_ascii_whitespace()).collect();
    digits
      .chunks(2)
      .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
      .collect()
  }

  fn frame(frame_type: u8, flags: u8, header: &[u8], data: &[u8]) -> Frame {
    Frame {
      frame_type,
      flags,
      header: Bytes::copy_from_slice(header),
      data: Bytes::copy_from_slice(data),
    }
  }

  fn encoded(frames: &[Frame]) -> Vec<u8> {
    let mut out = Vec::new();
    for frame in frames {
      frame.encode(&mut out).unwrap();
    }
    out
  }

  fn drain(decoder: &mut FrameDecoder) -> Result<Vec<Frame>, ProtocolError> {
    let mut frames = Vec::new();
    while let Some(frame) = decoder.next_frame()? {
      frames.push(frame);
    }
    Ok(frames)
  }

  fn decode_in_chunks(bytes: &[u8], chunk_lengths: &[usize]) -> Result<Vec<Frame>, ProtocolError> {
    let mut decoder = FrameDecoder::new(MAX_FRAME);
    let mut frames = Vec::new();
    let mut rest = bytes;
    let mut lengths = chunk_lengths.iter().cycle();
    while !rest.is_empty() {
      let length = lengths.next().copied().unwrap_or(rest.len()).clamp(1, rest.len());
      let (chunk, tail) = rest.split_at(length);
      decoder.feed(chunk)?;
      frames.extend(drain(&mut decoder)?);
      rest = tail;
    }
    decoder.finish()?;
    Ok(frames)
  }

  #[test]
  fn writes_the_prefix_header_length_header_and_data() {
    let ping = frame(0x04, 0, &[], &[]);
    assert_eq!(encoded(&[ping]), [0x04, 0x00, 0x01, 0x00]);

    let call = frame(0x10, 0, &[0xa0], &[0x01, 0x02]);
    assert_eq!(encoded(&[call]), [0x10, 0x00, 0x04, 0x01, 0xa0, 0x01, 0x02]);
  }

  #[test]
  fn counts_a_two_byte_header_length_in_the_payload() {
    let header = vec![0xa0; 64];
    let bytes = encoded(&[frame(0x01, 0, &header, &[])]);
    assert_eq!(&bytes[..5], [0x01, 0x00, 0x40, 0x42, 0x40]);
    assert_eq!(bytes.len(), 2 + 2 + 2 + 64);
  }

  #[test]
  fn decodes_a_frame_fed_whole() {
    let call = frame(0x10, 0, &[0xa0], &[0x01, 0x02]);
    let mut decoder = FrameDecoder::new(MAX_FRAME);
    decoder.feed(&encoded(std::slice::from_ref(&call))).unwrap();
    assert_eq!(decoder.next_frame().unwrap(), Some(call));
    assert_eq!(decoder.next_frame().unwrap(), None);
    decoder.finish().unwrap();
  }

  #[test]
  fn decodes_a_frame_fed_one_byte_at_a_time() {
    let call = frame(0x10, 0, &[0xa0], &[0x01, 0x02]);
    let frames = decode_in_chunks(&encoded(std::slice::from_ref(&call)), &[1]).unwrap();
    assert_eq!(frames, [call]);
  }

  #[test]
  fn yields_every_frame_completed_by_one_feed() {
    let frames = [
      frame(0x04, 0, &[], &[]),
      frame(0x10, 0, &[0xa0], &[0x01]),
      frame(0x05, 0, &[], &[]),
    ];
    let mut decoder = FrameDecoder::new(MAX_FRAME);
    decoder.feed(&encoded(&frames)).unwrap();
    assert_eq!(drain(&mut decoder).unwrap(), frames);
  }

  #[test]
  fn keeps_the_start_of_the_next_frame_for_the_next_feed() {
    let frames = [frame(0x04, 0, &[], &[]), frame(0x10, 0, &[0xa0], &[0x01])];
    let bytes = encoded(&frames);
    let mut decoder = FrameDecoder::new(MAX_FRAME);
    decoder.feed(&bytes[..6]).unwrap();
    assert_eq!(drain(&mut decoder).unwrap(), [frames[0].clone()]);
    decoder.feed(&bytes[6..]).unwrap();
    assert_eq!(drain(&mut decoder).unwrap(), [frames[1].clone()]);
    decoder.finish().unwrap();
  }

  #[test]
  fn passes_unknown_flag_bits_through() {
    let flagged = frame(0x04, 0xff, &[], &[]);
    let frames = decode_in_chunks(&encoded(std::slice::from_ref(&flagged)), &[]).unwrap();
    assert_eq!(frames[0].flags, 0xff);
  }

  #[test]
  fn rejects_a_length_above_max_frame_before_the_payload_arrives() {
    let mut decoder = FrameDecoder::new(100);
    assert!(matches!(
      decoder.feed(&[0x10, 0x00, 0x40, 0x65]),
      Err(ProtocolError::FrameTooLarge)
    ));
  }

  #[test]
  fn accepts_a_length_equal_to_max_frame_without_counting_the_prefix() {
    let data = vec![0; 99];
    let bytes = encoded(&[frame(0x10, 0, &[], &data)]);
    assert_eq!(bytes.len(), 2 + 2 + 100);
    let mut decoder = FrameDecoder::new(100);
    decoder.feed(&bytes).unwrap();
    assert!(decoder.next_frame().unwrap().is_some());
  }

  #[test]
  fn rejects_a_header_length_past_the_end_of_the_payload() {
    let mut decoder = FrameDecoder::new(MAX_FRAME);
    decoder.feed(&hex_bytes("04 00 03 05 a0 00")).unwrap();
    assert!(matches!(decoder.next_frame(), Err(ProtocolError::MalformedHeader)));
  }

  #[test]
  fn rejects_a_payload_without_a_header_length() {
    let mut decoder = FrameDecoder::new(MAX_FRAME);
    decoder.feed(&[0x04, 0x00, 0x00]).unwrap();
    assert!(matches!(decoder.next_frame(), Err(ProtocolError::MalformedHeader)));
  }

  #[test]
  fn reports_a_stream_cut_at_every_offset() {
    let bytes = encoded(&[frame(0x10, 0, &[0xa1, 0x01, 0x02], &[0x01, 0x02, 0x03])]);
    for cut in 1..bytes.len() {
      let mut decoder = FrameDecoder::new(MAX_FRAME);
      decoder.feed(&bytes[..cut]).unwrap();
      assert_eq!(decoder.next_frame().unwrap(), None, "cut at {cut}");
      assert!(
        matches!(decoder.finish(), Err(ProtocolError::StreamEndedMidFrame)),
        "cut at {cut}"
      );
    }
  }

  #[test]
  fn does_not_buffer_the_payload_of_an_oversized_frame() {
    let mut bytes = vec![0x10, 0x00, 0x40, 0x65];
    bytes.extend([0; 101]);
    let mut decoder = FrameDecoder::new(100);
    assert!(matches!(decoder.feed(&bytes), Err(ProtocolError::FrameTooLarge)));
    assert_eq!(decoder.buffer.len(), 4);
  }

  #[test]
  fn checks_the_length_of_a_later_frame_in_the_same_chunk() {
    let mut bytes = encoded(&[frame(0x04, 0, &[], &[])]);
    bytes.extend([0x10, 0x00, 0x40, 0x65]);
    bytes.extend([0; 101]);
    let mut decoder = FrameDecoder::new(100);
    assert!(matches!(decoder.feed(&bytes), Err(ProtocolError::FrameTooLarge)));
    assert_eq!(decoder.buffer.len(), 8);
  }

  #[test]
  fn rejects_a_frame_length_that_does_not_fit_in_usize() {
    let largest = u64::try_from(usize::MAX).unwrap();
    assert!(matches!(frame_length(10, largest), Err(ProtocolError::FrameTooLarge)));
    assert!(matches!(frame_length(2, largest - 1), Err(ProtocolError::FrameTooLarge)));
    assert_eq!(frame_length(2, largest - 2).unwrap(), usize::MAX);
  }

  #[test]
  fn ends_cleanly_with_complete_frames_still_buffered() {
    let frames = [frame(0x04, 0, &[], &[]), frame(0x10, 0, &[0xa0], &[0x01])];
    let mut decoder = FrameDecoder::new(MAX_FRAME);
    decoder.feed(&encoded(&frames)).unwrap();
    decoder.finish().unwrap();
    assert_eq!(drain(&mut decoder).unwrap(), frames);
  }

  #[test]
  fn reports_a_cut_frame_after_complete_ones_at_the_end_of_the_stream() {
    let bytes = encoded(&[frame(0x04, 0, &[], &[]), frame(0x10, 0, &[0xa0], &[0x01])]);
    for cut in [5, bytes.len() - 1] {
      let mut decoder = FrameDecoder::new(MAX_FRAME);
      decoder.feed(&bytes[..cut]).unwrap();
      assert!(
        matches!(decoder.finish(), Err(ProtocolError::StreamEndedMidFrame)),
        "cut at {cut}"
      );
    }
  }

  #[test]
  fn ends_cleanly_on_an_empty_stream() {
    FrameDecoder::new(MAX_FRAME).finish().unwrap();
  }

  #[test]
  fn shares_the_buffer_instead_of_copying_the_payload() {
    let bytes = encoded(&[frame(0x10, 0, &[0xa0], &[0x01, 0x02])]);
    let mut decoder = FrameDecoder::new(MAX_FRAME);
    decoder.feed(&bytes).unwrap();
    let decoded = decoder.next_frame().unwrap().unwrap();
    assert_eq!(decoded.header.as_ptr_range().end, decoded.data.as_ptr());
  }

  #[test]
  fn matches_the_webtransport_frame_vectors() {
    for file in [
      include_str!("../../../../vectors/frames/handshake.json"),
      include_str!("../../../../vectors/frames/invalid.json"),
    ] {
      let doc: serde_json::Value = serde_json::from_str(file).unwrap();
      for case in doc["cases"].as_array().unwrap() {
        if case["binding"] != "wt" {
          continue;
        }
        let id = case["id"].as_str().unwrap();
        let bytes = hex_bytes(case["hex"].as_str().unwrap());
        for chunk_length in [bytes.len(), 1, 3] {
          let mut decoder = FrameDecoder::new(MAX_FRAME);
          let mut frames = Vec::new();
          for chunk in bytes.chunks(chunk_length) {
            decoder.feed(chunk).unwrap_or_else(|e| panic!("{id}: {e}"));
            frames.extend(drain(&mut decoder).unwrap_or_else(|e| panic!("{id}: {e}")));
          }
          if case["fin"] == true {
            let finished = decoder.finish();
            assert!(
              matches!(finished, Err(ProtocolError::StreamEndedMidFrame)),
              "{id}: {finished:?}"
            );
          }
          let Some(expected) = case["frames"].as_array() else {
            continue;
          };
          assert_eq!(frames.len(), expected.len(), "{id}");
          for (frame, want) in frames.iter().zip(expected) {
            assert_eq!(u64::from(frame.frame_type), want["type"].as_u64().unwrap(), "{id}");
            assert_eq!(u64::from(frame.flags), want["flags"].as_u64().unwrap(), "{id}");
            assert_eq!(frame.header.is_empty(), want["header"].is_null(), "{id}");
            assert_eq!(frame.data.is_empty(), want["data"].is_null(), "{id}");
            if case["direction"] == "both" {
              assert_eq!(encoded(std::slice::from_ref(frame)), bytes, "{id}: encoder output");
            }
          }
        }
      }
    }
  }

  fn any_frame() -> impl Strategy<Value = Frame> {
    (
      any::<u8>(),
      any::<u8>(),
      prop::collection::vec(any::<u8>(), 0..80),
      prop::collection::vec(any::<u8>(), 0..200),
    )
      .prop_map(|(frame_type, flags, header, data)| frame(frame_type, flags, &header, &data))
  }

  proptest! {
    #[test]
    fn decodes_the_same_frames_however_the_bytes_are_chunked(
      frames in prop::collection::vec(any_frame(), 0..6),
      chunk_lengths in prop::collection::vec(1usize..40, 1..10),
    ) {
      let bytes = encoded(&frames);
      prop_assert_eq!(decode_in_chunks(&bytes, &chunk_lengths).unwrap(), frames);
    }
  }
}
