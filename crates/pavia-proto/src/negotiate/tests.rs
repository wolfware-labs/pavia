use super::{codec, version};
use crate::data::Codec;

const SERVER_CODECS: [Codec; 2] = [Codec::Cbor, Codec::Json];

#[test]
fn picks_the_highest_common_version() {
  let rows: [(&[u64], &[u64], Option<u64>); 6] = [
    (&[1], &[1], Some(1)),
    (&[2], &[1], None),
    (&[1, 2, 3], &[1, 2], Some(2)),
    (&[3, 1, 2], &[2, 1], Some(2)),
    (&[1], &[1, 2], Some(1)),
    (&[4, 5], &[1, 2], None),
  ];
  for (client, server, expected) in rows {
    assert_eq!(version(client, server), expected, "client {client:?}, server {server:?}");
  }
}

#[test]
fn picks_the_first_supported_codec_in_client_order() {
  let rows: [(&[&str], Option<Codec>); 6] = [
    (&["cbor"], Some(Codec::Cbor)),
    (&["json", "cbor"], Some(Codec::Json)),
    (&["cbor", "json"], Some(Codec::Cbor)),
    (&["msgpack"], None),
    (&["msgpack", "cbor"], Some(Codec::Cbor)),
    (&["CBOR"], None),
  ];
  for (client, expected) in rows {
    assert_eq!(codec(client, &SERVER_CODECS), expected, "client {client:?}");
  }
}

#[test]
fn skips_a_known_codec_this_side_does_not_support() {
  assert_eq!(codec(&["json", "cbor"], &[Codec::Cbor]), Some(Codec::Cbor));
  assert_eq!(codec(&["json"], &[Codec::Cbor]), None);
}
