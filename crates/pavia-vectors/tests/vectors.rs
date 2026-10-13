//! Checks every file under `vectors/` against an independent CBOR implementation (`ciborium`).

use std::collections::HashSet;

use ciborium::Value;
use pavia_vectors::diag::{self, Diag};
use pavia_vectors::{files, hex, load};
use serde_json::Value as Json;

/// QUIC variable-length integer (RFC 9000 section 16), shortest form.
fn varint(n: u64) -> Vec<u8> {
  match n {
    0..=0x3f => vec![n as u8],
    0x40..=0x3fff => ((n as u16) | 0x4000).to_be_bytes().to_vec(),
    0x4000..=0x3fff_ffff => ((n as u32) | 0x8000_0000).to_be_bytes().to_vec(),
    _ => (n | 0xc000_0000_0000_0000).to_be_bytes().to_vec(),
  }
}

/// Reads a varint at `pos`, returning the value and the position after it.
fn read_varint(buf: &[u8], pos: usize) -> (u64, usize) {
  let len = 1 << (buf[pos] >> 6);
  let mut v = u64::from(buf[pos] & 0x3f);
  for b in &buf[pos + 1..pos + len] {
    v = (v << 8) | u64::from(*b);
  }
  (v, pos + len)
}

fn encode(value: &Value) -> Vec<u8> {
  let mut out = Vec::new();
  ciborium::into_writer(value, &mut out).expect("ciborium encodes any Value");
  out
}

fn decode(bytes: &[u8]) -> Value {
  ciborium::from_reader(bytes).expect("valid CBOR")
}

/// Equality that ignores map key order, for `decode` cases whose input is not in canonical order.
fn same(a: &Value, b: &Value) -> bool {
  match (a, b) {
    (Value::Map(x), Value::Map(y)) => {
      x.len() == y.len() && x.iter().all(|(k, v)| y.iter().any(|(k2, v2)| same(k, k2) && same(v, v2)))
    }
    (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
    (Value::Tag(t, x), Value::Tag(u, y)) => t == u && same(x, y),
    (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits() || x == y,
    _ => a == b,
  }
}

fn plain(text: &str, ctx: &str) -> Value {
  let d = diag::parse(text).unwrap_or_else(|e| panic!("{ctx}: {e} in {text:?}"));
  d.to_cbor().unwrap_or_else(|e| panic!("{ctx}: {e}"))
}

fn str_field<'a>(case: &'a Json, key: &str) -> &'a str {
  case[key].as_str().unwrap_or_else(|| panic!("{key} missing in {case}"))
}

/// Builds the bytes of one frame from its decoded form (spec 5.2, and 5.4 for the stream ID).
fn frame_bytes(frame: &Json, binding: &str, ctx: &str) -> Vec<u8> {
  let header = match frame["header"].as_str() {
    Some(h) => encode(&plain(h, ctx)),
    None => Vec::new(),
  };
  let data = match (frame["data"].as_str(), frame["data_hex"].as_str()) {
    (Some(d), _) if frame["codec"] == "json" => d.as_bytes().to_vec(),
    (Some(d), _) => encode(&plain(d, ctx)),
    (None, Some(h)) => hex(h),
    (None, None) => Vec::new(),
  };
  let mut payload = varint(header.len() as u64);
  payload.extend(&header);
  payload.extend(&data);
  let mut out = Vec::new();
  if binding == "ws" {
    out.extend(varint(frame["stream"].as_u64().expect("ws frames carry stream")));
  }
  out.push(frame["type"].as_u64().expect("type") as u8);
  out.push(frame["flags"].as_u64().expect("flags") as u8);
  out.extend(varint(payload.len() as u64));
  out.extend(payload);
  out
}

/// Splits `buf` into (stream, header bytes) per frame, for decode-only cases.
fn frame_headers(buf: &[u8], binding: &str) -> Vec<Vec<u8>> {
  let mut pos = 0;
  let mut out = Vec::new();
  while pos < buf.len() {
    if binding == "ws" {
      pos = read_varint(buf, pos).1;
    }
    let (len, start) = read_varint(buf, pos + 2);
    let (hlen, hstart) = read_varint(buf, start);
    out.push(buf[hstart..hstart + hlen as usize].to_vec());
    pos = start + len as usize;
  }
  out
}

fn unique_ids(kind: &str) -> Vec<(String, Json)> {
  let mut seen = HashSet::new();
  let mut cases = Vec::new();
  for path in files(kind) {
    let doc = load(&path);
    let name = path.file_stem().unwrap().to_string_lossy().into_owned();
    assert_eq!(doc["group"], name.as_str(), "{}: group must match file name", path.display());
    for case in doc["cases"].as_array().expect("cases") {
      let id = str_field(case, "id").to_owned();
      assert!(seen.insert(id.clone()), "{kind}: duplicate id {id}");
      cases.push((format!("{kind}/{name}.json#{id}"), case.clone()));
    }
  }
  assert!(!cases.is_empty(), "no {kind} vectors found");
  cases
}

#[test]
fn frame_vectors_match_their_hex() {
  for (ctx, case) in unique_ids("frames") {
    let Some(frames) = case["frames"].as_array() else {
      continue;
    }; // error cases
    let binding = str_field(&case, "binding");
    let bytes = hex(str_field(&case, "hex"));
    if case["direction"] == "both" {
      let built: Vec<u8> = frames.iter().flat_map(|f| frame_bytes(f, binding, &ctx)).collect();
      assert_eq!(built, bytes, "{ctx}: decoded form does not encode to hex");
    } else if !frames.is_empty() {
      let headers = frame_headers(&bytes, binding);
      assert_eq!(headers.len(), frames.len(), "{ctx}: frame count");
      for (raw, frame) in headers.iter().zip(frames) {
        if let Some(h) = frame["header"].as_str() {
          assert!(same(&decode(raw), &plain(h, &ctx)), "{ctx}: header differs");
        }
      }
    }
  }
}

#[test]
fn codec_vectors_match_their_cbor_and_json() {
  let cases = unique_ids("codecs");
  for path in files("codecs") {
    let doc = load(&path);
    let empty = serde_json::Map::new();
    let types = doc["types"].as_object().unwrap_or(&empty);
    for case in doc["cases"].as_array().expect("cases") {
      let ctx = format!("{}#{}", path.display(), str_field(case, "id"));
      if let Some(json) = case["json"].as_str() {
        serde_json::from_str::<Json>(json).unwrap_or_else(|e| panic!("{ctx}: json {e}"));
      }
      let (Some(value), Some(cbor)) = (case["value"].as_str(), case["cbor"].as_str()) else {
        continue;
      };
      let expected = plain(value, &ctx);
      let bytes = hex(cbor);
      if case["direction"] == "both" {
        assert_eq!(encode(&expected), bytes, "{ctx}: value does not encode to cbor");
        if let Some(json) = case["json"].as_str() {
          let rendered = pavia_vectors::json::render(&case["type"], &expected, types).unwrap_or_else(|e| panic!("{ctx}: {e}"));
          assert_eq!(rendered, json, "{ctx}: json is not the canonical form of value");
        }
      }
      assert!(same(&decode(&bytes), &expected), "{ctx}: cbor does not decode to value");
    }
  }
  assert!(!cases.is_empty());
}

#[test]
fn scripts_parse_and_captures_resolve() {
  for (ctx, case) in unique_ids("scripts") {
    let mut captured: HashSet<String> = HashSet::new();
    let steps = case["steps"].as_array().expect("steps");
    for (i, step) in steps.iter().enumerate() {
      let (verb, body) = step.as_object().and_then(|o| o.iter().next()).expect("one verb");
      if verb != "send" && verb != "expect" {
        continue;
      }
      for key in ["header", "data"] {
        let Some(text) = body[key].as_str() else {
          continue;
        };
        let d = diag::parse(text).unwrap_or_else(|e| panic!("{ctx} step {i}: {e}"));
        if verb == "send" {
          assert_eq!(
            count_patterns(&d),
            d.refs().len(),
            "{ctx} step {i}: send may use $name but not * or ..."
          );
        }
        for name in d.refs() {
          assert!(captured.contains(name), "{ctx} step {i}: ${name} used before capture");
        }
      }
      if let Some(capture) = body["capture"].as_object() {
        captured.extend(capture.keys().cloned());
      }
    }
    if ctx.contains("abuse") {
      let last = steps.last().and_then(|s| s.as_object()).expect("steps");
      assert!(
        last.contains_key("expect_close"),
        "{ctx}: abuse script must end with expect_close"
      );
    }
  }
}

#[test]
fn varint_vectors_match_their_hex() {
  const MAX: u64 = (1 << 62) - 1;
  for (ctx, case) in unique_ids("varints") {
    let number = |key: &str| {
      case[key]
        .as_str()
        .map(|v| v.parse::<u64>().unwrap_or_else(|e| panic!("{ctx}: {key}: {e}")))
    };
    match (case["hex"].as_str().map(hex), number("value"), number("max")) {
      (Some(bytes), None, None) if case["need_more"] == true => {
        assert!(bytes.len() < 1 << (bytes[0] >> 6), "{ctx}: input is not truncated");
      }
      (Some(bytes), None, Some(max)) => {
        let (value, end) = read_varint(&bytes, 0);
        assert_eq!(end, bytes.len(), "{ctx}: hex is one complete varint");
        assert!(value > max, "{ctx}: value is within max");
      }
      (Some(bytes), Some(value), max) => {
        assert_eq!(read_varint(&bytes, 0), (value, bytes.len()), "{ctx}: decode");
        assert!(max.is_none_or(|max| value <= max), "{ctx}: value is above max");
        if str_field(&case, "direction") == "both" {
          assert_eq!(varint(value), bytes, "{ctx}: encode");
        } else {
          assert_ne!(varint(value), bytes, "{ctx}: a decode-only case is not the shortest form");
        }
      }
      (None, Some(value), None) => assert!(value > MAX, "{ctx}: value has an encoding"),
      _ => panic!("{ctx}: unknown case shape"),
    }
  }
}

/// Number of pattern elements (`*`, `$name`, open maps) in an item.
fn count_patterns(d: &Diag) -> usize {
  match d {
    Diag::Any | Diag::Ref(_) => 1,
    Diag::Map { entries, open } => {
      usize::from(*open)
        + entries
          .iter()
          .map(|(k, v)| count_patterns(k) + count_patterns(v))
          .sum::<usize>()
    }
    Diag::Array(items) => items.iter().map(count_patterns).sum(),
    Diag::Tag(_, inner) => count_patterns(inner),
    _ => 0,
  }
}

/// The oracle relies on `ciborium` choosing the shortest exact float width (spec 12.3), as the
/// float codec vectors require. Pinned here so a dependency update that changes it fails loudly.
#[test]
fn ciborium_writes_shortest_float_width() {
  for (x, expected) in [
    (1.5_f64, &[0xf9, 0x3e, 0x00][..]),
    (100000.0, &[0xfa, 0x47, 0xc3, 0x50, 0x00][..]),
    (0.1, &[0xfb, 0x3f, 0xb9, 0x99, 0x99, 0x99, 0x99, 0x99, 0x9a][..]),
  ] {
    assert_eq!(encode(&Value::Float(x)), expected, "{x}");
  }
}
