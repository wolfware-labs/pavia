//! The example app's contract: the services of the spec 12.2 example, registered as an application
//! would, exported, and compared with the committed `tests/manifest/example.json`.
//!
//! After a deliberate contract change, regenerate the file with
//! `PAVIA_UPDATE_MANIFEST=1 cargo test -p pavia-server --test manifest`.
//!
//! `scripts/check-exit-criterion.sh` applies `scripts/exit-criterion/send-gains-a-parameter.patch`,
//! which gives `send` a third parameter; a change to `send` needs that patch made again.
#![expect(dead_code, reason = "the types exist for their derived impls, which never read a field")]

use std::env;
use std::fs;
use std::path::PathBuf;

use pavia_contract::{Bytes, Contract, Timestamp, Uuid};
use pavia_proto::data::Value;
use pavia_proto::stream::Shape;
use pavia_server::manifest;
use pavia_server::methods::{Handler, MethodEntry, MethodRegistry};
use serde_json::json;

#[derive(Contract)]
struct RoomId(String);

#[derive(Contract)]
struct MessageId(Uuid);

#[derive(Contract)]
struct Message {
  id: MessageId,
  author: String,
  text: String,
  sent_at: Timestamp,
}

#[derive(Contract)]
enum SendError {
  RoomNotFound,
  TooLong { max: u32 },
}

const NAME: &str = "example-app";

fn handler(params: usize) -> Handler {
  Handler::new(params, |_| async { Value::Null })
}

fn send() -> MethodEntry<Handler> {
  MethodEntry::call(handler(2))
    .param::<RoomId>("room")
    .param::<String>("text")
    .returns::<MessageId>()
    .error::<SendError>()
}

fn server_methods() -> Vec<(&'static str, MethodEntry<Handler>)> {
  vec![
    ("chat/send", send()),
    ("chat/typing", MethodEntry::notify(handler(1)).param::<RoomId>("room")),
    (
      "chat/history",
      MethodEntry::call(handler(1))
        .with_shape(Shape::ServerStream)
        .param::<RoomId>("room")
        .yields::<Message>(),
    ),
    (
      "chat/upload",
      MethodEntry::call(handler(0))
        .with_shape(Shape::ClientStream)
        .accepts::<Bytes>()
        .returns::<u64>(),
    ),
  ]
}

fn client_methods() -> Vec<(&'static str, MethodEntry<Handler>)> {
  vec![
    (
      "chat/confirm",
      MethodEntry::call(handler(1)).param::<String>("prompt").returns::<bool>(),
    ),
    ("chat/systemNotice", MethodEntry::notify(handler(1)).param::<String>("text")),
  ]
}

fn registry(server: Vec<(&str, MethodEntry<Handler>)>, client: Vec<(&str, MethodEntry<Handler>)>) -> MethodRegistry<Handler> {
  let mut methods = MethodRegistry::new();
  for (name, method) in server {
    methods.register_server_method(name, method).unwrap();
  }
  for (name, method) in client {
    methods.register_client_method(name, method).unwrap();
  }
  methods
}

fn committed() -> PathBuf {
  PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("tests/manifest/example.json")
}

#[test]
fn keeps_the_committed_manifest_current() {
  let methods = registry(server_methods(), client_methods());
  if env::var_os("PAVIA_UPDATE_MANIFEST").is_some() {
    manifest::export(NAME, &methods, committed()).unwrap();
  }
  let committed = fs::read_to_string(committed()).unwrap();
  assert!(
    manifest::render(NAME, &methods).unwrap() == committed,
    "tests/manifest/example.json is out of date: run PAVIA_UPDATE_MANIFEST=1 cargo test -p pavia-server --test manifest"
  );
}

#[test]
fn notices_a_changed_signature() {
  let mut server = server_methods();
  server[0].1 = MethodEntry::call(handler(2))
    .param::<RoomId>("room")
    .param::<String>("text")
    .returns::<String>()
    .error::<SendError>();
  let changed = manifest::render(NAME, &registry(server, client_methods())).unwrap();
  assert_ne!(changed, fs::read_to_string(committed()).unwrap());
}

#[test]
fn writes_the_same_bytes_whatever_the_registration_order() {
  let mut server = server_methods();
  server.reverse();
  let mut client = client_methods();
  client.reverse();
  assert_eq!(
    manifest::render(NAME, &registry(server, client)).unwrap(),
    manifest::render(NAME, &registry(server_methods(), client_methods())).unwrap()
  );
}

#[test]
fn matches_the_services_and_types_of_the_spec_example() {
  let exported: serde_json::Value = serde_json::from_str(&fs::read_to_string(committed()).unwrap()).unwrap();
  let expected = json!({
    "pavia": 1,
    "name": "example-app",
    "services": [{
      "name": "chat",
      "server_methods": [
        {
          "name": "history", "kind": "call", "shape": "server_stream",
          "params": [{"name": "room", "type": {"ref": "RoomId"}}],
          "yields": {"ref": "Message"}
        },
        {
          "name": "send", "kind": "call", "shape": "unary",
          "params": [{"name": "room", "type": {"ref": "RoomId"}}, {"name": "text", "type": "string"}],
          "returns": {"ref": "MessageId"},
          "error": {"ref": "SendError"}
        },
        {"name": "typing", "kind": "notify", "params": [{"name": "room", "type": {"ref": "RoomId"}}]},
        {"name": "upload", "kind": "call", "shape": "client_stream", "accepts": "bytes", "returns": "u64"}
      ],
      "client_methods": [
        {
          "name": "confirm", "kind": "call", "shape": "unary",
          "params": [{"name": "prompt", "type": "string"}],
          "returns": "bool"
        },
        {"name": "systemNotice", "kind": "notify", "params": [{"name": "text", "type": "string"}]}
      ]
    }],
    "types": {
      "RoomId": {"alias": "string"},
      "MessageId": {"alias": "uuid"},
      "Message": {"struct": {"fields": [
        {"name": "id", "type": {"ref": "MessageId"}},
        {"name": "author", "type": "string"},
        {"name": "text", "type": "string"},
        {"name": "sent_at", "type": "timestamp"}
      ]}},
      "SendError": {"enum": {"variants": [
        {"name": "RoomNotFound"},
        {"name": "TooLong", "type": {"struct": {"fields": [{"name": "max", "type": "u32"}]}}}
      ]}}
    },
    "channels": [],
    "datagrams": []
  });
  assert_eq!(exported, expected);
  let text = fs::read_to_string(committed()).unwrap();
  for forbidden in ["\"idempotent\": false", "\"returns\": \"unit\"", "\"params\": []", "null"] {
    assert!(!text.contains(forbidden), "{forbidden}");
  }
}

#[test]
fn fingerprints_the_committed_manifest() {
  let methods = registry(server_methods(), client_methods());
  let committed: serde_json::Value = serde_json::from_str(&fs::read_to_string(committed()).unwrap()).unwrap();
  let expected = pavia_contract::fingerprint::fingerprint(&committed).unwrap();
  assert_eq!(manifest::fingerprint(NAME, &methods).unwrap().bytes(), expected);
  assert_eq!(manifest::fingerprint("another-name", &methods).unwrap().bytes(), expected);
}

#[test]
fn reads_the_committed_manifest_back_as_the_same_contract() {
  let text = fs::read_to_string(committed()).unwrap();
  let read = pavia_contract::manifest::Manifest::read(&text).unwrap();
  assert_eq!(read.render().unwrap(), text);
  let methods = registry(server_methods(), client_methods());
  assert_eq!(
    read.fingerprint().unwrap(),
    manifest::fingerprint(NAME, &methods).unwrap().bytes()
  );
}

#[test]
fn fingerprints_no_contract_whose_manifest_it_cannot_write() {
  let names: Vec<String> = (0..20_000)
    .map(|index| format!("chat/{}{index:05}", "a".repeat(200)))
    .collect();
  let methods = registry(names.iter().map(|name| (name.as_str(), send())).collect(), Vec::new());
  for result in [
    manifest::fingerprint(NAME, &methods).map(|_| ()),
    manifest::render(NAME, &methods).map(|_| ()),
  ] {
    match result {
      Err(manifest::ExportError::Manifest(errors)) => assert!(
        matches!(errors.as_slice(), [pavia_contract::manifest::ManifestError::TooLarge { .. }]),
        "{errors:?}"
      ),
      other => panic!("expected a size error, got {other:?}"),
    }
  }
}
