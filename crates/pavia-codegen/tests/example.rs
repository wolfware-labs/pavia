//! The TypeScript modules of two manifests, committed where the client package's typecheck and tests
//! read them, and compared here with what the generator writes now: the example app's, and a corpus
//! with every type form, every place a client sends or receives a value, and names TypeScript treats
//! apart. The example app's module has a second copy in the `@pavia/example-chat` package, the one
//! the exit-criterion job regenerates, and a third generated under the camelCase naming policy.
//!
//! After a deliberate change to the generator or a manifest, regenerate the files with
//! `PAVIA_UPDATE_GENERATED=1 cargo test -p pavia-codegen --test example`.

use std::path::{Path, PathBuf};
use std::{env, fs};

use pavia_codegen::typescript::{Naming, generate_with};
use pavia_contract::manifest::Manifest;

fn root() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn keeps_current(manifest: &str, naming: Naming, modules: &[&str]) {
  let manifest = fs::read_to_string(root().join(manifest)).unwrap();
  let code = generate_with(&Manifest::read(&manifest).unwrap(), naming).unwrap();
  for module in modules {
    let committed = root().join(module);
    if env::var_os("PAVIA_UPDATE_GENERATED").is_some() {
      fs::write(&committed, &code).unwrap();
    }
    assert_eq!(
      fs::read_to_string(&committed).unwrap_or_else(|error| panic!("{module}: {error}")),
      code,
      "{module}: regenerate with PAVIA_UPDATE_GENERATED=1"
    );
  }
}

#[test]
fn keeps_the_committed_typescript_modules_current() {
  keeps_current(
    "crates/pavia-server/tests/manifest/example.json",
    Naming::Identity,
    &[
      "clients/typescript/packages/client/test/generated/contract.ts",
      "clients/typescript/packages/example-chat/src/contract.ts",
    ],
  );
}

#[test]
fn keeps_the_committed_corpus_module_current() {
  keeps_current(
    "crates/pavia-codegen/tests/manifest/corpus.json",
    Naming::Identity,
    &["clients/typescript/packages/client/test/generated/corpus.ts"],
  );
}

#[test]
fn keeps_the_committed_camel_case_module_current() {
  keeps_current(
    "crates/pavia-server/tests/manifest/example.json",
    Naming::Camel,
    &["clients/typescript/packages/client/test/generated/contract-camel.ts"],
  );
}
