//! Contract types (§12.1) and the named-type definitions of the manifest's `types` table (§12.2).
//!
//! [`Type`] is what a parameter, field or variant carries: a primitive, a composite of other types, or a
//! [`Type::Ref`] to a named type. [`Definition`] is what the `types` table holds under a name: a struct,
//! an enum or an alias. Both turn into their manifest JSON with `to_json`.

use serde_json::{Map, Value, json};

/// A primitive contract type (§12.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Primitive {
  /// `unit`: no value.
  Unit,
  /// `bool`.
  Bool,
  /// `i8`.
  I8,
  /// `i16`.
  I16,
  /// `i32`.
  I32,
  /// `i64`.
  I64,
  /// `u8`.
  U8,
  /// `u16`.
  U16,
  /// `u32`.
  U32,
  /// `u64`.
  U64,
  /// `f32`.
  F32,
  /// `f64`.
  F64,
  /// `string`: UTF-8 text.
  String,
  /// `bytes`: a byte string.
  Bytes,
  /// `timestamp`: an instant, millisecond precision.
  Timestamp,
  /// `uuid`: a 128-bit UUID.
  Uuid,
  /// `any`: an untyped value; generated code exposes it as `unknown`.
  Any,
}

impl Primitive {
  /// The name in the manifest: `"u32"`, `"string"`.
  #[must_use]
  pub const fn name(self) -> &'static str {
    match self {
      Self::Unit => "unit",
      Self::Bool => "bool",
      Self::I8 => "i8",
      Self::I16 => "i16",
      Self::I32 => "i32",
      Self::I64 => "i64",
      Self::U8 => "u8",
      Self::U16 => "u16",
      Self::U32 => "u32",
      Self::U64 => "u64",
      Self::F32 => "f32",
      Self::F64 => "f64",
      Self::String => "string",
      Self::Bytes => "bytes",
      Self::Timestamp => "timestamp",
      Self::Uuid => "uuid",
      Self::Any => "any",
    }
  }
}

/// A contract type (§12.1).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Type {
  /// A primitive: `"u32"`, `"string"`.
  Primitive(Primitive),
  /// `{"option": T}`: `T`, or absent. `T` is never `unit` (§12.1); the `Option<T>` mapping checks it, and
  /// a type built by hand is checked by [`Manifest::validate`](crate::manifest::Manifest::validate).
  Option(Box<Type>),
  /// `{"list": T}`.
  List(Box<Type>),
  /// `{"map": T}`: string keys, `T` values.
  Map(Box<Type>),
  /// `{"tuple": [T...]}`.
  Tuple(Vec<Type>),
  /// `{"struct": ...}` written in place, as an enum variant's payload is (§12.2).
  Struct(Struct),
  /// `{"ref": "Name"}`: the named type `Name` of the `types` table.
  Ref(String),
}

impl Type {
  /// The type as manifest JSON: `"u32"`, `{"list": "string"}`, `{"ref": "Message"}`.
  #[must_use]
  pub fn to_json(&self) -> Value {
    match self {
      Self::Primitive(primitive) => Value::from(primitive.name()),
      Self::Option(inner) => json!({ "option": inner.to_json() }),
      Self::List(inner) => json!({ "list": inner.to_json() }),
      Self::Map(inner) => json!({ "map": inner.to_json() }),
      Self::Tuple(items) => json!({ "tuple": items.iter().map(Self::to_json).collect::<Vec<_>>() }),
      Self::Struct(definition) => definition.to_json(),
      Self::Ref(name) => json!({ "ref": name }),
    }
  }
}

impl From<Primitive> for Type {
  fn from(primitive: Primitive) -> Self {
    Self::Primitive(primitive)
  }
}

/// A named type of the `types` table (§12.1).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Definition {
  /// `{"struct": {"fields": [...]}}`.
  Struct(Struct),
  /// `{"enum": {"variants": [...], "open": true}}`.
  Enum(Enum),
  /// `{"alias": T}`.
  Alias(Type),
}

impl Definition {
  /// The definition as manifest JSON.
  #[must_use]
  pub fn to_json(&self) -> Value {
    match self {
      Self::Struct(definition) => definition.to_json(),
      Self::Enum(definition) => definition.to_json(),
      Self::Alias(target) => json!({ "alias": target.to_json() }),
    }
  }
}

/// A struct: its fields in declaration order, which is their encoding order (§12.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Struct {
  fields: Vec<Field>,
}

impl Struct {
  /// A struct with `fields`, in declaration order.
  #[must_use]
  pub const fn new(fields: Vec<Field>) -> Self {
    Self { fields }
  }

  /// The fields, in declaration order.
  #[must_use]
  pub fn fields(&self) -> &[Field] {
    &self.fields
  }

  fn to_json(&self) -> Value {
    json!({ "struct": { "fields": self.fields.iter().map(Field::to_json).collect::<Vec<_>>() } })
  }
}

/// A struct field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
  name: String,
  ty: Type,
  optional: bool,
}

impl Field {
  /// A field named `name` of type `ty` whose key must be present on the wire.
  #[must_use]
  pub fn new(name: impl Into<String>, ty: Type) -> Self {
    Self {
      name: name.into(),
      ty,
      optional: false,
    }
  }

  /// The same field, whose key may be missing on the wire, which decodes as absent. Its type is an
  /// [`Type::Option`] (§12.1).
  #[must_use]
  pub const fn optional(mut self) -> Self {
    self.optional = true;
    self
  }

  /// The field's name.
  #[must_use]
  pub fn name(&self) -> &str {
    &self.name
  }

  /// The field's type.
  #[must_use]
  pub const fn ty(&self) -> &Type {
    &self.ty
  }

  /// Whether the field's key may be missing on the wire.
  #[must_use]
  pub const fn is_optional(&self) -> bool {
    self.optional
  }

  fn to_json(&self) -> Value {
    let mut field = Map::new();
    field.insert("name".into(), Value::from(self.name.as_str()));
    field.insert("type".into(), self.ty.to_json());
    if self.optional {
      field.insert("optional".into(), Value::Bool(true));
    }
    Value::Object(field)
  }
}

/// An enum: its variants in declaration order, and whether it is open (§12.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enum {
  variants: Vec<Variant>,
  open: bool,
}

impl Enum {
  /// A closed enum with `variants`, in declaration order: decoders refuse an unknown variant.
  #[must_use]
  pub const fn new(variants: Vec<Variant>) -> Self {
    Self { variants, open: false }
  }

  /// The same enum, open: decoders map an unknown variant to a catch-all instead of refusing it (§12.1).
  #[must_use]
  pub const fn open(mut self) -> Self {
    self.open = true;
    self
  }

  /// The variants, in declaration order.
  #[must_use]
  pub fn variants(&self) -> &[Variant] {
    &self.variants
  }

  /// Whether the enum is open.
  #[must_use]
  pub const fn is_open(&self) -> bool {
    self.open
  }

  fn to_json(&self) -> Value {
    let mut definition = Map::new();
    definition.insert("variants".into(), self.variants.iter().map(Variant::to_json).collect());
    if self.open {
      definition.insert("open".into(), Value::Bool(true));
    }
    json!({ "enum": definition })
  }
}

/// An enum variant: a unit variant, or one that carries a value of its type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
  name: String,
  ty: Option<Type>,
}

impl Variant {
  /// A unit variant named `name`.
  #[must_use]
  pub fn unit(name: impl Into<String>) -> Self {
    Self {
      name: name.into(),
      ty: None,
    }
  }

  /// A variant named `name` that carries a value of type `ty`.
  #[must_use]
  pub fn new(name: impl Into<String>, ty: Type) -> Self {
    Self {
      name: name.into(),
      ty: Some(ty),
    }
  }

  /// The variant's name.
  #[must_use]
  pub fn name(&self) -> &str {
    &self.name
  }

  /// The type of the value the variant carries, or `None` for a unit variant.
  #[must_use]
  pub const fn ty(&self) -> Option<&Type> {
    self.ty.as_ref()
  }

  fn to_json(&self) -> Value {
    let mut variant = Map::new();
    variant.insert("name".into(), Value::from(self.name.as_str()));
    if let Some(ty) = &self.ty {
      variant.insert("type".into(), ty.to_json());
    }
    Value::Object(variant)
  }
}

#[cfg(test)]
mod tests;
