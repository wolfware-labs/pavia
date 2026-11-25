//! The contract manifest (§12.2): the services, channels, datagram topics and named types of an endpoint,
//! as the JSON document client generators read.
//!
//! A [`Manifest`] is built from its parts, or read back from its JSON with [`Manifest::read`], then
//! [`Manifest::render`] checks it and writes its JSON. The model leaves out what §12.2 forbids instead of
//! checking for it: a notification has no shape or result types, an absent optional field has no `null` to
//! write, and a field equal to its default (`"returns": "unit"`, `"idempotent": false`, `"history": false`,
//! an empty `params`) is never written. What the model can hold but §12.1 and §12.2 forbid,
//! [`Manifest::validate`] reports: a name outside the grammar,
//! a `ref` to no named type, an `Option` of `unit` (directly or through aliases), an `optional` field whose
//! type is not an `option`, an open enum with a variant named `Unknown`, an alias that leads back to itself,
//! and two entries of one list with one name. It also refuses a type nested deeper than [`MAX_TYPE_DEPTH`],
//! and [`Manifest::render`] text longer than [`MAX_MANIFEST_BYTES`]: [`Manifest::read`] would refuse both.
//!
//! The output depends only on the manifest's content: services, methods, channels and datagram topics
//! are sorted by name in byte order, while params, struct fields and enum variants keep the order they
//! were declared in, which is their encoding order (§8.2).
//!
//! ```
//! use pavia_contract::manifest::{Call, Manifest, Method, Param, Service};
//! use pavia_contract::schema::{Primitive, Type};
//!
//! let manifest = Manifest::new("example-app").service(
//!   Service::new("chat").server_method(
//!     Method::call("send", Call::unary(Primitive::U64.into())).param(Param::new("text", Primitive::String.into())),
//!   ),
//! );
//! let json: serde_json::Value = serde_json::from_str(&manifest.render().unwrap()).unwrap();
//! assert_eq!(json["services"][0]["server_methods"][0]["returns"], "u64");
//! ```

use std::collections::BTreeSet;

use serde_json::{Map, Value};
use thiserror::Error;

use crate::name;
use crate::schema::{Definition, Primitive, Struct, Type, Types};
use crate::table::{Chain, Chains};

mod read;

pub use read::ReadError;

/// The largest manifest text [`Manifest::read`] takes, and [`Manifest::render`] writes, in bytes (§12.2,
/// §16).
pub const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;

/// How deep a type may nest (§12.2, §16), counted form by form: a primitive such as `"u8"` and a `ref` are
/// 1 deep, so `{"list": {"list": "u8"}}` is 3; a tuple's items sit 2 deeper than the tuple and an inline
/// struct's field types 4 deeper (the JSON levels between them), while a tuple with no items and a struct
/// with no fields are 1 deep. Each type a manifest names (a param, an `accepts`, `yields`, `returns` or
/// `error` type, a channel or datagram type, an alias, and a field or variant type of a named type) starts
/// at 1. [`Manifest::validate`] refuses a deeper type and [`Manifest::read`] stops at one, so every
/// manifest one writes the other reads.
pub const MAX_TYPE_DEPTH: usize = 32;

/// How much deeper than an `option`, `list` or `map` its inner type sits.
const INNER_STEP: usize = 1;
/// How much deeper than a `tuple` its items sit: `{"tuple": [item]}`.
const ITEM_STEP: usize = 2;
/// How much deeper than an inline struct its field types sit: `{"struct": {"fields": [{"type": t}]}}`.
const FIELD_STEP: usize = 4;

/// The manifest format version this crate writes, the top-level `pavia` key.
pub const FORMAT_VERSION: u64 = 1;

/// A contract manifest (§12.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
  name: String,
  docs: Option<String>,
  services: Vec<Service>,
  channels: Vec<Channel>,
  datagrams: Vec<Datagram>,
  types: Types,
}

impl Manifest {
  /// A manifest named `name`, with nothing in it. The name labels the endpoint for people and is outside
  /// the fingerprint (§12.5), so it follows no grammar.
  #[must_use]
  pub fn new(name: impl Into<String>) -> Self {
    Self {
      name: name.into(),
      docs: None,
      services: Vec::new(),
      channels: Vec::new(),
      datagrams: Vec::new(),
      types: Types::new(),
    }
  }

  /// The same manifest, documented.
  #[must_use]
  pub fn docs(mut self, docs: impl Into<String>) -> Self {
    self.docs = Some(docs.into());
    self
  }

  /// The same manifest with `service`.
  #[must_use]
  pub fn service(mut self, service: Service) -> Self {
    self.services.push(service);
    self
  }

  /// The same manifest with the channel declaration `channel`.
  #[must_use]
  pub fn channel(mut self, channel: Channel) -> Self {
    self.channels.push(channel);
    self
  }

  /// The same manifest with the datagram topic `datagram`.
  #[must_use]
  pub fn datagram(mut self, datagram: Datagram) -> Self {
    self.datagrams.push(datagram);
    self
  }

  /// The same manifest with `types` as its `types` table, such as the one a
  /// [`TypeTable`](crate::TypeTable) collects.
  #[must_use]
  pub fn types(mut self, types: Types) -> Self {
    self.types = types;
    self
  }

  /// Checks what the model can hold but §12.1 and §12.2 forbid, and a type deeper than [`MAX_TYPE_DEPTH`].
  ///
  /// # Errors
  ///
  /// Every [`ManifestError`] found, in the order of the output.
  pub fn validate(&self) -> Result<(), Vec<ManifestError>> {
    let mut check = Check {
      types: &self.types,
      chains: Chains::new(&self.types, &BTreeSet::new()),
      errors: Vec::new(),
    };
    check.unique("service", self.services.iter().map(|service| service.name.as_str()));
    for service in sorted(&self.services, |service| &service.name) {
      check.name("service", &service.name);
      for (side, methods) in [("server", &service.server_methods), ("client", &service.client_methods)] {
        check.unique(
          &format!("{side} method of service `{}`", service.name),
          methods.iter().map(|method| method.name.as_str()),
        );
        for method in sorted(methods, |method| &method.name) {
          check.method(&service.name, side, method);
        }
      }
    }
    check.unique(
      "channel namespace",
      self.channels.iter().map(|channel| channel.namespace.as_str()),
    );
    for channel in sorted(&self.channels, |channel| &channel.namespace) {
      check.channel(channel);
    }
    check.unique(
      "datagram topic",
      self.datagrams.iter().map(|datagram| datagram.topic.as_str()),
    );
    for datagram in sorted(&self.datagrams, |datagram| &datagram.topic) {
      check.name("datagram topic", &datagram.topic);
      check.ty(&datagram.payload, &format!("datagram topic `{}`", datagram.topic), 1);
    }
    for (name, definition) in &self.types {
      check.name("type", name);
      check.definition(name, definition);
    }
    for name in self.types.keys() {
      if matches!(check.chains.from(name), Chain::Loops) {
        check.errors.push(ManifestError::AliasCycle { name: name.clone() });
      }
    }
    if check.errors.is_empty() { Ok(()) } else { Err(check.errors) }
  }

  /// The manifest as JSON text: pretty-printed with two spaces, object keys sorted, named entries sorted
  /// by name, and a final newline. The same manifest always gives the same bytes.
  ///
  /// # Errors
  ///
  /// The errors of [`Manifest::validate`], or [`ManifestError::TooLarge`] for text over
  /// [`MAX_MANIFEST_BYTES`]; nothing is written.
  pub fn render(&self) -> Result<String, Vec<ManifestError>> {
    self.validate()?;
    let mut json = self.to_json();
    json.sort_all_objects();
    let mut text = serde_json::to_string_pretty(&json).expect("a JSON value with text keys serializes");
    text.push('\n');
    if text.len() > MAX_MANIFEST_BYTES {
      return Err(vec![ManifestError::TooLarge {
        size: text.len(),
        limit: MAX_MANIFEST_BYTES,
      }]);
    }
    Ok(text)
  }

  /// The contract fingerprint of the manifest (§12.5): the SHA-256 of its canonical form, without its
  /// `docs` and its `name`. The same contract always gives the same fingerprint.
  ///
  /// # Errors
  ///
  /// The errors of [`Manifest::validate`]. The size bound is not checked: it is a bound on text, and a
  /// manifest read from compact text has a fingerprint even when its pretty form is past the bound.
  pub fn fingerprint(&self) -> Result<[u8; 32], Vec<ManifestError>> {
    self.validate()?;
    Ok(crate::fingerprint::fingerprint(&self.to_json()).expect("the writer's output is in canonical form"))
  }

  fn to_json(&self) -> Value {
    let mut manifest = Map::new();
    manifest.insert("pavia".into(), Value::from(FORMAT_VERSION));
    manifest.insert("name".into(), Value::from(self.name.as_str()));
    insert_docs(&mut manifest, self.docs.as_deref());
    manifest.insert(
      "services".into(),
      sorted(&self.services, |service| &service.name)
        .map(Service::to_json)
        .collect(),
    );
    manifest.insert(
      "channels".into(),
      sorted(&self.channels, |channel| &channel.namespace)
        .map(Channel::to_json)
        .collect(),
    );
    manifest.insert(
      "datagrams".into(),
      sorted(&self.datagrams, |datagram| &datagram.topic)
        .map(Datagram::to_json)
        .collect(),
    );
    manifest.insert(
      "types".into(),
      self
        .types
        .iter()
        .map(|(name, definition)| (name.clone(), definition.to_json()))
        .collect::<Map<_, _>>()
        .into(),
    );
    Value::Object(manifest)
  }
}

/// A service: a group of methods under one name, the part before the `/` of a method name (§8.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
  name: String,
  docs: Option<String>,
  server_methods: Vec<Method>,
  client_methods: Vec<Method>,
}

impl Service {
  /// A service named `name`, with no methods.
  #[must_use]
  pub fn new(name: impl Into<String>) -> Self {
    Self {
      name: name.into(),
      docs: None,
      server_methods: Vec::new(),
      client_methods: Vec::new(),
    }
  }

  /// The same service, documented.
  #[must_use]
  pub fn docs(mut self, docs: impl Into<String>) -> Self {
    self.docs = Some(docs.into());
    self
  }

  /// The same service with the server method `method`, which the client calls.
  #[must_use]
  pub fn server_method(mut self, method: Method) -> Self {
    self.server_methods.push(method);
    self
  }

  /// The same service with the client method `method`, which the server calls (§8.8).
  #[must_use]
  pub fn client_method(mut self, method: Method) -> Self {
    self.client_methods.push(method);
    self
  }

  fn to_json(&self) -> Value {
    let mut service = Map::new();
    service.insert("name".into(), Value::from(self.name.as_str()));
    insert_docs(&mut service, self.docs.as_deref());
    service.insert(
      "server_methods".into(),
      sorted(&self.server_methods, |method| &method.name)
        .map(Method::to_json)
        .collect(),
    );
    service.insert(
      "client_methods".into(),
      sorted(&self.client_methods, |method| &method.name)
        .map(Method::to_json)
        .collect(),
    );
    Value::Object(service)
  }
}

/// A method: a CALL method with its shape and types, or a NOTIFY method (§8.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Method {
  name: String,
  docs: Option<String>,
  params: Vec<Param>,
  call: Option<Call>,
}

impl Method {
  /// A CALL method named `name`, with no parameters.
  #[must_use]
  pub fn call(name: impl Into<String>, call: Call) -> Self {
    Self {
      name: name.into(),
      docs: None,
      params: Vec::new(),
      call: Some(call),
    }
  }

  /// A NOTIFY method named `name`, with no parameters.
  #[must_use]
  pub fn notify(name: impl Into<String>) -> Self {
    Self {
      name: name.into(),
      docs: None,
      params: Vec::new(),
      call: None,
    }
  }

  /// The same method with `param` after its other parameters. Arguments are encoded in parameter order
  /// (§8.2).
  #[must_use]
  pub fn param(mut self, param: Param) -> Self {
    self.params.push(param);
    self
  }

  /// The same method, documented.
  #[must_use]
  pub fn docs(mut self, docs: impl Into<String>) -> Self {
    self.docs = Some(docs.into());
    self
  }

  fn to_json(&self) -> Value {
    let mut method = Map::new();
    method.insert("name".into(), Value::from(self.name.as_str()));
    insert_docs(&mut method, self.docs.as_deref());
    if !self.params.is_empty() {
      method.insert("params".into(), self.params.iter().map(Param::to_json).collect());
    }
    match &self.call {
      Some(call) => {
        method.insert("kind".into(), Value::from("call"));
        call.insert_into(&mut method);
      }
      None => {
        method.insert("kind".into(), Value::from("notify"));
      }
    }
    Value::Object(method)
  }
}

/// A method parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
  name: String,
  ty: Type,
  docs: Option<String>,
}

impl Param {
  /// A parameter named `name` of type `ty`.
  #[must_use]
  pub fn new(name: impl Into<String>, ty: Type) -> Self {
    Self {
      name: name.into(),
      ty,
      docs: None,
    }
  }

  /// The same parameter, documented.
  #[must_use]
  pub fn docs(mut self, docs: impl Into<String>) -> Self {
    self.docs = Some(docs.into());
    self
  }

  fn to_json(&self) -> Value {
    let mut param = Map::new();
    param.insert("name".into(), Value::from(self.name.as_str()));
    param.insert("type".into(), self.ty.to_json());
    insert_docs(&mut param, self.docs.as_deref());
    Value::Object(param)
  }
}

/// Who streams in a call (§8.1), as the manifest's `shape` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
  /// `unary` (0): one request, one result.
  Unary,
  /// `server_stream` (1): one request, then items from the callee.
  ServerStream,
  /// `client_stream` (2): items from the caller, then one result.
  ClientStream,
  /// `bidi` (3): items in both directions.
  Bidi,
}

impl Shape {
  /// Every shape, in wire order (§8.1).
  pub const ALL: [Self; 4] = [Self::Unary, Self::ServerStream, Self::ClientStream, Self::Bidi];

  /// The name in the manifest: `"unary"`, `"server_stream"`, `"client_stream"` or `"bidi"`.
  #[must_use]
  pub const fn name(self) -> &'static str {
    match self {
      Self::Unary => "unary",
      Self::ServerStream => "server_stream",
      Self::ClientStream => "client_stream",
      Self::Bidi => "bidi",
    }
  }
}

/// What a CALL method streams and returns, and how it fails (§8.1, §8.4). Each shape has its own
/// constructor, so a call carries exactly the types its shape uses: `returns` for `unary` and
/// `client_stream`, `yields` for `server_stream` and `bidi`, `accepts` for `client_stream` and `bidi`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
  streams: Streams,
  error: Option<Type>,
  idempotent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Streams {
  Unary { returns: Type },
  ServerStream { yields: Type },
  ClientStream { accepts: Type, returns: Type },
  Bidi { accepts: Type, yields: Type },
}

impl Call {
  /// A `unary` call that returns `returns`; `unit` for one that returns nothing.
  #[must_use]
  pub const fn unary(returns: Type) -> Self {
    Self::with(Streams::Unary { returns })
  }

  /// A `server_stream` call whose callee streams items of type `yields`.
  #[must_use]
  pub const fn server_stream(yields: Type) -> Self {
    Self::with(Streams::ServerStream { yields })
  }

  /// A `client_stream` call whose caller streams items of type `accepts`, answered with `returns`.
  #[must_use]
  pub const fn client_stream(accepts: Type, returns: Type) -> Self {
    Self::with(Streams::ClientStream { accepts, returns })
  }

  /// A `bidi` call: the caller streams `accepts` items, the callee `yields` items.
  #[must_use]
  pub const fn bidi(accepts: Type, yields: Type) -> Self {
    Self::with(Streams::Bidi { accepts, yields })
  }

  const fn with(streams: Streams) -> Self {
    Self {
      streams,
      error: None,
      idempotent: false,
    }
  }

  /// The same call with the typed error detail `error`, carried in ERROR's data section (§8.4).
  #[must_use]
  pub fn error(mut self, error: Type) -> Self {
    self.error = Some(error);
    self
  }

  /// The same call, idempotent: a client may retry it after a failure whose outcome is unknown (§12.2).
  #[must_use]
  pub const fn idempotent(mut self) -> Self {
    self.idempotent = true;
    self
  }

  /// The call's shape.
  #[must_use]
  pub const fn shape(&self) -> Shape {
    match self.streams {
      Streams::Unary { .. } => Shape::Unary,
      Streams::ServerStream { .. } => Shape::ServerStream,
      Streams::ClientStream { .. } => Shape::ClientStream,
      Streams::Bidi { .. } => Shape::Bidi,
    }
  }

  fn types(&self) -> impl Iterator<Item = (&'static str, &Type)> {
    let (accepts, yields, returns) = match &self.streams {
      Streams::Unary { returns } => (None, None, Some(returns)),
      Streams::ServerStream { yields } => (None, Some(yields), None),
      Streams::ClientStream { accepts, returns } => (Some(accepts), None, Some(returns)),
      Streams::Bidi { accepts, yields } => (Some(accepts), Some(yields), None),
    };
    [
      ("accepts", accepts),
      ("yields", yields),
      (
        "returns",
        returns.filter(|returns| **returns != Type::Primitive(Primitive::Unit)),
      ),
      ("error", self.error.as_ref()),
    ]
    .into_iter()
    .filter_map(|(key, ty)| ty.map(|ty| (key, ty)))
  }

  fn insert_into(&self, method: &mut Map<String, Value>) {
    method.insert("shape".into(), Value::from(self.shape().name()));
    for (key, ty) in self.types() {
      method.insert(key.into(), ty.to_json());
    }
    if self.idempotent {
      method.insert("idempotent".into(), Value::Bool(true));
    }
  }
}

/// A channel declaration (§10.1, §12.2): the types a namespace's channels carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
  namespace: String,
  docs: Option<String>,
  publication: Type,
  presence_info: Option<Type>,
  snapshot: Option<Type>,
  subscribe: Option<Type>,
  history: bool,
}

impl Channel {
  /// The namespace `namespace`, whose publications are of type `publication`.
  #[must_use]
  pub fn new(namespace: impl Into<String>, publication: Type) -> Self {
    Self {
      namespace: namespace.into(),
      docs: None,
      publication,
      presence_info: None,
      snapshot: None,
      subscribe: None,
      history: false,
    }
  }

  /// The same channel, whose members carry presence info of type `presence_info`.
  #[must_use]
  pub fn presence_info(mut self, presence_info: Type) -> Self {
    self.presence_info = Some(presence_info);
    self
  }

  /// The same channel, whose SUBSCRIBED carries a snapshot of type `snapshot` (§10.2).
  #[must_use]
  pub fn snapshot(mut self, snapshot: Type) -> Self {
    self.snapshot = Some(snapshot);
    self
  }

  /// The same channel, whose SUBSCRIBE carries subscription parameters of type `subscribe` (§10.2).
  #[must_use]
  pub fn subscribe(mut self, subscribe: Type) -> Self {
    self.subscribe = Some(subscribe);
    self
  }

  /// The same channel, with history.
  #[must_use]
  pub const fn history(mut self) -> Self {
    self.history = true;
    self
  }

  /// The same channel, documented.
  #[must_use]
  pub fn docs(mut self, docs: impl Into<String>) -> Self {
    self.docs = Some(docs.into());
    self
  }

  fn types(&self) -> impl Iterator<Item = (&'static str, &Type)> {
    [
      ("publication", Some(&self.publication)),
      ("presence_info", self.presence_info.as_ref()),
      ("snapshot", self.snapshot.as_ref()),
      ("subscribe", self.subscribe.as_ref()),
    ]
    .into_iter()
    .filter_map(|(key, ty)| ty.map(|ty| (key, ty)))
  }

  fn to_json(&self) -> Value {
    let mut channel = Map::new();
    channel.insert("namespace".into(), Value::from(self.namespace.as_str()));
    insert_docs(&mut channel, self.docs.as_deref());
    for (key, ty) in self.types() {
      channel.insert(key.into(), ty.to_json());
    }
    if self.history {
      channel.insert("history".into(), Value::Bool(true));
    }
    Value::Object(channel)
  }
}

/// Which way a datagram topic's datagrams travel (§11.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
  /// `to_server`.
  ToServer,
  /// `to_client`.
  ToClient,
  /// `both`.
  Both,
}

impl Direction {
  /// Every direction, in the order of §12.2.
  pub const ALL: [Self; 3] = [Self::ToServer, Self::ToClient, Self::Both];

  /// The name in the manifest.
  #[must_use]
  pub const fn name(self) -> &'static str {
    match self {
      Self::ToServer => "to_server",
      Self::ToClient => "to_client",
      Self::Both => "both",
    }
  }
}

/// Which of a datagram topic's datagrams a receiver keeps (§11.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
  /// `latest`: only the newest.
  Latest,
  /// `all`: every one that arrives.
  All,
}

impl Mode {
  /// Every mode.
  pub const ALL: [Self; 2] = [Self::Latest, Self::All];

  /// The name in the manifest.
  #[must_use]
  pub const fn name(self) -> &'static str {
    match self {
      Self::Latest => "latest",
      Self::All => "all",
    }
  }
}

/// A datagram topic (§11.1, §12.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datagram {
  topic: String,
  docs: Option<String>,
  direction: Direction,
  mode: Mode,
  payload: Type,
}

impl Datagram {
  /// The topic `topic`, whose datagrams travel `direction`, kept as `mode` says, and carry `payload`.
  #[must_use]
  pub fn new(topic: impl Into<String>, direction: Direction, mode: Mode, payload: Type) -> Self {
    Self {
      topic: topic.into(),
      docs: None,
      direction,
      mode,
      payload,
    }
  }

  /// The same topic, documented.
  #[must_use]
  pub fn docs(mut self, docs: impl Into<String>) -> Self {
    self.docs = Some(docs.into());
    self
  }

  fn to_json(&self) -> Value {
    let mut datagram = Map::new();
    datagram.insert("topic".into(), Value::from(self.topic.as_str()));
    insert_docs(&mut datagram, self.docs.as_deref());
    datagram.insert("direction".into(), Value::from(self.direction.name()));
    datagram.insert("mode".into(), Value::from(self.mode.name()));
    datagram.insert("payload".into(), self.payload.to_json());
    Value::Object(datagram)
  }
}

/// What keeps a manifest from being valid (§12.2).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ManifestError {
  /// A service, method, type, field, variant or datagram topic name outside `[A-Za-z_][A-Za-z0-9_]*`.
  #[error("{what} name `{name}` is not a contract name: names match [A-Za-z_][A-Za-z0-9_]* (spec 12.2)")]
  InvalidName {
    /// What the name names: `"service"`, `"field of type `Message`"`.
    what: String,
    /// The name.
    name: String,
  },
  /// A channel namespace outside the channel name characters of §10.1, or with a `:`.
  #[error("channel namespace `{namespace}` is not a namespace: 1 to 255 of [A-Za-z0-9_-./@], no `:` (spec 10.1, 12.2)")]
  InvalidNamespace {
    /// The namespace.
    namespace: String,
  },
  /// Two entries of one list with one name.
  #[error("more than one {what} is named `{name}`")]
  Duplicate {
    /// The list: `"service"`, `"server method of service `chat`"`.
    what: String,
    /// The name both entries have.
    name: String,
  },
  /// A `{"ref": name}` to a name the `types` table does not have.
  #[error("{at} refers to `{name}`, which is not in `types`")]
  UnknownType {
    /// The missing name.
    name: String,
    /// Where the reference is: `"param `room` of server method `chat/send`"`.
    at: String,
  },
  /// An `Option` of `unit`, directly or through aliases: `{"option": "unit"}`, which §12.1 does not allow.
  #[error("{at} is an `option` of `unit`, directly or through aliases, which is not allowed (spec 12.1)")]
  OptionOfUnit {
    /// Where the type is.
    at: String,
  },
  /// A field marked `optional` whose type is not an `option` (§12.1).
  #[error("{at} is `optional` but its type is not an `option` (spec 12.1)")]
  OptionalNotOption {
    /// Where the field is.
    at: String,
  },
  /// An open enum that declares a variant named `Unknown`, the name of its decode-only catch-all
  /// (§12.1).
  #[error("{at} is an open enum with a variant named `Unknown`, the name of its catch-all (spec 12.1)")]
  UnknownInOpenEnum {
    /// Where the enum is: "type `Status`".
    at: String,
  },
  /// A type that nests deeper than [`MAX_TYPE_DEPTH`], which [`Manifest::read`] would refuse.
  #[error("{at} nests types more than {limit} deep")]
  TooDeep {
    /// Where the type is.
    at: String,
    /// [`MAX_TYPE_DEPTH`].
    limit: usize,
  },
  /// Text longer than [`MAX_MANIFEST_BYTES`], which [`Manifest::read`] would refuse; only
  /// [`Manifest::render`] reports it.
  #[error("the manifest is {size} bytes, more than the {limit} a manifest may have")]
  TooLarge {
    /// The length of the text, in bytes.
    size: usize,
    /// [`MAX_MANIFEST_BYTES`].
    limit: usize,
  },
  /// A named type that leads back to itself through aliases only, so it never reaches a type.
  #[error("type `{name}` leads back to itself through aliases only, so it never reaches a type")]
  AliasCycle {
    /// The name of an alias on the cycle, or leading to it.
    name: String,
  },
}

struct Check<'a> {
  types: &'a Types,
  chains: Chains<'a>,
  errors: Vec<ManifestError>,
}

impl Check<'_> {
  fn name(&mut self, what: &str, name: &str) {
    if !name::is_valid(name) {
      self.errors.push(ManifestError::InvalidName {
        what: what.to_owned(),
        name: name.to_owned(),
      });
    }
  }

  fn unique<'n>(&mut self, what: &str, names: impl Iterator<Item = &'n str>) {
    let mut seen = BTreeSet::new();
    let mut reported = BTreeSet::new();
    for name in names {
      if !seen.insert(name) && reported.insert(name) {
        self.errors.push(ManifestError::Duplicate {
          what: what.to_owned(),
          name: name.to_owned(),
        });
      }
    }
  }

  fn method(&mut self, service: &str, side: &str, method: &Method) {
    self.name(&format!("{side} method of service `{service}`"), &method.name);
    let at = format!("{side} method `{service}/{}`", method.name);
    self.unique(
      &format!("param of {at}"),
      method.params.iter().map(|param| param.name.as_str()),
    );
    for param in &method.params {
      self.name(&format!("param of {at}"), &param.name);
      self.ty(&param.ty, &format!("param `{}` of {at}", param.name), 1);
    }
    if let Some(call) = &method.call {
      for (key, ty) in call.types() {
        self.ty(ty, &format!("`{key}` of {at}"), 1);
      }
    }
  }

  fn channel(&mut self, channel: &Channel) {
    if !name::is_valid_namespace(&channel.namespace) {
      self.errors.push(ManifestError::InvalidNamespace {
        namespace: channel.namespace.clone(),
      });
    }
    for (key, ty) in channel.types() {
      self.ty(ty, &format!("`{key}` of channel namespace `{}`", channel.namespace), 1);
    }
  }

  fn definition(&mut self, name: &str, definition: &Definition) {
    let at = format!("type `{name}`");
    match definition {
      Definition::Struct(fields) => self.fields(fields, &at, 1),
      Definition::Enum(variants) => {
        if variants.is_open() && variants.variants().iter().any(|variant| variant.name() == "Unknown") {
          self.errors.push(ManifestError::UnknownInOpenEnum { at: at.clone() });
        }
        self.unique(
          &format!("variant of {at}"),
          variants.variants().iter().map(|variant| variant.name()),
        );
        for variant in variants.variants() {
          self.name(&format!("variant of {at}"), variant.name());
          if let Some(ty) = variant.ty() {
            self.ty(ty, &format!("variant `{}` of {at}", variant.name()), 1);
          }
        }
      }
      Definition::Alias(target) => self.ty(target, &at, 1),
    }
  }

  /// Checks the fields of a struct, whose types sit at nesting `depth` ([`MAX_TYPE_DEPTH`]).
  fn fields(&mut self, fields: &Struct, at: &str, depth: usize) {
    self.unique(&format!("field of {at}"), fields.fields().iter().map(|field| field.name()));
    for field in fields.fields() {
      let place = format!("field `{}` of {at}", field.name());
      self.name(&format!("field of {at}"), field.name());
      if field.is_optional() && !matches!(field.ty(), Type::Option(_)) {
        self.errors.push(ManifestError::OptionalNotOption { at: place.clone() });
      }
      self.ty(field.ty(), &place, depth);
    }
  }

  /// Checks `ty`, which sits at nesting `depth` ([`MAX_TYPE_DEPTH`]); a type too deep is reported once
  /// and not entered.
  fn ty(&mut self, ty: &Type, at: &str, depth: usize) {
    if depth > MAX_TYPE_DEPTH {
      let error = ManifestError::TooDeep {
        at: at.to_owned(),
        limit: MAX_TYPE_DEPTH,
      };
      if !self.errors.contains(&error) {
        self.errors.push(error);
      }
      return;
    }
    match ty {
      Type::Primitive(_) => {}
      Type::Option(inner) => {
        if self.is_unit(inner) {
          self.errors.push(ManifestError::OptionOfUnit { at: at.to_owned() });
        }
        self.ty(inner, at, depth + INNER_STEP);
      }
      Type::List(inner) | Type::Map(inner) => self.ty(inner, at, depth + INNER_STEP),
      Type::Tuple(items) => {
        for item in items {
          self.ty(item, at, depth + ITEM_STEP);
        }
      }
      Type::Struct(fields) => self.fields(fields, at, depth + FIELD_STEP),
      Type::Ref(name) => {
        if !self.types.contains_key(name) {
          self.errors.push(ManifestError::UnknownType {
            name: name.clone(),
            at: at.to_owned(),
          });
        }
      }
    }
  }

  /// Whether `ty` is `unit`, directly or through aliases.
  fn is_unit(&self, ty: &Type) -> bool {
    match ty {
      Type::Primitive(Primitive::Unit) => true,
      Type::Ref(name) => self.chains.from(name).ends_in_unit(),
      _ => false,
    }
  }
}

fn sorted<T>(items: &[T], name: impl Fn(&T) -> &String) -> impl Iterator<Item = &T> {
  let mut items: Vec<&T> = items.iter().collect();
  items.sort_by(|left, right| name(left).cmp(name(right)));
  items.into_iter()
}

fn insert_docs(object: &mut Map<String, Value>, docs: Option<&str>) {
  if let Some(docs) = docs {
    object.insert("docs".into(), Value::from(docs));
  }
}

#[cfg(test)]
mod tests;
