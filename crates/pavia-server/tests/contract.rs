//! The contract fingerprint in the handshake (§7.1, §7.2, §12.5) under both policies, played as
//! scripts against a real session.

mod runner;

use std::sync::Arc;

use pavia_proto::contract::{ContractPolicy, Fingerprint};
use pavia_proto::session::Config as SessionConfig;
use pavia_server::auth::{AllowAll, Permissive};
use pavia_server::methods::MethodRegistry;
use pavia_server::session;
use pavia_server::sessions::Registry;
use pavia_server::websocket::{Config, Endpoint, Transport};
use runner::{Script, Verdict};
use serde_json::{Value, json};

const SERVER: [u8; 32] = [0xab; 32];

fn hex(bytes: &[u8]) -> String {
  bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn run(contract: Option<[u8; 32]>, policy: ContractPolicy, steps: Value) -> Verdict {
  let mut config = SessionConfig::default();
  config.contract = contract.map(Fingerprint::new);
  config.contract_policy = policy;
  let config = config.validate().unwrap();
  let methods = Arc::new(MethodRegistry::new());
  let endpoint = Endpoint::new(Config::new("/rt", &config), move |transport: Transport| {
    let methods = Arc::clone(&methods);
    async move {
      session::run(transport, &Registry::new(), &methods, &Permissive, &Arc::new(AllowAll)).await;
    }
  });
  let script = Script::parse(&json!({
    "id": "contract", "spec": ["7.2", "12.5"], "note": "", "binding": "ws", "codec": "cbor", "steps": steps,
  }))
  .unwrap();
  runner::run_in_both_modes(&script, endpoint).unwrap()
}

fn hello(contract: Option<&str>) -> Value {
  let contract = contract.map_or_else(String::new, |value| format!(", 24: {value}"));
  let header = format!("{{14: [1], 16: [\"cbor\"], 18: [\"calls\"]{contract}}}");
  json!({"send": {"stream": 0, "type": 1, "header": header, "data": null}})
}

fn bytes(fingerprint: &[u8]) -> String {
  format!("h'{}'", hex(fingerprint))
}

fn welcome_with_server_fingerprint() -> Value {
  json!({"expect": {"stream": 0, "type": 2, "header": format!("{{24: {}, ...}}", bytes(&SERVER)), "data": null}})
}

fn rejected() -> Value {
  json!([
    {"expect": {"stream": 0, "type": 3, "header": "{6: 13, 7: \"contract mismatch\"}", "data": null}},
    {"expect_close": {"code": "CONTRACT_MISMATCH", "within_ms": 1000}},
  ])
}

fn steps(first: Value, then: Value) -> Value {
  let mut steps = vec![first];
  match then {
    Value::Array(more) => steps.extend(more),
    one => steps.push(one),
  }
  Value::Array(steps)
}

#[test]
fn rejects_a_wrong_or_missing_fingerprint_under_strict() {
  let wrong = bytes(&[0xff; 32]);
  for hello in [hello(Some(&wrong)), hello(None)] {
    assert_eq!(
      run(Some(SERVER), ContractPolicy::Strict, steps(hello, rejected())),
      Verdict::Passed
    );
  }
}

#[test]
fn welcomes_a_wrong_or_missing_fingerprint_under_warn() {
  let wrong = bytes(&[0xff; 32]);
  for hello in [hello(Some(&wrong)), hello(None)] {
    assert_eq!(
      run(
        Some(SERVER),
        ContractPolicy::Warn,
        steps(hello, welcome_with_server_fingerprint())
      ),
      Verdict::Passed
    );
  }
}

#[test]
fn welcomes_the_matching_fingerprint_under_strict() {
  let matching = bytes(&SERVER);
  assert_eq!(
    run(
      Some(SERVER),
      ContractPolicy::Strict,
      steps(hello(Some(&matching)), welcome_with_server_fingerprint())
    ),
    Verdict::Passed
  );
}

#[test]
fn welcomes_any_fingerprint_under_strict_without_a_server_one() {
  let any = bytes(&[0xff; 32]);
  let welcome = json!({"expect": {"stream": 0, "type": 2, "header": "{...}", "data": null}});
  assert_eq!(
    run(None, ContractPolicy::Strict, steps(hello(Some(&any)), welcome)),
    Verdict::Passed
  );
}

#[test]
fn closes_with_protocol_error_on_a_fingerprint_that_is_not_32_bytes() {
  let short = bytes(&[0xff; 31]);
  let close = json!({"expect_close": {"code": "PROTOCOL_ERROR", "within_ms": 1000}});
  for policy in [ContractPolicy::Warn, ContractPolicy::Strict] {
    for contract in [short.as_str(), "\"ff\""] {
      assert_eq!(
        run(Some(SERVER), policy, steps(hello(Some(contract)), close.clone())),
        Verdict::Passed,
        "{policy:?} {contract}"
      );
    }
  }
}
