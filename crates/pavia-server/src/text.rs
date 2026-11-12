/// `text` cut to at most `max` bytes, on a character boundary.
pub(crate) fn truncate(text: &str, max: usize) -> &str {
  let mut end = text.len().min(max);
  while !text.is_char_boundary(end) {
    end -= 1;
  }
  &text[..end]
}
