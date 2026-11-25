//! `pavia codegen ts`: the TypeScript module for a manifest file, written into a folder.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::{env, fs};

use pavia_contract::manifest::{MAX_MANIFEST_BYTES, Manifest};

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

#[test]
fn writes_control_characters_from_a_manifest_as_escapes() {
  let dir = scratch("control");
  fs::create_dir_all(&dir).unwrap();
  let manifest = dir.join("control.json");
  fs::write(
    &manifest,
    r#"{"pavia": 1, "name": "app", "types": {"\u001b[2J\u0007Bad\nName": {"alias": "u8"}}}"#,
  )
  .unwrap();
  let out = dir.join("out");
  let output = pavia(&["codegen", "ts", manifest.to_str().unwrap(), "--out", out.to_str().unwrap()]);
  assert_eq!(output.status.code(), Some(1));
  let stderr = stderr(&output);
  assert!(!stderr.trim_end_matches('\n').chars().any(char::is_control), "{stderr:?}");
  assert!(stderr.contains(r"\u{1b}[2J\u{7}Bad\nName"), "{stderr:?}");
  fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn cuts_a_long_message_short_even_inside_a_character() {
  let dir = scratch("long");
  fs::create_dir_all(&dir).unwrap();
  let out = dir.join("out");
  for pad in ["", "v"] {
    let manifest = dir.join("long.json");
    fs::write(
      &manifest,
      format!(r#"{{"pavia": "{pad}{}", "name": "app"}}"#, "é".repeat(1 << 19)),
    )
    .unwrap();
    let output = pavia(&["codegen", "ts", manifest.to_str().unwrap(), "--out", out.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("the message stays UTF-8");
    assert!(stderr.contains("format version"), "{}", &stderr[..200]);
    assert!(stderr.len() < 70_000, "{} bytes", stderr.len());
    assert!(stderr.trim_end().ends_with("more bytes)"), "{pad:?}");
  }
  fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn refuses_a_manifest_over_the_bound_even_when_the_bound_cuts_a_character() {
  let dir = scratch("large");
  fs::create_dir_all(&dir).unwrap();
  let start = r#"{"pavia": 1, "name": "x"#;
  for pad in [0, 1] {
    let large = dir.join(format!("large-{pad}.json"));
    let fill = "é".repeat((MAX_MANIFEST_BYTES - start.len()) / 2 + 2);
    fs::write(&large, format!("{start}{}{fill}", " ".repeat(pad))).unwrap();
    let output = pavia(&[
      "codegen",
      "ts",
      large.to_str().unwrap(),
      "--out",
      dir.join("out").to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
      stderr(&output).contains("more than the 4194304 a manifest may have"),
      "{}",
      stderr(&output)
    );
  }
  fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn reports_a_manifest_path_that_is_not_utf8_as_a_file_it_cannot_read() {
  use std::ffi::OsStr;
  use std::os::unix::ffi::OsStrExt as _;

  let output = Command::new(env!("CARGO_BIN_EXE_pavia"))
    .args([OsStr::new("codegen"), OsStr::new("ts"), OsStr::from_bytes(b"bad\xff.json")])
    .args(["--out", scratch("not-utf8").to_str().unwrap()])
    .output()
    .expect("the pavia binary runs");
  assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
  assert!(stderr(&output).contains("cannot read bad"), "{}", stderr(&output));
}
