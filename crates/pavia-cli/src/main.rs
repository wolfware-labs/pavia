//! The `pavia` command-line tool.
//!
//! `pavia codegen ts <manifest.json> --out <dir>` reads a manifest file (§12.2) and writes the
//! TypeScript module for it to `<dir>/contract.ts`, creating `<dir>` when it is missing.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use pavia_contract::manifest::{MAX_MANIFEST_BYTES, Manifest};

const USAGE: &str = "usage: pavia codegen ts <manifest.json> --out <dir>";

/// Why a command did not run: the command line is wrong, or the work failed.
enum Failure {
  Usage,
  Failed(String),
}

fn main() -> ExitCode {
  let args: Vec<OsString> = std::env::args_os().skip(1).collect();
  match run(&args) {
    Ok(written) => {
      println!("pavia: wrote {}", written.display());
      ExitCode::SUCCESS
    }
    Err(Failure::Usage) => {
      eprintln!("{USAGE}");
      ExitCode::from(2)
    }
    Err(Failure::Failed(message)) => {
      eprintln!("pavia: {message}");
      ExitCode::FAILURE
    }
  }
}

fn run(args: &[OsString]) -> Result<PathBuf, Failure> {
  match args {
    [command, target, rest @ ..] if command == "codegen" && target == "ts" => {
      let (manifest, out) = codegen_args(rest).ok_or(Failure::Usage)?;
      codegen(&manifest, &out).map_err(Failure::Failed)
    }
    _ => Err(Failure::Usage),
  }
}

/// The manifest path and the `--out` folder, each given exactly once and nothing else.
fn codegen_args(args: &[OsString]) -> Option<(PathBuf, PathBuf)> {
  let mut manifest = None;
  let mut out = None;
  let mut args = args.iter();
  while let Some(arg) = args.next() {
    if arg == "--out" {
      if out.replace(PathBuf::from(args.next()?)).is_some() {
        return None;
      }
    } else if arg.as_encoded_bytes().starts_with(b"--") || manifest.replace(PathBuf::from(arg)).is_some() {
      return None;
    }
  }
  Some((manifest?, out?))
}

fn codegen(manifest: &Path, out: &Path) -> Result<PathBuf, String> {
  let text = read_bounded(manifest).map_err(|error| format!("cannot read {}: {error}", manifest.display()))?;
  let manifest = Manifest::read(&text).map_err(|error| format!("{}: {error}", manifest.display()))?;
  let code = pavia_codegen::typescript::generate(&manifest).map_err(|error| error.to_string())?;
  fs::create_dir_all(out).map_err(|error| format!("cannot create {}: {error}", out.display()))?;
  let path = out.join("contract.ts");
  fs::write(&path, code).map_err(|error| format!("cannot write {}: {error}", path.display()))?;
  Ok(path)
}

/// The file's text, reading at most one byte past [`MAX_MANIFEST_BYTES`]: enough for
/// [`Manifest::read`] to refuse a file that is too long without reading all of it. Bytes past the
/// bound are read with lossy UTF-8, since the bound may cut a character, and stay past it.
fn read_bounded(path: &Path) -> io::Result<String> {
  let limit = u64::try_from(MAX_MANIFEST_BYTES).expect("the bound fits a u64") + 1;
  let mut bytes = Vec::new();
  File::open(path)?.take(limit).read_to_end(&mut bytes)?;
  if bytes.len() > MAX_MANIFEST_BYTES {
    return Ok(String::from_utf8_lossy(&bytes).into_owned());
  }
  String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
