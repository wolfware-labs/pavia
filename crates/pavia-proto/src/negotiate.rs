//! Version and codec selection for WELCOME (§7.2). Both functions are pure: they take the
//! client's HELLO list and what this side supports, and return `None` when nothing overlaps,
//! which the session answers with REJECT UNSUPPORTED_VERSION (§7.3).

use crate::data::Codec;

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
/// use pavia_proto::data::Codec;
/// use pavia_proto::negotiate::codec;
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
