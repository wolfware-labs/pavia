mod runner;

use std::future::Future;
use std::pin::Pin;

use pavia_proto::registry::CloseCode;
use pavia_server::websocket::{Config, Endpoint, Received, Transport};
use runner::{Failure, Mode, Script, Verdict};
use serde_json::{Value, json};

type Session = Pin<Box<dyn Future<Output = ()> + Send>>;

const CLOSE_STREAM: u8 = 9;

async fn echo(mut transport: Transport) {
  while let Some(Ok(Received::Binary(message))) = transport.recv().await {
    if message.first() == Some(&CLOSE_STREAM) {
      if let Ok(code) = CloseCode::try_from(u16::from(message[2])) {
        let _ = transport.close(code, "").await;
      }
      return;
    }
    if transport.send(message).await.is_err() {
      return;
    }
  }
}

fn endpoint() -> Endpoint<fn(Transport) -> Session> {
  Endpoint::new(Config::new("/rt"), |transport| Box::pin(echo(transport)))
}

fn script(steps: Value) -> Script {
  Script::parse(&json!({
    "id": "self-test", "spec": ["17.2"], "note": "", "binding": "ws", "codec": "cbor", "steps": steps,
  }))
  .unwrap()
}

fn both(script: &Script) -> Result<Verdict, Failure> {
  let in_memory = runner::run(script, Mode::InMemory, endpoint(), &[]);
  let socket = runner::run(script, Mode::Socket, endpoint(), &[]);
  assert_eq!(in_memory, socket, "the two modes disagree");
  in_memory
}

fn frame(stream: u64, frame_type: u64, header: Option<&str>, data: Option<&str>) -> Value {
  json!({"stream": stream, "type": frame_type, "header": header, "data": data})
}

#[test]
fn echoes_with_captures_and_wildcards() {
  let mut expect = frame(2, 16, Some("{2: *, ...}"), Some("[*, 1]"));
  expect["capture"] = json!({"method": "header.2", "first": "data.0"});
  let steps = json!([
    {"send": frame(2, 16, Some("{2: \"chat/send\", 5: 0}"), Some("[\"a\", 1]"))},
    {"expect": expect},
    {"send": frame(4, 16, Some("{2: $method}"), Some("[$first]"))},
    {"expect": frame(4, 16, Some("{2: \"chat/send\"}"), Some("[\"a\"]"))},
  ]);
  assert_eq!(both(&script(steps)), Ok(Verdict::Passed));
}

#[test]
fn matches_frames_per_stream() {
  let steps = json!([
    {"send": frame(2, 16, None, Some("1"))},
    {"send": frame(4, 16, None, Some("2"))},
    {"expect": frame(4, 16, None, Some("2"))},
    {"expect": frame(2, 16, None, Some("1"))},
  ]);
  assert_eq!(both(&script(steps)), Ok(Verdict::Passed));
}

#[test]
fn answers_pings_without_passing_them_to_expect() {
  let steps = json!([
    {"send": frame(0, 4, None, None)},
    {"expect_nothing_within": {"ms": 200}},
    {"send": frame(2, 16, None, None)},
    {"expect": frame(2, 16, None, None)},
  ]);
  assert_eq!(both(&script(steps)), Ok(Verdict::Passed));
}

#[test]
fn passes_a_ping_to_an_expect_for_ping() {
  let steps = json!([
    {"send": frame(0, 4, None, None)},
    {"expect": frame(0, 4, None, None)},
  ]);
  assert_eq!(both(&script(steps)), Ok(Verdict::Passed));
}

#[test]
fn sends_each_repeat_as_its_own_message() {
  let mut send = frame(2, 16, None, Some("7"));
  send["repeat"] = json!(3);
  let steps = json!([
    {"send": send},
    {"expect": frame(2, 16, None, Some("7"))},
    {"expect": frame(2, 16, None, Some("7"))},
    {"expect": frame(2, 16, None, Some("7"))},
    {"expect_nothing_within": {"ms": 100}},
  ]);
  assert_eq!(both(&script(steps)), Ok(Verdict::Passed));
}

#[test]
fn opens_a_new_transport_after_close_transport() {
  let steps = json!([
    {"send": frame(2, 16, None, Some("1"))},
    {"close_transport": {}},
    {"send": frame(2, 16, None, Some("2"))},
    {"expect": frame(2, 16, None, Some("2"))},
  ]);
  assert_eq!(both(&script(steps)), Ok(Verdict::Passed));
}

#[test]
fn sees_the_close_code() {
  let mut close = frame(u64::from(CLOSE_STREAM), 16, None, None);
  close["flags"] = json!(10);
  let steps = json!([
    {"send": close},
    {"expect_close": {"code": "LIMIT_EXCEEDED", "within_ms": 1000}},
  ]);
  assert_eq!(both(&script(steps)), Ok(Verdict::Passed));
}

#[test]
fn compares_raw_and_json_data_exactly() {
  let mut raw = frame(0, 4, None, None);
  raw["data_hex"] = json!("01 02");
  let mut text = frame(2, 16, None, Some("{\"a\":1}"));
  text["codec"] = json!("json");
  let steps = json!([
    {"send": raw.clone()},
    {"expect": raw},
    {"send": text.clone()},
    {"expect": text},
  ]);
  assert_eq!(both(&script(steps)), Ok(Verdict::Passed));
}

#[test]
fn advances_the_clock_in_memory() {
  let steps = json!([
    {"send": frame(2, 16, None, None)},
    {"wait": {"ms": 600_000}},
    {"expect": frame(2, 16, None, None)},
    {"expect_nothing_within": {"ms": 600_000}},
  ]);
  let started = std::time::Instant::now();
  assert_eq!(
    runner::run(&script(steps), Mode::InMemory, endpoint(), &[]),
    Ok(Verdict::Passed)
  );
  assert!(started.elapsed().as_secs() < 5);
}

#[test]
fn names_the_step_that_fails() {
  let steps = json!([
    {"send": frame(2, 16, Some("{2: \"a\"}"), None)},
    {"expect": frame(2, 16, Some("{2: \"b\"}"), None)},
  ]);
  let failure = both(&script(steps)).unwrap_err();
  assert_eq!((failure.step, failure.verb), (2, "expect"));
  assert!(failure.message.starts_with("header does not match"), "{failure}");
  assert!(failure.to_string().starts_with("self-test: step 2 (expect): "), "{failure}");
}

#[test]
fn fails_on_a_closed_map_with_extra_keys() {
  let steps = json!([
    {"send": frame(2, 16, Some("{2: \"a\", 5: 0}"), None)},
    {"expect": frame(2, 16, Some("{2: \"a\"}"), None)},
  ]);
  assert_eq!(both(&script(steps)).unwrap_err().step, 2);
}

#[test]
fn fails_when_something_arrives_during_expect_nothing_within() {
  let steps = json!([
    {"send": frame(2, 16, None, None)},
    {"expect_nothing_within": {"ms": 200}},
  ]);
  let failure = both(&script(steps)).unwrap_err();
  assert_eq!((failure.step, failure.verb), (2, "expect_nothing_within"));
}

#[test]
fn fails_on_another_close_code() {
  let mut close = frame(u64::from(CLOSE_STREAM), 16, None, None);
  close["flags"] = json!(1);
  let steps = json!([
    {"send": close},
    {"expect_close": {"code": "LIMIT_EXCEEDED", "within_ms": 1000}},
  ]);
  let failure = both(&script(steps)).unwrap_err();
  assert_eq!(failure.message, "closed with code 4001, expected 4010");
}

#[test]
fn fails_when_nothing_arrives() {
  let steps = json!([
    {"send": frame(2, 16, None, None)},
    {"expect": frame(4, 16, None, None)},
  ]);
  let failure = runner::run(&script(steps), Mode::InMemory, endpoint(), &[]).unwrap_err();
  assert_eq!(failure.message, "nothing arrived on stream 4 within 5000 ms");
}

#[test]
fn rejects_a_capture_used_before_it_is_made() {
  let error = Script::parse(&json!({
    "id": "bad", "binding": "ws", "codec": "cbor",
    "steps": [{"send": frame(0, 1, Some("{13: $token}"), None)}],
  }))
  .unwrap_err();
  assert_eq!(error, "bad: step 1: $token is used before any step captures it");
}

#[test]
fn rejects_a_wildcard_in_send() {
  let error = Script::parse(&json!({
    "id": "bad", "binding": "ws", "codec": "cbor",
    "steps": [{"send": frame(0, 1, Some("{13: *}"), None)}],
  }))
  .unwrap_err();
  assert_eq!(error, "bad: step 1: a send may use $name but not * or ...");
}

#[test]
fn skips_what_the_endpoint_cannot_run() {
  let mut needs_resume = script(json!([]));
  needs_resume.requires = vec!["resume".into()];
  assert_eq!(
    runner::run(&needs_resume, Mode::InMemory, endpoint(), &[]),
    Ok(Verdict::Skipped("requires resume".into()))
  );
  assert_eq!(
    runner::run(&needs_resume, Mode::InMemory, endpoint(), &["resume"]),
    Ok(Verdict::Passed)
  );
  let mut webtransport = script(json!([]));
  webtransport.binding = "wt".into();
  assert!(matches!(
    runner::run(&webtransport, Mode::InMemory, endpoint(), &[]),
    Ok(Verdict::Skipped(_))
  ));
}

#[test]
fn parses_every_vector_script() {
  for path in pavia_vectors::files("scripts") {
    for case in pavia_vectors::load(&path)["cases"].as_array().unwrap() {
      Script::parse(case).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    }
  }
}

fn message_with_header(header: &[u8]) -> Vec<u8> {
  let mut message = vec![
    0x00,
    0x02,
    0x00,
    u8::try_from(header.len() + 1).unwrap(),
    u8::try_from(header.len()).unwrap(),
  ];
  message.extend_from_slice(header);
  message
}

#[test]
fn rejects_an_empty_message() {
  assert_eq!(runner::wire::decode(&[]), Err("an empty message holds no frame".into()));
}

#[test]
fn accepts_a_header_in_deterministic_encoding() {
  let frames = runner::wire::decode(&message_with_header(&[0xa2, 0x01, 0x01, 0x18, 0x18, 0x61, 0x61])).unwrap();
  assert_eq!(frames.len(), 1);
}

#[test]
fn rejects_headers_a_sender_must_not_write() {
  let cases: [(&str, &[u8]); 6] = [
    ("not a map", &[0x01]),
    ("tag", &[0xa1, 0x01, 0xc1, 0x01]),
    ("duplicate key", &[0xa2, 0x01, 0x01, 0x01, 0x02]),
    ("indefinite length", &[0xbf, 0x01, 0x01, 0xff]),
    ("keys out of order", &[0xa2, 0x02, 0x01, 0x01, 0x01]),
    ("longer integer", &[0xa1, 0x01, 0x18, 0x01]),
  ];
  for (what, header) in cases {
    assert!(runner::wire::decode(&message_with_header(header)).is_err(), "{what}");
  }
}

#[test]
fn refuses_a_stream_id_no_varint_holds() {
  let frame = runner::wire::Frame {
    stream: 1 << 62,
    frame_type: 16,
    flags: 0,
    header: None,
    data: Vec::new(),
  };
  assert_eq!(
    runner::wire::encode(&frame),
    Err(format!("{} is above 2^62 - 1", 1_u64 << 62))
  );
}

#[test]
fn matches_a_wildcard_map_key() {
  let pattern = pavia_vectors::diag::parse("{*: 1}").unwrap();
  let value = ciborium::Value::Map(vec![(ciborium::Value::Text("a".into()), ciborium::Value::Integer(1.into()))]);
  assert!(runner::pattern::matches(&pattern, &value, &runner::pattern::Captures::new()));
}

fn decode_header(header: ciborium::Value) -> Result<Vec<runner::wire::Frame>, String> {
  let frame = runner::wire::Frame {
    stream: 0,
    frame_type: 2,
    flags: 0,
    header: Some(header),
    data: Vec::new(),
  };
  runner::wire::decode(&runner::wire::encode(&frame).unwrap())
}

fn nested(depth: usize) -> ciborium::Value {
  let mut value = ciborium::Value::Integer(0.into());
  for _ in 1..depth {
    value = ciborium::Value::Array(vec![value]);
  }
  ciborium::Value::Map(vec![(ciborium::Value::Integer(1.into()), value)])
}

#[test]
fn enforces_the_header_limits() {
  assert!(decode_header(nested(16)).is_ok());
  assert_eq!(decode_header(nested(17)), Err("header nests deeper than 16".into()));
  let sized = |length| {
    ciborium::Value::Map(vec![(
      ciborium::Value::Integer(1.into()),
      ciborium::Value::Bytes(vec![0; length]),
    )])
  };
  assert!(decode_header(sized(16_384 - 5)).is_ok());
  assert_eq!(
    decode_header(sized(16_384 - 4)),
    Err("header of 16385 bytes is above 16384".into())
  );
}

#[test]
fn rejects_a_top_level_key_that_is_not_an_unsigned_integer() {
  for key in [ciborium::Value::Text("a".into()), ciborium::Value::Integer((-1).into())] {
    let header = ciborium::Value::Map(vec![(key, ciborium::Value::Integer(1.into()))]);
    assert!(decode_header(header).is_err());
  }
  let text_keys_inside = ciborium::Value::Map(vec![(
    ciborium::Value::Integer(34.into()),
    ciborium::Value::Map(vec![(ciborium::Value::Text("a".into()), ciborium::Value::Integer(1.into()))]),
  )]);
  assert!(decode_header(text_keys_inside).is_ok());
}

#[test]
fn matches_each_map_entry_once() {
  let pattern = pavia_vectors::diag::parse("{\"a\": 1, *: 1, ...}").unwrap();
  let entry = |key: &str, value: i64| (ciborium::Value::Text(key.into()), ciborium::Value::Integer(value.into()));
  let captures = runner::pattern::Captures::new();
  assert!(!runner::pattern::matches(
    &pattern,
    &ciborium::Value::Map(vec![entry("a", 1), entry("b", 2)]),
    &captures
  ));
  assert!(runner::pattern::matches(
    &pattern,
    &ciborium::Value::Map(vec![entry("b", 1), entry("a", 1)]),
    &captures
  ));
}

#[test]
fn captures_from_json_data() {
  let json_frame = |data: &str| {
    let mut frame = frame(2, 16, None, Some(data));
    frame["codec"] = json!("json");
    frame
  };
  let mut expect = json_frame("{\"a\":[1,\"x\"]}");
  expect["capture"] = json!({"x": "data.a.1"});
  let steps = json!([
    {"send": json_frame("{\"a\":[1,\"x\"]}")},
    {"expect": expect},
    {"send": frame(4, 16, Some("{1: $x}"), None)},
    {"expect": frame(4, 16, Some("{1: \"x\"}"), None)},
  ]);
  assert_eq!(both(&script(steps)), Ok(Verdict::Passed));
}

#[test]
fn accepts_shortest_float_widths_in_a_header() {
  for header in [
    &[0xa1, 0x01, 0xf9, 0x3e, 0x00][..],
    &[0xa1, 0x01, 0xfa, 0x3f, 0x80, 0x00, 0x01],
  ] {
    assert!(runner::wire::decode(&message_with_header(header)).is_ok(), "{header:02x?}");
  }
  assert!(runner::wire::decode(&message_with_header(&[0xa1, 0x01, 0xfa, 0x3f, 0xc0, 0x00, 0x00])).is_err());
}

#[test]
fn fails_on_bytes_after_the_cbor_data_item() {
  let mut trailing = frame(2, 16, None, None);
  trailing["data_hex"] = json!("01 02");
  let steps = json!([
    {"send": trailing},
    {"expect": frame(2, 16, None, Some("1"))},
  ]);
  let failure = both(&script(steps)).unwrap_err();
  assert!(failure.message.starts_with("data does not match"), "{failure}");
}

#[test]
fn rejects_a_send_repeated_zero_times() {
  let mut send = frame(0, 4, None, None);
  send["repeat"] = json!(0);
  let error = Script::parse(&json!({"id": "bad", "binding": "ws", "codec": "cbor", "steps": [{"send": send}]})).unwrap_err();
  assert_eq!(error, "bad: step 1: repeat is not a count of at least 1");
}

#[test]
fn matches_and_reuses_nan() {
  let mut nan = frame(2, 16, None, None);
  nan["data_hex"] = json!("f9 7e 00");
  let mut expect = frame(2, 16, None, Some("NaN"));
  expect["capture"] = json!({"n": "data"});
  let steps = json!([
    {"send": nan},
    {"expect": expect},
    {"send": frame(4, 16, Some("{1: $n}"), None)},
    {"expect": frame(4, 16, Some("{1: $n}"), None)},
  ]);
  assert_eq!(both(&script(steps)), Ok(Verdict::Passed));
  let zero = pavia_vectors::diag::parse("0.0").unwrap();
  assert!(!runner::pattern::matches(
    &zero,
    &ciborium::Value::Float(-0.0),
    &runner::pattern::Captures::new()
  ));
}

#[test]
fn reuses_a_captured_map_in_any_key_order() {
  let entry = |key: &str, value: i64| (ciborium::Value::Text(key.into()), ciborium::Value::Integer(value.into()));
  let mut captures = runner::pattern::Captures::new();
  captures.insert("m".into(), ciborium::Value::Map(vec![entry("a", 1), entry("b", 2)]));
  let pattern = pavia_vectors::diag::parse("$m").unwrap();
  assert!(runner::pattern::matches(
    &pattern,
    &ciborium::Value::Map(vec![entry("b", 2), entry("a", 1)]),
    &captures
  ));
  assert!(!runner::pattern::matches(
    &pattern,
    &ciborium::Value::Map(vec![entry("b", 2), entry("a", 2)]),
    &captures
  ));
  captures.insert("d".into(), ciborium::Value::Map(vec![entry("a", 1), entry("a", 2)]));
  let duplicate = pavia_vectors::diag::parse("$d").unwrap();
  assert!(!runner::pattern::matches(
    &duplicate,
    &ciborium::Value::Map(vec![entry("a", 1), entry("a", 1)]),
    &captures
  ));
  assert!(runner::pattern::matches(
    &duplicate,
    &ciborium::Value::Map(vec![entry("a", 2), entry("a", 1)]),
    &captures
  ));
  captures.insert("e".into(), ciborium::Value::Map(vec![entry("a", 1), entry("a", 1)]));
  assert!(!runner::pattern::matches(
    &pavia_vectors::diag::parse("$e").unwrap(),
    &ciborium::Value::Map(vec![entry("a", 1), entry("a", 2)]),
    &captures
  ));
}
