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
//! use pavia_codec::{decode, encode};
//! use pavia_contract::schema::{Primitive, Type, Types};
//! use pavia_proto::data::{Codec, DecodeLimits, Value};
//!
//! let ty = Type::from(Primitive::U64);
//! let types = Types::new();
//! let json = encode(&ty, &types, &Value::Unsigned(u64::MAX), Codec::Json, &DecodeLimits::default()).unwrap();
//! assert_eq!(json, b"\"18446744073709551615\"");
//! assert_eq!(decode(&ty, &types, &json, Codec::Json, &DecodeLimits::default()).unwrap(), Value::Unsigned(u64::MAX));
//! ```
//!
//! Unknown struct fields and unknown enum variants are refused for now: ignoring them, and the
//! catch-all of open enums, arrive with #70.
#![forbid(unsafe_code)]

use std::cmp::Ordering;
use std::collections::BTreeSet;

use data_encoding::BASE64URL_NOPAD;
use pavia_contract::schema::{Definition, Enum, Primitive, Struct, Type, Types, Variant};
use pavia_proto::data::{self, Codec, DecodeLimits, Value};
use thiserror::Error;

mod scalar;

const TIMESTAMP_TAG: u64 = 1;
const UUID_TAG: u64 = 37;

/// Encodes `value`, of contract type `ty`, as a data section in `codec` that a receiver with
/// `limits` accepts.
///
/// # Errors
///
/// [`TypedError`] for a value that does not fit `ty`, a type that refers to a name `types` does not
/// have, an `any` value with no form in `codec`, or a value that `limits` do not admit
/// ([`Problem::TooDeep`]): every receiver would refuse it, so it is refused here, before anything is
/// written.
///
/// The depth is checked twice. Before the walk, `value` may be one level past `limits`, so a value
/// nested much deeper never reaches the walk and its recursion stays bounded; such a value is
/// refused with `TooDeep` even when an item inside it has the wrong type. The walk never makes an
/// item deeper, and in `json` it writes a `timestamp` or `uuid` tag as text, which can take the value
/// back within `limits`: the encoded value is checked against `limits` exactly.
pub fn encode(ty: &Type, types: &Types, value: &Value, codec: Codec, limits: &DecodeLimits) -> Result<Vec<u8>, TypedError> {
  let slack = DecodeLimits {
    max_depth: limits.max_depth.saturating_add(1),
  };
  if !slack.admits(value) {
    return Err(TypedError::new(Problem::TooDeep));
  }
  let wire = Walk { types, codec }.wire_of(ty, value)?;
  if !limits.admits(&wire) {
    return Err(TypedError::new(Problem::TooDeep));
  }
  let mut out = Vec::new();
  data::encode(&wire, codec, &mut out).map_err(|error| TypedError::new(Problem::Encode(error)))?;
  Ok(out)
}

/// Decodes a data section in `codec` as a value of contract type `ty`, within `limits`.
///
/// # Errors
///
/// [`TypedError`] for a data section that is not one value of `codec` within `limits`, or whose value
/// does not fit `ty`.
pub fn decode(ty: &Type, types: &Types, data: &[u8], codec: Codec, limits: &DecodeLimits) -> Result<Value, TypedError> {
  let wire = data::decode(data, codec, limits).map_err(|error| TypedError::new(Problem::Decode(error)))?;
  Walk { types, codec }.typed_of(ty, wire)
}

/// A value that does not fit its type, and where in the value it is.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{at}: {problem}")]
pub struct TypedError {
  at: String,
  problem: Problem,
}

impl TypedError {
  fn new(problem: Problem) -> Self {
    Self {
      at: "$".to_owned(),
      problem,
    }
  }

  /// The same problem, one `step` further from the root: the walk adds each step on its way back up,
  /// so a value that fits never pays for its location.
  fn under(mut self, step: &str) -> Self {
    self.at.insert_str(1, step);
    self
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
  /// A `{"ref": name}` to a name that is not in the types, or one whose aliases and options lead back
  /// to it before any item of the value is read.
  #[error("type {0:?} is not defined")]
  UnknownType(String),
  /// A value to encode that nests deeper than the receiver's depth limit, or holds a longer run of
  /// tags than it takes (§12.3, §5.4.1, [`DecodeLimits::admits`]).
  #[error("value nests deeper than the depth limit")]
  TooDeep,
}

/// The form a value takes once `ref`s, aliases and the options around it are followed.
enum Shape<'t> {
  Null,
  Primitive(Primitive),
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
  /// The shape `value` takes as a `ty`. An option around a value that is not `null` is the value's own
  /// type, so a name met twice on the way is a loop that reads nothing and is refused.
  fn shape(&self, mut ty: &'t Type, value: &Value) -> Result<Shape<'t>, TypedError> {
    let mut followed = BTreeSet::new();
    loop {
      return Ok(match ty {
        Type::Primitive(primitive) => Shape::Primitive(*primitive),
        Type::Option(_) if matches!(value, Value::Null) => Shape::Null,
        Type::Option(inner) => {
          ty = inner;
          continue;
        }
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
          _ => return Err(TypedError::new(Problem::UnknownType(name.clone()))),
        },
        _ => return Err(TypedError::new(Problem::Expected("a known contract type"))),
      });
    }
  }

  /// Whether `primitive` is written as decimal text in this codec: the 64-bit integers in `json`.
  fn is_decimal_text(&self, primitive: Primitive) -> bool {
    self.codec == Codec::Json && matches!(primitive, Primitive::I64 | Primitive::U64)
  }

  fn wire_of(&self, ty: &Type, value: &Value) -> Result<Value, TypedError> {
    match self.shape(ty, value)? {
      Shape::Null => Ok(Value::Null),
      Shape::Primitive(primitive) => self.primitive_wire_of(primitive, value),
      Shape::List(inner) => {
        let Value::Array(items) = value else {
          return Err(TypedError::new(Problem::Expected("an array")));
        };
        let items = items
          .iter()
          .enumerate()
          .map(|(index, item)| self.wire_of(inner, item).map_err(|error| error.under(&format!("[{index}]"))))
          .collect::<Result<_, _>>()?;
        Ok(Value::Array(items))
      }
      Shape::Tuple(types) => {
        let Value::Array(items) = value else {
          return Err(TypedError::new(Problem::Expected("an array")));
        };
        tuple_length(types, items)?;
        let items = types
          .iter()
          .zip(items)
          .enumerate()
          .map(|(index, (ty, item))| self.wire_of(ty, item).map_err(|error| error.under(&format!("[{index}]"))))
          .collect::<Result<_, _>>()?;
        Ok(Value::Array(items))
      }
      Shape::Map(inner) => {
        let mut entries = text_entries(value)?
          .into_iter()
          .map(|(key, item)| {
            let wire = self
              .wire_of(inner, item)
              .map_err(|error| error.under(&format!("[{key:?}]")))?;
            Ok((key, wire))
          })
          .collect::<Result<Vec<_>, TypedError>>()?;
        match self.codec {
          Codec::Cbor => entries.sort_by(|(left, _), (right, _)| cbor_order(left, right)),
          Codec::Json => entries.sort_by_key(|(key, _)| *key),
        }
        Ok(Value::Map(
          entries
            .into_iter()
            .map(|(key, item)| (Value::Text(key.to_owned()), item))
            .collect(),
        ))
      }
      Shape::Struct(fields) => self.struct_wire_of(fields, value),
      Shape::Enum(variants) => {
        let (name, payload) = variant(value)?;
        match payload_type(variants, name, payload.is_some())?.zip(payload) {
          Some((ty, payload)) => {
            let payload = self.wire_of(ty, payload).map_err(|error| error.under(&format!(".{name}")))?;
            Ok(Value::Map(vec![(Value::Text(name.to_owned()), payload)]))
          }
          None => Ok(Value::Text(name.to_owned())),
        }
      }
    }
  }

  fn struct_wire_of(&self, fields: &Struct, value: &Value) -> Result<Value, TypedError> {
    let given = text_entries(value)?;
    if let Some(key) = unknown_field(fields, given.iter().map(|(key, _)| *key)) {
      return Err(TypedError::new(Problem::UnknownField(key.to_owned())));
    }
    let mut entries = Vec::new();
    for field in fields.fields() {
      match given.iter().find(|(key, _)| *key == field.name()) {
        Some((_, Value::Null)) if field.is_optional() => {}
        Some((key, item)) => {
          let wire = self
            .wire_of(field.ty(), item)
            .map_err(|error| error.under(&format!(".{key}")))?;
          entries.push((Value::Text((*key).to_owned()), wire));
        }
        None if field.is_optional() => {}
        None => return Err(TypedError::new(Problem::MissingField(field.name().to_owned()))),
      }
    }
    Ok(Value::Map(entries))
  }

  fn primitive_wire_of(&self, primitive: Primitive, value: &Value) -> Result<Value, TypedError> {
    let json = self.codec == Codec::Json;
    match (primitive, value) {
      (Primitive::Unit, Value::Null) | (Primitive::Bool, Value::Bool(_)) | (Primitive::String, Value::Text(_)) => {
        Ok(value.clone())
      }
      (Primitive::Any, _) => Ok(value.clone()),
      (Primitive::F32 | Primitive::F64, Value::Float(float)) => {
        if primitive == Primitive::F32 && f64::from(*float as f32) != *float && !float.is_nan() {
          return Err(TypedError::new(Problem::OutOfRange("f32")));
        }
        Ok(value.clone())
      }
      (Primitive::Bytes, Value::Bytes(bytes)) if json => Ok(Value::Text(BASE64URL_NOPAD.encode(bytes))),
      (Primitive::Bytes, Value::Bytes(_)) => Ok(value.clone()),
      (Primitive::Timestamp, Value::Tag(TIMESTAMP_TAG, content)) => {
        let millis =
          scalar::millis_of_epoch_seconds(content).ok_or_else(|| TypedError::new(Problem::OutOfRange("timestamp")))?;
        Ok(if json {
          Value::Text(scalar::rfc3339(millis))
        } else {
          Value::Tag(TIMESTAMP_TAG, Box::new(scalar::epoch_seconds(millis)))
        })
      }
      (Primitive::Uuid, Value::Tag(UUID_TAG, content)) => {
        let bytes = uuid_bytes(content).ok_or_else(|| TypedError::new(Problem::Expected("16 bytes in tag 37")))?;
        Ok(if json {
          Value::Text(scalar::uuid_text(&bytes))
        } else {
          value.clone()
        })
      }
      (primitive, value) => {
        let Some((name, bounds)) = range(primitive) else {
          return Err(TypedError::new(Problem::Expected(primitive.name())));
        };
        let number = scalar::integer_of(value).ok_or_else(|| TypedError::new(Problem::Expected(name)))?;
        if !bounds.contains(&number) {
          return Err(TypedError::new(Problem::OutOfRange(name)));
        }
        Ok(if self.is_decimal_text(primitive) {
          Value::Text(number.to_string())
        } else {
          scalar::integer(number)
        })
      }
    }
  }

  fn typed_of(&self, ty: &Type, value: Value) -> Result<Value, TypedError> {
    match self.shape(ty, &value)? {
      Shape::Null => Ok(Value::Null),
      Shape::Primitive(primitive) => self.primitive_typed_of(primitive, value),
      Shape::List(inner) => {
        let Value::Array(items) = value else {
          return Err(TypedError::new(Problem::Expected("an array")));
        };
        let items = items
          .into_iter()
          .enumerate()
          .map(|(index, item)| self.typed_of(inner, item).map_err(|error| error.under(&format!("[{index}]"))))
          .collect::<Result<_, _>>()?;
        Ok(Value::Array(items))
      }
      Shape::Tuple(types) => {
        let Value::Array(items) = value else {
          return Err(TypedError::new(Problem::Expected("an array")));
        };
        tuple_length(types, &items)?;
        let items = types
          .iter()
          .zip(items)
          .enumerate()
          .map(|(index, (ty, item))| self.typed_of(ty, item).map_err(|error| error.under(&format!("[{index}]"))))
          .collect::<Result<_, _>>()?;
        Ok(Value::Array(items))
      }
      Shape::Map(inner) => {
        let mut entries = owned_text_entries(value)?
          .into_iter()
          .map(|(key, item)| {
            let typed = self
              .typed_of(inner, item)
              .map_err(|error| error.under(&format!("[{key:?}]")))?;
            Ok((key, typed))
          })
          .collect::<Result<Vec<_>, TypedError>>()?;
        entries.sort_by(|(left, _), (right, _)| cbor_order(left, right));
        Ok(Value::Map(
          entries.into_iter().map(|(key, item)| (Value::Text(key), item)).collect(),
        ))
      }
      Shape::Struct(fields) => self.struct_typed_of(fields, value),
      Shape::Enum(variants) => {
        let (name, payload) = owned_variant(value)?;
        match payload_type(variants, &name, payload.is_some())?.zip(payload) {
          Some((ty, payload)) => {
            let typed = self.typed_of(ty, payload).map_err(|error| error.under(&format!(".{name}")))?;
            Ok(Value::Map(vec![(Value::Text(name), typed)]))
          }
          None => Ok(Value::Text(name)),
        }
      }
    }
  }

  fn struct_typed_of(&self, fields: &Struct, value: Value) -> Result<Value, TypedError> {
    let mut given = owned_text_entries(value)?;
    if let Some(key) = unknown_field(fields, given.iter().map(|(key, _)| key.as_str())) {
      return Err(TypedError::new(Problem::UnknownField(key.to_owned())));
    }
    let mut entries = Vec::new();
    for field in fields.fields() {
      match given.iter().position(|(key, _)| key == field.name()) {
        Some(at) => {
          let (key, item) = given.swap_remove(at);
          if field.is_optional() && item == Value::Null {
            continue;
          }
          let typed = self
            .typed_of(field.ty(), item)
            .map_err(|error| error.under(&format!(".{key}")))?;
          entries.push((Value::Text(key), typed));
        }
        None if field.is_optional() => {}
        None => return Err(TypedError::new(Problem::MissingField(field.name().to_owned()))),
      }
    }
    Ok(Value::Map(entries))
  }

  fn primitive_typed_of(&self, primitive: Primitive, value: Value) -> Result<Value, TypedError> {
    let json = self.codec == Codec::Json;
    match (primitive, value) {
      (Primitive::Any, value) => Ok(value),
      (Primitive::Unit, Value::Null) => Ok(Value::Null),
      (Primitive::Bool, value @ Value::Bool(_)) | (Primitive::String, value @ Value::Text(_)) => Ok(value),
      (Primitive::F32 | Primitive::F64, value) => {
        let float = match value {
          Value::Float(float) => float,
          integer @ (Value::Unsigned(_) | Value::Negative(_)) if json => {
            scalar::integer_of(&integer).expect("an `Unsigned` or `Negative` value is an integer") as f64
          }
          _ => return Err(TypedError::new(Problem::Expected(primitive.name()))),
        };
        if primitive == Primitive::F32 {
          let single = float as f32;
          if single.is_infinite() && float.is_finite() {
            return Err(TypedError::new(Problem::OutOfRange("f32")));
          }
          return Ok(Value::Float(f64::from(single)));
        }
        Ok(Value::Float(float))
      }
      (Primitive::Bytes, Value::Text(text)) if json => BASE64URL_NOPAD
        .decode(text.as_bytes())
        .map(Value::Bytes)
        .map_err(|_| TypedError::new(Problem::Expected("unpadded base64url"))),
      (Primitive::Bytes, value @ Value::Bytes(_)) => Ok(value),
      (Primitive::Timestamp, Value::Text(text)) if json => scalar::millis_of_rfc3339(&text)
        .map(|millis| Value::Tag(TIMESTAMP_TAG, Box::new(scalar::epoch_seconds(millis))))
        .ok_or_else(|| TypedError::new(Problem::Expected("an RFC 3339 timestamp in UTC with milliseconds"))),
      (Primitive::Timestamp, Value::Tag(TIMESTAMP_TAG, content)) => {
        let millis =
          scalar::millis_of_epoch_seconds(&content).ok_or_else(|| TypedError::new(Problem::OutOfRange("timestamp")))?;
        Ok(Value::Tag(TIMESTAMP_TAG, Box::new(scalar::epoch_seconds(millis))))
      }
      (Primitive::Uuid, Value::Text(text)) if json => scalar::uuid_of_text(&text)
        .map(|bytes| Value::Tag(UUID_TAG, Box::new(Value::Bytes(bytes.to_vec()))))
        .ok_or_else(|| TypedError::new(Problem::Expected("a lowercase hyphenated uuid"))),
      (Primitive::Uuid, Value::Tag(UUID_TAG, content)) => uuid_bytes(&content)
        .map(|bytes| Value::Tag(UUID_TAG, Box::new(Value::Bytes(bytes.to_vec()))))
        .ok_or_else(|| TypedError::new(Problem::Expected("16 bytes in tag 37"))),
      (_, Value::Tag(tag, _)) => Err(TypedError::new(Problem::Tag(tag))),
      (primitive, value) => {
        let Some((name, bounds)) = range(primitive) else {
          return Err(TypedError::new(Problem::Expected(primitive.name())));
        };
        let number = match value {
          Value::Text(text) if self.is_decimal_text(primitive) => scalar::integer_of_text(&text),
          value if !self.is_decimal_text(primitive) => scalar::integer_of(&value),
          _ => None,
        };
        let number = number.ok_or_else(|| TypedError::new(Problem::Expected(name)))?;
        if !bounds.contains(&number) {
          return Err(TypedError::new(Problem::OutOfRange(name)));
        }
        Ok(scalar::integer(number))
      }
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

/// The order of text keys in deterministic CBOR (RFC 8949 §4.2.1), the order of their encoded bytes:
/// shorter keys first, keys of one length by their bytes.
fn cbor_order(left: &str, right: &str) -> Ordering {
  (left.len(), left).cmp(&(right.len(), right))
}

fn uuid_bytes(content: &Value) -> Option<[u8; 16]> {
  match content {
    Value::Bytes(bytes) => bytes.as_slice().try_into().ok(),
    _ => None,
  }
}

fn tuple_length(types: &[Type], items: &[Value]) -> Result<(), TypedError> {
  if items.len() == types.len() {
    return Ok(());
  }
  Err(TypedError::new(Problem::TupleLength {
    expected: types.len(),
    found: items.len(),
  }))
}

/// The name of an enum value and its payload, if it has one.
fn variant(value: &Value) -> Result<(&str, Option<&Value>), TypedError> {
  match value {
    Value::Text(name) => Ok((name, None)),
    Value::Map(entries) => match entries.as_slice() {
      [(Value::Text(name), payload)] => Ok((name, Some(payload))),
      _ => Err(TypedError::new(Problem::Expected("a variant name or a one-entry map"))),
    },
    _ => Err(TypedError::new(Problem::Expected("a variant name or a one-entry map"))),
  }
}

/// As [`variant`], taking the value apart.
fn owned_variant(value: Value) -> Result<(String, Option<Value>), TypedError> {
  match value {
    Value::Text(name) => Ok((name, None)),
    Value::Map(mut entries) if entries.len() == 1 => match entries.pop() {
      Some((Value::Text(name), payload)) => Ok((name, Some(payload))),
      _ => Err(TypedError::new(Problem::Expected("a variant name or a one-entry map"))),
    },
    _ => Err(TypedError::new(Problem::Expected("a variant name or a one-entry map"))),
  }
}

/// The payload type of the variant `name` of `variants`: `None` for a unit variant. A name the enum does
/// not declare, or a payload given to a unit variant or missing from a data variant, is refused.
fn payload_type<'e>(variants: &'e Enum, name: &str, has_payload: bool) -> Result<Option<&'e Type>, TypedError> {
  variants
    .variants()
    .iter()
    .find(|declared| declared.name() == name)
    .map(Variant::ty)
    .filter(|ty| ty.is_some() == has_payload)
    .ok_or_else(|| TypedError::new(Problem::UnknownVariant(name.to_owned())))
}

/// The first of `keys` that is not a field of `fields`.
fn unknown_field<'k>(fields: &Struct, mut keys: impl Iterator<Item = &'k str>) -> Option<&'k str> {
  keys.find(|key| fields.fields().iter().all(|field| field.name() != *key))
}

/// The entries of a map with text keys, each key once.
fn text_entries(value: &Value) -> Result<Vec<(&str, &Value)>, TypedError> {
  let Value::Map(entries) = value else {
    return Err(TypedError::new(Problem::Expected("a map")));
  };
  let mut seen = BTreeSet::new();
  entries
    .iter()
    .map(|(key, item)| match key {
      Value::Text(key) if seen.insert(key.as_str()) => Ok((key.as_str(), item)),
      Value::Text(key) => Err(TypedError::new(Problem::DuplicateKey(key.clone()))),
      _ => Err(TypedError::new(Problem::Expected("text keys"))),
    })
    .collect()
}

/// As [`text_entries`], taking the map apart.
fn owned_text_entries(value: Value) -> Result<Vec<(String, Value)>, TypedError> {
  let Value::Map(entries) = value else {
    return Err(TypedError::new(Problem::Expected("a map")));
  };
  let mut seen = BTreeSet::new();
  entries
    .into_iter()
    .map(|(key, item)| match key {
      Value::Text(key) if seen.insert(key.clone()) => Ok((key, item)),
      Value::Text(key) => Err(TypedError::new(Problem::DuplicateKey(key))),
      _ => Err(TypedError::new(Problem::Expected("text keys"))),
    })
    .collect()
}

#[cfg(test)]
mod tests;
