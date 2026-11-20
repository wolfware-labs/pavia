mod runner;

use std::collections::BTreeMap;
use std::sync::Arc;

use pavia_proto::data::{Codec, Value};
use pavia_proto::error::ErrorStatus;
use pavia_proto::registry::StatusCode;
use pavia_proto::session::Config as SessionConfig;
use pavia_server::auth::{AllowAll, Permissive};
use pavia_server::methods::{Handler, MethodEntry, MethodRegistry, StatusError};
use pavia_server::session;
use pavia_server::sessions::Registry;
use pavia_server::websocket::{Config, Endpoint, Transport};
use runner::{Mode, Script, Verdict};
use serde_json::json;

fn not_found() -> Handler {
  Handler::fallible(0, |_| async {
    let code = ErrorStatus::try_from(StatusCode::NotFound).unwrap();
    let detail = Value::Map(vec![(Value::Text("id".into()), Value::Text("u1".into()))]);
    Err(StatusError::new(code, "no such user").with_detail(detail).into())
  })
}

fn methods() -> Arc<MethodRegistry<Handler>> {
  let mut methods = MethodRegistry::new();
  methods
    .register_server_method("users/get", MethodEntry::call(not_found()))
    .unwrap();
  methods
    .register_server_method(
      "users/find",
      MethodEntry::call(not_found()).error::<BTreeMap<String, String>>(),
    )
    .unwrap();
  Arc::new(methods)
}

fn run(codec: &str, steps: serde_json::Value) -> Verdict {
  let mut config = SessionConfig::default();
  config.codecs = vec![Codec::Cbor, Codec::Json];
  let config = config.validate().unwrap();
  let methods = methods();
  let endpoint = Endpoint::new(Config::new("/rt", &config), move |transport: Transport| {
    let methods = Arc::clone(&methods);
    async move {
      session::run(transport, &Registry::new(), &methods, &Permissive, &Arc::new(AllowAll)).await;
    }
  });
  let script = Script::parse(&json!({
    "id": format!("error-data-{codec}"), "spec": ["8.4"], "note": "", "binding": "ws", "codec": codec,
    "steps": steps,
  }))
  .unwrap();
  let in_memory = runner::run(&script, Mode::InMemory, endpoint.clone(), &[]);
  let socket = runner::run(&script, Mode::Socket, endpoint, &[]);
  assert_eq!(in_memory, socket, "the two modes disagree");
  in_memory.unwrap()
}

fn handshake(codec: &str) -> serde_json::Value {
  json!([
    {"send": {"stream": 0, "type": 1, "header": format!("{{14: [1], 16: [\"{codec}\"], 18: [\"calls\"]}}"), "data": null}},
    {"expect": {"stream": 0, "type": 2, "header": "{...}", "data": null}},
  ])
}

fn call(stream: u64, method: &str, codec: &str) -> serde_json::Value {
  json!({"send": {
    "stream": stream, "type": 16, "header": format!("{{2: \"{method}\", 5: 0}}"), "data": "[]", "codec": codec,
  }})
}

#[test]
fn sends_error_with_no_data_for_a_method_that_declares_no_error_type() {
  for codec in ["cbor", "json"] {
    let mut steps = handshake(codec);
    let steps_list = steps.as_array_mut().unwrap();
    steps_list.push(call(2, "users/get", codec));
    steps_list.push(json!({"expect": {"stream": 2, "type": 20, "header": "{6: 5, 7: \"no such user\"}", "data": null}}));
    assert_eq!(run(codec, steps), Verdict::Passed, "{codec}");
  }
}

#[test]
fn sends_the_detail_of_a_method_that_declares_an_error_type() {
  for (codec, detail) in [("cbor", "{\"id\": \"u1\"}"), ("json", "{\"id\":\"u1\"}")] {
    let mut steps = handshake(codec);
    let steps_list = steps.as_array_mut().unwrap();
    steps_list.push(call(2, "users/find", codec));
    steps_list.push(json!({"expect": {
      "stream": 2, "type": 20, "header": "{6: 5, 7: \"no such user\"}", "data": detail, "codec": codec,
    }}));
    assert_eq!(run(codec, steps), Verdict::Passed, "{codec}");
  }
}
