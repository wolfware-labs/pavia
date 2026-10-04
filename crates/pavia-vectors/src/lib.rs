//! Test support for the vectors under `vectors/`: a diagnostic-notation parser and helpers to load
//! the vector files. Not published; used only by tests.

pub mod diag;

use std::path::{Path, PathBuf};

/// The repository's `vectors/` directory.
pub fn vectors_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vectors")
}

/// Every `*.json` file of one vector kind (`frames`, `codecs` or `scripts`), sorted by path.
pub fn files(kind: &str) -> Vec<PathBuf> {
    let dir = vectors_dir().join(kind);
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| entry.expect("directory entry").path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
        .collect();
    out.sort();
    out
}

/// Parses a vector file as JSON.
pub fn load(path: &Path) -> serde_json::Value {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parsing {}: {e}", path.display()))
}

/// Decodes a vector `hex` string, ignoring the spaces between bytes.
pub fn hex(s: &str) -> Vec<u8> {
    let digits: Vec<u8> = s.bytes().filter(|b| *b != b' ').collect();
    diag::hex_decode(&digits).unwrap_or_else(|e| panic!("hex {s:?}: {e}"))
}
