use std::hint::black_box;
use std::time::Instant;

use bytes::Bytes;
use criterion::{Criterion, criterion_group, criterion_main};
use pavia_proto::data::{self, Codec, DecodeLimits, Value};
use pavia_proto::session::{Config, Input, Session};
use pavia_proto::session_id::SessionId;

const HELLO_WS: [u8; 24] = [
  0x00, 0x01, 0x00, 0x14, 0x13, 0xa3, 0x0e, 0x81, 0x01, 0x10, 0x81, 0x64, 0x63, 0x62, 0x6f, 0x72, 0x12, 0x81, 0x65, 0x63, 0x61,
  0x6c, 0x6c, 0x73,
];

fn order(index: u64) -> Value {
  let index_f64 = f64::from(u32::try_from(index).unwrap_or(u32::MAX));
  Value::Map(vec![
    (Value::Text("id".into()), Value::Unsigned(index)),
    (Value::Text("sku".into()), Value::Text(format!("SKU-{index:05}"))),
    (Value::Text("price".into()), Value::Float(index_f64 * 1.25)),
    (Value::Text("delta".into()), Value::Negative(index)),
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
  let config = Config::default();
  let hello = Bytes::from_static(&HELLO_WS);
  c.bench_function("session/hello", |b| {
    b.iter(|| {
      let now = Instant::now();
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
