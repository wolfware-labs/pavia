//! The scalar forms of §12.3 that are not a single CBOR or JSON item of the same kind: timestamps,
//! UUIDs and 64-bit integers in `json`.

use half::f16;
use pavia_proto::data::Value;

/// The first instant RFC 3339 can write, 0000-01-01T00:00:00.000Z, in milliseconds since the epoch.
pub const MIN_MILLIS: i64 = -62_167_219_200_000;
/// The last instant RFC 3339 can write, 9999-12-31T23:59:59.999Z, in milliseconds since the epoch.
pub const MAX_MILLIS: i64 = 253_402_300_799_999;

const MILLIS_PER_DAY: i64 = 86_400_000;

/// The tag 1 content for `millis`: whole seconds as an integer, otherwise the narrowest float, half,
/// single or double, whose value rounds back to the same millisecond (§12.3).
pub fn epoch_seconds(millis: i64) -> Value {
  if millis % 1000 == 0 {
    return integer(i128::from(millis / 1000));
  }
  let seconds = millis as f64 / 1000.0;
  let half = f16::from_f64(seconds).to_f64();
  let single = f64::from(seconds as f32);
  let narrowest = [half, single]
    .into_iter()
    .find(|candidate| to_millis(*candidate) == Some(millis))
    .unwrap_or(seconds);
  Value::Float(narrowest)
}

/// The milliseconds of a tag 1 content: an integer or a float of seconds, within the RFC 3339 range.
pub fn millis_of_epoch_seconds(content: &Value) -> Option<i64> {
  match content {
    Value::Float(seconds) => to_millis(*seconds),
    other => integer_of(other)
      .and_then(|seconds| seconds.checked_mul(1000))
      .and_then(|millis| i64::try_from(millis).ok())
      .filter(|millis| (MIN_MILLIS..=MAX_MILLIS).contains(millis)),
  }
}

fn to_millis(seconds: f64) -> Option<i64> {
  let millis = (seconds * 1000.0).round();
  (millis.is_finite() && (MIN_MILLIS as f64..=MAX_MILLIS as f64).contains(&millis)).then_some(millis as i64)
}

/// `millis` as RFC 3339 in UTC with milliseconds: `2025-09-22T00:00:00.123Z`.
pub fn rfc3339(millis: i64) -> String {
  let days = millis.div_euclid(MILLIS_PER_DAY);
  let of_day = millis.rem_euclid(MILLIS_PER_DAY);
  let (year, month, day) = civil_from_days(days);
  format!(
    "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
    of_day / 3_600_000,
    of_day / 60_000 % 60,
    of_day / 1000 % 60,
    of_day % 1000
  )
}

/// The milliseconds of `text` in exactly the form [`rfc3339`] writes, or `None` for any other text,
/// an offset other than `Z` or a date that does not exist.
pub fn millis_of_rfc3339(text: &str) -> Option<i64> {
  let bytes = text.as_bytes();
  let shape = b"dddd-dd-ddTdd:dd:dd.dddZ";
  let fits = bytes.len() == shape.len()
    && bytes.iter().zip(shape).all(|(byte, expected)| {
      if *expected == b'd' {
        byte.is_ascii_digit()
      } else {
        byte == expected
      }
    });
  if !fits {
    return None;
  }
  let number = |range: std::ops::Range<usize>| text[range].parse::<i64>().ok();
  let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
  let (hour, minute, second, milli) = (number(11..13)?, number(14..16)?, number(17..19)?, number(20..23)?);
  let valid =
    (1..=12).contains(&month) && (1..=days_in_month(year, month)).contains(&day) && hour < 24 && minute < 60 && second < 60;
  valid.then(|| days_from_civil(year, month, day) * MILLIS_PER_DAY + ((hour * 60 + minute) * 60 + second) * 1000 + milli)
}

const fn days_in_month(year: i64, month: i64) -> i64 {
  match month {
    2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
    2 => 28,
    4 | 6 | 9 | 11 => 30,
    _ => 31,
  }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
  let year = if month <= 2 { year - 1 } else { year };
  let era = year.div_euclid(400);
  let year_of_era = year - era * 400;
  let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
  let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
  era * 146_097 + day_of_era - 719_468
}

/// The proleptic Gregorian date of `days` since 1970-01-01 (Howard Hinnant's `civil_from_days`).
const fn civil_from_days(days: i64) -> (i64, i64, i64) {
  let shifted = days + 719_468;
  let era = shifted.div_euclid(146_097);
  let day_of_era = shifted - era * 146_097;
  let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
  let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
  let shifted_month = (5 * day_of_year + 2) / 153;
  let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
  let month = if shifted_month < 10 {
    shifted_month + 3
  } else {
    shifted_month - 9
  };
  (
    if month <= 2 {
      year_of_era + era * 400 + 1
    } else {
      year_of_era + era * 400
    },
    month,
    day,
  )
}

/// The integer of an `Unsigned` or `Negative` value.
pub fn integer_of(value: &Value) -> Option<i128> {
  match value {
    Value::Unsigned(n) => Some(i128::from(*n)),
    Value::Negative(n) => Some(-1 - i128::from(*n)),
    _ => None,
  }
}

/// `number` as an `Unsigned` or `Negative` value. It must lie in −2^64..2^64, as every contract
/// integer does.
pub fn integer(number: i128) -> Value {
  match u64::try_from(number) {
    Ok(unsigned) => Value::Unsigned(unsigned),
    Err(_) => Value::Negative(u64::try_from(-1 - number).unwrap_or(u64::MAX)),
  }
}

/// The integer of a decimal string in the one form `json` writes 64-bit integers in:
/// `0|-?[1-9][0-9]*`, with no sign on zero, no leading zeros and nothing around it.
pub fn integer_of_text(text: &str) -> Option<i128> {
  let digits = text.strip_prefix('-').unwrap_or(text);
  let canonical = match digits.as_bytes() {
    [b'0'] => digits.len() == text.len(),
    [b'1'..=b'9', rest @ ..] => rest.iter().all(u8::is_ascii_digit) && digits.len() <= 20,
    _ => false,
  };
  canonical.then(|| text.parse().ok()).flatten()
}

/// `bytes` as a lowercase hyphenated UUID: `7d444840-9dc0-11d1-b245-5ffdce74fad2`.
pub fn uuid_text(bytes: &[u8; 16]) -> String {
  let hex = data_encoding::HEXLOWER.encode(bytes);
  format!(
    "{}-{}-{}-{}-{}",
    &hex[0..8],
    &hex[8..12],
    &hex[12..16],
    &hex[16..20],
    &hex[20..32]
  )
}

/// The 16 bytes of a lowercase hyphenated UUID, or `None` for any other text.
pub fn uuid_of_text(text: &str) -> Option<[u8; 16]> {
  let bytes = text.as_bytes();
  let hyphens = [8, 13, 18, 23];
  if bytes.len() != 36 || hyphens.iter().any(|at| bytes[*at] != b'-') {
    return None;
  }
  let hex: String = text.chars().filter(|char| *char != '-').collect();
  data_encoding::HEXLOWER
    .decode(hex.as_bytes())
    .ok()
    .and_then(|decoded| decoded.try_into().ok())
}

#[cfg(test)]
mod tests;
