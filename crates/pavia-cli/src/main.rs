//! The `pavia` command-line tool.
//!
//! `pavia codegen ts <manifest.json> --out <dir> [--names identity|camel]` reads a manifest file
//! (§12.2) and writes the TypeScript module for it to `<dir>/contract.ts`, creating `<dir>` when it
//! is missing. `--names` picks the naming policy for fields, params and methods: the manifest's
//! names as they are (`identity`, the default), or their camelCase form (`camel`), which the module
//! maps back to the manifest's names on the wire (§12.3).

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use pavia_codegen::typescript::Naming;
use pavia_contract::manifest::{MAX_MANIFEST_BYTES, Manifest, escaped};

const USAGE: &str = "usage: pavia codegen ts <manifest.json> --out <dir> [--names identity|camel]";

/// Why a command did not run: the command line is wrong, or the work failed.
enum Failure {
  Usage,
  Failed(String),
}

fn main() -> ExitCode {
  let args: Vec<OsString> = std::env::args_os().skip(1).collect();
  match run(&args) {
    Ok(written) => {
      println!("pavia: wrote {}", printable(&written.display().to_string()));
      ExitCode::SUCCESS
    }
    Err(Failure::Usage) => {
      eprintln!("{USAGE}");
      ExitCode::from(2)
    }
    Err(Failure::Failed(message)) => {
      eprintln!("pavia: {}", printable(&message));
      ExitCode::FAILURE
    }
  }
}

/// The most of a message [`printable`] keeps, in bytes, so a manifest with many long errors still
/// prints a bounded message.
const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// `message` as the terminal may show it: [`escaped`], and cut after [`MAX_MESSAGE_BYTES`] with a
/// count of the bytes left out. Manifest errors already arrive escaped; this also covers the paths
/// and system errors a message carries.
fn printable(message: &str) -> String {
  let (kept, rest) = message.split_at(message.floor_char_boundary(MAX_MESSAGE_BYTES));
  match rest.len() {
    0 => escaped(kept).to_string(),
    1 => format!("{}… (1 more byte)", escaped(kept)),
    left => format!("{}… ({left} more bytes)", escaped(kept)),
  }
}

fn run(args: &[OsString]) -> Result<PathBuf, Failure> {
  match args {
    [command, target, rest @ ..] if command == "codegen" && target == "ts" => {
      let args = codegen_args(rest).ok_or(Failure::Usage)?;
      codegen(&args).map_err(Failure::Failed)
    }
    _ => Err(Failure::Usage),
  }
}

/// What `pavia codegen ts` was asked to do.
struct CodegenArgs {
  manifest: PathBuf,
  out: PathBuf,
  naming: Naming,
}

/// The manifest path, the `--out` folder and, at most once, `--names`; the first two exactly once,
/// and nothing else.
fn codegen_args(args: &[OsString]) -> Option<CodegenArgs> {
  let mut manifest = None;
  let mut out = None;
  let mut naming = None;
  let mut args = args.iter();
  while let Some(arg) = args.next() {
    if arg == "--out" {
      if out.replace(PathBuf::from(args.next()?)).is_some() {
        return None;
      }
    } else if arg == "--names" {
      if naming.replace(naming_of(args.next()?)?).is_some() {
        return None;
      }
    } else if arg.as_encoded_bytes().starts_with(b"--") || manifest.replace(PathBuf::from(arg)).is_some() {
      return None;
    }
  }
  Some(CodegenArgs {
    manifest: manifest?,
    out: out?,
    naming: naming.unwrap_or_default(),
  })
}

/// The naming policy a `--names` value names.
fn naming_of(value: &OsStr) -> Option<Naming> {
  match value.to_str()? {
    "identity" => Some(Naming::Identity),
    "camel" => Some(Naming::Camel),
    _ => None,
  }
}

fn codegen(args: &CodegenArgs) -> Result<PathBuf, String> {
  let CodegenArgs { manifest, out, naming } = args;
  let text = read_bounded(manifest).map_err(|error| format!("cannot read {}: {error}", manifest.display()))?;
  let manifest = Manifest::read(&text).map_err(|error| format!("{}: {error}", manifest.display()))?;
  let code = pavia_codegen::typescript::generate_with(&manifest, *naming).map_err(|error| error.to_string())?;
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

#[cfg(test)]
mod tests;
