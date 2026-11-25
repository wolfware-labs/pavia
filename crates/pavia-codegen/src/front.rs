//! The front end every back end shares: it validates the manifest, computes its fingerprint, works out
//! which named types a client sends in another form than it receives them, and hands out the names a
//! back end derives, refusing two parts that would get one name.

use std::collections::{BTreeMap, BTreeSet};

use pavia_contract::manifest::{Manifest, ManifestError};
use pavia_contract::schema::{Definition, Enum, Primitive, Struct, Type};
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

pub(crate) fn unsupported(json: &serde_json::Value) -> GenerateError {
  GenerateError::Unsupported {
    form: serde_json::to_string(json).expect("a JSON value serializes"),
  }
}

/// The named types that reach an open enum, the open enums included: grown until no definition
/// adds one, so a cycle of references ends.
fn inputs(manifest: &Manifest) -> Result<BTreeSet<&str>, GenerateError> {
  let mut inputs = BTreeSet::new();
  let mut uses = BTreeMap::new();
  for (name, definition) in manifest.type_table() {
    let mut names = Vec::new();
    match body(definition)? {
      Body::Struct(fields) => {
        for field in fields.fields() {
          refs(field.ty(), &mut names)?;
        }
      }
      Body::Enum(variants) => {
        if variants.is_open() {
          inputs.insert(name.as_str());
        }
        for ty in variants.variants().iter().filter_map(|variant| variant.ty()) {
          refs(ty, &mut names)?;
        }
      }
      Body::Alias(target) => refs(target, &mut names)?,
    }
    uses.insert(name.as_str(), names);
  }
  loop {
    let added: Vec<&str> = uses
      .iter()
      .filter(|(name, used)| !inputs.contains(*name) && used.iter().any(|used| inputs.contains(used)))
      .map(|(name, _)| *name)
      .collect();
    if added.is_empty() {
      return Ok(inputs);
    }
    inputs.extend(added);
  }
}

fn refs<'t>(ty: &'t Type, names: &mut Vec<&'t str>) -> Result<(), GenerateError> {
  match form(ty)? {
    Form::Primitive(_) => {}
    Form::Option(inner) | Form::List(inner) | Form::Map(inner) => refs(inner, names)?,
    Form::Tuple(items) => {
      for item in items {
        refs(item, names)?;
      }
    }
    Form::Struct(fields) => {
      for field in fields.fields() {
        refs(field.ty(), names)?;
      }
    }
    Form::Ref(name) => names.push(name),
  }
  Ok(())
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
