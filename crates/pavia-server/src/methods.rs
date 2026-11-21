//! The method registry (§8.1): the methods each side of a session exposes, by name.
//!
//! A method is addressed as `"<service>/<method>"`, each segment `[A-Za-z_][A-Za-z0-9_]*`, and
//! names are case-sensitive. Server methods, which the client calls, and client methods, which
//! the server calls, are separate namespaces, so one name can be both. A name is checked when it
//! is registered, so a bad or duplicate name fails when the application builds its registry, not
//! when a peer first calls it. A [`Handler`] is the code behind a method: the session runs it
//! with the CALL arguments as dynamic values. A handler that fails returns a [`HandlerError`]: a
//! deliberate [`StatusError`], answered as given, or an unexpected error, answered INTERNAL (§8.4).
//! A handler made with [`Handler::with_context`] also gets a [`HandlerContext`], through which it
//! can notify the client of the session that ran it (§9.1) or call one of its methods (§8.8).
//!
//! ```text
//!  register_server_method("chat/send") ─▶ name rule ─▶ not on the server side yet ─▶ stored
//!  CALL from the client "chat/send"    ─▶ server_method("chat/send")
//!  CALL from the server "ui/alert"     ─▶ client_method("ui/alert")
//! ```

use std::any::{self, TypeId};
use std::borrow::Borrow;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use pavia_contract::schema::Type;
use pavia_contract::{Contract, TypeTable};
use pavia_proto::data::{self, Codec, EncodeError, Value};
use pavia_proto::error::ErrorStatus;
use pavia_proto::frame::Meta;
use pavia_proto::session::Command;
use pavia_proto::stream::{Shape, Side};
use thiserror::Error;

use crate::sessions::{CallError, CallOptions, Handle};

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
      Some((service, method)) if pavia_contract::name::is_valid(service) && pavia_contract::name::is_valid(method) => {
        Ok(Self(name.into()))
      }
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

/// One registered method: its handler, the facts about it the session reads, and its signature, the
/// contract types the manifest lists for it (§12.2).
///
/// A signature is declared with [`param`](Self::param), [`returns`](Self::returns),
/// [`yields`](Self::yields), [`accepts`](Self::accepts) and [`error`](Self::error);
/// [`manifest::render`](crate::manifest::render) checks it against the kind, the shape and the handler.
/// The handler still takes and returns dynamic values; typed handlers, which decode them with
/// `pavia-codec`, arrive with the service macros (#218).
///
/// ```
/// use pavia_proto::stream::Shape;
/// use pavia_server::methods::MethodEntry;
///
/// let history = MethodEntry::call(()).with_shape(Shape::ServerStream).param::<String>("room").yields::<String>();
/// assert!(!history.declares_error());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MethodEntry<H> {
  /// What runs when the method is invoked.
  pub handler: H,
  /// CALL, with its shape and idempotence, or NOTIFY.
  pub kind: Kind,
  signature: Signature,
}

/// The contract types of a method's parameters, result, streams and error, and its docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Signature {
  pub(crate) params: Vec<(String, ContractType)>,
  pub(crate) returns: Option<ContractType>,
  pub(crate) yields: Option<ContractType>,
  pub(crate) accepts: Option<ContractType>,
  pub(crate) error: Option<ContractType>,
  pub(crate) docs: Option<String>,
}

impl Signature {
  const EMPTY: Self = Self {
    params: Vec::new(),
    returns: None,
    yields: None,
    accepts: None,
    error: None,
    docs: None,
  };
}

/// A Rust type that stands for a contract type in a signature, added to a [`TypeTable`] when the manifest
/// is exported. Two are equal when they stand for the same Rust type.
#[derive(Clone, Copy)]
pub(crate) struct ContractType {
  id: TypeId,
  name: &'static str,
  add: fn(&mut TypeTable) -> Type,
}

impl ContractType {
  fn of<T: Contract>() -> Self {
    Self {
      id: TypeId::of::<T>(),
      name: any::type_name::<T>(),
      add: TypeTable::add::<T>,
    }
  }

  pub(crate) fn add(self, table: &mut TypeTable) -> Type {
    (self.add)(table)
  }
}

impl PartialEq for ContractType {
  fn eq(&self, other: &Self) -> bool {
    self.id == other.id
  }
}

impl Eq for ContractType {}

impl fmt::Debug for ContractType {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str(self.name)
  }
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
      signature: Signature::EMPTY,
    }
  }

  /// A NOTIFY method.
  #[must_use]
  pub const fn notify(handler: H) -> Self {
    Self {
      handler,
      kind: Kind::Notify,
      signature: Signature::EMPTY,
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

  /// The same method with a parameter named `name` of `T`'s contract type, after its other parameters.
  /// Arguments are encoded in parameter order (§8.2).
  #[must_use]
  pub fn param<T: Contract>(mut self, name: impl Into<String>) -> Self {
    self.signature.params.push((name.into(), ContractType::of::<T>()));
    self
  }

  /// The same method, returning `T`: the RESULT value of a `unary` or `client_stream` call. A call that
  /// declares none returns `unit`.
  #[must_use]
  pub fn returns<T: Contract>(mut self) -> Self {
    self.signature.returns = Some(ContractType::of::<T>());
    self
  }

  /// The same method, whose callee streams items of `T`: a `server_stream` or `bidi` call.
  #[must_use]
  pub fn yields<T: Contract>(mut self) -> Self {
    self.signature.yields = Some(ContractType::of::<T>());
    self
  }

  /// The same method, whose caller streams items of `T`: a `client_stream` or `bidi` call.
  #[must_use]
  pub fn accepts<T: Contract>(mut self) -> Self {
    self.signature.accepts = Some(ContractType::of::<T>());
    self
  }

  /// The same method, whose ERROR may carry a detail of `T` (§8.4). A call that declares none sends
  /// every ERROR with an empty data section.
  #[must_use]
  pub fn error<T: Contract>(mut self) -> Self {
    self.signature.error = Some(ContractType::of::<T>());
    self
  }

  /// The same method, documented in the manifest.
  #[must_use]
  pub fn docs(mut self, docs: impl Into<String>) -> Self {
    self.signature.docs = Some(docs.into());
    self
  }

  /// Whether the method declares an error type, so that its ERROR may carry a detail (§8.4).
  #[must_use]
  pub const fn declares_error(&self) -> bool {
    self.signature.error.is_some()
  }

  pub(crate) const fn signature(&self) -> &Signature {
    &self.signature
  }
}

type Running = Pin<Box<dyn Future<Output = Result<Value, HandlerError>> + Send>>;

type Run = dyn Fn(HandlerContext, Vec<Value>) -> Running + Send + Sync;

/// The code behind a method: it takes the CALL arguments as dynamic values (§8.2, §12.3) and
/// returns the RESULT value, or with [`fallible`](Self::fallible) a [`HandlerError`]. A method
/// with no return value returns [`Value::Null`], the codec's unit value. A handler made with
/// [`with_context`](Self::with_context) also gets the [`HandlerContext`] of the session that
/// called it. Typed handlers arrive with the service macros (#218).
#[derive(Clone)]
pub struct Handler {
  params: usize,
  run: Arc<Run>,
}

impl Handler {
  /// A handler for a method with `params` parameters. The session checks the argument count
  /// before it runs `run`, so `run` always gets exactly `params` values.
  #[must_use]
  pub fn new<F, Fut>(params: usize, run: F) -> Self
  where
    F: Fn(Vec<Value>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Value> + Send + 'static,
  {
    Self::fallible(params, move |arguments| {
      let running = run(arguments);
      async move { Ok(running.await) }
    })
  }

  /// A handler for a method with `params` parameters that can fail. A [`HandlerError::Status`]
  /// is answered as given; any other error is unexpected and answered INTERNAL (§8.4).
  #[must_use]
  pub fn fallible<F, Fut>(params: usize, run: F) -> Self
  where
    F: Fn(Vec<Value>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, HandlerError>> + Send + 'static,
  {
    Self::with_context(params, move |_, arguments| run(arguments))
  }

  /// A handler for a method with `params` parameters that can fail and reaches the session that
  /// called it through its [`HandlerContext`], to notify the client for one.
  #[must_use]
  pub fn with_context<F, Fut>(params: usize, run: F) -> Self
  where
    F: Fn(HandlerContext, Vec<Value>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, HandlerError>> + Send + 'static,
  {
    Self {
      params,
      run: Arc::new(move |context, arguments| Box::pin(run(context, arguments))),
    }
  }

  /// How many arguments the method takes.
  #[must_use]
  pub const fn params(&self) -> usize {
    self.params
  }

  pub(crate) fn run(&self, context: HandlerContext, arguments: Vec<Value>) -> Running {
    (self.run)(context, arguments)
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

/// What a running handler knows of the session that called it. Cloning it is cheap.
#[derive(Debug, Clone)]
pub struct HandlerContext {
  session: Handle,
  codec: Codec,
}

impl HandlerContext {
  pub(crate) const fn new(session: Handle, codec: Codec) -> Self {
    Self { session, codec }
  }

  pub(crate) const fn codec(&self) -> Codec {
    self.codec
  }

  /// Sends NOTIFY to the session's client, calling its client method `method` with `arguments`
  /// encoded as one array in the session codec (§9.1). Notifications from one handler reach the
  /// client in the order they were sent, on the control stream; nothing orders them against the
  /// call's answer (§8.9). It waits while the session's command queue is full. The client never
  /// answers, so a notification it cannot take, one above its `max_frame` for one, is dropped and
  /// logged.
  ///
  /// Returns `true` once the notification is queued for the session, and `false`, sending
  /// nothing, once the session has ended, including while its last frames are still being
  /// written: a send to an ended session is a no-op (§13.1). A notification queued just before the
  /// session ends is dropped with it.
  ///
  /// # Errors
  ///
  /// [`EncodeError`] when the session codec cannot encode `arguments`, whether or not the session
  /// has ended: the arguments are the handler's mistake, so they are checked first. Nothing is
  /// sent.
  pub async fn notify(&self, method: &MethodName, arguments: Vec<Value>) -> Result<bool, EncodeError> {
    let data = encode(&Value::Array(arguments), self.codec)?;
    let notify = Command::Notify {
      method: method.as_str().to_owned(),
      data,
    };
    Ok(self.session.send(notify).await.is_ok())
  }

  /// Calls the client method `method` of the session that called this handler and returns its
  /// answer, as [`Handle::call`] does (§8.8).
  ///
  /// # Errors
  ///
  /// The errors of [`Handle::call`].
  pub async fn call(&self, method: &MethodName, arguments: Vec<Value>, options: CallOptions) -> Result<Value, CallError> {
    self.session.call(method, arguments, options).await
  }
}

pub(crate) fn encode(value: &Value, codec: Codec) -> Result<Bytes, EncodeError> {
  let mut encoded = BytesMut::new();
  data::encode(value, codec, &mut encoded)?;
  Ok(encoded.freeze())
}

/// How a handler failed (§8.4).
#[derive(Debug)]
#[non_exhaustive]
pub enum HandlerError {
  /// A deliberate error, answered exactly as given.
  Status(StatusError),
  /// Anything else, such as a database or I/O error. It is answered INTERNAL with the configured
  /// generic message, or its own text in development mode, and logged in full either way.
  Unexpected(Box<dyn Error + Send + Sync>),
}

impl HandlerError {
  /// An unexpected failure: anything that converts into a boxed error, an `anyhow::Error` too.
  #[must_use]
  pub fn unexpected(error: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
    Self::Unexpected(error.into())
  }
}

impl From<StatusError> for HandlerError {
  fn from(status: StatusError) -> Self {
    Self::Status(status)
  }
}

/// A deliberate error: a status that is never OK, a message, and optionally a typed detail and
/// `meta` trailers (§8.4). The detail of a method that declares no error type
/// ([`MethodEntry::error`]) is dropped and logged: its ERROR carries an empty data section.
///
/// ```
/// use pavia_proto::error::ErrorStatus;
/// use pavia_proto::registry::StatusCode;
/// use pavia_server::methods::StatusError;
///
/// let not_found = ErrorStatus::try_from(StatusCode::NotFound).unwrap();
/// let error = StatusError::new(not_found, "no such user");
/// assert_eq!(error.msg(), "no such user");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct StatusError {
  code: ErrorStatus,
  msg: String,
  detail: Option<Value>,
  meta: Option<Meta>,
}

impl StatusError {
  /// An error with `code` and `msg`, no detail and no trailers.
  #[must_use]
  pub fn new(code: ErrorStatus, msg: impl Into<String>) -> Self {
    Self {
      code,
      msg: msg.into(),
      detail: None,
      meta: None,
    }
  }

  /// The same error with a typed detail, sent in the ERROR data section.
  #[must_use]
  pub fn with_detail(mut self, detail: Value) -> Self {
    self.detail = Some(detail);
    self
  }

  /// The same error with `meta` trailers in the ERROR header.
  #[must_use]
  pub fn with_meta(mut self, meta: Meta) -> Self {
    self.meta = Some(meta);
    self
  }

  /// The status.
  #[must_use]
  pub const fn code(&self) -> ErrorStatus {
    self.code
  }

  /// The message.
  #[must_use]
  pub fn msg(&self) -> &str {
    &self.msg
  }

  pub(crate) fn into_parts(self) -> (ErrorStatus, String, Option<Value>, Option<Meta>) {
    (self.code, self.msg, self.detail, self.meta)
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
