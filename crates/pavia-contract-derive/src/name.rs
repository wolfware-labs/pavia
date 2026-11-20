//! The manifest name of a type, field or variant, checked against the grammar of §12.2.
//!
//! `pavia_contract::name::is_valid` holds the same grammar; a proc-macro crate depends on nothing else in
//! the workspace, so the check is repeated here to report it at compile time.

use proc_macro2::Span;
use syn::Ident;
use syn::ext::IdentExt;

use crate::attr::Rename;

/// The manifest name of `ident`: its rename if it has one, else the identifier without any `r#`.
pub fn of(ident: &Ident, rename: Option<&Rename>) -> syn::Result<String> {
  let (name, span) = match rename {
    Some(rename) => (rename.value.clone(), rename.span),
    None => (ident.unraw().to_string(), ident.span()),
  };
  check(&name, span)?;
  Ok(name)
}

fn check(name: &str, span: Span) -> syn::Result<()> {
  if is_valid(name) {
    Ok(())
  } else {
    Err(syn::Error::new(
      span,
      format!("`{name}` is not a contract name: names match [A-Za-z_][A-Za-z0-9_]* (spec 12.2)"),
    ))
  }
}

fn is_valid(name: &str) -> bool {
  let mut bytes = name.bytes();
  bytes.next().is_some_and(|first| first == b'_' || first.is_ascii_alphabetic())
    && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests;
