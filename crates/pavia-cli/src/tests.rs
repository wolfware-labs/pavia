use std::{env, fs, io};

use pavia_contract::manifest::MAX_MANIFEST_BYTES;

use super::{MAX_MESSAGE_BYTES, printable, read_bounded};

#[test]
fn keeps_a_short_message_as_it_is() {
  assert_eq!(printable("app.json: type `T` is missing"), "app.json: type `T` is missing");
}

#[test]
fn keeps_a_message_of_64_kib_whole_and_cuts_one_byte_more() {
  let whole = "x".repeat(65_536);
  assert_eq!(printable(&whole), whole);
  assert_eq!(printable(&format!("{whole}y")), format!("{whole}… (1 more byte)"));
  assert_eq!(printable(&format!("{whole}yz")), format!("{whole}… (2 more bytes)"));
}

#[test]
fn refuses_a_file_of_exactly_the_bound_that_is_not_utf8() {
  let path = env::temp_dir().join(format!("pavia-cli-{}-bound-not-utf8.json", std::process::id()));
  let mut bytes = vec![b' '; MAX_MANIFEST_BYTES];
  bytes[0] = 0xff;
  fs::write(&path, bytes).unwrap();
  let read = read_bounded(&path);
  fs::remove_file(&path).unwrap();
  assert_eq!(read.map(|text| text.len()).unwrap_err().kind(), io::ErrorKind::InvalidData);
}

#[test]
fn cuts_a_long_message_on_a_character_boundary_and_counts_the_rest() {
  for (pad, kept) in [("", MAX_MESSAGE_BYTES), ("v", MAX_MESSAGE_BYTES - 1)] {
    let message = format!("{pad}{}", "é".repeat(MAX_MESSAGE_BYTES));
    assert_eq!(
      printable(&message),
      format!("{}… ({} more bytes)", &message[..kept], message.len() - kept),
      "{pad:?}"
    );
  }
}

#[test]
fn writes_control_characters_as_escapes() {
  assert_eq!(printable("a\u{1b}[2J\nb"), r"a\u{1b}[2J\nb");
}

#[test]
fn writes_line_separators_and_direction_marks_as_escapes() {
  assert_eq!(printable("a\u{2028}b\u{202e}c"), r"a\u{2028}b\u{202e}c");
}
