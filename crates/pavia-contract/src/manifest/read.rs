//! Reading a manifest back from its JSON text (§12.2), for tools that take a manifest written elsewhere:
//! a file exported by an application, or one a server published.
//!
//! The text may come from anyone, so [`Manifest::read`] bounds it before it trusts it: its size, the
//! depth of every type, and one value per object key. It then builds the model and runs
//! [`Manifest::validate`] on it, so what it returns is a manifest [`Manifest::render`] accepts. It reads
//! what the writer omits as its default, accepts a default written out, and takes named entries in any
//! order; it refuses `null` for an optional field, a key §12.2 does not define, and a format version
//! other than [`FORMAT_VERSION`]. `docs` is read where the model keeps it and skipped elsewhere: §12.2
//! lets any object carry one, and it is outside the fingerprint (§12.5).

use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use thiserror::Error;

use super::{Call, Channel, Datagram, Direction, FORMAT_VERSION, Manifest, ManifestError, Method, Mode, Param, Service};
use crate::schema::{Definition, Enum, Field, Primitive, Struct, Type, Types, Variant};

/// The largest manifest text [`Manifest::read`] takes, in bytes.
pub const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;

/// How deep [`Manifest::read`] lets types nest: `{"list": {"list": "u8"}}` is 3 deep.
pub const MAX_TYPE_DEPTH: usize = 32;

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
    /// Where, as a path from the top: `services[0].server_methods[1].shape`.
    at: String,
    /// What is wrong there.
    problem: String,
  },
  /// A type nests deeper than [`MAX_TYPE_DEPTH`].
  #[error("{at}: types nest more than {limit} deep")]
  TooDeep {
    /// Where the type is.
    at: String,
    /// [`MAX_TYPE_DEPTH`].
    limit: usize,
  },
  /// The top-level `pavia` is not [`FORMAT_VERSION`].
  #[error("the manifest is format version {found}, and this reader reads version {FORMAT_VERSION}")]
  Version {
    /// The `pavia` value as written.
    found: String,
  },
  /// The document has the form §12.2 gives it, but [`Manifest::validate`] refuses what it says.
  #[error("the manifest is not valid: {}", list(.0))]
  Invalid(Vec<ManifestError>),
}

fn list(errors: &[ManifestError]) -> String {
  errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")
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
  let top = Object::new(document, String::new())?;
  top.keys(&["pavia", "name", "docs", "services", "channels", "datagrams", "types"])?;
  let version = top.required("pavia")?;
  if version.as_u64() != Some(FORMAT_VERSION) {
    return Err(ReadError::Version {
      found: version.to_string(),
    });
  }
  let mut manifest = Manifest::new(top.string("name")?);
  manifest.docs = top.docs()?;
  manifest.services = top.list("services", service)?;
  manifest.channels = top.list("channels", channel)?;
  manifest.datagrams = top.list("datagrams", datagram)?;
  manifest.types = match top.optional("types")? {
    None => Types::new(),
    Some(value) => {
      let types = Object::new(value, top.at("types"))?;
      types
        .entries()
        .map(|(name, value)| Ok((name.clone(), definition(value, types.at(name))?)))
        .collect::<Result<_, ReadError>>()?
    }
  };
  Ok(manifest)
}

fn service(value: &Value, at: String) -> Result<Service, ReadError> {
  let object = Object::new(value, at)?;
  object.keys(&["name", "docs", "server_methods", "client_methods"])?;
  let mut service = Service::new(object.string("name")?);
  service.docs = object.docs()?;
  service.server_methods = object.list("server_methods", method)?;
  service.client_methods = object.list("client_methods", method)?;
  Ok(service)
}

fn method(value: &Value, at: String) -> Result<Method, ReadError> {
  let object = Object::new(value, at)?;
  let call = match object.string("kind")?.as_str() {
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
    _ => return Err(object.problem("kind", "is neither `call` nor `notify`")),
  };
  Ok(Method {
    name: object.string("name")?,
    docs: object.docs()?,
    params: object.list("params", param)?,
    call,
  })
}

fn call(object: &Object<'_>) -> Result<Call, ReadError> {
  let shape = object.string("shape")?;
  let returns = || -> Result<Type, ReadError> { Ok(object.ty("returns")?.unwrap_or(Type::Primitive(Primitive::Unit))) };
  let mut call = match shape.as_str() {
    "unary" => {
      object.absent(&["accepts", "yields"], "a `unary` call")?;
      Call::unary(returns()?)
    }
    "server_stream" => {
      object.absent(&["accepts", "returns"], "a `server_stream` call")?;
      Call::server_stream(object.required_ty("yields")?)
    }
    "client_stream" => {
      object.absent(&["yields"], "a `client_stream` call")?;
      Call::client_stream(object.required_ty("accepts")?, returns()?)
    }
    "bidi" => {
      object.absent(&["returns"], "a `bidi` call")?;
      Call::bidi(object.required_ty("accepts")?, object.required_ty("yields")?)
    }
    _ => return Err(object.problem("shape", "is not `unary`, `server_stream`, `client_stream` or `bidi`")),
  };
  call.error = object.ty("error")?;
  call.idempotent = object.flag("idempotent")?;
  Ok(call)
}

fn param(value: &Value, at: String) -> Result<Param, ReadError> {
  let object = Object::new(value, at)?;
  object.keys(&["name", "type", "docs"])?;
  Ok(Param {
    name: object.string("name")?,
    ty: object.required_ty("type")?,
    docs: object.docs()?,
  })
}

fn channel(value: &Value, at: String) -> Result<Channel, ReadError> {
  let object = Object::new(value, at)?;
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
    publication: object.required_ty("publication")?,
    presence_info: object.ty("presence_info")?,
    snapshot: object.ty("snapshot")?,
    subscribe: object.ty("subscribe")?,
    history: object.flag("history")?,
  })
}

fn datagram(value: &Value, at: String) -> Result<Datagram, ReadError> {
  let object = Object::new(value, at)?;
  object.keys(&["topic", "docs", "direction", "mode", "payload"])?;
  let direction = match object.string("direction")?.as_str() {
    "to_server" => Direction::ToServer,
    "to_client" => Direction::ToClient,
    "both" => Direction::Both,
    _ => return Err(object.problem("direction", "is not `to_server`, `to_client` or `both`")),
  };
  let mode = match object.string("mode")?.as_str() {
    "latest" => Mode::Latest,
    "all" => Mode::All,
    _ => return Err(object.problem("mode", "is neither `latest` nor `all`")),
  };
  let mut datagram = Datagram::new(object.string("topic")?, direction, mode, object.required_ty("payload")?);
  datagram.docs = object.docs()?;
  Ok(datagram)
}

fn definition(value: &Value, at: String) -> Result<Definition, ReadError> {
  let object = Object::new(value, at)?;
  let form = object.form(&["struct", "enum", "alias"])?;
  Ok(match form {
    "struct" => Definition::Struct(structure(object.required(form)?, object.at(form), 1)?),
    "enum" => Definition::Enum(enumeration(object.required(form)?, object.at(form))?),
    _ => Definition::Alias(ty(object.required(form)?, object.at(form), 1)?),
  })
}

fn enumeration(value: &Value, at: String) -> Result<Enum, ReadError> {
  let object = Object::new(value, at)?;
  object.keys(&["variants", "open", "docs"])?;
  let variants = object.list("variants", |value, at| {
    let variant = Object::new(value, at)?;
    variant.keys(&["name", "type", "docs"])?;
    let name = variant.string("name")?;
    Ok(match variant.optional("type")? {
      None => Variant::unit(name),
      Some(value) => Variant::new(name, ty(value, variant.at("type"), 1)?),
    })
  })?;
  let definition = Enum::new(variants);
  Ok(if object.flag("open")? { definition.open() } else { definition })
}

fn structure(value: &Value, at: String, depth: usize) -> Result<Struct, ReadError> {
  let object = Object::new(value, at)?;
  object.keys(&["fields", "docs"])?;
  let fields = object.list("fields", |value, at| {
    let field = Object::new(value, at)?;
    field.keys(&["name", "type", "optional", "docs"])?;
    let built = Field::new(
      field.string("name")?,
      ty(field.required("type")?, field.at("type"), depth + 1)?,
    );
    Ok(if field.flag("optional")? { built.optional() } else { built })
  })?;
  Ok(Struct::new(fields))
}

fn ty(value: &Value, at: String, depth: usize) -> Result<Type, ReadError> {
  if depth > MAX_TYPE_DEPTH {
    return Err(ReadError::TooDeep {
      at,
      limit: MAX_TYPE_DEPTH,
    });
  }
  if let Value::String(name) = value {
    return Primitive::ALL
      .into_iter()
      .find(|primitive| primitive.name() == name)
      .map(Type::Primitive)
      .ok_or_else(|| ReadError::Form {
        at,
        problem: format!("`{name}` is not a primitive type"),
      });
  }
  let object = Object::new(value, at)?;
  let form = object.form(&["option", "list", "map", "tuple", "struct", "ref"])?;
  let inner = object.required(form)?;
  let place = object.at(form);
  Ok(match form {
    "option" => Type::Option(Box::new(ty(inner, place, depth + 1)?)),
    "list" => Type::List(Box::new(ty(inner, place, depth + 1)?)),
    "map" => Type::Map(Box::new(ty(inner, place, depth + 1)?)),
    "tuple" => {
      let Value::Array(items) = inner else {
        return Err(ReadError::Form {
          at: place,
          problem: "is not an array".into(),
        });
      };
      Type::Tuple(
        items
          .iter()
          .enumerate()
          .map(|(index, item)| ty(item, format!("{place}[{index}]"), depth + 1))
          .collect::<Result<_, _>>()?,
      )
    }
    "struct" => Type::Struct(structure(inner, place, depth)?),
    _ => match inner {
      Value::String(name) => Type::Ref(name.clone()),
      _ => {
        return Err(ReadError::Form {
          at: place,
          problem: "is not a string".into(),
        });
      }
    },
  })
}

/// An object of the document and where it is, with the checks every part of the reader makes.
struct Object<'a> {
  map: &'a Map<String, Value>,
  at: String,
}

impl<'a> Object<'a> {
  fn new(value: &'a Value, at: String) -> Result<Self, ReadError> {
    match value {
      Value::Object(map) => Ok(Self { map, at }),
      _ => Err(ReadError::Form {
        at: if at.is_empty() { "the manifest".into() } else { at },
        problem: "is not an object".into(),
      }),
    }
  }

  fn at(&self, key: &str) -> String {
    if self.at.is_empty() {
      key.to_owned()
    } else {
      format!("{}.{key}", self.at)
    }
  }

  fn problem(&self, key: &str, problem: &str) -> ReadError {
    ReadError::Form {
      at: self.at(key),
      problem: problem.to_owned(),
    }
  }

  fn entries(&self) -> impl Iterator<Item = (&'a String, &'a Value)> {
    self.map.iter()
  }

  fn keys(&self, allowed: &[&str]) -> Result<(), ReadError> {
    match self.map.keys().find(|key| !allowed.contains(&key.as_str())) {
      Some(key) => Err(self.problem(key, "is not a key §12.2 defines here")),
      None => Ok(()),
    }
  }

  fn absent(&self, keys: &[&str], what: &str) -> Result<(), ReadError> {
    match keys.iter().find(|key| self.map.contains_key(**key)) {
      Some(key) => Err(self.problem(key, &format!("is not a key of {what}"))),
      None => Ok(()),
    }
  }

  fn form(&self, forms: &[&'static str]) -> Result<&'static str, ReadError> {
    let mut keys = self.map.keys().filter(|key| key.as_str() != "docs");
    match (keys.next(), keys.next()) {
      (Some(key), None) => forms
        .iter()
        .find(|form| **form == key)
        .copied()
        .ok_or_else(|| self.problem(key, &format!("is not one of {}", forms.join(", ")))),
      _ => Err(ReadError::Form {
        at: self.at.clone(),
        problem: format!("has not exactly one of {}", forms.join(", ")),
      }),
    }
  }

  fn optional(&self, key: &str) -> Result<Option<&'a Value>, ReadError> {
    match self.map.get(key) {
      Some(Value::Null) => Err(self.problem(key, "is `null`, which §12.2 does not allow: leave the key out")),
      found => Ok(found),
    }
  }

  fn required(&self, key: &str) -> Result<&'a Value, ReadError> {
    self.optional(key)?.ok_or_else(|| self.problem(key, "is missing"))
  }

  fn string(&self, key: &str) -> Result<String, ReadError> {
    match self.required(key)? {
      Value::String(text) => Ok(text.clone()),
      _ => Err(self.problem(key, "is not a string")),
    }
  }

  fn docs(&self) -> Result<Option<String>, ReadError> {
    match self.optional("docs")? {
      None => Ok(None),
      Some(Value::String(text)) => Ok(Some(text.clone())),
      Some(_) => Err(self.problem("docs", "is not a string")),
    }
  }

  fn flag(&self, key: &str) -> Result<bool, ReadError> {
    match self.optional(key)? {
      None => Ok(false),
      Some(Value::Bool(flag)) => Ok(*flag),
      Some(_) => Err(self.problem(key, "is not `true` or `false`")),
    }
  }

  fn ty(&self, key: &str) -> Result<Option<Type>, ReadError> {
    self.optional(key)?.map(|value| ty(value, self.at(key), 1)).transpose()
  }

  fn required_ty(&self, key: &str) -> Result<Type, ReadError> {
    ty(self.required(key)?, self.at(key), 1)
  }

  fn list<T>(&self, key: &str, read: impl Fn(&Value, String) -> Result<T, ReadError>) -> Result<Vec<T>, ReadError> {
    match self.optional(key)? {
      None => Ok(Vec::new()),
      Some(Value::Array(items)) => items
        .iter()
        .enumerate()
        .map(|(index, item)| read(item, format!("{}[{index}]", self.at(key))))
        .collect(),
      Some(_) => Err(self.problem(key, "is not an array")),
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
    Number::from_f64(value)
      .map(Value::Number)
      .ok_or_else(|| E::custom("a number JSON cannot hold"))
  }

  fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
    Ok(Value::from(value))
  }

  fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
    Ok(Value::String(value))
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
        return Err(de::Error::custom(format!("an object has the key `{key}` twice")));
      }
      let Strict(value) = map.next_value()?;
      object.insert(key, value);
    }
    Ok(Value::Object(object))
  }
}

#[cfg(test)]
mod tests;
