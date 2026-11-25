//! Benchmarks of the manifest (§12.2): reading, writing and fingerprinting the example application's
//! manifest, and validating a `types` table that is one long chain of aliases.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use pavia_contract::manifest::Manifest;
use pavia_contract::schema::{Definition, Primitive, Type, Types};

const EXAMPLE: &str = include_str!("../../pavia-server/tests/manifest/example.json");

fn alias_chain(length: usize) -> Manifest {
  let types: Types = (0..length)
    .map(|index| {
      let target = if index + 1 == length {
        Type::from(Primitive::U8)
      } else {
        Type::Ref(format!("A{:05}", index + 1))
      };
      (format!("A{index:05}"), Definition::Alias(target))
    })
    .collect();
  Manifest::new("chain").types(types)
}

fn example(c: &mut Criterion) {
  c.bench_function("manifest/read", |b| {
    b.iter(|| Manifest::read(black_box(EXAMPLE)).expect("the example manifest reads"));
  });
  let manifest = Manifest::read(EXAMPLE).expect("the example manifest reads");
  c.bench_function("manifest/render", |b| {
    b.iter(|| black_box(&manifest).render().expect("the example manifest renders"));
  });
  c.bench_function("manifest/fingerprint", |b| {
    b.iter(|| {
      black_box(&manifest)
        .fingerprint()
        .expect("the example manifest has a fingerprint")
    });
  });
}

fn chain(c: &mut Criterion) {
  let manifest = alias_chain(1_000);
  c.bench_function("manifest/validate/alias_chain_1000", |b| {
    b.iter(|| black_box(&manifest).validate().expect("the chain is valid"));
  });
}

criterion_group!(benches, example, chain);
criterion_main!(benches);
