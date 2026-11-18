//! The `#[pavia(...)]` attributes on a type, a field or a variant.

use proc_macro2::Span;
use syn::meta::ParseNestedMeta;
use syn::spanned::Spanned;
use syn::{Attribute, LitStr};

/// A `rename = "..."`: the name, and where it was written.
pub struct Rename {
  pub value: String,
  pub span: Span,
}

/// The attributes of one item.
#[derive(Default)]
pub struct Attrs {
  pub rename: Option<Rename>,
  pub optional: Option<Span>,
  pub open: Option<Span>,
}

impl Attrs {
  /// The `#[pavia(...)]` attributes in `attrs`; any other key is an error.
  pub fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
    let mut parsed = Self::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("pavia")) {
      attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("rename") {
          let value: LitStr = meta.value()?.parse()?;
          refuse_repeated(parsed.rename.is_some(), &meta)?;
          parsed.rename = Some(Rename {
            value: value.value(),
            span: value.span(),
          });
        } else if meta.path.is_ident("optional") {
          refuse_repeated(parsed.optional.is_some(), &meta)?;
          parsed.optional = Some(meta.path.span());
        } else if meta.path.is_ident("open") {
          refuse_repeated(parsed.open.is_some(), &meta)?;
          parsed.open = Some(meta.path.span());
        } else {
          return Err(meta.error("unknown `pavia` attribute: expected `rename`, `optional` or `open`"));
        }
        Ok(())
      })?;
    }
    Ok(parsed)
  }

  /// An error when `optional` is present, which `what` does not take.
  pub fn refuse_optional(&self, what: &str) -> syn::Result<()> {
    refuse(
      self.optional,
      &format!("`optional` applies to a named field of type `Option<T>`, not to {what}"),
    )
  }

  /// An error when `open` is present, which `what` does not take.
  pub fn refuse_open(&self, what: &str) -> syn::Result<()> {
    refuse(self.open, &format!("`open` applies to an enum, not to {what}"))
  }
}

fn refuse_repeated(seen: bool, meta: &ParseNestedMeta<'_>) -> syn::Result<()> {
  if seen {
    let key = meta.path.require_ident()?;
    Err(meta.error(format!("`{key}` is given twice")))
  } else {
    Ok(())
  }
}

fn refuse(span: Option<Span>, message: &str) -> syn::Result<()> {
  span.map_or(Ok(()), |span| Err(syn::Error::new(span, message)))
}
