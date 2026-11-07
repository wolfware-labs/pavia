//! Benchmarks of the public API: the data-section codecs (§5.5) on a 100-item value, a server session
//! handling the minimal WebSocket HELLO vector (§7.1), and session-ID parsing.

use std::hint::black_box;
use std::time::Instant;

use bytes::Bytes;
use criterion::{Criterion, criterion_group, criterion_main};
use pavia_proto::data::{self, Codec, DecodeLimits, Value};
use pavia_proto::session::{Config, Input, Session};
use pavia_proto::session_id::SessionId;

fn hello_ws() -> Bytes {
  let doc = pavia_vectors::load(&pavia_vectors::vectors_dir().join("frames/handshake.json"));
  let case = doc["cases"]
    .as_array()
    .and_then(|cases| cases.iter().find(|case| case["id"] == "hello-minimal-ws"))
    .expect("handshake.json has the hello-minimal-ws case");
  Bytes::from(pavia_vectors::hex(case["hex"].as_str().expect("the case has hex bytes")))
}

fn order(index: u32) -> Value {
  Value::Map(vec![
    (Value::Text("id".into()), Value::Unsigned(index.into())),
    (Value::Text("sku".into()), Value::Text(format!("SKU-{index:05}"))),
    (Value::Text("price".into()), Value::Float(f64::from(index) * 1.25)),
    (Value::Text("delta".into()), Value::Negative(index.into())),
    (Value::Text("paid".into()), Value::Bool(index.is_multiple_of(2))),
    (Value::Text("note".into()), Value::Null),
  ])
}

fn orders() -> Value {
  Value::Map(vec![
    (Value::Text("page".into()), Value::Unsigned(1)),
    (Value::Text("items".into()), Value::Array((0..100).map(order).collect())),
  ])
}

fn data_codecs(c: &mut Criterion) {
  let value = orders();
  let limits = DecodeLimits::default();
  for codec in [Codec::Cbor, Codec::Json] {
    let mut encoded = Vec::new();
    data::encode(&value, codec, &mut encoded).expect("the value encodes");
    c.bench_function(&format!("data/encode/{}", codec.name()), |b| {
      b.iter(|| {
        let mut out = Vec::with_capacity(encoded.len());
        data::encode(black_box(&value), codec, &mut out).expect("the value encodes");
        out
      });
    });
    c.bench_function(&format!("data/decode/{}", codec.name()), |b| {
      b.iter(|| data::decode(black_box(&encoded), codec, &limits).expect("the bytes decode"));
    });
  }
}

fn session_hello(c: &mut Criterion) {
  let config = Config::default().validate().expect("the default config is valid");
  let hello = hello_ws();
  let now = Instant::now();
  c.bench_function("session/hello", |b| {
    b.iter(|| {
      let mut session = Session::server(&config, now);
      session.handle_input(Input::Message(black_box(hello.clone())), now);
      while let Some(output) = session.poll_output() {
        black_box(output);
      }
    });
  });
}

fn session_ids(c: &mut Criterion) {
  let id = SessionId::from_random([0x5a; 16]).to_string();
  c.bench_function("session_id/parse", |b| {
    b.iter(|| SessionId::parse(black_box(&id)).expect("the ID parses"));
  });
}

criterion_group!(benches, data_codecs, session_hello, session_ids);
criterion_main!(benches);
