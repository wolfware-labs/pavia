//! The method registry (§8.1): the methods each side of a session exposes, by name.
//!
//! A method is addressed as `"<service>/<method>"`, each segment `[A-Za-z_][A-Za-z0-9_]*`, and
//! names are case-sensitive. Server methods, which the client calls, and client methods, which
//! the server calls, are separate namespaces, so one name can be both. A name is checked when it
//! is registered, so a bad or duplicate name fails when the application builds its registry, not
//! when a peer first calls it.
//!
//! ```text
//!  register_server_method("chat/send") ─▶ name rule ─▶ not on the server side yet ─▶ stored
//!  CALL from the client "chat/send"    ─▶ server_method("chat/send")
//!  CALL from the server "ui/alert"     ─▶ client_method("ui/alert")
//! ```

use std::borrow::Borrow;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use pavia_proto::stream::{Shape, Side};
use thiserror::Error;

/// A method name that follows the rule of §8.1: `"<service>/<method>"`, each segment
/// `[A-Za-z_][A-Za-z0-9_]*`.
///
/// ```
/// use pavia_server::methods::MethodName;
///
/// assert!(MethodName::parse("chat/send").is_ok());
/// assert!(MethodName::parse("chat/1bad").is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MethodName(Arc<str>);

impl MethodName {
  /// Checks `name` against the rule of §8.1.
  ///
  /// # Errors
  ///
  /// [`InvalidMethodName`] unless `name` is two segments joined by one `/`, each segment an ASCII
  /// letter or `_` followed by ASCII letters, digits or `_`.
  pub fn parse(name: &str) -> Result<Self, InvalidMethodName> {
    match name.split_once('/') {
      Some((service, method)) if is_segment(service) && is_segment(method) => Ok(Self(name.into())),
      _ => Err(InvalidMethodName(name.to_owned())),
    }
  }

  /// The name as written.
  #[must_use]
  pub fn as_str(&self) -> &str {
    &self.0
  }
}

fn is_segment(segment: &str) -> bool {
  let mut bytes = segment.bytes();
  bytes.next().is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
    && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

impl FromStr for MethodName {
  type Err = InvalidMethodName;

  fn from_str(name: &str) -> Result<Self, InvalidMethodName> {
    Self::parse(name)
  }
}

impl Borrow<str> for MethodName {
  fn borrow(&self) -> &str {
    &self.0
  }
}

impl fmt::Display for MethodName {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str(&self.0)
  }
}

/// A name that breaks the method name rule of §8.1.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid method name {0:?}: expected \"<service>/<method>\", each segment [A-Za-z_][A-Za-z0-9_]*")]
pub struct InvalidMethodName(pub String);

/// Whether a method answers CALL or takes NOTIFY (§8.2, §9.1). A CALL naming a `Notify` method is
/// refused, and a NOTIFY naming a `Call` method is treated as one to an unknown method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Kind {
  /// Invoked with CALL; the callee answers.
  Call,
  /// Invoked with NOTIFY; nothing answers.
  Notify,
}

/// One registered method: its handler and the facts about it the session reads.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Method<H> {
  /// What runs when the method is invoked.
  pub handler: H,
  /// CALL or NOTIFY.
  pub kind: Kind,
  /// Who streams in a call (§8.1); [`Method::notify`] sets [`Shape::Unary`].
  pub shape: Shape,
  /// Whether a caller may retry the call (§12.2).
  pub idempotent: bool,
}

impl<H> Method<H> {
  /// A unary CALL method, not idempotent.
  pub const fn call(handler: H) -> Self {
    Self {
      handler,
      kind: Kind::Call,
      shape: Shape::Unary,
      idempotent: false,
    }
  }

  /// A NOTIFY method.
  pub const fn notify(handler: H) -> Self {
    Self {
      handler,
      kind: Kind::Notify,
      shape: Shape::Unary,
      idempotent: false,
    }
  }

  /// The same method with `shape`.
  #[must_use]
  pub const fn with_shape(mut self, shape: Shape) -> Self {
    self.shape = shape;
    self
  }

  /// The same method, marked idempotent.
  #[must_use]
  pub const fn idempotent(mut self) -> Self {
    self.idempotent = true;
    self
  }
}

/// Why a method could not be registered.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum RegisterError {
  /// The name breaks the rule of §8.1.
  #[error(transparent)]
  InvalidName(#[from] InvalidMethodName),
  /// A method with this name is already registered on the same side.
  #[error("method {0} is already registered")]
  Duplicate(MethodName),
}

/// The methods of both sides, built once when the application starts and then only read.
#[derive(Debug, Clone)]
pub struct MethodRegistry<H> {
  server: HashMap<MethodName, Method<H>>,
  client: HashMap<MethodName, Method<H>>,
}

impl<H> Default for MethodRegistry<H> {
  fn default() -> Self {
    Self {
      server: HashMap::new(),
      client: HashMap::new(),
    }
  }
}

impl<H> MethodRegistry<H> {
  /// A registry with no methods.
  #[must_use]
  pub fn new() -> Self {
    Self::default()
  }

  /// Registers `method` as a server method, which the client calls.
  ///
  /// # Errors
  ///
  /// [`RegisterError`] for a name that breaks §8.1 or that the server side already has; the
  /// registered method stays as it was.
  pub fn register_server_method(&mut self, name: &str, method: Method<H>) -> Result<(), RegisterError> {
    self.register(Side::Server, name, method)
  }

  /// Registers `method` as a client method, which the server calls (§8.8).
  ///
  /// # Errors
  ///
  /// [`RegisterError`] for a name that breaks §8.1 or that the client side already has; the
  /// registered method stays as it was.
  pub fn register_client_method(&mut self, name: &str, method: Method<H>) -> Result<(), RegisterError> {
    self.register(Side::Client, name, method)
  }

  /// The server method `name`, which a CALL or NOTIFY from the client resolves to.
  #[must_use]
  pub fn server_method(&self, name: &str) -> Option<&Method<H>> {
    self.server.get(name)
  }

  /// The client method `name`, which a CALL or NOTIFY from the server resolves to.
  #[must_use]
  pub fn client_method(&self, name: &str) -> Option<&Method<H>> {
    self.client.get(name)
  }

  fn register(&mut self, side: Side, name: &str, method: Method<H>) -> Result<(), RegisterError> {
    let name = MethodName::parse(name)?;
    let table = match side {
      Side::Server => &mut self.server,
      Side::Client => &mut self.client,
    };
    match table.entry(name) {
      Entry::Occupied(entry) => Err(RegisterError::Duplicate(entry.key().clone())),
      Entry::Vacant(entry) => {
        entry.insert(method);
        Ok(())
      }
    }
  }
}

#[cfg(test)]
mod tests;
