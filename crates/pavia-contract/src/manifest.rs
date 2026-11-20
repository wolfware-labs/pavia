//! The contract manifest (§12.2): the services, channels, datagram topics and named types of an endpoint,
//! as the JSON document client generators read.
//!
//! A [`Manifest`] is built from its parts, then [`Manifest::render`] checks it and writes its JSON. The
//! model leaves out what §12.2 forbids instead of checking for it: a notification has no shape or result
//! types, an absent optional field has no `null` to write, and a field equal to its default (`"returns":
//! "unit"`, `"idempotent": false`, `"history": false`, an empty `params`) is never written. What the
//! model can hold but §12.2 forbids, [`Manifest::validate`] reports: a name outside the grammar, a `ref`
//! to no named type, an `Option` of `unit`, and two entries with one name.
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

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::name;
use crate::schema::{Definition, Primitive, Struct, Type};

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
  types: BTreeMap<String, Definition>,
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
      types: BTreeMap::new(),
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
  pub fn types(mut self, types: BTreeMap<String, Definition>) -> Self {
    self.types = types;
    self
  }

  /// Checks what the model can hold but §12.2 forbids.
  ///
  /// # Errors
  ///
  /// Every [`ManifestError`] found, in the order of the output.
  pub fn validate(&self) -> Result<(), Vec<ManifestError>> {
    let mut check = Check {
      types: &self.types,
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
      check.ty(&datagram.payload, &format!("datagram topic `{}`", datagram.topic));
    }
    for (name, definition) in &self.types {
      check.name("type", name);
      check.definition(name, definition);
    }
    if check.errors.is_empty() { Ok(()) } else { Err(check.errors) }
  }

  /// The manifest as JSON text: pretty-printed with two spaces, object keys sorted, named entries sorted
  /// by name, and a final newline. The same manifest always gives the same bytes.
  ///
  /// # Errors
  ///
  /// The errors of [`Manifest::validate`]; nothing is written.
  pub fn render(&self) -> Result<String, Vec<ManifestError>> {
    self.validate()?;
    let mut text = serde_json::to_string_pretty(&self.to_json()).unwrap_or_default();
    text.push('\n');
    Ok(text)
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
    if !self.channels.is_empty() {
      manifest.insert(
        "channels".into(),
        sorted(&self.channels, |channel| &channel.namespace)
          .map(Channel::to_json)
          .collect(),
      );
    }
    if !self.datagrams.is_empty() {
      manifest.insert(
        "datagrams".into(),
        sorted(&self.datagrams, |datagram| &datagram.topic)
          .map(Datagram::to_json)
          .collect(),
      );
    }
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
    let mut datagram = json!({
      "topic": self.topic,
      "direction": self.direction.name(),
      "mode": self.mode.name(),
      "payload": self.payload.to_json(),
    });
    if let Value::Object(datagram) = &mut datagram {
      insert_docs(datagram, self.docs.as_deref());
    }
    datagram
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
  #[error("two of the {what} entries are named `{name}`")]
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
  /// An `Option` of `unit`, `{"option": "unit"}`, which §12.1 does not allow.
  #[error("{at} is `{{\"option\": \"unit\"}}`, which is not allowed (spec 12.1)")]
  OptionOfUnit {
    /// Where the type is.
    at: String,
  },
}

struct Check<'a> {
  types: &'a BTreeMap<String, Definition>,
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
    for param in &method.params {
      self.ty(&param.ty, &format!("param `{}` of {at}", param.name));
    }
    if let Some(call) = &method.call {
      for (key, ty) in call.types() {
        self.ty(ty, &format!("`{key}` of {at}"));
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
      self.ty(ty, &format!("`{key}` of channel namespace `{}`", channel.namespace));
    }
  }

  fn definition(&mut self, name: &str, definition: &Definition) {
    let at = format!("type `{name}`");
    match definition {
      Definition::Struct(fields) => self.fields(fields, &at),
      Definition::Enum(variants) => {
        for variant in variants.variants() {
          self.name(&format!("variant of {at}"), variant.name());
          if let Some(ty) = variant.ty() {
            self.ty(ty, &format!("variant `{}` of {at}", variant.name()));
          }
        }
      }
      Definition::Alias(target) => self.ty(target, &at),
    }
  }

  fn fields(&mut self, fields: &Struct, at: &str) {
    for field in fields.fields() {
      self.name(&format!("field of {at}"), field.name());
      self.ty(field.ty(), &format!("field `{}` of {at}", field.name()));
    }
  }

  fn ty(&mut self, ty: &Type, at: &str) {
    match ty {
      Type::Primitive(_) => {}
      Type::Option(inner) => {
        if **inner == Type::Primitive(Primitive::Unit) {
          self.errors.push(ManifestError::OptionOfUnit { at: at.to_owned() });
        }
        self.ty(inner, at);
      }
      Type::List(inner) | Type::Map(inner) => self.ty(inner, at),
      Type::Tuple(items) => {
        for item in items {
          self.ty(item, at);
        }
      }
      Type::Struct(fields) => self.fields(fields, at),
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
