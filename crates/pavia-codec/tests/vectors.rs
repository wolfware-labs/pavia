//! Every codec vector under `vectors/codecs/` (§17.2), run through the typed codecs: a `both` case
//! encodes its value to exactly its `cbor` and `json` and decodes both back to the value; a `decode`
//! case decodes its input to the value, or fails when it carries an `error`.

use pavia_codec::{decode, encode};
use pavia_contract::schema::{Definition, Enum, Field, Primitive, Struct, Type, Types, Variant};
use pavia_proto::data::{Codec, DecodeLimits, Value};
use pavia_vectors::diag::{self, Diag};
use serde_json::Value as Json;

fn ty(json: &Json) -> Type {
  if let Some(name) = json.as_str() {
    let primitive = Primitive::ALL.into_iter().find(|primitive| primitive.name() == name);
    return Type::from(primitive.unwrap_or_else(|| panic!("unknown primitive {name}")));
  }
  let (key, inner) = json
    .as_object()
    .and_then(|object| object.iter().next())
    .expect("a type object");
  match key.as_str() {
    "option" => Type::Option(Box::new(ty(inner))),
    "list" => Type::List(Box::new(ty(inner))),
    "map" => Type::Map(Box::new(ty(inner))),
    "tuple" => Type::Tuple(inner.as_array().expect("tuple items").iter().map(ty).collect()),
    "ref" => Type::Ref(inner.as_str().expect("a name").to_owned()),
    "struct" => Type::Struct(fields(inner)),
    other => panic!("unknown type form {other}"),
  }
}

fn fields(json: &Json) -> Struct {
  Struct::new(
    json["fields"]
      .as_array()
      .expect("fields")
      .iter()
      .map(|field| {
        let declared = Field::new(field["name"].as_str().expect("a field name"), ty(&field["type"]));
        if field["optional"] == true {
          declared.optional()
        } else {
          declared
        }
      })
      .collect(),
  )
}

fn definition(json: &Json) -> Definition {
  let (key, inner) = json
    .as_object()
    .and_then(|object| object.iter().next())
    .expect("a definition");
  match key.as_str() {
    "struct" => Definition::Struct(fields(inner)),
    "alias" => Definition::Alias(ty(inner)),
    "enum" => {
      let variants = inner["variants"]
        .as_array()
        .expect("variants")
        .iter()
        .map(|variant| {
          let name = variant["name"].as_str().expect("a variant name");
          variant
            .get("type")
            .map_or_else(|| Variant::unit(name), |payload| Variant::new(name, ty(payload)))
        })
        .collect();
      let declared = Enum::new(variants);
      Definition::Enum(if inner["open"] == true { declared.open() } else { declared })
    }
    other => panic!("unknown definition form {other}"),
  }
}

fn value(diag: &Diag) -> Value {
  match diag {
    Diag::Uint(number) => Value::Unsigned(*number),
    Diag::Nint(number) => Value::Negative(*number),
    Diag::Float(float) => Value::Float(*float),
    Diag::Text(text) => Value::Text(text.clone()),
    Diag::Bytes(bytes) => Value::Bytes(bytes.clone()),
    Diag::Array(items) => Value::Array(items.iter().map(value).collect()),
    Diag::Map { entries, .. } => Value::Map(entries.iter().map(|(key, item)| (value(key), value(item))).collect()),
    Diag::Tag(tag, content) => Value::Tag(*tag, Box::new(value(content))),
    Diag::Bool(flag) => Value::Bool(*flag),
    Diag::Null => Value::Null,
    other => panic!("{other:?} is a pattern, not a value"),
  }
}

struct Case {
  id: String,
  ty: Type,
  both: bool,
  value: Option<Value>,
  cbor: Option<Vec<u8>>,
  json: Option<String>,
  error: Option<Codec>,
}

fn cases() -> Vec<(Types, Case)> {
  let mut all = Vec::new();
  for path in pavia_vectors::files("codecs") {
    let doc = pavia_vectors::load(&path);
    let types: Types = doc["types"]
      .as_object()
      .map(|types| types.iter().map(|(name, json)| (name.clone(), definition(json))).collect())
      .unwrap_or_default();
    for case in doc["cases"].as_array().expect("cases") {
      let error = case.get("error").map(|error| match error["codec"].as_str() {
        Some("cbor") => Codec::Cbor,
        _ => Codec::Json,
      });
      all.push((
        types.clone(),
        Case {
          id: format!("{}#{}", path.display(), case["id"].as_str().expect("an id")),
          ty: ty(&case["type"]),
          both: case["direction"] == "both",
          value: case["value"]
            .as_str()
            .map(|text| value(&diag::parse(text).unwrap_or_else(|error| panic!("{text}: {error}")))),
          cbor: case["cbor"].as_str().map(pavia_vectors::hex),
          json: case["json"].as_str().map(str::to_owned),
          error,
        },
      ));
    }
  }
  all
}

fn decoded(case: &Case, types: &Types, codec: Codec, input: &[u8]) -> Result<Value, pavia_codec::TypedError> {
  decode(&case.ty, types, input, codec, &DecodeLimits::default())
}

#[test]
fn runs_every_codec_vector() {
  let all = cases();
  assert!(all.len() >= 90, "only {} codec vectors", all.len());
  for (types, case) in &all {
    let id = &case.id;
    if let Some(codec) = case.error {
      let input = match codec {
        Codec::Cbor => case.cbor.clone().expect("cbor input"),
        Codec::Json => case.json.clone().expect("json input").into_bytes(),
      };
      assert!(decoded(case, types, codec, &input).is_err(), "{id}: decoding must fail");
      continue;
    }
    let value = case.value.as_ref().unwrap_or_else(|| panic!("{id}: no value"));
    for (codec, input) in [
      (Codec::Cbor, case.cbor.clone()),
      (Codec::Json, case.json.clone().map(String::into_bytes)),
    ] {
      let Some(input) = input else {
        continue;
      };
      if case.both {
        let encoded =
          encode(&case.ty, types, value, codec, &DecodeLimits::default()).unwrap_or_else(|error| panic!("{id}: {error}"));
        assert_eq!(encoded, input, "{id}: {} encoding", codec.name());
      }
      let back = decoded(case, types, codec, &input).unwrap_or_else(|error| panic!("{id}: {} {error}", codec.name()));
      assert!(same(&back, value), "{id}: {} decodes to {back:?}", codec.name());
    }
  }
}

fn same(left: &Value, right: &Value) -> bool {
  match (left, right) {
    (Value::Float(left), Value::Float(right)) => left.to_bits() == right.to_bits() || (left.is_nan() && right.is_nan()),
    (Value::Array(left), Value::Array(right)) => {
      left.len() == right.len() && left.iter().zip(right).all(|(left, right)| same(left, right))
    }
    (Value::Map(left), Value::Map(right)) => {
      left.len() == right.len()
        && left
          .iter()
          .zip(right)
          .all(|((left_key, left), (right_key, right))| same(left_key, right_key) && same(left, right))
    }
    (Value::Tag(left_tag, left), Value::Tag(right_tag, right)) => left_tag == right_tag && same(left, right),
    _ => left == right,
  }
}
