use pavia_proto::data::Value;
use proptest::prelude::*;

use super::{
  MAX_MILLIS, MIN_MILLIS, epoch_seconds, integer, integer_of, integer_of_text, millis_of_epoch_seconds, millis_of_rfc3339,
  rfc3339, uuid_of_text, uuid_text,
};

#[test]
fn writes_rfc3339_in_utc_with_milliseconds() {
  for (millis, text) in [
    (0, "1970-01-01T00:00:00.000Z"),
    (-1, "1969-12-31T23:59:59.999Z"),
    (1_758_499_200_123, "2025-09-22T00:00:00.123Z"),
    (951_782_400_000, "2000-02-29T00:00:00.000Z"),
    (1_709_164_799_999, "2024-02-28T23:59:59.999Z"),
    (MIN_MILLIS, "0000-01-01T00:00:00.000Z"),
    (MAX_MILLIS, "9999-12-31T23:59:59.999Z"),
  ] {
    assert_eq!(rfc3339(millis), text);
    assert_eq!(millis_of_rfc3339(text), Some(millis), "{text}");
  }
}

#[test]
fn refuses_rfc3339_text_in_any_other_form() {
  for text in [
    "2025-09-22T00:00:00Z",
    "2025-09-22T00:00:00.12Z",
    "2025-09-22T00:00:00.1234Z",
    "2025-09-22T01:00:00.000+01:00",
    "2025-09-22t00:00:00.000z",
    "2025-09-22 00:00:00.000Z",
    "2023-02-29T00:00:00.000Z",
    "2100-02-29T00:00:00.000Z",
    "2025-13-01T00:00:00.000Z",
    "2025-00-01T00:00:00.000Z",
    "2025-04-31T00:00:00.000Z",
    "2025-09-00T00:00:00.000Z",
    "2025-09-22T24:00:00.000Z",
    "2025-09-22T00:60:00.000Z",
    "2025-09-22T00:00:60.000Z",
    "+025-09-22T00:00:00.000Z",
    "",
  ] {
    assert_eq!(millis_of_rfc3339(text), None, "{text}");
  }
  assert_eq!(millis_of_rfc3339("2024-02-29T00:00:00.000Z"), Some(1_709_164_800_000));
}

#[test]
fn writes_whole_seconds_as_integers_and_others_as_the_narrowest_exact_float() {
  assert_eq!(epoch_seconds(1_758_499_200_000), Value::Unsigned(1_758_499_200));
  assert_eq!(epoch_seconds(-2000), Value::Negative(1));
  assert_eq!(epoch_seconds(500), Value::Float(0.5));
  assert_eq!(epoch_seconds(123), Value::Float(half::f16::from_f64(0.123).to_f64()));
  assert_eq!(epoch_seconds(100_100), Value::Float(f64::from(100.1_f32)));
  assert_eq!(epoch_seconds(1_758_499_200_123), Value::Float(1_758_499_200.123));
}

#[test]
fn reads_epoch_seconds_within_the_rfc3339_range() {
  assert_eq!(millis_of_epoch_seconds(&Value::Unsigned(1)), Some(1000));
  assert_eq!(millis_of_epoch_seconds(&Value::Negative(0)), Some(-1000));
  assert_eq!(millis_of_epoch_seconds(&Value::Float(0.0005)), Some(1));
  assert_eq!(millis_of_epoch_seconds(&Value::Float(-0.25)), Some(-250));
  let last = Value::Unsigned(u64::try_from(MAX_MILLIS / 1000).unwrap());
  assert_eq!(millis_of_epoch_seconds(&last), Some(MAX_MILLIS - 999));
  for out in [
    Value::Unsigned(u64::try_from(MAX_MILLIS / 1000 + 1).unwrap()),
    Value::Negative(u64::try_from(-MIN_MILLIS / 1000).unwrap()),
    Value::Unsigned(u64::MAX),
    Value::Negative(u64::MAX),
    Value::Float(f64::NAN),
    Value::Float(f64::INFINITY),
    Value::Float(1e300),
    Value::Text("0".into()),
  ] {
    assert_eq!(millis_of_epoch_seconds(&out), None, "{out:?}");
  }
}

#[test]
fn reads_and_writes_integers() {
  for (number, value) in [
    (0, Value::Unsigned(0)),
    (-1, Value::Negative(0)),
    (i128::from(u64::MAX), Value::Unsigned(u64::MAX)),
    (-1 - i128::from(u64::MAX), Value::Negative(u64::MAX)),
  ] {
    assert_eq!(integer(number), value);
    assert_eq!(integer_of(&value), Some(number));
  }
  assert_eq!(integer_of(&Value::Float(1.0)), None);
}

#[test]
fn reads_decimal_text_in_its_one_form_only() {
  for (text, number) in [
    ("0", 0),
    ("7", 7),
    ("-7", -7),
    ("18446744073709551615", i128::from(u64::MAX)),
    ("-9223372036854775808", i128::from(i64::MIN)),
  ] {
    assert_eq!(integer_of_text(text), Some(number), "{text}");
  }
  for text in [
    "",
    "-",
    "-0",
    "+5",
    "05",
    "0x10",
    " 5",
    "5 ",
    "1e3",
    "1.0",
    "123456789012345678901",
  ] {
    assert_eq!(integer_of_text(text), None, "{text}");
  }
}

#[test]
fn reads_and_writes_lowercase_hyphenated_uuids() {
  let bytes = [
    0x7d, 0x44, 0x48, 0x40, 0x9d, 0xc0, 0x11, 0xd1, 0xb2, 0x45, 0x5f, 0xfd, 0xce, 0x74, 0xfa, 0xd2,
  ];
  let text = "7d444840-9dc0-11d1-b245-5ffdce74fad2";
  assert_eq!(uuid_text(&bytes), text);
  assert_eq!(uuid_of_text(text), Some(bytes));
  for text in [
    "7D444840-9DC0-11D1-B245-5FFDCE74FAD2",
    "7d4448409dc011d1b2455ffdce74fad2",
    "7d444840-9dc0-11d1-b245-5ffdce74fad",
    "7d444840-9dc0-11d1-b245-5ffdce74-ad2",
    "7d444840-9dc0-11d1-b245_5ffdce74fad2",
    "{7d444840-9dc0-11d1-b245-5ffdce74fad2}",
  ] {
    assert_eq!(uuid_of_text(text), None, "{text}");
  }
}

proptest! {
  #[test]
  fn every_instant_round_trips_through_both_forms(millis in MIN_MILLIS..=MAX_MILLIS) {
    prop_assert_eq!(millis_of_rfc3339(&rfc3339(millis)), Some(millis));
    prop_assert_eq!(millis_of_epoch_seconds(&epoch_seconds(millis)), Some(millis));
  }
}
