//! Version and codec selection for WELCOME (§7.2). Both functions are pure: they take the
//! client's HELLO list and what this side supports, and return `None` when nothing overlaps,
//! which the session answers with REJECT UNSUPPORTED_VERSION (§7.3).

/// A data-section codec (§5.5). The header section is always CBOR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Codec {
  /// `cbor`, which every implementation supports.
  Cbor,
  /// `json`, optional and meant for debugging.
  Json,
}

impl Codec {
  /// The name used in HELLO `codecs` and WELCOME `codec`.
  #[must_use]
  pub const fn name(self) -> &'static str {
    match self {
      Self::Cbor => "cbor",
      Self::Json => "json",
    }
  }

  /// The codec with this name, or `None` for a name this crate does not know.
  #[must_use]
  pub fn from_name(name: &str) -> Option<Self> {
    match name {
      "cbor" => Some(Self::Cbor),
      "json" => Some(Self::Json),
      _ => None,
    }
  }
}

/// The highest protocol version present in both lists.
///
/// ```
/// use pavia_proto::negotiate::version;
///
/// assert_eq!(version(&[1, 2, 3], &[1, 2]), Some(2));
/// assert_eq!(version(&[2], &[1]), None);
/// ```
#[must_use]
pub fn version(client: &[u64], supported: &[u64]) -> Option<u64> {
  client.iter().copied().filter(|offered| supported.contains(offered)).max()
}

/// The first codec in the client's list, in its preference order, that this side supports.
/// Names this crate does not know are skipped.
///
/// ```
/// use pavia_proto::negotiate::{Codec, codec};
///
/// let supported = [Codec::Cbor, Codec::Json];
/// assert_eq!(codec(&["msgpack", "json", "cbor"], &supported), Some(Codec::Json));
/// assert_eq!(codec(&["msgpack"], &supported), None);
/// ```
#[must_use]
pub fn codec<S: AsRef<str>>(client: &[S], supported: &[Codec]) -> Option<Codec> {
  client
    .iter()
    .filter_map(|name| Codec::from_name(name.as_ref()))
    .find(|offered| supported.contains(offered))
}

#[cfg(test)]
mod tests;
