//! The TypeScript module of the example app's manifest, committed where the client package's
//! typecheck and tests read it, and compared here with what the generator writes now.
//!
//! After a deliberate change to the generator or the manifest, regenerate the file with
//! `PAVIA_UPDATE_GENERATED=1 cargo test -p pavia-codegen --test example`.

use std::path::{Path, PathBuf};
use std::{env, fs};

use pavia_contract::manifest::Manifest;

fn root() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn keeps_the_committed_typescript_module_current() {
  let manifest = fs::read_to_string(root().join("crates/pavia-server/tests/manifest/example.json")).unwrap();
  let code = pavia_codegen::typescript::generate(&Manifest::read(&manifest).unwrap()).unwrap();
  let committed = root().join("clients/typescript/packages/client/test/generated/contract.ts");
  if env::var_os("PAVIA_UPDATE_GENERATED").is_some() {
    fs::write(&committed, &code).unwrap();
  }
  assert_eq!(
    fs::read_to_string(&committed).unwrap(),
    code,
    "regenerate with PAVIA_UPDATE_GENERATED=1"
  );
}
