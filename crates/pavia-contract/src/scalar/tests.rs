use super::{Bytes, Timestamp, Uuid};

#[test]
fn keeps_the_bytes_it_was_given() {
  let bytes = Bytes::new(vec![1, 2, 3]);
  assert_eq!(bytes.as_slice(), [1, 2, 3]);
  assert_eq!(Bytes::from(vec![1, 2, 3]), bytes);
  assert_eq!(bytes.into_vec(), [1, 2, 3]);
}

#[test]
fn counts_milliseconds_from_the_unix_epoch() {
  assert_eq!(Timestamp::from_unix_millis(-5).unix_millis(), -5);
  assert!(Timestamp::from_unix_millis(1) < Timestamp::from_unix_millis(2));
}

#[test]
fn keeps_the_uuid_bytes_in_order() {
  let bytes: [u8; 16] = std::array::from_fn(|index| u8::try_from(index).unwrap());
  assert_eq!(Uuid::from_bytes(bytes).bytes(), bytes);
}
