//! The front end every back end shares: it validates the manifest, computes its fingerprint, works out
//! which named types a client sends in another form than it receives them, and hands out the names a
//! back end derives, refusing two parts that would get one name.

use std::collections::{BTreeMap, BTreeSet};

use pavia_contract::fingerprint::jcs;
use pavia_contract::manifest::{Manifest, ManifestError};
use pavia_contract::schema::{Definition, Enum, Field, Primitive, Struct, Type, Variant};
use thiserror::Error;

/// Why a back end could not generate code for a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum GenerateError {
  /// The manifest breaks a rule of §12.2, so it has no fingerprint.
  #[error("the manifest is not valid: {}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
  Invalid(Vec<ManifestError>),
  /// A name in the manifest is one the target language or the generated code reserves.
  #[error("{what} `{name}` is a name {target} code reserves")]
  Reserved {
    /// What the name names: "type", "server method of service `chat`".
    what: String,
    /// The name.
    name: String,
    /// The target language.
    target: &'static str,
  },
  /// A type or definition in a form this generator does not know: one a newer contract model added.
  #[error("{form} is in a form this generator does not know")]
  Unsupported {
    /// The type or definition, as manifest JSON.
    form: String,
  },
  /// A named type that is an option of itself through aliases and options alone, such as
  /// `A = option<A>`: its only value is null, and the target language cannot declare it.
  #[error("type `{name}` is an option of itself, which {target} code cannot declare")]
  OptionCycle {
    /// A type on the cycle.
    name: String,
    /// The target language.
    target: &'static str,
  },
  /// Two parts of the manifest would get one name in the generated code.
  #[error("`{name}` would name both {first} and {second}")]
  Collision {
    /// The generated name.
    name: String,
    /// What claimed it first.
    first: String,
    /// What claimed it second.
    second: String,
  },
}

/// What the front end works out once for a manifest.
pub(crate) struct Front<'m> {
  pub(crate) manifest: &'m Manifest,
  pub(crate) fingerprint: [u8; 32],
  inputs: BTreeSet<&'m str>,
}

impl<'m> Front<'m> {
  pub(crate) fn new(manifest: &'m Manifest) -> Result<Self, GenerateError> {
    let fingerprint = manifest.fingerprint().map_err(GenerateError::Invalid)?;
    Ok(Self {
      manifest,
      fingerprint,
      inputs: inputs(manifest)?,
    })
  }

  /// Whether a client sends the named type `name` in another form than it receives it: the type is
  /// an open enum, whose decoded values include the catch-all no sender may use (§12.1), or reaches
  /// one through its parts.
  pub(crate) fn has_input_form(&self, name: &str) -> bool {
    self.inputs.contains(name)
  }
}

/// A contract type in one of the forms of §12.1 this generator knows. The model's own enums may gain
/// forms; a back end matches on this one, so a form it does not know fails generation instead.
pub(crate) enum Form<'t> {
  Primitive(Primitive),
  Option(&'t Type),
  List(&'t Type),
  Map(&'t Type),
  Tuple(&'t [Type]),
  Struct(&'t Struct),
  Ref(&'t str),
}

/// `ty` in a form this generator knows.
pub(crate) fn form(ty: &Type) -> Result<Form<'_>, GenerateError> {
  Ok(match ty {
    Type::Primitive(primitive) => Form::Primitive(*primitive),
    Type::Option(inner) => Form::Option(inner),
    Type::List(inner) => Form::List(inner),
    Type::Map(inner) => Form::Map(inner),
    Type::Tuple(items) => Form::Tuple(items),
    Type::Struct(fields) => Form::Struct(fields),
    Type::Ref(name) => Form::Ref(name),
    _ => return Err(unsupported(&ty.to_json())),
  })
}

/// A named type's definition in one of the forms of §12.2 this generator knows.
pub(crate) enum Body<'d> {
  Struct(&'d Struct),
  Enum(&'d Enum),
  Alias(&'d Type),
}

/// `definition` in a form this generator knows.
pub(crate) fn body(definition: &Definition) -> Result<Body<'_>, GenerateError> {
  Ok(match definition {
    Definition::Struct(fields) => Body::Struct(fields),
    Definition::Enum(variants) => Body::Enum(variants),
    Definition::Alias(target) => Body::Alias(target),
    _ => return Err(unsupported(&definition.to_json())),
  })
}

impl<'d> Body<'d> {
  /// The types the definition holds: its fields' types, its variants' value types or its target.
  pub(crate) fn types(&self) -> Vec<&'d Type> {
    match self {
      Self::Struct(fields) => fields.fields().iter().map(Field::ty).collect(),
      Self::Enum(variants) => variants.variants().iter().filter_map(Variant::ty).collect(),
      Self::Alias(target) => vec![target],
    }
  }
}

/// Hands `visit` the form of `ty` and of every type inside it, outermost first; a named type's
/// definition is not entered. The manifest's depth bound (§12.2) bounds the recursion.
pub(crate) fn walk<'t>(
  ty: &'t Type,
  visit: &mut impl FnMut(&Form<'t>) -> Result<(), GenerateError>,
) -> Result<(), GenerateError> {
  let form = form(ty)?;
  visit(&form)?;
  match form {
    Form::Primitive(_) | Form::Ref(_) => Ok(()),
    Form::Option(inner) | Form::List(inner) | Form::Map(inner) => walk(inner, visit),
    Form::Tuple(items) => items.iter().try_for_each(|item| walk(item, visit)),
    Form::Struct(fields) => fields.fields().iter().try_for_each(|field| walk(field.ty(), visit)),
  }
}

pub(crate) fn unsupported(value: &serde_json::Value) -> GenerateError {
  GenerateError::Unsupported { form: jcs(value) }
}

/// The named types that reach an open enum, the open enums included: a walk back from each open
/// enum along the references to it, which visits each name once, so a cycle of references ends.
fn inputs(manifest: &Manifest) -> Result<BTreeSet<&str>, GenerateError> {
  let mut users: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
  let mut pending = Vec::new();
  for (name, definition) in manifest.type_table() {
    let body = body(definition)?;
    if matches!(body, Body::Enum(variants) if variants.is_open()) {
      pending.push(name.as_str());
    }
    for ty in body.types() {
      walk(ty, &mut |form| {
        if let Form::Ref(used) = form {
          users.entry(used).or_default().push(name.as_str());
        }
        Ok(())
      })?;
    }
  }
  let mut inputs = BTreeSet::new();
  while let Some(name) = pending.pop() {
    if inputs.insert(name) {
      pending.extend(users.get(name).into_iter().flatten());
    }
  }
  Ok(inputs)
}

/// `name` with each part between underscores capitalized and the underscores dropped: `system_notice`
/// gives `SystemNotice`, the form a back end derives class names from.
pub(crate) fn pascal(name: &str) -> String {
  name
    .split('_')
    .flat_map(|part| {
      let mut chars = part.chars();
      chars.next().map(|first| first.to_ascii_uppercase()).into_iter().chain(chars)
    })
    .collect()
}

/// The top-level names of one generated file and what each names.
#[derive(Default)]
pub(crate) struct Names {
  taken: BTreeMap<String, String>,
}

impl Names {
  /// Takes `name` for `what`, or says what already has it.
  pub(crate) fn claim(&mut self, name: &str, what: impl Into<String>) -> Result<(), GenerateError> {
    let what = what.into();
    match self.taken.get(name) {
      Some(first) => Err(GenerateError::Collision {
        name: name.to_owned(),
        first: first.clone(),
        second: what,
      }),
      None => {
        self.taken.insert(name.to_owned(), what);
        Ok(())
      }
    }
  }

  /// Whether a top-level name is taken.
  pub(crate) fn contains(&self, name: &str) -> bool {
    self.taken.contains_key(name)
  }
}
