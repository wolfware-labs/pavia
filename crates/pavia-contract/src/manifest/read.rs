//! Reading a manifest back from its JSON text (§12.2), for tools that take a manifest written elsewhere:
//! a file exported by an application, or one a server published.
//!
//! The text may come from anyone, so [`Manifest::read`] bounds it before it trusts it: its size, the
//! depth of every type ([`MAX_TYPE_DEPTH`], the bound [`Manifest::validate`] keeps too), and one value per
//! object key. serde_json's own nesting limit bounds every other value; a manifest within those bounds
//! nests far less. It checks the format version before anything else, since a newer format may add keys.
//! It then builds the model and runs [`Manifest::validate`] on it, so what it returns is a manifest
//! [`Manifest::render`] accepts. It reads what the writer omits as its default, accepts a default written
//! out, and takes named entries in any order; it refuses `null` for an optional field, a key §12.2 does not
//! define, and a format version other than [`FORMAT_VERSION`]. `docs` is read where the model keeps it and
//! checked, then skipped, elsewhere: §12.2 lets any object carry one, and it is outside the fingerprint
//! (§12.5).
//!
//! Where a problem is, [`ReadError`] says as a path from the root `$`, in the form of
//! [`FingerprintError`](crate::fingerprint::FingerprintError): `$.services[0].server_methods[1].shape`.
//! Each level adds its step on the way back up, so a valid document never pays for a location.

use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use thiserror::Error;

use super::{
  Call, Channel, Datagram, Direction, FIELD_STEP, FORMAT_VERSION, INNER_STEP, ITEM_STEP, MAX_MANIFEST_BYTES, MAX_TYPE_DEPTH,
  Manifest, ManifestError, Method, Mode, Param, Service, Shape, shown,
};
use crate::schema::{Definition, Enum, Field, Primitive, Struct, Type, Types, Variant};

/// Why [`Manifest::read`] refused a text.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ReadError {
  /// The text is longer than [`MAX_MANIFEST_BYTES`].
  #[error("the manifest is {size} bytes, more than the {limit} a manifest may have")]
  TooLarge {
    /// The text's length in bytes.
    size: usize,
    /// [`MAX_MANIFEST_BYTES`].
    limit: usize,
  },
  /// The text is not one JSON value, or an object in it has one key twice.
  #[error("the manifest is not JSON: {0}")]
  Json(String),
  /// A part of the document does not have the form §12.2 gives it.
  #[error("{at}: {problem}")]
  Form {
    /// Where, as a path from the root: `$.services[0].server_methods[1].shape`.
    at: String,
    /// What is wrong there.
    problem: String,
  },
  /// A type nests deeper than [`MAX_TYPE_DEPTH`].
  #[error("{at}: types nest more than {limit} deep")]
  TooDeep {
    /// Where the type is, as a path from the root.
    at: String,
    /// [`MAX_TYPE_DEPTH`].
    limit: usize,
  },
  /// The top-level `pavia` is not [`FORMAT_VERSION`].
  #[error("the manifest is format version {found}, and this reader reads version {FORMAT_VERSION}")]
  Version {
    /// The `pavia` value as serde_json writes it back (`1.5`, `"1"`), cut and escaped as
    /// [`MAX_NAME_BYTES`](super::MAX_NAME_BYTES) says.
    found: String,
  },
  /// The document has the form §12.2 gives it, but [`Manifest::validate`] refuses what it says.
  #[error("the manifest is not valid: {}", joined(.0))]
  Invalid(Vec<ManifestError>),
}

impl ReadError {
  /// A problem with the value at the root of the walk; the levels above add their steps.
  fn form(problem: impl Into<String>) -> Self {
    Self::Form {
      at: "$".to_owned(),
      problem: problem.into(),
    }
  }

  /// A problem with the value under `key`.
  fn form_at(key: &str, problem: impl Into<String>) -> Self {
    Self::form(problem).under(&format!(".{}", shown(key)))
  }

  /// The same problem, one `step` further from the root.
  fn under(mut self, step: &str) -> Self {
    if let Self::Form { at, .. } | Self::TooDeep { at, .. } = &mut self {
      at.insert_str(1, step);
    }
    self
  }
}

/// Moves a problem found in the value under `key` to its place in the object.
fn under_key(key: &str) -> impl FnOnce(ReadError) -> ReadError + '_ {
  move |error| error.under(&format!(".{}", shown(key)))
}

fn joined(errors: &[ManifestError]) -> String {
  errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")
}

/// `names` as a list for a message: "`a`, `b`, `c`".
fn quoted<'n>(names: impl IntoIterator<Item = &'n str>) -> String {
  names
    .into_iter()
    .map(|name| format!("`{name}`"))
    .collect::<Vec<_>>()
    .join(", ")
}

/// The item of `all` whose manifest name is `text`.
fn named<T: Copy>(text: &str, all: &[T], name: fn(T) -> &'static str) -> Option<T> {
  all.iter().copied().find(|item| name(*item) == text)
}

impl Manifest {
  /// Reads a manifest from its JSON text and validates it.
  ///
  /// # Errors
  ///
  /// [`ReadError`]: a text above [`MAX_MANIFEST_BYTES`], one that is not JSON or repeats a key in an
  /// object, a part not in the form §12.2 gives it, a type nested deeper than [`MAX_TYPE_DEPTH`], another
  /// format version, or the errors of [`Manifest::validate`].
  pub fn read(text: &str) -> Result<Self, ReadError> {
    if text.len() > MAX_MANIFEST_BYTES {
      return Err(ReadError::TooLarge {
        size: text.len(),
        limit: MAX_MANIFEST_BYTES,
      });
    }
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let Strict(document) = Strict::deserialize(&mut deserializer).map_err(|error| ReadError::Json(error.to_string()))?;
    deserializer.end().map_err(|error| ReadError::Json(error.to_string()))?;
    let manifest = manifest(&document)?;
    manifest.validate().map_err(ReadError::Invalid)?;
    Ok(manifest)
  }
}

fn manifest(document: &Value) -> Result<Manifest, ReadError> {
  let top = Object::new(document)?;
  let version = top.required("pavia")?;
  if version.as_u64() != Some(FORMAT_VERSION) {
    return Err(ReadError::Version {
      found: shown(&version.to_string()).to_string(),
    });
  }
  top.keys(&["pavia", "name", "docs", "services", "channels", "datagrams", "types"])?;
  let mut manifest = Manifest::new(top.string("name")?);
  manifest.docs = top.docs()?;
  manifest.services = top.list("services", service)?;
  manifest.channels = top.list("channels", channel)?;
  manifest.datagrams = top.list("datagrams", datagram)?;
  manifest.types = match top.optional("types")? {
    None => Types::new(),
    Some(value) => types(value).map_err(under_key("types"))?,
  };
  Ok(manifest)
}

fn types(value: &Value) -> Result<Types, ReadError> {
  Object::new(value)?
    .map
    .iter()
    .map(|(name, value)| Ok((name.clone(), definition(value).map_err(under_key(name))?)))
    .collect()
}

fn service(value: &Value) -> Result<Service, ReadError> {
  let object = Object::new(value)?;
  object.keys(&["name", "docs", "server_methods", "client_methods"])?;
  let mut service = Service::new(object.string("name")?);
  service.docs = object.docs()?;
  service.server_methods = object.list("server_methods", method)?;
  service.client_methods = object.list("client_methods", method)?;
  Ok(service)
}

fn method(value: &Value) -> Result<Method, ReadError> {
  let object = Object::new(value)?;
  let call = match object.text("kind")? {
    "notify" => {
      object.keys(&["name", "docs", "params", "kind"])?;
      None
    }
    "call" => {
      object.keys(&[
        "name",
        "docs",
        "params",
        "kind",
        "shape",
        "accepts",
        "yields",
        "returns",
        "error",
        "idempotent",
      ])?;
      Some(call(&object)?)
    }
    _ => {
      return Err(ReadError::form_at(
        "kind",
        format!("is not one of {}", quoted(["call", "notify"])),
      ));
    }
  };
  Ok(Method {
    name: object.string("name")?,
    docs: object.docs()?,
    params: object.list("params", param)?,
    call,
  })
}

fn call(object: &Object<'_>) -> Result<Call, ReadError> {
  let shape = object.choice("shape", &Shape::ALL, Shape::name)?;
  let returns = || -> Result<Type, ReadError> { Ok(object.ty("returns", 1)?.unwrap_or(Type::Primitive(Primitive::Unit))) };
  let mut call = match shape {
    Shape::Unary => {
      object.absent(&["accepts", "yields"], shape)?;
      Call::unary(returns()?)
    }
    Shape::ServerStream => {
      object.absent(&["accepts", "returns"], shape)?;
      Call::server_stream(object.required_ty("yields", 1)?)
    }
    Shape::ClientStream => {
      object.absent(&["yields"], shape)?;
      Call::client_stream(object.required_ty("accepts", 1)?, returns()?)
    }
    Shape::Bidi => {
      object.absent(&["returns"], shape)?;
      Call::bidi(object.required_ty("accepts", 1)?, object.required_ty("yields", 1)?)
    }
  };
  call.error = object.ty("error", 1)?;
  call.idempotent = object.flag("idempotent")?;
  Ok(call)
}

fn param(value: &Value) -> Result<Param, ReadError> {
  let object = Object::new(value)?;
  object.keys(&["name", "type", "docs"])?;
  Ok(Param {
    name: object.string("name")?,
    ty: object.required_ty("type", 1)?,
    docs: object.docs()?,
  })
}

fn channel(value: &Value) -> Result<Channel, ReadError> {
  let object = Object::new(value)?;
  object.keys(&[
    "namespace",
    "docs",
    "publication",
    "presence_info",
    "snapshot",
    "subscribe",
    "history",
  ])?;
  Ok(Channel {
    namespace: object.string("namespace")?,
    docs: object.docs()?,
    publication: object.required_ty("publication", 1)?,
    presence_info: object.ty("presence_info", 1)?,
    snapshot: object.ty("snapshot", 1)?,
    subscribe: object.ty("subscribe", 1)?,
    history: object.flag("history")?,
  })
}

fn datagram(value: &Value) -> Result<Datagram, ReadError> {
  let object = Object::new(value)?;
  object.keys(&["topic", "docs", "direction", "mode", "payload"])?;
  let mut datagram = Datagram::new(
    object.string("topic")?,
    object.choice("direction", &Direction::ALL, Direction::name)?,
    object.choice("mode", &Mode::ALL, Mode::name)?,
    object.required_ty("payload", 1)?,
  );
  datagram.docs = object.docs()?;
  Ok(datagram)
}

fn definition(value: &Value) -> Result<Definition, ReadError> {
  let object = Object::new(value)?;
  let form = object.form(&["struct", "enum", "alias"])?;
  let inner = object.required(form)?;
  match form {
    "struct" => structure(inner, 1).map(Definition::Struct),
    "enum" => enumeration(inner, 1).map(Definition::Enum),
    _ => ty(inner, 1).map(Definition::Alias),
  }
  .map_err(under_key(form))
}

/// An enum body, whose variant types sit at nesting `depth`.
fn enumeration(value: &Value, depth: usize) -> Result<Enum, ReadError> {
  let object = Object::new(value)?;
  object.keys(&["variants", "open", "docs"])?;
  let variants = object.list("variants", |value| {
    let variant = Object::new(value)?;
    variant.keys(&["name", "type", "docs"])?;
    let name = variant.string("name")?;
    Ok(match variant.ty("type", depth)? {
      None => Variant::unit(name),
      Some(ty) => Variant::new(name, ty),
    })
  })?;
  let definition = Enum::new(variants);
  Ok(if object.flag("open")? { definition.open() } else { definition })
}

/// A struct body, whose field types sit at nesting `depth`.
fn structure(value: &Value, depth: usize) -> Result<Struct, ReadError> {
  let object = Object::new(value)?;
  object.keys(&["fields", "docs"])?;
  let fields = object.list("fields", |value| {
    let field = Object::new(value)?;
    field.keys(&["name", "type", "optional", "docs"])?;
    let built = Field::new(field.string("name")?, field.required_ty("type", depth)?);
    Ok(if field.flag("optional")? { built.optional() } else { built })
  })?;
  Ok(Struct::new(fields))
}

fn ty(value: &Value, depth: usize) -> Result<Type, ReadError> {
  if depth > MAX_TYPE_DEPTH {
    return Err(ReadError::TooDeep {
      at: "$".to_owned(),
      limit: MAX_TYPE_DEPTH,
    });
  }
  if let Value::String(name) = value {
    return named(name, &Primitive::ALL, Primitive::name)
      .map(Type::Primitive)
      .ok_or_else(|| ReadError::form(format!("`{}` is not a primitive type", shown(name))));
  }
  let object = Object::new(value)?;
  let form = object.form(&["option", "list", "map", "tuple", "struct", "ref"])?;
  let inner = object.required(form)?;
  match form {
    "option" => ty(inner, depth + INNER_STEP).map(|inner| Type::Option(Box::new(inner))),
    "list" => ty(inner, depth + INNER_STEP).map(|inner| Type::List(Box::new(inner))),
    "map" => ty(inner, depth + INNER_STEP).map(|inner| Type::Map(Box::new(inner))),
    "tuple" => match inner {
      Value::Array(items) => each(items, |item| ty(item, depth + ITEM_STEP)).map(Type::Tuple),
      _ => Err(ReadError::form("is not an array")),
    },
    "struct" => structure(inner, depth + FIELD_STEP).map(Type::Struct),
    _ => match inner {
      Value::String(name) => Ok(Type::Ref(name.clone())),
      _ => Err(ReadError::form("is not a string")),
    },
  }
  .map_err(under_key(form))
}

/// Reads every item of an array, and says which item a problem is in.
fn each<T>(items: &[Value], read: impl Fn(&Value) -> Result<T, ReadError>) -> Result<Vec<T>, ReadError> {
  items
    .iter()
    .enumerate()
    .map(|(index, item)| read(item).map_err(|error| error.under(&format!("[{index}]"))))
    .collect()
}

/// An object of the document, with the checks every part of the reader makes. Its problems are at `$`
/// or under one of its keys; the caller moves them to the object's place.
struct Object<'a> {
  map: &'a Map<String, Value>,
}

impl<'a> Object<'a> {
  fn new(value: &'a Value) -> Result<Self, ReadError> {
    match value {
      Value::Object(map) => Ok(Self { map }),
      _ => Err(ReadError::form("is not an object")),
    }
  }

  /// Refuses a key outside `allowed`, and a `docs` that is not a string, which any object may carry
  /// (§12.2) and every `allowed` lists.
  fn keys(&self, allowed: &[&str]) -> Result<(), ReadError> {
    if let Some(key) = self.map.keys().find(|key| !allowed.contains(&key.as_str())) {
      return Err(ReadError::form_at(key, "is not a key of this object (spec 12.2)"));
    }
    self.docs().map(drop)
  }

  fn absent(&self, keys: &[&str], shape: Shape) -> Result<(), ReadError> {
    match keys.iter().find(|key| self.map.contains_key(**key)) {
      Some(key) => Err(ReadError::form_at(key, format!("is not a key of a `{}` call", shape.name()))),
      None => Ok(()),
    }
  }

  /// The one key of `forms` the object has, beside an optional `docs` string.
  fn form(&self, forms: &[&'static str]) -> Result<&'static str, ReadError> {
    self.docs()?;
    let mut keys = self.map.keys().filter(|key| key.as_str() != "docs");
    match (keys.next(), keys.next()) {
      (Some(key), None) => named(key, forms, |form| form)
        .ok_or_else(|| ReadError::form_at(key, format!("is not one of {}", quoted(forms.iter().copied())))),
      _ => Err(ReadError::form(format!(
        "does not have exactly one of {}",
        quoted(forms.iter().copied())
      ))),
    }
  }

  fn optional(&self, key: &str) -> Result<Option<&'a Value>, ReadError> {
    match self.map.get(key) {
      Some(Value::Null) => Err(ReadError::form_at(
        key,
        "is `null`, which is not allowed: leave the key out (spec 12.2)",
      )),
      found => Ok(found),
    }
  }

  fn required(&self, key: &str) -> Result<&'a Value, ReadError> {
    self.optional(key)?.ok_or_else(|| ReadError::form_at(key, "is missing"))
  }

  fn text(&self, key: &str) -> Result<&'a str, ReadError> {
    match self.required(key)? {
      Value::String(text) => Ok(text),
      _ => Err(ReadError::form_at(key, "is not a string")),
    }
  }

  fn string(&self, key: &str) -> Result<String, ReadError> {
    self.text(key).map(str::to_owned)
  }

  /// The item of `all` that `key` names.
  fn choice<T: Copy>(&self, key: &str, all: &[T], name: fn(T) -> &'static str) -> Result<T, ReadError> {
    named(self.text(key)?, all, name)
      .ok_or_else(|| ReadError::form_at(key, format!("is not one of {}", quoted(all.iter().map(|item| name(*item))))))
  }

  fn docs(&self) -> Result<Option<String>, ReadError> {
    match self.optional("docs")? {
      None => Ok(None),
      Some(Value::String(text)) => Ok(Some(text.clone())),
      Some(_) => Err(ReadError::form_at("docs", "is not a string")),
    }
  }

  fn flag(&self, key: &str) -> Result<bool, ReadError> {
    match self.optional(key)? {
      None => Ok(false),
      Some(Value::Bool(flag)) => Ok(*flag),
      Some(_) => Err(ReadError::form_at(key, "is not `true` or `false`")),
    }
  }

  /// The type under `key`, if any, at nesting `depth`.
  fn ty(&self, key: &str, depth: usize) -> Result<Option<Type>, ReadError> {
    self
      .optional(key)?
      .map(|value| ty(value, depth).map_err(under_key(key)))
      .transpose()
  }

  fn required_ty(&self, key: &str, depth: usize) -> Result<Type, ReadError> {
    ty(self.required(key)?, depth).map_err(under_key(key))
  }

  fn list<T>(&self, key: &str, read: impl Fn(&Value) -> Result<T, ReadError>) -> Result<Vec<T>, ReadError> {
    match self.optional(key)? {
      None => Ok(Vec::new()),
      Some(Value::Array(items)) => each(items, read).map_err(under_key(key)),
      Some(_) => Err(ReadError::form_at(key, "is not an array")),
    }
  }
}

/// A JSON value read with one value per object key: a repeated key is an error, not the last one
/// winning, so two readers of one manifest never see two different documents.
struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
  fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    deserializer.deserialize_any(StrictVisitor).map(Strict)
  }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
  type Value = Value;

  fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str("a JSON value")
  }

  fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
    Ok(Value::Bool(value))
  }

  fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
    Ok(Value::from(value))
  }

  fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
    Ok(Value::from(value))
  }

  fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
    Ok(Value::Number(
      Number::from_f64(value).expect("serde_json refuses a number out of the finite range"),
    ))
  }

  fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
    Ok(Value::from(value))
  }

  fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
    Ok(Value::Null)
  }

  fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
    let mut items = Vec::new();
    while let Some(Strict(item)) = seq.next_element()? {
      items.push(item);
    }
    Ok(Value::Array(items))
  }

  fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
    let mut object = Map::new();
    while let Some(key) = map.next_key::<String>()? {
      if object.contains_key(&key) {
        return Err(de::Error::custom(format!("an object has the key `{}` twice", shown(&key))));
      }
      let Strict(value) = map.next_value()?;
      object.insert(key, value);
    }
    Ok(Value::Object(object))
  }
}

#[cfg(test)]
mod tests;
