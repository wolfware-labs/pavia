use super::{MAX_MESSAGE_BYTES, printable};

#[test]
fn keeps_a_short_message_as_it_is() {
  assert_eq!(printable("app.json: type `T` is missing"), "app.json: type `T` is missing");
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
