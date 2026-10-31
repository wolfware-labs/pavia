mod runner;

use std::future::Future;
use std::pin::Pin;

use pavia_server::websocket::{Config, Endpoint, Received, Transport};
use runner::{Failure, Mode, Script, Verdict};
use serde_json::{Value, json};

type Session = Pin<Box<dyn Future<Output = ()> + Send>>;

const CLOSE_STREAM: u8 = 9;

async fn echo(mut transport: Transport) {
  while let Some(Ok(Received::Binary(message))) = transport.recv().await {
    if message.first() == Some(&CLOSE_STREAM) {
      let _ = transport.close(4000 + u16::from(message[2]), "").await;
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
