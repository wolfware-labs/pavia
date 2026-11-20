//! Typed data sections (§12.3): a value of a contract type (§12.1) in the `cbor` and `json` codecs.
//!
//! A typed value is a [`Value`] in its `cbor` form, the form the codec vectors write in diagnostic
//! notation: integers as `Unsigned` or `Negative`, a `timestamp` as tag 1 over epoch seconds, a
//! `uuid` as tag 37 over 16 bytes, `bytes` as a byte string, a struct as a map from field names, a
//! unit enum variant as its name and a data variant as a one-entry map.
//!
//! [`encode`] checks the value against its type and writes the one encoding of §12.3: struct fields
//! in declaration order, `map` keys sorted (by encoded bytes in `cbor`, by UTF-8 bytes in `json`), a
//! timestamp as integer seconds when it is whole and otherwise as the narrowest float that keeps its
//! milliseconds, and in `json` 64-bit integers as decimal strings, `bytes` as unpadded base64url, a
//! `timestamp` as RFC 3339 and a `uuid` as lowercase hyphenated text. [`decode`] accepts any order
//! and any valid encoding, refuses what the type does not allow (an undeclared tag, an integer out of
//! range, a duplicate key, padded base64url) and returns the value in its `cbor` form, so the two
//! codecs decode to equal values.
//!
//! ```
//! use std::collections::BTreeMap;
//!
//! use pavia_codec::{decode, encode};
//! use pavia_contract::schema::{Primitive, Type};
//! use pavia_proto::data::{Codec, DecodeLimits, Value};
//!
//! let ty = Type::from(Primitive::U64);
//! let types = BTreeMap::new();
//! let json = encode(&ty, &types, &Value::Unsigned(u64::MAX), Codec::Json).unwrap();
//! assert_eq!(json, b"\"18446744073709551615\"");
//! assert_eq!(decode(&ty, &types, &json, Codec::Json, &DecodeLimits::default()).unwrap(), Value::Unsigned(u64::MAX));
//! ```
//!
//! Unknown struct fields and unknown enum variants are refused for now: ignoring them, and the
//! catch-all of open enums, arrive with #70.
#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use data_encoding::BASE64URL_NOPAD;
use pavia_contract::schema::{Definition, Enum, Primitive, Struct, Type};
use pavia_proto::data::{self, Codec, DecodeLimits, Value};
use thiserror::Error;

mod scalar;

/// The named types of a manifest (§12.2), which `{"ref": name}` types resolve in.
pub type Types = BTreeMap<String, Definition>;

const TIMESTAMP_TAG: u64 = 1;
const UUID_TAG: u64 = 37;

/// Encodes `value`, of contract type `ty`, as a data section in `codec`.
///
/// # Errors
///
/// [`TypedError`] for a value that does not fit `ty`, a type that refers to a name `types` does not
/// have, or an `any` value with no form in `codec`.
pub fn encode(ty: &Type, types: &Types, value: &Value, codec: Codec) -> Result<Vec<u8>, TypedError> {
  let wire = Walk { types, codec }.wire_of(ty, value, &mut Path::default())?;
  let mut out = Vec::new();
  data::encode(&wire, codec, &mut out).map_err(|error| TypedError::at_root(Problem::Encode(error)))?;
  Ok(out)
}

/// Decodes a data section in `codec` as a value of contract type `ty`, within `limits`.
///
/// # Errors
///
/// [`TypedError`] for a data section that is not one value of `codec` within `limits`, or whose value
/// does not fit `ty`.
pub fn decode(ty: &Type, types: &Types, data: &[u8], codec: Codec, limits: &DecodeLimits) -> Result<Value, TypedError> {
  let wire = data::decode(data, codec, limits).map_err(|error| TypedError::at_root(Problem::Decode(error)))?;
  Walk { types, codec }.typed_of(ty, wire, &mut Path::default())
}

/// A value that does not fit its type, and where in the value it is.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{at}: {problem}")]
pub struct TypedError {
  at: String,
  problem: Problem,
}

impl TypedError {
  fn at_root(problem: Problem) -> Self {
    Self {
      at: "$".to_owned(),
      problem,
    }
  }

  /// Where the problem is: `$` for the whole value, then `.field`, `[index]` or `["key"]` down to the
  /// item, so `$.items[2].sent_at`.
  #[must_use]
  pub fn at(&self) -> &str {
    &self.at
  }

  /// What is wrong there.
  #[must_use]
  pub const fn problem(&self) -> &Problem {
    &self.problem
  }
}

/// What does not fit a typed value.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum Problem {
  /// The data section is not one value of its codec (§12.3).
  #[error(transparent)]
  Decode(data::DecodeError),
  /// The value has no form in the codec: an `any` value with a tag, for one, in `json`.
  #[error(transparent)]
  Encode(data::EncodeError),
  /// The item is not the kind the type needs, such as text where a `u32` goes.
  #[error("expected {0}")]
  Expected(&'static str),
  /// An integer outside the range of its type, or a timestamp outside 0000-01-01 to 9999-12-31.
  #[error("{0} out of range")]
  OutOfRange(&'static str),
  /// A CBOR tag the type does not declare: only `timestamp` (1) and `uuid` (37) carry one.
  #[error("tag {0} is not the type's tag")]
  Tag(u64),
  /// A struct or `map` key that appears twice.
  #[error("key {0:?} appears twice")]
  DuplicateKey(String),
  /// A struct value without a field that is not optional.
  #[error("missing field {0:?}")]
  MissingField(String),
  /// A struct value with a field its type does not declare. Decoders ignore these from #70 on.
  #[error("field {0:?} is not in the type")]
  UnknownField(String),
  /// An enum variant the type does not declare, or a unit variant written with a value or a data
  /// variant without one. Open enums map unknown variants to a catch-all from #70 on.
  #[error("variant {0:?} is not in the type")]
  UnknownVariant(String),
  /// A tuple with another number of items than its type.
  #[error("{found} items where the tuple has {expected}")]
  TupleLength {
    /// Items in the type.
    expected: usize,
    /// Items in the value.
    found: usize,
  },
  /// A `{"ref": name}` to a name that is not in the types.
  #[error("type {0:?} is not defined")]
  UnknownType(String),
}

#[derive(Default)]
struct Path(String);

impl Path {
  fn error(&self, problem: Problem) -> TypedError {
    TypedError {
      at: format!("${}", self.0),
      problem,
    }
  }

  fn within<T>(&mut self, step: &str, walk: impl FnOnce(&mut Self) -> Result<T, TypedError>) -> Result<T, TypedError> {
    let length = self.0.len();
    self.0.push_str(step);
    let result = walk(self);
    self.0.truncate(length);
    result
  }
}

/// The form of a type once `ref`s and aliases are followed.
enum Shape<'t> {
  Primitive(Primitive),
  Option(&'t Type),
  List(&'t Type),
  Map(&'t Type),
  Tuple(&'t [Type]),
  Struct(&'t Struct),
  Enum(&'t Enum),
}

struct Walk<'t> {
  types: &'t Types,
  codec: Codec,
}

impl<'t> Walk<'t> {
  fn shape(&self, mut ty: &'t Type, path: &Path) -> Result<Shape<'t>, TypedError> {
    let mut followed = BTreeSet::new();
    loop {
      return Ok(match ty {
        Type::Primitive(primitive) => Shape::Primitive(*primitive),
        Type::Option(inner) => Shape::Option(inner),
        Type::List(inner) => Shape::List(inner),
        Type::Map(inner) => Shape::Map(inner),
        Type::Tuple(items) => Shape::Tuple(items),
        Type::Struct(fields) => Shape::Struct(fields),
        Type::Ref(name) => match self.types.get(name) {
          Some(Definition::Struct(fields)) => Shape::Struct(fields),
          Some(Definition::Enum(variants)) => Shape::Enum(variants),
          Some(Definition::Alias(target)) if followed.insert(name) => {
            ty = target;
            continue;
          }
          _ => return Err(path.error(Problem::UnknownType(name.clone()))),
        },
        _ => return Err(path.error(Problem::Expected("a known contract type"))),
      });
    }
  }

  fn wire_of(&self, ty: &Type, value: &Value, path: &mut Path) -> Result<Value, TypedError> {
    match self.shape(ty, path)? {
      Shape::Primitive(primitive) => self.primitive_wire_of(primitive, value, path),
      Shape::Option(inner) => match value {
        Value::Null => Ok(Value::Null),
        present => self.wire_of(inner, present, path),
      },
      Shape::List(inner) => {
        let Value::Array(items) = value else {
          return Err(path.error(Problem::Expected("an array")));
        };
        let items = items
          .iter()
          .enumerate()
          .map(|(index, item)| path.within(&format!("[{index}]"), |path| self.wire_of(inner, item, path)))
          .collect::<Result<_, _>>()?;
        Ok(Value::Array(items))
      }
      Shape::Tuple(types) => {
        let items = tuple(types, value, path)?;
        let items = types
          .iter()
          .zip(items)
          .enumerate()
          .map(|(index, (ty, item))| path.within(&format!("[{index}]"), |path| self.wire_of(ty, item, path)))
          .collect::<Result<_, _>>()?;
        Ok(Value::Array(items))
      }
      Shape::Map(inner) => {
        let mut entries = text_entries(value, path)?
          .into_iter()
          .map(|(key, item)| {
            let wire = path.within(&format!("[{key:?}]"), |path| self.wire_of(inner, item, path))?;
            Ok((key, wire))
          })
          .collect::<Result<Vec<_>, TypedError>>()?;
        match self.codec {
          Codec::Cbor => entries.sort_by(|(left, _), (right, _)| (left.len(), left).cmp(&(right.len(), right))),
          Codec::Json => entries.sort_by_key(|(key, _)| *key),
        }
        Ok(Value::Map(
          entries
            .into_iter()
            .map(|(key, item)| (Value::Text(key.to_owned()), item))
            .collect(),
        ))
      }
      Shape::Struct(fields) => self.struct_wire_of(fields, value, path),
      Shape::Enum(variants) => {
        let (name, payload) = variant(value, path)?;
        let Some(declared) = variants.variants().iter().find(|declared| declared.name() == name) else {
          return Err(path.error(Problem::UnknownVariant(name.to_owned())));
        };
        match (declared.ty(), payload) {
          (None, None) => Ok(Value::Text(name.to_owned())),
          (Some(ty), Some(payload)) => {
            let payload = path.within(&format!(".{name}"), |path| self.wire_of(ty, payload, path))?;
            Ok(Value::Map(vec![(Value::Text(name.to_owned()), payload)]))
          }
          _ => Err(path.error(Problem::UnknownVariant(name.to_owned()))),
        }
      }
    }
  }

  fn struct_wire_of(&self, fields: &Struct, value: &Value, path: &mut Path) -> Result<Value, TypedError> {
    let given = text_entries(value, path)?;
    if let Some((key, _)) = given
      .iter()
      .find(|(key, _)| fields.fields().iter().all(|field| field.name() != *key))
    {
      return Err(path.error(Problem::UnknownField((*key).to_owned())));
    }
    let mut entries = Vec::new();
    for field in fields.fields() {
      match given.iter().find(|(key, _)| *key == field.name()) {
        Some((_, Value::Null)) if field.is_optional() => {}
        Some((key, item)) => {
          let wire = path.within(&format!(".{key}"), |path| self.wire_of(field.ty(), item, path))?;
          entries.push((Value::Text((*key).to_owned()), wire));
        }
        None if field.is_optional() => {}
        None => return Err(path.error(Problem::MissingField(field.name().to_owned()))),
      }
    }
    Ok(Value::Map(entries))
  }

  fn primitive_wire_of(&self, primitive: Primitive, value: &Value, path: &Path) -> Result<Value, TypedError> {
    let json = self.codec == Codec::Json;
    match (primitive, value) {
      (Primitive::Unit, Value::Null) | (Primitive::Bool, Value::Bool(_)) | (Primitive::String, Value::Text(_)) => {
        Ok(value.clone())
      }
      (Primitive::Any, _) => Ok(value.clone()),
      (Primitive::F32 | Primitive::F64, Value::Float(float)) => {
        if primitive == Primitive::F32 && f64::from(*float as f32) != *float && !float.is_nan() {
          return Err(path.error(Problem::OutOfRange("f32")));
        }
        Ok(value.clone())
      }
      (Primitive::Bytes, Value::Bytes(bytes)) if json => Ok(Value::Text(BASE64URL_NOPAD.encode(bytes))),
      (Primitive::Bytes, Value::Bytes(_)) => Ok(value.clone()),
      (Primitive::Timestamp, Value::Tag(TIMESTAMP_TAG, content)) => {
        let millis = scalar::millis_of_epoch_seconds(content).ok_or_else(|| path.error(Problem::OutOfRange("timestamp")))?;
        Ok(if json {
          Value::Text(scalar::rfc3339(millis))
        } else {
          Value::Tag(TIMESTAMP_TAG, Box::new(scalar::epoch_seconds(millis)))
        })
      }
      (Primitive::Uuid, Value::Tag(UUID_TAG, content)) => {
        let bytes = uuid_bytes(content).ok_or_else(|| path.error(Problem::Expected("16 bytes in tag 37")))?;
        Ok(if json {
          Value::Text(scalar::uuid_text(&bytes))
        } else {
          value.clone()
        })
      }
      (integer, value) if range(integer).is_some() => {
        let (name, bounds) = range(integer).unwrap_or(("", 0..=0));
        let number = scalar::integer_of(value).ok_or_else(|| path.error(Problem::Expected(name)))?;
        if !bounds.contains(&number) {
          return Err(path.error(Problem::OutOfRange(name)));
        }
        Ok(if json && matches!(integer, Primitive::I64 | Primitive::U64) {
          Value::Text(number.to_string())
        } else {
          scalar::integer(number)
        })
      }
      (primitive, _) => Err(path.error(Problem::Expected(primitive.name()))),
    }
  }

  fn typed_of(&self, ty: &Type, value: Value, path: &mut Path) -> Result<Value, TypedError> {
    match self.shape(ty, path)? {
      Shape::Primitive(primitive) => self.primitive_typed_of(primitive, value, path),
      Shape::Option(inner) => match value {
        Value::Null => Ok(Value::Null),
        present => self.typed_of(inner, present, path),
      },
      Shape::List(inner) => {
        let Value::Array(items) = value else {
          return Err(path.error(Problem::Expected("an array")));
        };
        let items = items
          .into_iter()
          .enumerate()
          .map(|(index, item)| path.within(&format!("[{index}]"), |path| self.typed_of(inner, item, path)))
          .collect::<Result<_, _>>()?;
        Ok(Value::Array(items))
      }
      Shape::Tuple(types) => {
        let Value::Array(items) = value else {
          return Err(path.error(Problem::Expected("an array")));
        };
        if items.len() != types.len() {
          return Err(path.error(Problem::TupleLength {
            expected: types.len(),
            found: items.len(),
          }));
        }
        let items = types
          .iter()
          .zip(items)
          .enumerate()
          .map(|(index, (ty, item))| path.within(&format!("[{index}]"), |path| self.typed_of(ty, item, path)))
          .collect::<Result<_, _>>()?;
        Ok(Value::Array(items))
      }
      Shape::Map(inner) => {
        let mut entries = owned_text_entries(value, path)?
          .into_iter()
          .map(|(key, item)| {
            let typed = path.within(&format!("[{key:?}]"), |path| self.typed_of(inner, item, path))?;
            Ok((key, typed))
          })
          .collect::<Result<Vec<_>, TypedError>>()?;
        entries.sort_by(|(left, _), (right, _)| (left.len(), left).cmp(&(right.len(), right)));
        Ok(Value::Map(
          entries.into_iter().map(|(key, item)| (Value::Text(key), item)).collect(),
        ))
      }
      Shape::Struct(fields) => {
        let mut given = owned_text_entries(value, path)?;
        if let Some((key, _)) = given
          .iter()
          .find(|(key, _)| fields.fields().iter().all(|field| field.name() != key))
        {
          return Err(path.error(Problem::UnknownField(key.clone())));
        }
        let mut entries = Vec::new();
        for field in fields.fields() {
          match given.iter().position(|(key, _)| key == field.name()) {
            Some(at) => {
              let (key, item) = given.swap_remove(at);
              if field.is_optional() && item == Value::Null {
                continue;
              }
              let typed = path.within(&format!(".{key}"), |path| self.typed_of(field.ty(), item, path))?;
              entries.push((Value::Text(key), typed));
            }
            None if field.is_optional() => {}
            None => return Err(path.error(Problem::MissingField(field.name().to_owned()))),
          }
        }
        Ok(Value::Map(entries))
      }
      Shape::Enum(variants) => {
        let (name, payload) = match value {
          Value::Text(name) => (name, None),
          Value::Map(mut entries) if entries.len() == 1 => match entries.pop() {
            Some((Value::Text(name), payload)) => (name, Some(payload)),
            _ => return Err(path.error(Problem::Expected("a variant name or a one-entry map"))),
          },
          _ => return Err(path.error(Problem::Expected("a variant name or a one-entry map"))),
        };
        let declared = variants.variants().iter().find(|declared| declared.name() == name);
        match (declared.map(|declared| declared.ty()), payload) {
          (Some(None), None) => Ok(Value::Text(name)),
          (Some(Some(ty)), Some(payload)) => {
            let typed = path.within(&format!(".{name}"), |path| self.typed_of(ty, payload, path))?;
            Ok(Value::Map(vec![(Value::Text(name), typed)]))
          }
          _ => Err(path.error(Problem::UnknownVariant(name))),
        }
      }
    }
  }

  fn primitive_typed_of(&self, primitive: Primitive, value: Value, path: &Path) -> Result<Value, TypedError> {
    let json = self.codec == Codec::Json;
    match (primitive, value) {
      (Primitive::Any, value) => Ok(value),
      (Primitive::Unit, Value::Null) => Ok(Value::Null),
      (Primitive::Bool, value @ Value::Bool(_)) | (Primitive::String, value @ Value::Text(_)) => Ok(value),
      (Primitive::F32 | Primitive::F64, value) => {
        let float = match value {
          Value::Float(float) => float,
          integer @ (Value::Unsigned(_) | Value::Negative(_)) if json => {
            scalar::integer_of(&integer).map_or(f64::NAN, |number| number as f64)
          }
          _ => return Err(path.error(Problem::Expected(primitive.name()))),
        };
        if primitive == Primitive::F32 {
          let single = float as f32;
          if single.is_infinite() && float.is_finite() {
            return Err(path.error(Problem::OutOfRange("f32")));
          }
          return Ok(Value::Float(f64::from(single)));
        }
        Ok(Value::Float(float))
      }
      (Primitive::Bytes, Value::Text(text)) if json => BASE64URL_NOPAD
        .decode(text.as_bytes())
        .map(Value::Bytes)
        .map_err(|_| path.error(Problem::Expected("unpadded base64url"))),
      (Primitive::Bytes, value @ Value::Bytes(_)) if !json => Ok(value),
      (Primitive::Timestamp, Value::Text(text)) if json => scalar::millis_of_rfc3339(&text)
        .map(|millis| Value::Tag(TIMESTAMP_TAG, Box::new(scalar::epoch_seconds(millis))))
        .ok_or_else(|| path.error(Problem::Expected("an RFC 3339 timestamp in UTC with milliseconds"))),
      (Primitive::Timestamp, Value::Tag(TIMESTAMP_TAG, content)) if !json => {
        let millis = scalar::millis_of_epoch_seconds(&content).ok_or_else(|| path.error(Problem::OutOfRange("timestamp")))?;
        Ok(Value::Tag(TIMESTAMP_TAG, Box::new(scalar::epoch_seconds(millis))))
      }
      (Primitive::Uuid, Value::Text(text)) if json => scalar::uuid_of_text(&text)
        .map(|bytes| Value::Tag(UUID_TAG, Box::new(Value::Bytes(bytes.to_vec()))))
        .ok_or_else(|| path.error(Problem::Expected("a lowercase hyphenated uuid"))),
      (Primitive::Uuid, Value::Tag(UUID_TAG, content)) if !json => uuid_bytes(&content)
        .map(|bytes| Value::Tag(UUID_TAG, Box::new(Value::Bytes(bytes.to_vec()))))
        .ok_or_else(|| path.error(Problem::Expected("16 bytes in tag 37"))),
      (_, Value::Tag(tag, _)) if !json => Err(path.error(Problem::Tag(tag))),
      (integer, value) if range(integer).is_some() => {
        let (name, bounds) = range(integer).unwrap_or(("", 0..=0));
        let number = match value {
          Value::Text(text) if json && matches!(integer, Primitive::I64 | Primitive::U64) => {
            scalar::integer_of_text(&text).ok_or_else(|| path.error(Problem::Expected(name)))?
          }
          ref value if !(json && matches!(integer, Primitive::I64 | Primitive::U64)) => {
            scalar::integer_of(value).ok_or_else(|| path.error(Problem::Expected(name)))?
          }
          _ => return Err(path.error(Problem::Expected(name))),
        };
        if !bounds.contains(&number) {
          return Err(path.error(Problem::OutOfRange(name)));
        }
        Ok(scalar::integer(number))
      }
      (primitive, _) => Err(path.error(Problem::Expected(primitive.name()))),
    }
  }
}

/// The name and the range of an integer type.
fn range(primitive: Primitive) -> Option<(&'static str, std::ops::RangeInclusive<i128>)> {
  let bounds = |low: i128, high: i128| low..=high;
  Some(match primitive {
    Primitive::I8 => ("i8", bounds(i8::MIN.into(), i8::MAX.into())),
    Primitive::I16 => ("i16", bounds(i16::MIN.into(), i16::MAX.into())),
    Primitive::I32 => ("i32", bounds(i32::MIN.into(), i32::MAX.into())),
    Primitive::I64 => ("i64", bounds(i64::MIN.into(), i64::MAX.into())),
    Primitive::U8 => ("u8", bounds(0, u8::MAX.into())),
    Primitive::U16 => ("u16", bounds(0, u16::MAX.into())),
    Primitive::U32 => ("u32", bounds(0, u32::MAX.into())),
    Primitive::U64 => ("u64", bounds(0, u64::MAX.into())),
    _ => return None,
  })
}

fn uuid_bytes(content: &Value) -> Option<[u8; 16]> {
  match content {
    Value::Bytes(bytes) => bytes.as_slice().try_into().ok(),
    _ => None,
  }
}

fn tuple<'v>(types: &[Type], value: &'v Value, path: &Path) -> Result<&'v [Value], TypedError> {
  let Value::Array(items) = value else {
    return Err(path.error(Problem::Expected("an array")));
  };
  if items.len() != types.len() {
    return Err(path.error(Problem::TupleLength {
      expected: types.len(),
      found: items.len(),
    }));
  }
  Ok(items)
}

fn variant<'v>(value: &'v Value, path: &Path) -> Result<(&'v str, Option<&'v Value>), TypedError> {
  match value {
    Value::Text(name) => Ok((name, None)),
    Value::Map(entries) => match entries.as_slice() {
      [(Value::Text(name), payload)] => Ok((name, Some(payload))),
      _ => Err(path.error(Problem::Expected("a variant name or a one-entry map"))),
    },
    _ => Err(path.error(Problem::Expected("a variant name or a one-entry map"))),
  }
}

/// The entries of a map with text keys, each key once.
fn text_entries<'v>(value: &'v Value, path: &Path) -> Result<Vec<(&'v str, &'v Value)>, TypedError> {
  let Value::Map(entries) = value else {
    return Err(path.error(Problem::Expected("a map")));
  };
  let mut seen = BTreeSet::new();
  entries
    .iter()
    .map(|(key, item)| match key {
      Value::Text(key) if seen.insert(key.as_str()) => Ok((key.as_str(), item)),
      Value::Text(key) => Err(path.error(Problem::DuplicateKey(key.clone()))),
      _ => Err(path.error(Problem::Expected("text keys"))),
    })
    .collect()
}

/// As [`text_entries`], taking the map apart.
fn owned_text_entries(value: Value, path: &Path) -> Result<Vec<(String, Value)>, TypedError> {
  let Value::Map(entries) = value else {
    return Err(path.error(Problem::Expected("a map")));
  };
  let mut seen = BTreeSet::new();
  entries
    .into_iter()
    .map(|(key, item)| match key {
      Value::Text(key) if seen.insert(key.clone()) => Ok((key, item)),
      Value::Text(key) => Err(path.error(Problem::DuplicateKey(key))),
      _ => Err(path.error(Problem::Expected("text keys"))),
    })
    .collect()
}

#[cfg(test)]
mod tests;
