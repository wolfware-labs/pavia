//! Writing CBOR items straight to an output, without building a [`Value`](super::Value) first.
//!
//! The caller writes the items of a header in core deterministic order itself (spec §5.4.1): map
//! sizes up front, keys sorted. [`Measure`] counts the bytes a header takes, so a frame can write
//! its Length prefix before the header without buffering it.

use bytes::BufMut;

pub(super) const UNSIGNED: u8 = 0;
pub(super) const BYTES: u8 = 2;
pub(super) const TEXT: u8 = 3;
pub(super) const ARRAY: u8 = 4;
pub(super) const MAP: u8 = 5;

/// Where CBOR items go. Every method but [`Sink::put`] writes one item head or one whole item.
pub trait Sink {
  /// Appends raw bytes.
  fn put(&mut self, bytes: &[u8]);

  /// Writes an item head with the shortest argument (RFC 8949 §4.2.1).
  fn head(&mut self, major_type: u8, argument: u64) {
    let major_type = major_type << 5;
    match argument {
      0..24 => self.put(&[major_type | argument as u8]),
      24..0x100 => self.put(&[major_type | 24, argument as u8]),
      0x100..0x1_0000 => {
        self.put(&[major_type | 25]);
        self.put(&(argument as u16).to_be_bytes());
      }
      0x1_0000..0x1_0000_0000 => {
        self.put(&[major_type | 26]);
        self.put(&(argument as u32).to_be_bytes());
      }
      _ => {
        self.put(&[major_type | 27]);
        self.put(&argument.to_be_bytes());
      }
    }
  }

  /// Writes an unsigned integer.
  fn uint(&mut self, n: u64) {
    self.head(UNSIGNED, n);
  }

  /// Writes a text string.
  fn text(&mut self, text: &str) {
    self.head(TEXT, text.len() as u64);
    self.put(text.as_bytes());
  }

  /// Writes a byte string.
  fn bytes(&mut self, bytes: &[u8]) {
    self.head(BYTES, bytes.len() as u64);
    self.put(bytes);
  }

  /// Writes `false` or `true`.
  fn bool(&mut self, b: bool) {
    self.put(&[if b { 0xf5 } else { 0xf4 }]);
  }

  /// Writes the head of an array of `len` items; the items follow.
  fn array(&mut self, len: u64) {
    self.head(ARRAY, len);
  }

  /// Writes the head of a map of `len` entries; the keys and values follow, keys sorted.
  fn map(&mut self, len: u64) {
    self.head(MAP, len);
  }
}

/// A sink that only counts the bytes written to it.
#[derive(Debug, Default)]
pub struct Measure(pub usize);

impl Sink for Measure {
  fn put(&mut self, bytes: &[u8]) {
    self.0 += bytes.len();
  }
}

/// A sink that writes to a [`BufMut`].
pub struct Out<'b, B: BufMut>(pub &'b mut B);

impl<B: BufMut> Sink for Out<'_, B> {
  fn put(&mut self, bytes: &[u8]) {
    self.0.put_slice(bytes);
  }
}
