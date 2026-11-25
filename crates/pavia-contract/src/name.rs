//! The name grammar every manifest name follows (§12.2): `[A-Za-z_][A-Za-z0-9_]*`.

/// Whether `name` matches `[A-Za-z_][A-Za-z0-9_]*`, the grammar of type, field, variant, service, method,
/// param and datagram topic names (§12.2).
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

/// Whether `namespace` is a channel namespace (§12.2): 1 to 255 bytes of the channel name characters of
/// §10.1, `[A-Za-z0-9_\-./@]`, without the `:` that separates a namespace from the rest of a channel name.
///
/// ```
/// use pavia_contract::name::is_valid_namespace;
///
/// assert!(is_valid_namespace("room"));
/// assert!(is_valid_namespace("team.ops-1@eu"));
/// assert!(!is_valid_namespace("room:x"));
/// ```
#[must_use]
pub fn is_valid_namespace(namespace: &str) -> bool {
  (1..=255).contains(&namespace.len())
    && namespace
      .bytes()
      .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/' | b'@'))
}

#[cfg(test)]
mod tests;
