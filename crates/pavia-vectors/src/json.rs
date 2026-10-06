//! The `json` codec of spec 12.3, written as a test oracle: renders a CBOR value of a given
//! contract type as the canonical `json` text (compact, RFC 8785 strings and numbers, struct
//! members in declaration order, `map` members sorted by UTF-8 key bytes).

use ciborium::Value;
use serde_json::{Map, Value as Json};

/// Renders `value`, whose contract type is `ty` (manifest syntax), as canonical `json` text.
/// `types` is the `types` object of the vector file, for `{"ref": ...}`.
pub fn render(ty: &Json, value: &Value, types: &Map<String, Json>) -> Result<String, String> {
  let mut out = String::new();
  write(&mut out, ty, value, types)?;
  Ok(out)
}

fn write(out: &mut String, ty: &Json, v: &Value, types: &Map<String, Json>) -> Result<(), String> {
  if let Some(name) = ty.as_str() {
    return primitive(out, name, v);
  }
  let obj = ty.as_object().ok_or_else(|| format!("bad type {ty}"))?;
  let (kind, arg) = obj.iter().next().ok_or("empty type object")?;
  match kind.as_str() {
    "ref" => {
      let name = arg.as_str().ok_or("ref needs a name")?;
      let def = types.get(name).ok_or_else(|| format!("unknown type {name}"))?;
      write(out, def, v, types)
    }
    "alias" => write(out, arg, v, types),
    "option" => match v {
      Value::Null => {
        out.push_str("null");
        Ok(())
      }
      _ => write(out, arg, v, types),
    },
    "list" => {
      let items = v.as_array().ok_or("list expects an array")?;
      out.push('[');
      for (i, item) in items.iter().enumerate() {
        if i > 0 {
          out.push(',');
        }
        write(out, arg, item, types)?;
      }
      out.push(']');
      Ok(())
    }
    "map" => {
      let entries = v.as_map().ok_or("map expects a map")?;
      let mut sorted: Vec<(&str, &Value)> = entries
        .iter()
        .map(|(k, val)| Ok((k.as_text().ok_or("map keys are text")?, val)))
        .collect::<Result<_, String>>()?;
      sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
      out.push('{');
      for (i, (k, val)) in sorted.iter().enumerate() {
        if i > 0 {
          out.push(',');
        }
        string(out, k);
        out.push(':');
        write(out, arg, val, types)?;
      }
      out.push('}');
      Ok(())
    }
    "struct" => {
      let fields = arg["fields"].as_array().ok_or("struct needs fields")?;
      let entries = v.as_map().ok_or("struct expects a map")?;
      out.push('{');
      let mut first = true;
      for field in fields {
        let name = field["name"].as_str().ok_or("field needs a name")?;
        let Some((_, val)) = entries.iter().find(|(k, _)| k.as_text() == Some(name)) else {
          continue; // absent optional field
        };
        if !first {
          out.push(',');
        }
        first = false;
        string(out, name);
        out.push(':');
        write(out, &field["type"], val, types)?;
      }
      out.push('}');
      Ok(())
    }
    "enum" => {
      let variants = arg["variants"].as_array().ok_or("enum needs variants")?;
      match v {
        Value::Text(name) => {
          string(out, name);
          Ok(())
        }
        Value::Map(entries) if entries.len() == 1 => {
          let (k, val) = &entries[0];
          let name = k.as_text().ok_or("variant name is text")?;
          let variant = variants
            .iter()
            .find(|var| var["name"] == name)
            .ok_or_else(|| format!("unknown variant {name}"))?;
          out.push('{');
          string(out, name);
          out.push(':');
          write(out, &variant["type"], val, types)?;
          out.push('}');
          Ok(())
        }
        _ => Err("enum expects text or a one-entry map".into()),
      }
    }
    other => Err(format!("type {other} not supported by the oracle")),
  }
}

fn primitive(out: &mut String, name: &str, v: &Value) -> Result<(), String> {
  match (name, v) {
    ("unit", Value::Null) => out.push_str("null"),
    ("bool", Value::Bool(b)) => out.push_str(if *b { "true" } else { "false" }),
    ("u8" | "u16" | "u32" | "i8" | "i16" | "i32", Value::Integer(i)) => {
      out.push_str(&i128::from(*i).to_string());
    }
    ("u64" | "i64", Value::Integer(i)) => {
      out.push('"');
      out.push_str(&i128::from(*i).to_string());
      out.push('"');
    }
    ("f32" | "f64", Value::Float(f)) => out.push_str(&number(*f)?),
    ("string", Value::Text(s)) => string(out, s),
    ("bytes", Value::Bytes(b)) => {
      out.push('"');
      out.push_str(&base64url(b));
      out.push('"');
    }
    ("timestamp", Value::Tag(1, inner)) => {
      let secs = match inner.as_ref() {
        Value::Integer(i) => i128::from(*i) as f64,
        Value::Float(f) => *f,
        _ => return Err("timestamp tag 1 holds a number".into()),
      };
      out.push('"');
      out.push_str(&rfc3339_millis(secs));
      out.push('"');
    }
    ("uuid", Value::Tag(37, inner)) => {
      let b = inner.as_bytes().filter(|b| b.len() == 16).ok_or("uuid is 16 bytes")?;
      let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
      out.push('"');
      out.push_str(&format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
      ));
      out.push('"');
    }
    _ => return Err(format!("{name} does not accept {v:?}")),
  }
  Ok(())
}

/// A JSON string as RFC 8785 serializes it (the ECMAScript `JSON.stringify` rules).
fn string(out: &mut String, s: &str) {
  out.push_str(&serde_json::to_string(s).expect("strings always serialize"));
}

/// A number as RFC 8785 serializes it: the ECMAScript `Number.prototype.toString` format.
fn number(x: f64) -> Result<String, String> {
  if !x.is_finite() {
    return Err("NaN and infinities have no json form".into());
  }
  if x == 0.0 {
    return Ok("0".into());
  }
  let sign = if x < 0.0 { "-" } else { "" };
  // `{:e}` gives the shortest round-trip digits: "d.ddde<exp>".
  let sci = format!("{:e}", x.abs());
  let (mantissa, exp) = sci.split_once('e').expect("exponent form");
  let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
  let k = digits.len() as i32;
  let n = exp.parse::<i32>().expect("exponent") + 1;
  let body = if k <= n && n <= 21 {
    format!("{digits}{}", "0".repeat((n - k) as usize))
  } else if 0 < n && n <= 21 {
    format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
  } else if -6 < n && n <= 0 {
    format!("0.{}{digits}", "0".repeat((-n) as usize))
  } else {
    let e = n - 1;
    let tail = if k > 1 { format!(".{}", &digits[1..]) } else { String::new() };
    format!("{}{tail}e{}{}", &digits[..1], if e < 0 { "-" } else { "+" }, e.abs())
  };
  Ok(format!("{sign}{body}"))
}

fn base64url(bytes: &[u8]) -> String {
  const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
  let mut out = String::new();
  for chunk in bytes.chunks(3) {
    let n = chunk.iter().fold(0u32, |acc, b| (acc << 8) | u32::from(*b)) << (8 * (3 - chunk.len()));
    for i in 0..=chunk.len() {
      out.push(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize] as char);
    }
  }
  out
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` for seconds since the Unix epoch.
fn rfc3339_millis(secs: f64) -> String {
  let millis = (secs * 1000.0).round() as i64;
  let (days, ms_of_day) = (millis.div_euclid(86_400_000), millis.rem_euclid(86_400_000));
  // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
  let z = days + 719_468;
  let era = z.div_euclid(146_097);
  let doe = z.rem_euclid(146_097);
  let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
  let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
  let mp = (5 * doy + 2) / 153;
  let day = doy - (153 * mp + 2) / 5 + 1;
  let month = if mp < 10 { mp + 3 } else { mp - 9 };
  let year = yoe + era * 400 + i64::from(month <= 2);
  let (h, m, s, ms) = (
    ms_of_day / 3_600_000,
    ms_of_day / 60_000 % 60,
    ms_of_day / 1000 % 60,
    ms_of_day % 1000,
  );
  format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{ms:03}Z")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn numbers_follow_ecmascript() {
    for (x, s) in [
      (1.5, "1.5"),
      (100000.0, "100000"),
      (0.1, "0.1"),
      (1e21, "1e+21"),
      (0.000001, "0.000001"),
      (1e-7, "1e-7"),
      (-0.0, "0"),
      (3.4028234663852886e38, "3.4028234663852886e+38"),
    ] {
      assert_eq!(number(x).unwrap(), s, "{x}");
    }
  }

  #[test]
  fn dates_and_base64url() {
    assert_eq!(rfc3339_millis(0.0), "1970-01-01T00:00:00.000Z");
    assert_eq!(rfc3339_millis(1758499200.123), "2025-09-22T00:00:00.123Z");
    assert_eq!(base64url(&[0xfb, 0xff, 0x00, 0x01]), "-_8AAQ");
  }
}
