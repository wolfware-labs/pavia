//! The method registry (§8.1): the methods each side of a session exposes, by name.
//!
//! A method is addressed as `"<service>/<method>"`, each segment `[A-Za-z_][A-Za-z0-9_]*`, and
//! names are case-sensitive. Server methods, which the client calls, and client methods, which
//! the server calls, are separate namespaces, so one name can be both. A name is checked when it
//! is registered, so a bad or duplicate name fails when the application builds its registry, not
//! when a peer first calls it. A [`Handler`] is the code behind a method: the session runs it
//! with the CALL arguments as dynamic values.
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
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;

use pavia_proto::data::Value;
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

  /// The service segment, before the `/`.
  #[must_use]
  pub fn service(&self) -> &str {
    self.halves().0
  }

  /// The method segment, after the `/`.
  #[must_use]
  pub fn method(&self) -> &str {
    self.halves().1
  }

  fn halves(&self) -> (&str, &str) {
    self.0.split_once('/').unwrap_or((&self.0, ""))
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

/// Whether a method answers CALL or takes NOTIFY. A CALL naming a `Notify` method is answered
/// INVALID_ARGUMENT (§8.2 check 3, #48), and a NOTIFY naming a `Call` method is handled like one to
/// an unknown method (§9.1, #55); the registry only records the kind. Only a CALL method has a
/// shape and can be idempotent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Kind {
  /// Invoked with CALL; the callee answers.
  Call {
    /// Who streams in the call (§8.1).
    shape: Shape,
    /// Whether a caller may retry the call (§12.2).
    idempotent: bool,
  },
  /// Invoked with NOTIFY; nothing answers.
  Notify,
}

/// One registered method: its handler and the facts about it the session reads.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MethodEntry<H> {
  /// What runs when the method is invoked.
  pub handler: H,
  /// CALL, with its shape and idempotence, or NOTIFY.
  pub kind: Kind,
}

impl<H> MethodEntry<H> {
  /// A unary CALL method, not idempotent.
  #[must_use]
  pub const fn call(handler: H) -> Self {
    Self {
      handler,
      kind: Kind::Call {
        shape: Shape::Unary,
        idempotent: false,
      },
    }
  }

  /// A NOTIFY method.
  #[must_use]
  pub const fn notify(handler: H) -> Self {
    Self {
      handler,
      kind: Kind::Notify,
    }
  }

  /// The same method with `shape`. A NOTIFY method has no shape and comes back unchanged.
  #[must_use]
  pub const fn with_shape(mut self, shape: Shape) -> Self {
    if let Kind::Call { idempotent, .. } = self.kind {
      self.kind = Kind::Call { shape, idempotent };
    }
    self
  }

  /// The same method, marked idempotent. A NOTIFY method is never retried and comes back
  /// unchanged.
  #[must_use]
  pub const fn idempotent(mut self) -> Self {
    if let Kind::Call { shape, .. } = self.kind {
      self.kind = Kind::Call { shape, idempotent: true };
    }
    self
  }
}

type Running = Pin<Box<dyn Future<Output = Value> + Send>>;

type Run = dyn Fn(Vec<Value>) -> Running + Send + Sync;

/// The code behind a method: it takes the CALL arguments as dynamic values (§8.2, §12.3) and
/// returns the RESULT value. A method with no return value returns [`Value::Null`], the codec's
/// unit value. Typed handlers arrive with the typed codec mapping (#66).
#[derive(Clone)]
pub struct Handler {
  params: usize,
  run: Arc<Run>,
}

impl Handler {
  /// A handler for a method with `params` parameters. The session checks the argument count
  /// before it runs `run`, so `run` always gets exactly `params` values.
  pub fn new<F, Fut>(params: usize, run: F) -> Self
  where
    F: Fn(Vec<Value>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Value> + Send + 'static,
  {
    Self {
      params,
      run: Arc::new(move |arguments| Box::pin(run(arguments))),
    }
  }

  /// How many arguments the method takes.
  #[must_use]
  pub const fn params(&self) -> usize {
    self.params
  }

  pub(crate) fn run(&self, arguments: Vec<Value>) -> Running {
    (self.run)(arguments)
  }
}

impl fmt::Debug for Handler {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter
      .debug_struct("Handler")
      .field("params", &self.params)
      .finish_non_exhaustive()
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
  server: HashMap<MethodName, MethodEntry<H>>,
  client: HashMap<MethodName, MethodEntry<H>>,
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
  pub fn register_server_method(&mut self, name: &str, method: MethodEntry<H>) -> Result<(), RegisterError> {
    self.register(Side::Server, name, method)
  }

  /// Registers `method` as a client method, which the server calls (§8.8).
  ///
  /// # Errors
  ///
  /// [`RegisterError`] for a name that breaks §8.1 or that the client side already has; the
  /// registered method stays as it was.
  pub fn register_client_method(&mut self, name: &str, method: MethodEntry<H>) -> Result<(), RegisterError> {
    self.register(Side::Client, name, method)
  }

  /// The server method `name`, which a CALL or NOTIFY from the client resolves to.
  #[must_use]
  pub fn server_method(&self, name: &str) -> Option<&MethodEntry<H>> {
    self.server.get(name)
  }

  pub(crate) fn server_entry(&self, name: &str) -> Option<(&MethodName, &MethodEntry<H>)> {
    self.server.get_key_value(name)
  }

  /// The client method `name`, which a CALL or NOTIFY from the server resolves to.
  #[must_use]
  pub fn client_method(&self, name: &str) -> Option<&MethodEntry<H>> {
    self.client.get(name)
  }

  /// Every server method, in no particular order.
  pub fn server_methods(&self) -> impl Iterator<Item = (&MethodName, &MethodEntry<H>)> {
    self.server.iter()
  }

  /// Every client method, in no particular order.
  pub fn client_methods(&self) -> impl Iterator<Item = (&MethodName, &MethodEntry<H>)> {
    self.client.iter()
  }

  fn register(&mut self, side: Side, name: &str, method: MethodEntry<H>) -> Result<(), RegisterError> {
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
