//! The TypeScript modules of two manifests, committed where the client package's typecheck and tests
//! read them, and compared here with what the generator writes now: the example app's, and a corpus
//! with every type form, every place a client sends or receives a value, and names TypeScript treats
//! apart.
//!
//! After a deliberate change to the generator or a manifest, regenerate the files with
//! `PAVIA_UPDATE_GENERATED=1 cargo test -p pavia-codegen --test example`.

use std::path::{Path, PathBuf};
use std::{env, fs};

use pavia_contract::manifest::Manifest;

fn root() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn keeps_current(manifest: &str, module: &str) {
  let manifest = fs::read_to_string(root().join(manifest)).unwrap();
  let code = pavia_codegen::typescript::generate(&Manifest::read(&manifest).unwrap()).unwrap();
  let committed = root().join("clients/typescript/packages/client/test/generated").join(module);
  if env::var_os("PAVIA_UPDATE_GENERATED").is_some() {
    fs::write(&committed, &code).unwrap();
  }
  assert_eq!(
    fs::read_to_string(&committed).unwrap(),
    code,
    "regenerate with PAVIA_UPDATE_GENERATED=1"
  );
}

#[test]
fn keeps_the_committed_typescript_module_current() {
  keeps_current("crates/pavia-server/tests/manifest/example.json", "contract.ts");
}

#[test]
fn keeps_the_committed_corpus_module_current() {
  keeps_current("crates/pavia-codegen/tests/manifest/corpus.json", "corpus.ts");
}
