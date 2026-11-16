//! The name grammar every manifest name follows (§12.2): `[A-Za-z_][A-Za-z0-9_]*`.

/// Whether `name` matches `[A-Za-z_][A-Za-z0-9_]*`, the grammar of type, field, variant, service, method
/// and datagram topic names (§12.2).
///
/// ```
/// use pavia_contract::name::is_valid;
///
/// assert!(is_valid("sent_at"));
/// assert!(!is_valid("foo-bar"));
/// assert!(!is_valid("Größe"));
/// ```
#[must_use]
pub fn is_valid(name: &str) -> bool {
  let mut bytes = name.bytes();
  bytes.next().is_some_and(|first| first == b'_' || first.is_ascii_alphabetic())
    && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests;
