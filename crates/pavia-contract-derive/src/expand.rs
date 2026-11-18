//! The `Contract` and `OptionValue` impls `#[derive(Contract)]` writes.
//!
//! Each field type is checked where its contract type is added, `table.add::<T>()`, spanned at the field,
//! so an unsupported type (`HashMap<u32, T>`, `Option<()>`) fails there with the marker trait's message.
//! A type parameter is bounded by `OptionValue`, so `Option<T>`, `Vec<T>` and `Box<T>` fields need no
//! further bound; a recursive generic type would loop on a where clause per field.

use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Data, DataEnum, DeriveInput, Fields, FieldsNamed, FieldsUnnamed, GenericParam, Ident, Type, parse_quote};

use crate::attr::Attrs;
use crate::name;

/// The impls for `input`, or the first thing that keeps it from being a contract type.
pub fn contract(input: &DeriveInput) -> syn::Result<TokenStream> {
  let attrs = Attrs::parse(&input.attrs)?;
  attrs.refuse_optional("a type")?;
  let name = name::of(&input.ident, attrs.rename.as_ref())?;
  let definition = match &input.data {
    Data::Struct(data) => {
      attrs.refuse_open("a struct")?;
      structure(&data.fields)?
    }
    Data::Enum(data) => enumeration(data, attrs.open.is_some())?,
    Data::Union(data) => {
      return Err(syn::Error::new(
        data.union_token.span(),
        "a union has no contract type (spec 12.1)",
      ));
    }
  };

  let mut generics = input.generics.clone();
  let mut arguments = Vec::new();
  for param in &mut generics.params {
    match param {
      GenericParam::Type(param) => {
        param.bounds.push(parse_quote!(::pavia_contract::OptionValue));
        arguments.push(param.ident.clone());
      }
      GenericParam::Lifetime(param) => {
        return Err(syn::Error::new(
          param.span(),
          "a contract type cannot borrow: lifetime parameters are not supported",
        ));
      }
      GenericParam::Const(param) => {
        return Err(syn::Error::new(
          param.span(),
          "const parameters are not supported: a monomorphized name is built from type arguments (spec 12.1)",
        ));
      }
    }
  }
  let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
  let option_value_where = alias_target(&input.data).map(|target| {
    let mut clause = where_clause.cloned().unwrap_or_else(|| parse_quote!(where));
    clause
      .predicates
      .push(parse_quote!(for<'pavia> #target: ::pavia_contract::OptionValue));
    clause
  });
  let option_value_where = option_value_where.as_ref().or(where_clause);
  let ident = &input.ident;
  let type_name = monomorphized(&name, &arguments);

  Ok(quote! {
    #[automatically_derived]
    impl #impl_generics ::pavia_contract::Contract for #ident #type_generics #where_clause {
      fn name() -> ::std::string::String {
        #type_name
      }

      fn contract_type(table: &mut ::pavia_contract::TypeTable) -> ::pavia_contract::schema::Type {
        table.define::<Self>(|#[allow(unused_variables)] table| #definition)
      }
    }

    #[automatically_derived]
    impl #impl_generics ::pavia_contract::OptionValue for #ident #type_generics #option_value_where {}
  })
}

/// The aliased type of a one-field tuple struct, which is an `Option` value only when its target is one:
/// `struct Nothing(())` aliases `unit`, so `Option<Nothing>` would be `{"option": "unit"}` (§12.1). The bound
/// is higher-ranked so that a target that is never an `Option` value fails where the alias is wrapped, not
/// at the derive.
fn alias_target(data: &Data) -> Option<&Type> {
  match data {
    Data::Struct(data) => match &data.fields {
      Fields::Unnamed(fields) if fields.unnamed.len() == 1 => fields.unnamed.first().map(|field| &field.ty),
      _ => None,
    },
    _ => None,
  }
}

fn monomorphized(name: &str, arguments: &[Ident]) -> TokenStream {
  if arguments.is_empty() {
    return quote!(::std::string::String::from(#name));
  }
  quote! {
    let mut name = ::std::string::String::from(#name);
    #(
      name.push('_');
      name.push_str(&<#arguments as ::pavia_contract::Contract>::name());
    )*
    name
  }
}

fn structure(fields: &Fields) -> syn::Result<TokenStream> {
  Ok(match fields {
    Fields::Named(fields) => {
      let definition = named(fields)?;
      quote!(::pavia_contract::schema::Definition::Struct(#definition))
    }
    Fields::Unnamed(fields) => {
      let target = unnamed(fields)?;
      quote!(::pavia_contract::schema::Definition::Alias(#target))
    }
    Fields::Unit => quote!(::pavia_contract::schema::Definition::Struct(
      ::pavia_contract::schema::Struct::new(::std::vec::Vec::new())
    )),
  })
}

fn enumeration(data: &DataEnum, open: bool) -> syn::Result<TokenStream> {
  let mut names = Vec::new();
  let mut variants = Vec::new();
  for variant in &data.variants {
    let attrs = Attrs::parse(&variant.attrs)?;
    attrs.refuse_optional("a variant")?;
    attrs.refuse_open("a variant")?;
    let name = name::of(&variant.ident, attrs.rename.as_ref())?;
    unique(&mut names, &name, variant.ident.span(), "variants")?;
    variants.push(match &variant.fields {
      Fields::Unit => quote!(::pavia_contract::schema::Variant::unit(#name)),
      Fields::Unnamed(fields) => {
        let payload = unnamed(fields)?;
        quote!(::pavia_contract::schema::Variant::new(#name, #payload))
      }
      Fields::Named(fields) => {
        let payload = named(fields)?;
        quote!(::pavia_contract::schema::Variant::new(#name, ::pavia_contract::schema::Type::Struct(#payload)))
      }
    });
  }
  let open = open.then(|| quote!(.open()));
  Ok(quote! {
    ::pavia_contract::schema::Definition::Enum(::pavia_contract::schema::Enum::new(::std::vec![#(#variants),*])#open)
  })
}

fn named(fields: &FieldsNamed) -> syn::Result<TokenStream> {
  let mut names = Vec::new();
  let mut items = Vec::new();
  for (field, ident) in fields
    .named
    .iter()
    .filter_map(|field| field.ident.as_ref().map(|ident| (field, ident)))
  {
    let attrs = Attrs::parse(&field.attrs)?;
    attrs.refuse_open("a field")?;
    let name = name::of(ident, attrs.rename.as_ref())?;
    unique(&mut names, &name, ident.span(), "fields")?;
    let ty = &field.ty;
    let added = add(ty);
    items.push(if attrs.optional.is_some() {
      quote_spanned! {ty.span()=>
        ::pavia_contract::schema::Field::new(#name, {
          fn optional<T: ::pavia_contract::OptionField + ?::std::marker::Sized>() {}
          optional::<#ty>();
          #added
        })
        .optional()
      }
    } else {
      quote!(::pavia_contract::schema::Field::new(#name, #added))
    });
  }
  Ok(quote!(::pavia_contract::schema::Struct::new(::std::vec![#(#items),*])))
}

fn unnamed(fields: &FieldsUnnamed) -> syn::Result<TokenStream> {
  if fields.unnamed.is_empty() {
    return Err(syn::Error::new(
      fields.paren_token.span.join(),
      "empty parentheses have no contract type: drop them for a unit struct or variant (spec 12.1)",
    ));
  }
  let mut items = Vec::new();
  for field in &fields.unnamed {
    let attrs = Attrs::parse(&field.attrs)?;
    attrs.refuse_optional("a tuple field")?;
    attrs.refuse_open("a field")?;
    if let Some(rename) = &attrs.rename {
      return Err(syn::Error::new(
        rename.span,
        "`rename` applies to a named field: a tuple field has no name",
      ));
    }
    items.push(add(&field.ty));
  }
  Ok(match items.as_slice() {
    [single] => single.clone(),
    _ => quote!(::pavia_contract::schema::Type::Tuple(::std::vec![#(#items),*])),
  })
}

fn add(ty: &Type) -> TokenStream {
  quote_spanned!(ty.span()=> table.add::<#ty>())
}

fn unique(names: &mut Vec<String>, name: &str, span: Span, what: &str) -> syn::Result<()> {
  if names.iter().any(|seen| seen == name) {
    return Err(syn::Error::new(span, format!("two {what} are named `{name}`")));
  }
  names.push(name.to_owned());
  Ok(())
}
