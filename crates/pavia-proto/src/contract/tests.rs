use super::Fingerprint;

#[test]
fn shows_the_fingerprint_in_hex_in_debug_output_too() {
  let mut bytes = [0; 32];
  bytes[0] = 0x0f;
  let fingerprint = Fingerprint::new(bytes);
  assert_eq!(fingerprint.bytes(), bytes);
  assert_eq!(format!("{fingerprint:?}"), format!("Fingerprint(0f{})", "0".repeat(62)));
}
