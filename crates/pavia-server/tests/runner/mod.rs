//! The server script runner (§17.2, `vectors/README.md`): plays the client's side of a script
//! against a [`pavia_server::websocket::Endpoint`] and reports the first step that fails.
//!
//! [`Mode::InMemory`] connects over `tokio::io::duplex` on a paused clock, so `wait` and every
//! timeout advance time instead of sleeping. [`Mode::Socket`] connects over TCP on loopback and
//! sleeps. Both play the same steps and must reach the same verdict.

pub mod pattern;
pub mod wire;

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use hyper::header::{self, HeaderValue};
use pavia_server::websocket::{Endpoint, SUBPROTOCOL, Transport};
use pavia_vectors::diag::{self, Diag};
use serde_json::Value as Json;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{Instant, timeout_at};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{WebSocketStream, client_async};

use pattern::Captures;
use wire::Frame;

const PING: u8 = 0x04;
const PONG: u8 = 0x05;
const EXPECT_WITHIN: Duration = Duration::from_secs(5);
const CLOSE_CODES: [&str; 14] = [
  "NORMAL",
  "PROTOCOL_ERROR",
  "UNSUPPORTED_VERSION",
  "UNAUTHENTICATED",
  "FORBIDDEN",
  "IDLE_TIMEOUT",
  "SLOW_CONSUMER",
  "GOING_AWAY",
  "INTERNAL",
  "FRAME_TOO_LARGE",
  "LIMIT_EXCEEDED",
  "RESUME_FAILED",
  "HANDSHAKE_TIMEOUT",
  "CONTRACT_MISMATCH",
];

/// How the runner reaches the endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
  InMemory,
  Socket,
}

/// The result of a script that did not fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
  Passed,
  Skipped(String),
}

/// The first step that failed, counted from 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
  pub script: String,
  pub step: usize,
  pub verb: &'static str,
  pub message: String,
}

impl fmt::Display for Failure {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}: step {} ({}): {}", self.script, self.step, self.verb, self.message)
  }
}

/// One parsed script case.
#[derive(Debug, Clone)]
pub struct Script {
  pub id: String,
  pub binding: String,
  pub requires: Vec<String>,
  steps: Vec<Step>,
}

#[derive(Debug, Clone)]
enum Step {
  Send {
    frame: FrameSpec,
    repeat: u64,
  },
  Expect {
    frame: FrameSpec,
    capture: Vec<(String, String)>,
  },
  ExpectNothingWithin(Duration),
  Wait(Duration),
  CloseTransport,
  ExpectClose {
    code: u16,
    within: Duration,
  },
}

impl Step {
  const fn verb(&self) -> &'static str {
    match self {
      Self::Send { .. } => "send",
      Self::Expect { .. } => "expect",
      Self::ExpectNothingWithin(_) => "expect_nothing_within",
      Self::Wait(_) => "wait",
      Self::CloseTransport => "close_transport",
      Self::ExpectClose { .. } => "expect_close",
    }
  }
}

#[derive(Debug, Clone)]
struct FrameSpec {
  stream: u64,
  frame_type: u8,
  flags: u8,
  header: Option<Diag>,
  data: Data,
}

#[derive(Debug, Clone)]
enum Data {
  Empty,
  Cbor(Diag),
  Exact(Vec<u8>),
}

impl Script {
  /// Parses one case of a `scripts/` file. Every `$name` must be captured by an earlier step,
  /// and a `send` may not hold `*` or `...`; both are reported here, before anything runs.
  pub fn parse(case: &Json) -> Result<Self, String> {
    let id = text(case, "id")?.to_owned();
    let codec = text(case, "codec")?;
    let binding = text(case, "binding")?.to_owned();
    let requires = case["requires"]
      .as_array()
      .map(|names| names.iter().filter_map(Json::as_str).map(str::to_owned).collect())
      .unwrap_or_default();
    let mut captured = HashSet::new();
    let mut steps = Vec::new();
    for (index, step) in case["steps"].as_array().ok_or("steps is not a list")?.iter().enumerate() {
      let step = parse_step(step, codec, &mut captured).map_err(|message| format!("{id}: step {}: {message}", index + 1))?;
      steps.push(step);
    }
    Ok(Self {
      id,
      binding,
      requires,
      steps,
    })
  }
}

fn parse_step(step: &Json, codec: &str, captured: &mut HashSet<String>) -> Result<Step, String> {
  let Some((verb, body)) = step
    .as_object()
    .filter(|step| step.len() == 1)
    .and_then(|step| step.iter().next())
  else {
    return Err("a step has exactly one verb".into());
  };
  Ok(match verb.as_str() {
    "send" => {
      let frame = parse_frame(body, codec, captured)?;
      if frame.header.as_ref().is_some_and(has_wildcard) || matches!(&frame.data, Data::Cbor(data) if has_wildcard(data)) {
        return Err("a send may use $name but not * or ...".into());
      }
      let repeat = body
        .get("repeat")
        .map_or(Some(1), Json::as_u64)
        .ok_or("repeat is not a count")?;
      Step::Send { frame, repeat }
    }
    "expect" => {
      let frame = parse_frame(body, codec, captured)?;
      let mut capture = Vec::new();
      for (name, path) in body.get("capture").and_then(Json::as_object).into_iter().flatten() {
        let path = path.as_str().ok_or("a capture path is a string")?;
        if !matches!(path.split('.').next(), Some("header" | "data")) {
          return Err(format!("capture path {path} starts with neither header nor data"));
        }
        captured.insert(name.clone());
        capture.push((name.clone(), path.to_owned()));
      }
      Step::Expect { frame, capture }
    }
    "expect_nothing_within" => Step::ExpectNothingWithin(milliseconds(body, "ms")?),
    "wait" => Step::Wait(milliseconds(body, "ms")?),
    "close_transport" => Step::CloseTransport,
    "expect_close" => {
      let name = text(body, "code")?;
      let code = CLOSE_CODES
        .iter()
        .position(|candidate| *candidate == name)
        .ok_or_else(|| format!("unknown close code {name}"))?;
      Step::ExpectClose {
        code: u16::try_from(code).expect("fourteen codes"),
        within: milliseconds(body, "within_ms")?,
      }
    }
    other => return Err(format!("unknown verb {other}")),
  })
}

fn parse_frame(body: &Json, codec: &str, captured: &HashSet<String>) -> Result<FrameSpec, String> {
  let number = |key: &str| body[key].as_u64().ok_or_else(|| format!("{key} is not a number"));
  let small = |key: &str, value: u64| u8::try_from(value).map_err(|_| format!("{key} {value} does not fit a byte"));
  let header = match &body["header"] {
    Json::Null => None,
    Json::String(header) => Some(pattern(header, captured)?),
    _ => return Err("header is neither null nor a string".into()),
  };
  let data = match (&body["data"], body.get("data_hex")) {
    (Json::Null, None) => Data::Empty,
    (Json::Null, Some(raw)) => Data::Exact(pavia_vectors::hex(raw.as_str().ok_or("data_hex is not a string")?)),
    (Json::String(data), _) => match body.get("codec").and_then(Json::as_str).unwrap_or(codec) {
      "cbor" => Data::Cbor(pattern(data, captured)?),
      "json" => Data::Exact(data.as_bytes().to_vec()),
      other => return Err(format!("unknown codec {other}")),
    },
    _ => return Err("data is neither null nor a string".into()),
  };
  Ok(FrameSpec {
    stream: number("stream")?,
    frame_type: small("type", number("type")?)?,
    flags: small("flags", body.get("flags").map_or(Ok(0), |_| number("flags"))?)?,
    header,
    data,
  })
}

fn pattern(text: &str, captured: &HashSet<String>) -> Result<Diag, String> {
  let parsed = diag::parse(text).map_err(|error| format!("{text}: {error}"))?;
  if let Some(name) = parsed.refs().into_iter().find(|name| !captured.contains(*name)) {
    return Err(format!("${name} is used before any step captures it"));
  }
  Ok(parsed)
}

fn has_wildcard(pattern: &Diag) -> bool {
  match pattern {
    Diag::Any | Diag::Map { open: true, .. } => true,
    Diag::Map { entries, .. } => entries.iter().any(|(key, value)| has_wildcard(key) || has_wildcard(value)),
    Diag::Array(items) => items.iter().any(has_wildcard),
    Diag::Tag(_, inner) => has_wildcard(inner),
    _ => false,
  }
}

fn text<'j>(body: &'j Json, key: &str) -> Result<&'j str, String> {
  body[key].as_str().ok_or_else(|| format!("{key} is not a string"))
}

fn milliseconds(body: &Json, key: &str) -> Result<Duration, String> {
  body[key]
    .as_u64()
    .map(Duration::from_millis)
    .ok_or_else(|| format!("{key} is not a number of milliseconds"))
}

/// Plays `script` against `endpoint` on a runtime of its own. `granted` lists the capabilities
/// the endpoint is configured to grant; a script that requires others is skipped, as is one for
/// the WebTransport binding.
pub fn run<H, F>(script: &Script, mode: Mode, endpoint: Endpoint<H>, granted: &[&str]) -> Result<Verdict, Failure>
where
  H: Fn(Transport) -> F + Clone + Send + Sync + 'static,
  F: Future<Output = ()> + Send + 'static,
{
  if script.binding == "wt" {
    return Ok(Verdict::Skipped("WebTransport script".into()));
  }
  if let Some(missing) = script.requires.iter().find(|name| !granted.contains(&name.as_str())) {
    return Ok(Verdict::Skipped(format!("requires {missing}")));
  }
  let runtime = tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .start_paused(mode == Mode::InMemory)
    .build()
    .expect("a current-thread runtime");
  runtime.block_on(async {
    let connector = match mode {
      Mode::InMemory => Connector::InMemory(endpoint),
      Mode::Socket => {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a loopback port");
        let address = listener.local_addr().expect("a bound address");
        let path = endpoint.config().path.clone();
        tokio::spawn(endpoint.serve(listener));
        Connector::Socket { address, path }
      }
    };
    let mut player = Player {
      connector,
      socket: None,
      inbox: VecDeque::new(),
      captures: Captures::new(),
    };
    for (index, step) in script.steps.iter().enumerate() {
      player.play(step).await.map_err(|message| Failure {
        script: script.id.clone(),
        step: index + 1,
        verb: step.verb(),
        message,
      })?;
    }
    Ok(Verdict::Passed)
  })
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

type Socket = WebSocketStream<Box<dyn Io>>;

enum Connector<H> {
  InMemory(Endpoint<H>),
  Socket { address: SocketAddr, path: String },
}

impl<H, F> Connector<H>
where
  H: Fn(Transport) -> F + Clone + Send + Sync + 'static,
  F: Future<Output = ()> + Send + 'static,
{
  async fn connect(&self) -> Result<Socket, String> {
    let (io, path): (Box<dyn Io>, &str) = match self {
      Self::InMemory(endpoint) => {
        let (client, server) = tokio::io::duplex(1 << 20);
        let serving = endpoint.clone();
        tokio::spawn(async move { serving.serve_connection(server).await });
        (Box::new(client), &endpoint.config().path)
      }
      Self::Socket { address, path } => {
        let stream = TcpStream::connect(address)
          .await
          .map_err(|error| format!("connecting: {error}"))?;
        (Box::new(stream), path)
      }
    };
    let mut request = format!("ws://localhost{path}")
      .into_client_request()
      .map_err(|error| error.to_string())?;
    request
      .headers_mut()
      .insert(header::SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static(SUBPROTOCOL));
    let (socket, _) = client_async(request, io)
      .await
      .map_err(|error| format!("upgrade refused: {error}"))?;
    Ok(socket)
  }
}

enum Event {
  Frames,
  Closed(Option<u16>),
  Deadline,
}

struct Player<H> {
  connector: Connector<H>,
  socket: Option<Socket>,
  inbox: VecDeque<Frame>,
  captures: Captures,
}

impl<H, F> Player<H>
where
  H: Fn(Transport) -> F + Clone + Send + Sync + 'static,
  F: Future<Output = ()> + Send + 'static,
{
  async fn play(&mut self, step: &Step) -> Result<(), String> {
    match step {
      Step::Send { frame, repeat } => self.send(frame, *repeat).await,
      Step::Expect { frame, capture } => self.expect(frame, capture).await,
      Step::ExpectNothingWithin(within) => self.expect_nothing(*within).await,
      Step::Wait(duration) => self.wait(*duration).await,
      Step::CloseTransport => {
        self.socket = None;
        self.inbox.clear();
        Ok(())
      }
      Step::ExpectClose { code, within } => self.expect_close(*code, *within).await,
    }
  }

  async fn send(&mut self, spec: &FrameSpec, repeat: u64) -> Result<(), String> {
    let frame = Frame {
      stream: spec.stream,
      frame_type: spec.frame_type,
      flags: spec.flags,
      header: spec
        .header
        .as_ref()
        .map(|header| pattern::resolve(header, &self.captures))
        .transpose()?,
      data: match &spec.data {
        Data::Empty => Vec::new(),
        Data::Exact(bytes) => bytes.clone(),
        Data::Cbor(data) => {
          let mut bytes = Vec::new();
          ciborium::into_writer(&pattern::resolve(data, &self.captures)?, &mut bytes).map_err(|error| error.to_string())?;
          bytes
        }
      },
    };
    let message = wire::encode(&frame);
    if self.socket.is_none() {
      self.socket = Some(self.connector.connect().await?);
      self.inbox.clear();
    }
    for _ in 0..repeat {
      self.write(message.clone()).await?;
    }
    Ok(())
  }

  async fn expect(&mut self, spec: &FrameSpec, capture: &[(String, String)]) -> Result<(), String> {
    let deadline = Instant::now() + EXPECT_WITHIN;
    let frame = loop {
      if let Some(index) = self.inbox.iter().position(|frame| frame.stream == spec.stream) {
        break self.inbox.remove(index).expect("an index from position");
      }
      match self.receive(deadline, Some(spec.frame_type)).await? {
        Event::Frames => {}
        Event::Closed(code) => return Err(format!("transport closed ({}) before a frame arrived", describe(code))),
        Event::Deadline => {
          return Err(format!(
            "nothing arrived on stream {} within {} ms",
            spec.stream,
            EXPECT_WITHIN.as_millis()
          ));
        }
      }
    };
    self.check(spec, &frame)?;
    for (name, path) in capture {
      let mut segments = path.split('.');
      let root = match segments.next() {
        Some("header") => frame.header.clone(),
        _ => decode_data(&frame.data).ok(),
      };
      let segments: Vec<&str> = segments.collect();
      let value = root
        .as_ref()
        .and_then(|root| pattern::at_path(root, &segments))
        .ok_or_else(|| format!("capture {name}: nothing at {path} in {frame}"))?;
      self.captures.insert(name.clone(), value.clone());
    }
    Ok(())
  }

  fn check(&self, spec: &FrameSpec, frame: &Frame) -> Result<(), String> {
    let mismatch = |what: &str| Err(format!("{what} does not match: got {frame}"));
    if frame.frame_type != spec.frame_type {
      return mismatch(&format!("type {}", spec.frame_type));
    }
    if frame.flags != spec.flags {
      return mismatch(&format!("flags {}", spec.flags));
    }
    let header = match (&spec.header, &frame.header) {
      (None, None) => true,
      (Some(pattern), Some(header)) => pattern::matches(pattern, header, &self.captures),
      _ => false,
    };
    if !header {
      return mismatch("header");
    }
    let data = match &spec.data {
      Data::Empty => frame.data.is_empty(),
      Data::Exact(bytes) => frame.data == *bytes,
      Data::Cbor(pattern) => decode_data(&frame.data).is_ok_and(|data| pattern::matches(pattern, &data, &self.captures)),
    };
    if !data {
      return mismatch("data");
    }
    Ok(())
  }

  async fn expect_nothing(&mut self, within: Duration) -> Result<(), String> {
    let deadline = Instant::now() + within;
    loop {
      if let Some(frame) = self.inbox.front() {
        return Err(format!("received {frame}"));
      }
      match self.receive(deadline, None).await? {
        Event::Frames => {}
        Event::Closed(code) => return Err(format!("transport closed ({})", describe(code))),
        Event::Deadline => return Ok(()),
      }
    }
  }

  async fn wait(&mut self, duration: Duration) -> Result<(), String> {
    let deadline = Instant::now() + duration;
    if self.socket.is_none() {
      tokio::time::sleep_until(deadline).await;
      return Ok(());
    }
    loop {
      if let Event::Deadline | Event::Closed(_) = self.receive(deadline, None).await? {
        tokio::time::sleep_until(deadline).await;
        return Ok(());
      }
    }
  }

  async fn expect_close(&mut self, code: u16, within: Duration) -> Result<(), String> {
    let deadline = Instant::now() + within;
    loop {
      match self.receive(deadline, None).await? {
        Event::Frames => self.inbox.clear(),
        Event::Closed(Some(got)) if got == 4000 + code => return Ok(()),
        Event::Closed(got) => return Err(format!("closed with {}, expected {}", describe(got), 4000 + code)),
        Event::Deadline => return Err(format!("still open after {} ms", within.as_millis())),
      }
    }
  }

  /// Reads one message, or stops at `deadline`. PING is answered with PONG and dropped, and PONG
  /// is dropped, unless `keep` names that type. A close ends the transport.
  async fn receive(&mut self, deadline: Instant, keep: Option<u8>) -> Result<Event, String> {
    let Some(socket) = self.socket.as_mut() else {
      return Ok(Event::Closed(None));
    };
    let message = match timeout_at(deadline, socket.next()).await {
      Err(_) => return Ok(Event::Deadline),
      Ok(None | Some(Err(_))) => {
        self.socket = None;
        return Ok(Event::Closed(None));
      }
      Ok(Some(Ok(message))) => message,
    };
    match message {
      Message::Binary(message) => {
        for frame in wire::decode(&message)? {
          match frame.frame_type {
            PING if keep != Some(PING) => {
              let pong = Frame {
                frame_type: PONG,
                ..frame
              };
              self.write(wire::encode(&pong)).await?;
            }
            PONG if keep != Some(PONG) => {}
            _ => self.inbox.push_back(frame),
          }
        }
        Ok(Event::Frames)
      }
      Message::Close(frame) => {
        self.socket = None;
        Ok(Event::Closed(frame.map(|frame| u16::from(frame.code))))
      }
      Message::Text(_) => Err("the server sent a text message".into()),
      Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => Ok(Event::Frames),
    }
  }

  async fn write(&mut self, message: Vec<u8>) -> Result<(), String> {
    let socket = self.socket.as_mut().ok_or("no transport is open")?;
    socket
      .send(Message::binary(message))
      .await
      .map_err(|error| format!("sending: {error}"))
  }
}

fn decode_data(data: &[u8]) -> Result<ciborium::Value, String> {
  ciborium::from_reader(data).map_err(|error| error.to_string())
}

fn describe(code: Option<u16>) -> String {
  code.map_or_else(|| "no close code".to_owned(), |code| format!("code {code}"))
}
