use proptest::prelude::*;

use super::{InvalidSessionId, MAX_LEN, SessionId};

#[test]
fn encodes_bytes_as_unpadded_base64url() {
  let rows: [([u8; 16], &str); 4] = [
    ([0; 16], "AAAAAAAAAAAAAAAAAAAAAA"),
    ([0xff; 16], "_____________________w"),
    (
      [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
      "AAECAwQFBgcICQoLDA0ODw",
    ),
    (
      [
        0xfb, 0xff, 0xbf, 0xfb, 0xff, 0xbf, 0xfb, 0xff, 0xbf, 0xfb, 0xff, 0xbf, 0xfb, 0xff, 0xbf, 0xfe,
      ],
      "-_-_-_-_-_-_-_-_-_-__g",
    ),
  ];
  for (bytes, expected) in rows {
    assert_eq!(SessionId::from_random(bytes).as_str(), expected, "bytes {bytes:02x?}");
  }
}

#[test]
fn accepts_the_spec_format() {
  let longest = "a".repeat(MAX_LEN);
  for text in [
    "a",
    "s_q3Vd0p-8zXy1Lr9QmT2hAw",
    "n7f3_000000000042",
    "-",
    "_",
    longest.as_str(),
  ] {
    assert_eq!(
      SessionId::parse(text).map(|id| id.to_string()),
      Ok(text.to_owned()),
      "{text:?}"
    );
  }
}

#[test]
fn refuses_text_outside_the_spec_format() {
  let too_long = "a".repeat(MAX_LEN + 1);
  for text in ["", too_long.as_str(), "a+b", "a/b", "ab==", "a b", "a.b", "é", "a\0"] {
    assert_eq!(SessionId::parse(text), Err(InvalidSessionId), "{text:?}");
  }
}

proptest! {
  #[test]
  fn random_ids_are_22_characters_and_parse_back(bytes in any::<[u8; 16]>()) {
    let id = SessionId::from_random(bytes);
    prop_assert_eq!(id.as_str().len(), 22);
    prop_assert_eq!(SessionId::parse(id.as_str()), Ok(id));
  }
}
