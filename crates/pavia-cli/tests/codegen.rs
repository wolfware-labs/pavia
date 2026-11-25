//! `pavia codegen ts`: the TypeScript module for a manifest file, written into a folder.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::{env, fs};

use pavia_contract::manifest::Manifest;

fn example() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../pavia-server/tests/manifest/example.json")
}

fn pavia(args: &[&str]) -> Output {
  Command::new(env!("CARGO_BIN_EXE_pavia"))
    .args(args)
    .output()
    .expect("the pavia binary runs")
}

fn scratch(name: &str) -> PathBuf {
  let dir = env::temp_dir().join(format!("pavia-cli-{}-{name}", std::process::id()));
  let _ = fs::remove_dir_all(&dir);
  dir
}

fn stderr(output: &Output) -> String {
  String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn writes_the_module_of_a_manifest_file_into_a_new_folder() {
  let out = scratch("writes").join("generated");
  let output = pavia(&["codegen", "ts", example().to_str().unwrap(), "--out", out.to_str().unwrap()]);
  assert!(output.status.success(), "{}", stderr(&output));
  let manifest = Manifest::read(&fs::read_to_string(example()).unwrap()).unwrap();
  let written = fs::read_to_string(out.join("contract.ts")).unwrap();
  assert_eq!(written, pavia_codegen::typescript::generate(&manifest).unwrap());
  let again = pavia(&["codegen", "ts", "--out", out.to_str().unwrap(), example().to_str().unwrap()]);
  assert!(again.status.success(), "{}", stderr(&again));
  fs::remove_dir_all(out.parent().unwrap()).unwrap();
}

#[test]
fn refuses_a_command_line_it_does_not_know_with_the_usage() {
  for args in [
    &[][..],
    &["codegen"],
    &["codegen", "rust", "m.json", "--out", "x"],
    &["codegen", "ts", "m.json"],
    &["codegen", "ts", "--out", "x"],
    &["codegen", "ts", "m.json", "--out"],
    &["codegen", "ts", "a.json", "b.json", "--out", "x"],
    &["codegen", "ts", "m.json", "--out", "x", "--out", "y"],
    &["codegen", "ts", "m.json", "--force", "--out", "x"],
  ] {
    let output = pavia(args);
    assert_eq!(output.status.code(), Some(2), "{args:?}");
    assert!(
      stderr(&output).contains("usage: pavia codegen ts <manifest.json> --out <dir>"),
      "{args:?}"
    );
  }
}

#[test]
fn says_why_a_manifest_cannot_be_read_or_generated() {
  let dir = scratch("refuses");
  fs::create_dir_all(&dir).unwrap();
  let invalid = dir.join("invalid.json");
  fs::write(&invalid, r#"{"pavia": 2, "name": "app"}"#).unwrap();
  let reserved = dir.join("reserved.json");
  fs::write(
    &reserved,
    r#"{"pavia": 1, "name": "app", "types": {"Promise": {"alias": "u8"}}}"#,
  )
  .unwrap();
  let out = dir.join("out");
  for (manifest, message) in [
    (dir.join("missing.json"), "cannot read"),
    (invalid, "format version 2"),
    (reserved, "type `Promise` is a name TypeScript code reserves"),
  ] {
    let output = pavia(&["codegen", "ts", manifest.to_str().unwrap(), "--out", out.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1), "{}", manifest.display());
    assert!(
      stderr(&output).contains(message),
      "{}: {}",
      manifest.display(),
      stderr(&output)
    );
    assert!(!out.exists());
  }
  fs::remove_dir_all(&dir).unwrap();
}
