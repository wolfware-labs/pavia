//! Exporting the contract manifest (§12.2) from the methods an application registers.
//!
//! The application writes its manifest: only its binary has the method registry and the Rust types
//! behind each signature, and the `pavia` CLI never links application code. A test that exports the
//! manifest and compares it with the committed `manifest.json` keeps that file current; a build step or
//! a command-line flag the application wires works the same way.
//!
//! ```text
//!  MethodRegistry ─▶ signatures checked against shapes and handlers ─▶ types collected in a TypeTable
//!                 ─▶ Manifest, grouped by service ─▶ validated and rendered ─▶ manifest.json
//! ```
//!
//! ```
//! use pavia_proto::data::Value;
//! use pavia_server::manifest;
//! use pavia_server::methods::{Handler, MethodEntry, MethodRegistry};
//!
//! let mut methods = MethodRegistry::new();
//! let echo = Handler::new(1, |mut arguments| async move { arguments.remove(0) });
//! methods
//!   .register_server_method("chat/echo", MethodEntry::call(echo).param::<String>("text").returns::<String>())
//!   .unwrap();
//! let text = manifest::render("example-app", &methods).unwrap();
//! assert!(text.contains("\"returns\": \"string\""));
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use pavia_contract::manifest::{self as contract, Call, Manifest, ManifestError, Method, Param, Service};
use pavia_contract::schema::{Primitive, Type};
use pavia_contract::{TypeError, TypeTable};
use pavia_proto::contract::Fingerprint;
use pavia_proto::stream::{Shape, Side};
use thiserror::Error;

use crate::methods::{ContractType, Handler, Kind, MethodEntry, MethodName, MethodRegistry, Signature};

/// The manifest of the endpoint named `name` whose methods are `methods`, with no channels or datagram
/// topics: declaring those from Rust arrives with #171 and #138.
///
/// # Errors
///
/// [`ExportError::Signatures`] when a method's signature does not fit its kind, shape or handler, and
/// [`ExportError::Types`] when the Rust types of the signatures do not make a valid `types` table. The
/// manifest itself is checked when it is rendered.
pub fn manifest(name: &str, methods: &MethodRegistry<Handler>) -> Result<Manifest, ExportError> {
  let mut table = TypeTable::new();
  let mut problems = Vec::new();
  let mut services: BTreeMap<&str, Service> = BTreeMap::new();
  for (side, entries) in [
    (Side::Server, sorted(methods.server_methods())),
    (Side::Client, sorted(methods.client_methods())),
  ] {
    for (name, entry) in entries {
      let Some(method) = method(side, name, entry, &mut table, &mut problems) else {
        continue;
      };
      let service = services
        .remove(name.service())
        .unwrap_or_else(|| Service::new(name.service()));
      let service = match side {
        Side::Server => service.server_method(method),
        Side::Client => service.client_method(method),
      };
      services.insert(name.service(), service);
    }
  }
  if !problems.is_empty() {
    return Err(ExportError::Signatures(problems));
  }
  let types = table.finish().map_err(ExportError::Types)?;
  Ok(
    services
      .into_values()
      .fold(Manifest::new(name), Manifest::service)
      .types(types),
  )
}

/// The manifest of [`manifest`] as JSON text, the same bytes for the same methods.
///
/// # Errors
///
/// The errors of [`manifest`], and [`ExportError::Manifest`] for what the manifest's validation finds.
pub fn render(name: &str, methods: &MethodRegistry<Handler>) -> Result<String, ExportError> {
  manifest(name, methods)?.render().map_err(ExportError::Manifest)
}

/// The contract fingerprint (§12.5) of the manifest of [`manifest`], for
/// [`Config::contract`](pavia_proto::session::Config::contract) and the WELCOME of every session.
///
/// Unlike [`Manifest::fingerprint`](pavia_contract::manifest::Manifest::fingerprint), it renders the
/// manifest first: a server fingerprints only a contract whose manifest it can write, size bound included.
///
/// # Errors
///
/// The errors of [`render`].
pub fn fingerprint(name: &str, methods: &MethodRegistry<Handler>) -> Result<Fingerprint, ExportError> {
  let manifest = manifest(name, methods)?;
  manifest.render().map_err(ExportError::Manifest)?;
  manifest.fingerprint().map(Fingerprint::new).map_err(ExportError::Manifest)
}

/// Writes the manifest of [`render`] to `writer`.
///
/// # Errors
///
/// The errors of [`render`], and [`ExportError::Io`] when writing fails. Nothing is written unless the
/// manifest is valid.
pub fn write(name: &str, methods: &MethodRegistry<Handler>, writer: &mut impl io::Write) -> Result<(), ExportError> {
  let text = render(name, methods)?;
  writer.write_all(text.as_bytes())?;
  Ok(())
}

/// Writes the manifest of [`render`] to the file at `path`, replacing it.
///
/// # Errors
///
/// The errors of [`render`], and [`ExportError::Io`] when the file cannot be written. The file is left as
/// it was unless the manifest is valid.
pub fn export(name: &str, methods: &MethodRegistry<Handler>, path: impl AsRef<Path>) -> Result<(), ExportError> {
  let text = render(name, methods)?;
  fs::write(path, text)?;
  Ok(())
}

/// Why a manifest could not be exported.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExportError {
  /// Method signatures that do not fit their method.
  #[error("{}", lines(.0))]
  Signatures(Vec<SignatureError>),
  /// The Rust types of the signatures do not make a valid `types` table (§12.1).
  #[error("{}", lines(.0))]
  Types(Vec<TypeError>),
  /// The manifest breaks a rule of §12.2.
  #[error("{}", lines(.0))]
  Manifest(Vec<ManifestError>),
  /// Writing the manifest failed.
  #[error("cannot write the manifest: {0}")]
  Io(#[from] io::Error),
}

/// A method signature that does not fit its method.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{} method {method}: {problem}", side_name(*.side))]
pub struct SignatureError {
  side: Side,
  method: MethodName,
  problem: Problem,
}

impl SignatureError {
  /// The method.
  #[must_use]
  pub const fn method(&self) -> &MethodName {
    &self.method
  }

  /// What does not fit.
  #[must_use]
  pub const fn problem(&self) -> &Problem {
    &self.problem
  }
}

/// What in a method's signature does not fit the method.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum Problem {
  /// The signature declares a different number of parameters than the handler takes.
  #[error("the signature declares {declared} params, the handler takes {handler}")]
  ParamCount {
    /// Parameters declared with `param`.
    declared: usize,
    /// Arguments the handler takes.
    handler: usize,
  },
  /// The shape streams items whose type the signature does not declare.
  #[error("a {shape} call needs `{key}`")]
  Missing {
    /// The shape, as the manifest names it.
    shape: &'static str,
    /// `"yields"` or `"accepts"`.
    key: &'static str,
  },
  /// The signature declares a type the method's kind or shape does not carry.
  #[error("a {what} has no `{key}`")]
  Unused {
    /// `"notification"`, or the shape of the call: `"server_stream call"`.
    what: String,
    /// `"returns"`, `"yields"`, `"accepts"` or `"error"`.
    key: &'static str,
  },
}

/// The word for the side that exposes a method, as error messages name it.
const fn side_name(side: Side) -> &'static str {
  match side {
    Side::Server => "server",
    Side::Client => "client",
  }
}

fn sorted<'a>(
  methods: impl Iterator<Item = (&'a MethodName, &'a MethodEntry<Handler>)>,
) -> Vec<(&'a MethodName, &'a MethodEntry<Handler>)> {
  let mut methods: Vec<_> = methods.collect();
  methods.sort_by_key(|(name, _)| *name);
  methods
}

fn method(
  side: Side,
  name: &MethodName,
  entry: &MethodEntry<Handler>,
  table: &mut TypeTable,
  problems: &mut Vec<SignatureError>,
) -> Option<Method> {
  let signature = entry.signature();
  let mut found = Vec::new();
  if signature.params.len() != entry.handler.params() {
    found.push(Problem::ParamCount {
      declared: signature.params.len(),
      handler: entry.handler.params(),
    });
  }
  let call = match entry.kind {
    Kind::Call { shape, idempotent } => call(shape, idempotent, signature, table, &mut found),
    Kind::Notify => {
      unused("notification", signature, &[], &mut found);
      None
    }
  };
  if !found.is_empty() {
    problems.extend(found.into_iter().map(|problem| SignatureError {
      side,
      method: name.clone(),
      problem,
    }));
    return None;
  }
  let mut method = call.map_or_else(|| Method::notify(name.method()), |call| Method::call(name.method(), call));
  for (param, ty) in &signature.params {
    method = method.param(Param::new(param.as_str(), ty.add(table)));
  }
  if let Some(docs) = &signature.docs {
    method = method.docs(docs.as_str());
  }
  Some(method)
}

fn call(
  shape: Shape,
  idempotent: bool,
  signature: &Signature,
  table: &mut TypeTable,
  found: &mut Vec<Problem>,
) -> Option<Call> {
  let (manifest_shape, carried): (contract::Shape, &[&str]) = match shape {
    Shape::Unary => (contract::Shape::Unary, &["returns", "error"]),
    Shape::ServerStream => (contract::Shape::ServerStream, &["yields", "error"]),
    Shape::ClientStream => (contract::Shape::ClientStream, &["accepts", "returns", "error"]),
    Shape::Bidirectional => (contract::Shape::Bidi, &["accepts", "yields", "error"]),
  };
  let name = manifest_shape.name();
  unused(&format!("{name} call"), signature, carried, found);
  let mut needed = |key: &'static str, ty: Option<ContractType>| {
    if ty.is_none() {
      found.push(Problem::Missing { shape: name, key });
    }
    ty
  };
  let returns = signature.returns.map_or(Type::Primitive(Primitive::Unit), |ty| ty.add(table));
  let streams = match shape {
    Shape::Unary => Some(Call::unary(returns)),
    Shape::ServerStream => needed("yields", signature.yields).map(|yields| Call::server_stream(yields.add(table))),
    Shape::ClientStream => needed("accepts", signature.accepts).map(|accepts| Call::client_stream(accepts.add(table), returns)),
    Shape::Bidirectional => {
      let accepts = needed("accepts", signature.accepts);
      let yields = needed("yields", signature.yields);
      accepts
        .zip(yields)
        .map(|(accepts, yields)| Call::bidi(accepts.add(table), yields.add(table)))
    }
  }?;
  let call = match signature.error {
    Some(error) => streams.error(error.add(table)),
    None => streams,
  };
  Some(if idempotent { call.idempotent() } else { call })
}

/// Reports each type `signature` declares that is not one of the `carried` keys.
fn unused(what: &str, signature: &Signature, carried: &[&str], found: &mut Vec<Problem>) {
  let declared = [
    ("returns", signature.returns.is_some()),
    ("yields", signature.yields.is_some()),
    ("accepts", signature.accepts.is_some()),
    ("error", signature.error.is_some()),
  ];
  for (key, _) in declared.into_iter().filter(|(key, is)| *is && !carried.contains(key)) {
    found.push(Problem::Unused {
      what: what.to_owned(),
      key,
    });
  }
}

fn lines(errors: &[impl fmt::Display]) -> String {
  errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests;
