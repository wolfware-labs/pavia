use std::collections::{BTreeMap, HashMap};
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use ciborium::Value;
use futures_util::{SinkExt, StreamExt};
use hyper::header::{self, HeaderValue};
use pavia_proto::data::{Codec, Value as Arg};
use pavia_proto::error::ErrorStatus;
use pavia_proto::frame::{Auth, Call, Meta, MetaName, MetaValue};
use pavia_proto::registry::{CloseCode, StatusCode};
use pavia_proto::session::{Command, Config as SessionConfig, Ending, Input};
use pavia_proto::session_id::SessionId;
use pavia_proto::stream::{Shape, Side};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::{Barrier, Semaphore, mpsc, oneshot};
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::protocol::frame::Frame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode as WsCloseCode;
use tokio_tungstenite::tungstenite::protocol::frame::coding::{Data, OpCode};
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};
use tokio_tungstenite::{WebSocketStream, client_async};
use tracing::Dispatch;
use tracing::dispatcher::DefaultGuard;
use tracing::subscriber::NoSubscriber;

use super::{Calls, Disclosure, Failed, Refusal, RunningCall, authenticated, run, validate_call};
use crate::auth::{AllowAll, AuthFailure, Authenticator, Authorizer, Permissive, Principal, RequestContext};
use crate::methods::{Handler, HandlerError, MethodEntry, MethodName, MethodRegistry, StatusError};
use crate::sessions::Registry;
use crate::test_support::{EXPECT_WITHIN, floor_frame, has_ws_close, serve_on_loopback};
use crate::websocket::{Config, Endpoint, SUBPROTOCOL, Transport};

type Socket = WebSocketStream<TcpStream>;

const CODES: [CloseCode; 14] = [
  CloseCode::Normal,
  CloseCode::ProtocolError,
  CloseCode::UnsupportedVersion,
  CloseCode::Unauthenticated,
  CloseCode::Forbidden,
  CloseCode::IdleTimeout,
  CloseCode::SlowConsumer,
  CloseCode::GoingAway,
  CloseCode::Internal,
  CloseCode::FrameTooLarge,
  CloseCode::LimitExceeded,
  CloseCode::ResumeFailed,
  CloseCode::HandshakeTimeout,
  CloseCode::ContractMismatch,
];

async fn listen(config: SessionConfig) -> (SocketAddr, oneshot::Receiver<Ending>) {
  listen_with(Registry::new(), config, Arc::new(Permissive)).await
}

async fn listen_with<A: Authenticator + 'static>(
  registry: Registry,
  config: SessionConfig,
  authenticator: Arc<A>,
) -> (SocketAddr, oneshot::Receiver<Ending>) {
  listen_serving(
    registry,
    Arc::new(MethodRegistry::new()),
    config,
    authenticator,
    Arc::new(AllowAll),
  )
  .await
}

async fn listen_serving<A: Authenticator + 'static, Z: Authorizer + 'static>(
  registry: Registry,
  methods: Arc<MethodRegistry<Handler>>,
  config: SessionConfig,
  authenticator: Arc<A>,
  authorizer: Arc<Z>,
) -> (SocketAddr, oneshot::Receiver<Ending>) {
  let config = config.validate().unwrap();
  let (ended, ending) = oneshot::channel();
  let ended = Arc::new(Mutex::new(Some(ended)));
  let endpoint = Endpoint::new(Config::new("/rt", &config), move |transport: Transport| {
    let ended = ended.lock().unwrap().take();
    let registry = registry.clone();
    let methods = Arc::clone(&methods);
    let authenticator = Arc::clone(&authenticator);
    let authorizer = Arc::clone(&authorizer);
    async move {
      let ending = run(transport, &registry, &methods, &*authenticator, &authorizer).await;
      if let Some(ended) = ended {
        let _ = ended.send(ending);
      }
    }
  });
  (serve_on_loopback(endpoint).await, ending)
}

async fn connect_with(config: SessionConfig) -> (Socket, oneshot::Receiver<Ending>) {
  connect_on(Registry::new(), config).await
}

async fn connect_on(registry: Registry, config: SessionConfig) -> (Socket, oneshot::Receiver<Ending>) {
  let (address, ending) = listen_with(registry, config, Arc::new(Permissive)).await;
  (open(TcpStream::connect(address).await.unwrap()).await, ending)
}

fn request() -> Request {
  let mut request = "ws://localhost/rt".into_client_request().unwrap();
  request
    .headers_mut()
    .insert(header::SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static(SUBPROTOCOL));
  request
}

async fn open<S: AsyncRead + AsyncWrite + Unpin>(io: S) -> WebSocketStream<S> {
  client_async(request(), io).await.unwrap().0
}

async fn eventually(condition: impl Fn() -> bool, what: &str) {
  tokio::time::timeout(EXPECT_WITHIN, async {
    while !condition() {
      tokio::time::sleep(Duration::from_millis(5)).await;
    }
  })
  .await
  .expect(what);
}

async fn connect() -> (Socket, oneshot::Receiver<Ending>) {
  connect_with(SessionConfig::default()).await
}

fn hello_frame() -> Vec<u8> {
  hello_frame_with(b"cbor")
}

fn hello_frame_with(codec: &[u8; 4]) -> Vec<u8> {
  let mut header = vec![0xa2, 0x0e, 0x81, 0x01, 0x10, 0x81, 0x64];
  header.extend(codec);
  raw_frame(0x01, &header)
}

fn close_frame(code: u8) -> Vec<u8> {
  let mut header = vec![0xa1, 0x06];
  if code >= 24 {
    header.push(0x18);
  }
  header.push(code);
  raw_frame(0x07, &header)
}

async fn closed(socket: &mut Socket) -> (Vec<Vec<u8>>, WsCloseCode, Utf8Bytes) {
  let mut binary = Vec::new();
  loop {
    match socket.next().await.unwrap().unwrap() {
      Message::Binary(message) => binary.push(message.to_vec()),
      Message::Close(Some(frame)) => return (binary, frame.code, frame.reason),
      other => panic!("unexpected {other:?}"),
    }
  }
}

async fn mirrored_client_close(socket: &mut Socket, ending: oneshot::Receiver<Ending>) -> Vec<Vec<u8>> {
  let (binary, ws_code, _) = closed(socket).await;
  assert_eq!(ws_code, WsCloseCode::from(4000));
  assert_eq!(
    ending.await.unwrap(),
    Ending::Closed {
      code: CloseCode::Normal,
      msg: None,
      by: Side::Client
    }
  );
  binary
}

async fn closed_by_server(socket: &mut Socket, ending: oneshot::Receiver<Ending>, code: CloseCode) {
  let (binary, ws_code, _) = closed(socket).await;
  assert_eq!(binary, [close_frame(u8::try_from(u16::from(code)).unwrap())]);
  assert_eq!(ws_code, WsCloseCode::from(4000 + u16::from(code)));
  let end = tokio::time::timeout(Duration::from_millis(500), socket.next()).await;
  assert!(matches!(end, Ok(None)), "{end:?}");
  assert!(matches!(
    ending.await.unwrap(),
    Ending::Closed { code: ended, by: Side::Server, .. } if ended == code
  ));
}

async fn established<S: AsyncRead + AsyncWrite + Unpin>(socket: &mut WebSocketStream<S>) -> SessionId {
  socket.send(Message::binary(hello_frame())).await.unwrap();
  let welcome = next_welcome(socket).await;
  assert_eq!(welcome.len(), 7);
  let Value::Text(id) = &welcome[&19] else {
    panic!("WELCOME without a session ID");
  };
  SessionId::parse(id).unwrap()
}

async fn welcomed_in(registry: &Registry, config: SessionConfig) -> (Socket, oneshot::Receiver<Ending>, SessionId) {
  let (mut socket, ending) = connect_on(registry.clone(), config).await;
  let id = established(&mut socket).await;
  (socket, ending, id)
}

async fn commanded(command: Command) -> (Socket, oneshot::Receiver<Ending>) {
  let registry = Registry::new();
  let (socket, ending, id) = welcomed_in(&registry, SessionConfig::default()).await;
  eventually(|| registry.get(&id).is_some(), "the session is registered after WELCOME").await;
  registry.get(&id).unwrap().send(command).await.unwrap();
  (socket, ending)
}

#[tokio::test]
async fn sends_close_then_closes_with_every_code() {
  for code in CODES {
    let (mut socket, ending) = commanded(Command::Close { code, msg: None }).await;
    let (binary, ws_code, _) = closed(&mut socket).await;
    assert_eq!(binary, [close_frame(u16::from(code) as u8)], "{code:?}");
    assert_eq!(ws_code, WsCloseCode::from(4000 + u16::from(code)), "{code:?}");
    assert_eq!(
      ending.await.unwrap(),
      Ending::Closed {
        code,
        msg: None,
        by: Side::Server
      }
    );
  }
}

#[tokio::test]
async fn cuts_the_close_reason_on_a_character_boundary() {
  let msg = "€".repeat(67);
  let (mut socket, _) = commanded(Command::Close {
    code: CloseCode::GoingAway,
    msg: Some(msg),
  })
  .await;
  let (_, _, reason) = closed(&mut socket).await;
  assert_eq!(reason, "€".repeat(41));
}

#[tokio::test]
async fn mirrors_a_client_close() {
  let (mut socket, ending) = connect().await;
  established(&mut socket).await;
  socket.send(Message::binary(close_frame(0))).await.unwrap();
  assert!(mirrored_client_close(&mut socket, ending).await.is_empty());
}

#[tokio::test]
async fn records_an_unknown_client_code_as_internal() {
  let (mut socket, ending) = connect().await;
  socket.send(Message::binary(hello_frame())).await.unwrap();
  socket.send(Message::binary(close_frame(99))).await.unwrap();
  let (_, ws_code, _) = closed(&mut socket).await;
  assert_eq!(ws_code, WsCloseCode::from(4008));
  assert!(matches!(
    ending.await.unwrap(),
    Ending::Closed {
      code: CloseCode::Internal,
      by: Side::Client,
      ..
    }
  ));
}

#[tokio::test]
async fn ends_on_an_abrupt_drop() {
  let (socket, ending) = connect().await;
  drop(socket);
  assert_eq!(ending.await.unwrap(), Ending::TransportLost);
}

#[tokio::test]
async fn ends_on_a_websocket_close_without_close_frame() {
  let (mut socket, ending) = connect().await;
  socket.close(None).await.unwrap();
  assert!(matches!(socket.next().await, Some(Ok(Message::Close(_)))));
  assert_eq!(ending.await.unwrap(), Ending::TransportLost);
}

#[tokio::test]
async fn closes_with_protocol_error_on_a_malformed_message() {
  let (mut socket, ending) = connect().await;
  socket.send(Message::binary(&b"\x00\x7f\x00\x01\x00"[..])).await.unwrap();
  let (binary, ws_code, _) = closed(&mut socket).await;
  assert_eq!(binary.len(), 1);
  assert_eq!(&binary[0][..2], [0x00, 0x07]);
  assert_eq!(ws_code, WsCloseCode::from(4001));
  assert!(matches!(
    ending.await.unwrap(),
    Ending::Closed {
      code: CloseCode::ProtocolError,
      by: Side::Server,
      ..
    }
  ));
}

#[tokio::test]
async fn keeps_running_after_a_message_that_needs_no_answer() {
  let (mut socket, ending) = connect().await;
  established(&mut socket).await;
  socket.send(Message::binary(heartbeat_frame(0x05, b"x"))).await.unwrap();
  socket.send(Message::binary(close_frame(0))).await.unwrap();
  assert!(mirrored_client_close(&mut socket, ending).await.is_empty());
}

#[tokio::test]
async fn closes_with_protocol_error_on_a_text_message() {
  for text in ["hello", ""] {
    let (mut socket, ending) = connect().await;
    socket.send(Message::text(text)).await.unwrap();
    let (binary, ws_code, _) = closed(&mut socket).await;
    assert_eq!(binary.len(), 1, "{text:?}");
    assert_eq!(&binary[0][..2], [0x00, 0x07]);
    assert_eq!(ws_code, WsCloseCode::from(4001));
    assert!(matches!(
      ending.await.unwrap(),
      Ending::Closed {
        code: CloseCode::ProtocolError,
        by: Side::Server,
        ..
      }
    ));
  }
}

fn short_timeout() -> SessionConfig {
  let mut config = SessionConfig::default();
  config.handshake_timeout = Duration::from_millis(200);
  config
}

#[tokio::test]
async fn closes_with_handshake_timeout_when_nothing_arrives() {
  let started = std::time::Instant::now();
  let (mut socket, ending) = connect_with(short_timeout()).await;
  closed_by_server(&mut socket, ending, CloseCode::HandshakeTimeout).await;
  assert!(started.elapsed() >= Duration::from_millis(200));
}

#[tokio::test]
async fn sends_welcome_and_stops_the_handshake_timer() {
  let (mut socket, ending) = connect_with(short_timeout()).await;
  socket.send(Message::binary(hello_frame())).await.unwrap();
  let welcome = next_welcome(&mut socket).await;
  let keys: Vec<u64> = welcome.keys().copied().collect();
  assert_eq!(keys, [15, 17, 18, 19, 21, 22, 23]);
  assert_eq!(welcome[&15], Value::Integer(1.into()));
  assert_eq!(welcome[&17], Value::Text("cbor".into()));
  assert_eq!(welcome[&18], Value::Array(vec![Value::Text("calls".into())]));
  let Value::Text(id) = &welcome[&19] else {
    panic!("{welcome:?}");
  };
  assert!(SessionId::parse(id).is_ok(), "{id}");
  assert!(tokio::time::timeout(Duration::from_millis(400), socket.next()).await.is_err());
  socket.send(Message::binary(close_frame(0))).await.unwrap();
  assert!(mirrored_client_close(&mut socket, ending).await.is_empty());
}

async fn next_welcome<S: AsyncRead + AsyncWrite + Unpin>(socket: &mut WebSocketStream<S>) -> BTreeMap<u64, Value> {
  match socket.next().await.unwrap().unwrap() {
    Message::Binary(message) => welcome(&message),
    other => panic!("expected WELCOME, got {other:?}"),
  }
}

fn welcome(message: &[u8]) -> BTreeMap<u64, Value> {
  assert_eq!(message[..3], [0x00, 0x02, 0x00], "{message:02x?}");
  let mut rest = &message[3..];
  let _payload = varint(&mut rest);
  let header = varint(&mut rest);
  header_map(&rest[..header as usize])
}

fn header_map(header: &[u8]) -> BTreeMap<u64, Value> {
  let header: Value = ciborium::from_reader(header).unwrap();
  let Value::Map(entries) = header else {
    panic!("{header:?}");
  };
  entries
    .into_iter()
    .map(|(key, value)| (u64::try_from(key.as_integer().unwrap()).unwrap(), value))
    .collect()
}

fn varint(bytes: &mut &[u8]) -> u64 {
  let len = 1 << (bytes[0] >> 6);
  let value = bytes[..len].iter().fold(0, |value, &byte| (value << 8) | u64::from(byte)) & (u64::MAX >> (64 - 8 * len + 2));
  *bytes = &bytes[len..];
  value
}

#[tokio::test]
async fn closes_a_slow_hello_with_handshake_timeout() {
  let (address, ending) = listen(short_timeout()).await;
  let mut stream = TcpStream::connect(address).await.unwrap();
  let request = concat!(
    "GET /rt HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n",
    "Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
    "Sec-WebSocket-Protocol: pavia.1\r\n\r\n",
  );
  stream.write_all(request.as_bytes()).await.unwrap();
  let head = tokio::time::timeout(EXPECT_WITHIN, async {
    let mut received = Vec::new();
    while !received.ends_with(b"\r\n\r\n") {
      received.push(stream.read_u8().await.unwrap());
    }
    received
  })
  .await
  .expect("the server never answered the upgrade");
  assert!(head.starts_with(b"HTTP/1.1 101"), "{}", String::from_utf8_lossy(&head));
  let masked_hello_start = [0x82, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01];
  for byte in masked_hello_start {
    if stream.write_all(&[byte]).await.is_err() {
      break;
    }
    tokio::time::sleep(Duration::from_millis(60)).await;
  }
  let mut rest = Vec::new();
  let read = tokio::time::timeout(EXPECT_WITHIN, stream.read_to_end(&mut rest)).await;
  assert!(read.is_ok(), "the server never closed the connection");
  assert!(has_ws_close(&rest, 4012), "{rest:02x?}");
  assert!(matches!(
    ending.await.unwrap(),
    Ending::Closed {
      code: CloseCode::HandshakeTimeout,
      ..
    }
  ));
}

#[tokio::test]
async fn closes_with_protocol_error_when_the_first_frame_is_not_hello() {
  let (mut socket, ending) = connect().await;
  socket.send(Message::binary(heartbeat_frame(0x04, &[]))).await.unwrap();
  closed_by_server(&mut socket, ending, CloseCode::ProtocolError).await;
}

fn raw_frame(frame_type: u8, header: &[u8]) -> Vec<u8> {
  raw_frame_on(0, frame_type, header)
}

fn raw_frame_on(stream: u8, frame_type: u8, header: &[u8]) -> Vec<u8> {
  let mut frame = vec![stream, frame_type, 0x00, header.len() as u8 + 1, header.len() as u8];
  frame.extend(header);
  frame
}

fn heartbeat_frame(frame_type: u8, data: &[u8]) -> Vec<u8> {
  let mut frame = vec![0x00, frame_type, 0x00, data.len() as u8 + 1, 0x00];
  frame.extend(data);
  frame
}

#[tokio::test]
async fn rejects_a_hello_with_no_common_version_or_codec() {
  let msg = "no common protocol version or codec";
  let mut reject = vec![0xa2, 0x06, 0x02, 0x07, 0x78, msg.len() as u8];
  reject.extend(msg.as_bytes());
  let hellos = [
    [&[0xa2, 0x0e, 0x81, 0x02, 0x10, 0x81, 0x64][..], b"cbor"].concat(),
    [&[0xa2, 0x0e, 0x81, 0x01, 0x10, 0x81, 0x67][..], b"msgpack"].concat(),
  ];
  for hello in hellos {
    let (mut socket, ending) = connect().await;
    socket.send(Message::binary(raw_frame(0x01, &hello))).await.unwrap();
    let (binary, ws_code, reason) = closed(&mut socket).await;
    assert_eq!(binary, [raw_frame(0x03, &reject)], "{hello:02x?}");
    assert_eq!(ws_code, WsCloseCode::from(4002));
    assert_eq!(reason.as_str(), msg);
    assert_eq!(
      ending.await.unwrap(),
      Ending::Rejected {
        code: CloseCode::UnsupportedVersion,
        msg: msg.into()
      }
    );
  }
}

#[tokio::test]
async fn rejects_a_resume_with_resume_failed() {
  let msg = "unknown session";
  let mut reject = vec![0xa2, 0x06, 0x0b, 0x07, 0x60 + msg.len() as u8];
  reject.extend(msg.as_bytes());
  let mut hello = vec![0xa3, 0x0e, 0x81, 0x01, 0x10, 0x81, 0x64];
  hello.extend(b"cbor");
  hello.extend([0x14, 0xa3, 0x0d, 0x41, 0x00, 0x13, 0x63]);
  hello.extend(b"s_x");
  hello.extend([0x18, 0x2c, 0xa1, 0x00, 0x00]);
  let (mut socket, ending) = connect().await;
  socket.send(Message::binary(raw_frame(0x01, &hello))).await.unwrap();
  let (binary, ws_code, reason) = closed(&mut socket).await;
  assert_eq!(binary, [raw_frame(0x03, &reject)]);
  assert_eq!(ws_code, WsCloseCode::from(4011));
  assert_eq!(reason.as_str(), msg);
  assert_eq!(
    ending.await.unwrap(),
    Ending::Rejected {
      code: CloseCode::ResumeFailed,
      msg: msg.into()
    }
  );
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
  scheme: Option<String>,
  header: Option<Vec<u8>>,
  cookie: Option<String>,
  peer: Option<SocketAddr>,
}

struct Recording {
  answer: Option<Result<Principal, AuthFailure>>,
  seen: Mutex<Vec<Seen>>,
}

impl Recording {
  fn answering(answer: Option<Result<Principal, AuthFailure>>) -> Arc<Self> {
    Arc::new(Self {
      answer,
      seen: Mutex::new(Vec::new()),
    })
  }

  fn seen(&self) -> Vec<Seen> {
    self.seen.lock().unwrap().clone()
  }
}

impl Authenticator for Recording {
  fn schemes(&self) -> &[String] {
    &[]
  }

  async fn authenticate(&self, auth: Option<&Auth>, context: &RequestContext) -> Result<Principal, AuthFailure> {
    self.seen.lock().unwrap().push(Seen {
      scheme: auth.map(|auth| auth.scheme.clone()),
      header: context.header("x-pavia-test").map(<[u8]>::to_vec),
      cookie: context.cookie("session").map(str::to_owned),
      peer: context.peer(),
    });
    match &self.answer {
      Some(answer) => answer.clone(),
      None => std::future::pending().await,
    }
  }
}

fn bearer_hello() -> Vec<u8> {
  let mut hello = vec![0xa3, 0x0e, 0x81, 0x01, 0x10, 0x81, 0x64];
  hello.extend(b"cbor");
  hello.extend([0x18, 0x21, 0xa2, 0x0d, 0x64]);
  hello.extend(b"good");
  hello.extend([0x18, 0x23, 0x66]);
  hello.extend(b"bearer");
  raw_frame(0x01, &hello)
}

async fn connect_to(
  registry: Registry,
  authenticator: Arc<Recording>,
  config: SessionConfig,
) -> (Socket, SocketAddr, oneshot::Receiver<Ending>) {
  let (address, ending) = listen_with(registry, config, authenticator).await;
  let mut request = request();
  let headers = request.headers_mut();
  headers.insert("x-pavia-test", HeaderValue::from_static("yes"));
  headers.insert(header::COOKIE, HeaderValue::from_static("theme=dark; session=c00kie"));
  let stream = TcpStream::connect(address).await.unwrap();
  let local = stream.local_addr().unwrap();
  let (socket, _) = client_async(request, stream).await.unwrap();
  (socket, local, ending)
}

#[tokio::test]
async fn rejects_with_the_code_the_authenticator_chose() {
  for (failure, code, msg) in [
    (AuthFailure::Unauthenticated, CloseCode::Unauthenticated, "unauthenticated"),
    (AuthFailure::Forbidden, CloseCode::Forbidden, "forbidden"),
  ] {
    let mut reject = vec![0xa2, 0x06, u16::from(code) as u8, 0x07, 0x60 + msg.len() as u8];
    reject.extend(msg.as_bytes());
    let (mut socket, _, ending) = connect_to(
      Registry::new(),
      Recording::answering(Some(Err(failure))),
      SessionConfig::default(),
    )
    .await;
    socket.send(Message::binary(bearer_hello())).await.unwrap();
    let (binary, ws_code, reason) = closed(&mut socket).await;
    assert_eq!(binary, [raw_frame(0x03, &reject)], "{failure:?}");
    assert_eq!(ws_code, WsCloseCode::from(4000 + u16::from(code)));
    assert_eq!(reason.as_str(), msg);
    assert_eq!(ending.await.unwrap(), Ending::Rejected { code, msg: msg.into() });
  }
}

#[tokio::test]
async fn gives_the_authenticator_the_credential_and_the_upgrade_request() {
  let authenticator = Recording::answering(Some(Ok(Principal::for_user("u-42"))));
  let (mut socket, local, ending) = connect_to(Registry::new(), Arc::clone(&authenticator), SessionConfig::default()).await;
  socket.send(Message::binary(bearer_hello())).await.unwrap();
  assert_eq!(next_welcome(&mut socket).await.get(&36), Some(&Value::Text("u-42".into())));
  socket.send(Message::binary(close_frame(0))).await.unwrap();
  assert!(mirrored_client_close(&mut socket, ending).await.is_empty());
  assert_eq!(
    authenticator.seen(),
    [Seen {
      scheme: Some("bearer".into()),
      header: Some(b"yes".to_vec()),
      cookie: Some("c00kie".into()),
      peer: Some(local),
    }]
  );
}

#[tokio::test]
async fn keeps_reading_the_transport_while_the_authenticator_runs() {
  let authenticator = Recording::answering(None);
  let (mut socket, _, ending) = connect_to(Registry::new(), Arc::clone(&authenticator), SessionConfig::default()).await;
  socket.send(Message::binary(bearer_hello())).await.unwrap();
  eventually(|| !authenticator.seen().is_empty(), "the authenticator runs").await;
  socket.send(Message::binary(close_frame(0))).await.unwrap();
  mirrored_client_close(&mut socket, ending).await;
  assert_eq!(authenticator.seen().len(), 1);
}

#[tokio::test]
async fn closes_a_slow_authenticator_with_handshake_timeout() {
  let (mut socket, _, ending) = connect_to(Registry::new(), Recording::answering(None), short_timeout()).await;
  socket.send(Message::binary(bearer_hello())).await.unwrap();
  let (binary, ws_code, _) = closed(&mut socket).await;
  assert_eq!(binary, [close_frame(12)]);
  assert_eq!(ws_code, WsCloseCode::from(4012));
  assert!(matches!(
    ending.await.unwrap(),
    Ending::Closed {
      code: CloseCode::HandshakeTimeout,
      by: Side::Server,
      ..
    }
  ));
}

#[test]
fn closes_with_internal_when_the_session_id_cannot_be_generated() {
  let failed = std::io::Error::other("no entropy");
  let input = authenticated(&Principal::for_user("ada"), Err(failed));
  assert!(matches!(
    input,
    Input::Command(Command::Close {
      code: CloseCode::Internal,
      msg: None
    })
  ));
}

#[test]
fn hands_the_generated_id_and_user_to_the_session() {
  let id = SessionId::from_random([7; 16]);
  let input = authenticated(&Principal::for_user("ada"), Ok(id.clone()));
  assert!(matches!(
    input,
    Input::Authenticated { session, user: Some(user) } if session == id && user == "ada"
  ));
}

fn fast_heartbeat() -> SessionConfig {
  let mut config = SessionConfig::default();
  config.heartbeat = Duration::from_millis(200);
  config
}

async fn welcomed(config: SessionConfig) -> (Socket, oneshot::Receiver<Ending>) {
  let (socket, ending, _) = welcomed_in(&Registry::new(), config).await;
  (socket, ending)
}

#[tokio::test]
async fn pings_an_idle_session_every_heartbeat() {
  let (mut socket, _) = welcomed(fast_heartbeat()).await;
  let start = std::time::Instant::now();
  tokio::time::timeout(Duration::from_secs(3), async {
    for _ in 0..4 {
      let message = socket.next().await.unwrap().unwrap();
      assert_eq!(message, Message::binary(heartbeat_frame(0x04, &[])));
    }
  })
  .await
  .expect("four PINGs arrive");
  let elapsed = start.elapsed();
  assert!(elapsed >= Duration::from_millis(700), "{elapsed:?}");
  assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
}

fn short_idle() -> SessionConfig {
  let mut config = SessionConfig::default();
  config.heartbeat = Duration::from_millis(100);
  config.idle_timeout = Duration::from_millis(300);
  config
}

#[tokio::test]
async fn closes_a_silent_client_with_idle_timeout() {
  let start = std::time::Instant::now();
  let (mut socket, ending) = welcomed(short_idle()).await;
  let (binary, ws_code, _) = tokio::time::timeout(Duration::from_secs(3), closed(&mut socket))
    .await
    .expect("the server closes");
  let elapsed = start.elapsed();
  assert!(elapsed >= Duration::from_millis(300), "{elapsed:?}");
  assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
  let (close, pings) = binary.split_last().expect("CLOSE was sent");
  assert!(pings.iter().all(|ping| *ping == heartbeat_frame(0x04, &[])), "{pings:02x?}");
  assert_eq!(*close, close_frame(5));
  assert_eq!(ws_code, WsCloseCode::from(4005));
  assert!(matches!(
    ending.await.unwrap(),
    Ending::Closed {
      code: CloseCode::IdleTimeout,
      by: Side::Server,
      ..
    }
  ));
}

#[tokio::test]
async fn keeps_a_client_that_answers_every_ping() {
  let mut config = short_idle();
  config.idle_timeout = Duration::from_secs(1);
  let (mut socket, _) = welcomed(config).await;
  let held = tokio::time::timeout(Duration::from_secs(2), async {
    loop {
      match socket.next().await.unwrap().unwrap() {
        Message::Binary(ping) if ping == heartbeat_frame(0x04, &[]) => {
          socket.send(Message::binary(heartbeat_frame(0x05, &[]))).await.unwrap();
        }
        other => panic!("unexpected {other:?}"),
      }
    }
  })
  .await;
  assert!(held.is_err());
}

#[tokio::test]
async fn reads_a_waiting_message_before_a_passed_idle_deadline() {
  let mut config = short_idle();
  config.idle_timeout = Duration::from_millis(200);
  let (mut socket, _) = welcomed(config).await;
  for _ in 0..10 {
    socket.send(Message::binary(heartbeat_frame(0x04, &[7]))).await.unwrap();
    std::thread::sleep(Duration::from_millis(300));
    loop {
      match socket.next().await.unwrap().unwrap() {
        Message::Binary(pong) if pong == heartbeat_frame(0x05, &[7]) => break,
        Message::Binary(ping) if ping == heartbeat_frame(0x04, &[]) => {}
        other => panic!("unexpected {other:?}"),
      }
    }
  }
}

#[tokio::test]
async fn mirrors_a_close_waiting_past_the_idle_deadline() {
  let mut config = short_idle();
  config.idle_timeout = Duration::from_millis(200);
  for _ in 0..10 {
    let (mut socket, ending) = welcomed(config.clone()).await;
    socket.send(Message::binary(close_frame(0))).await.unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let binary = mirrored_client_close(&mut socket, ending).await;
    assert!(binary.iter().all(|ping| *ping == heartbeat_frame(0x04, &[])), "{binary:02x?}");
  }
}

#[tokio::test]
async fn answers_a_ping_with_its_data() {
  let (mut socket, _) = welcomed(SessionConfig::default()).await;
  let data = [1, 2, 3, 4, 5, 6, 7, 8];
  socket.send(Message::binary(heartbeat_frame(0x04, &data))).await.unwrap();
  assert_eq!(
    socket.next().await.unwrap().unwrap(),
    Message::binary(heartbeat_frame(0x05, &data))
  );
}

#[tokio::test]
async fn closes_a_length_above_max_frame_with_frame_too_large_from_the_prefix() {
  let (mut socket, ending) = connect_with(floor_frame()).await;
  let prefix_of_65537 = vec![0x00, 0x04, 0x00, 0x80, 0x01, 0x00, 0x01];
  socket.send(Message::binary(prefix_of_65537)).await.unwrap();
  closed_by_server(&mut socket, ending, CloseCode::FrameTooLarge).await;
}

#[tokio::test]
async fn closes_a_message_above_twice_max_frame_with_protocol_error() {
  let (mut socket, ending) = connect_with(floor_frame()).await;
  socket.send(Message::binary(vec![0; 131_073])).await.unwrap();
  closed_by_server(&mut socket, ending, CloseCode::ProtocolError).await;
}

#[tokio::test]
async fn closes_a_fragmented_message_above_twice_max_frame_with_protocol_error() {
  let (mut socket, ending) = connect_with(floor_frame()).await;
  let first = Frame::message(vec![0; 70_000], OpCode::Data(Data::Binary), false);
  let second = Frame::message(vec![0; 70_000], OpCode::Data(Data::Continue), true);
  socket.send(Message::Frame(first)).await.unwrap();
  socket.send(Message::Frame(second)).await.unwrap();
  closed_by_server(&mut socket, ending, CloseCode::ProtocolError).await;
}

fn notify_frame(payload_length: usize) -> Vec<u8> {
  let mut frame = vec![0x02, 0x20, 0x00];
  frame.extend((u32::try_from(payload_length).unwrap() | 0x8000_0000).to_be_bytes());
  let mut payload = vec![0x04, 0xa1, 0x02, 0x61, b'm'];
  payload.resize(payload_length, 0);
  frame.extend(payload);
  frame
}

async fn welcomes_a_pipelined_message(message: Vec<u8>) {
  let (mut socket, ending) = connect_with(floor_frame()).await;
  socket.send(Message::binary(message)).await.unwrap();
  assert_eq!(next_welcome(&mut socket).await.len(), 7);
  socket.send(Message::binary(close_frame(0))).await.unwrap();
  assert!(mirrored_client_close(&mut socket, ending).await.is_empty());
}

#[tokio::test]
async fn accepts_a_max_frame_payload_pipelined_behind_hello() {
  let mut message = hello_frame();
  message.extend(notify_frame(65_536));
  welcomes_a_pipelined_message(message).await;
}

#[tokio::test]
async fn accepts_a_message_of_exactly_twice_max_frame() {
  let mut message = hello_frame();
  message.extend(notify_frame(65_536));
  let rest = 131_072 - message.len();
  message.extend(notify_frame(rest - 7));
  assert_eq!(message.len(), 131_072);
  welcomes_a_pipelined_message(message).await;
}

#[tokio::test]
async fn delivers_the_close_to_a_client_still_sending_a_refused_message() {
  let (mut socket, ending) = connect_with(floor_frame()).await;
  socket.send(Message::binary(vec![0; 4 << 20])).await.unwrap();
  closed_by_server(&mut socket, ending, CloseCode::ProtocolError).await;
}

#[tokio::test]
async fn registers_at_welcome_and_removes_when_the_session_ends() {
  let registry = Registry::new();
  let authenticator = Recording::answering(None);
  let (mut socket, _, _) = connect_to(registry.clone(), Arc::clone(&authenticator), SessionConfig::default()).await;
  socket.send(Message::binary(bearer_hello())).await.unwrap();
  eventually(|| !authenticator.seen().is_empty(), "the authenticator runs").await;
  assert!(registry.is_empty());
  let (mut socket, ending, id) = welcomed_in(&registry, SessionConfig::default()).await;
  eventually(|| registry.len() == 1, "the session is registered after WELCOME").await;
  assert!(registry.get(&id).is_some());
  socket.send(Message::binary(heartbeat_frame(0x04, b"1"))).await.unwrap();
  assert_eq!(
    socket.next().await.unwrap().unwrap(),
    Message::binary(heartbeat_frame(0x05, b"1"))
  );
  assert!(registry.get(&id).is_some(), "a later send must keep the session registered");
  socket.send(Message::binary(close_frame(0))).await.unwrap();
  mirrored_client_close(&mut socket, ending).await;
  assert!(registry.is_empty());
  assert!(registry.get(&id).is_none());
}

#[tokio::test]
async fn never_registers_a_rejected_session() {
  let registry = Registry::new();
  let authenticator = Recording::answering(Some(Err(AuthFailure::Forbidden)));
  let (mut socket, _, ending) = connect_to(registry.clone(), authenticator, SessionConfig::default()).await;
  socket.send(Message::binary(bearer_hello())).await.unwrap();
  let _ = closed(&mut socket).await;
  assert!(matches!(ending.await.unwrap(), Ending::Rejected { .. }));
  assert!(registry.is_empty());
}

type Boxed = Pin<Box<dyn Future<Output = ()> + Send>>;

fn recording(
  registry: &Registry,
  config: SessionConfig,
) -> (
  Endpoint<impl Fn(Transport) -> Boxed + Clone + Send + Sync + 'static>,
  mpsc::UnboundedReceiver<Ending>,
) {
  let (ended, endings) = mpsc::unbounded_channel();
  let registry = registry.clone();
  let config = config.validate().unwrap();
  let endpoint = Endpoint::new(Config::new("/rt", &config), move |transport: Transport| -> Boxed {
    let registry = registry.clone();
    let ended = ended.clone();
    Box::pin(async move {
      let _ = ended.send(run(transport, &registry, &MethodRegistry::new(), &Permissive, &Arc::new(AllowAll)).await);
    })
  });
  (endpoint, endings)
}

async fn welcomed_over_duplex<H, F>(endpoint: &Endpoint<H>) -> (WebSocketStream<DuplexStream>, SessionId)
where
  H: Fn(Transport) -> F + Clone + Send + Sync + 'static,
  F: Future<Output = ()> + Send + 'static,
{
  let (client, server) = tokio::io::duplex(4096);
  let endpoint = endpoint.clone();
  tokio::spawn(async move { endpoint.serve_connection(server, None).await });
  let mut socket = open(client).await;
  let id = established(&mut socket).await;
  (socket, id)
}

async fn collect(endings: &mut mpsc::UnboundedReceiver<Ending>, count: usize) -> Vec<Ending> {
  let mut collected = Vec::with_capacity(count);
  let _ = tokio::time::timeout(EXPECT_WITHIN, async {
    while collected.len() < count {
      let Some(ending) = endings.recv().await else {
        break;
      };
      collected.push(ending);
    }
  })
  .await;
  assert_eq!(collected.len(), count, "sessions that reported an ending");
  collected
}

fn closed_with(endings: &[Ending], code: CloseCode, by: Side) -> usize {
  endings
    .iter()
    .filter(|ending| matches!(ending, Ending::Closed { code: ended, by: side, .. } if *ended == code && *side == by))
    .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn leaves_the_registry_empty_after_ten_thousand_sessions() {
  let registry = Registry::new();
  let (endpoint, mut endings) = recording(&registry, SessionConfig::default());
  let mut clients = tokio::task::JoinSet::new();
  for client in 0..50 {
    let endpoint = endpoint.clone();
    let registry = registry.clone();
    clients.spawn(async move {
      for cycle in 0..200 {
        let (mut socket, id) = welcomed_over_duplex(&endpoint).await;
        eventually(|| registry.get(&id).is_some(), "the session is registered after WELCOME").await;
        if (client + cycle) % 2 == 0 {
          socket.send(Message::binary(close_frame(0))).await.unwrap();
          while let Some(Ok(message)) = socket.next().await {
            if message.is_close() {
              break;
            }
          }
        }
      }
    });
  }
  while let Some(finished) = clients.join_next().await {
    finished.unwrap();
  }
  collect(&mut endings, 10_000).await;
  eventually(|| registry.is_empty(), "every session leaves the registry").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removes_a_session_once_when_its_close_meets_the_idle_timeout() {
  let registry = Registry::new();
  let mut config = SessionConfig::default();
  config.heartbeat = Duration::from_millis(5);
  config.idle_timeout = Duration::from_millis(20);
  let (endpoint, mut endings) = recording(&registry, config);
  for cycle in 0..200 {
    let (mut socket, _) = welcomed_over_duplex(&endpoint).await;
    tokio::time::sleep(Duration::from_micros(cycle * 200)).await;
    let _ = socket.send(Message::binary(close_frame(0))).await;
    drop(socket);
  }
  let endings = collect(&mut endings, 200).await;
  let idle = closed_with(&endings, CloseCode::IdleTimeout, Side::Server);
  let by_client = closed_with(&endings, CloseCode::Normal, Side::Client);
  assert!(
    idle > 0 && by_client > 0,
    "idle {idle}, client close {by_client}: the close never met the timer"
  );
  eventually(|| registry.is_empty(), "every session leaves the registry").await;
}

struct FailingWrites {
  inner: DuplexStream,
  failing: Arc<AtomicBool>,
  registry: Registry,
  registered_at_failure: Arc<Mutex<Option<usize>>>,
}

impl AsyncRead for FailingWrites {
  fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
    Pin::new(&mut self.inner).poll_read(cx, buf)
  }
}

impl AsyncWrite for FailingWrites {
  fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
    if self.failing.load(Ordering::SeqCst) {
      let registered = self.registry.len();
      self.registered_at_failure.lock().unwrap().get_or_insert(registered);
      return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
    }
    Pin::new(&mut self.inner).poll_write(cx, buf)
  }

  fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
    Pin::new(&mut self.inner).poll_flush(cx)
  }

  fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
    Pin::new(&mut self.inner).poll_shutdown(cx)
  }
}

struct BreakingWrites(Arc<AtomicBool>);

impl Authenticator for BreakingWrites {
  fn schemes(&self) -> &[String] {
    &[]
  }

  async fn authenticate(&self, _: Option<&Auth>, _: &RequestContext) -> Result<Principal, AuthFailure> {
    self.0.store(true, Ordering::SeqCst);
    Ok(Principal::anonymous())
  }
}

#[tokio::test]
async fn never_registers_a_session_whose_welcome_cannot_be_written() {
  let registry = Registry::new();
  let failing = Arc::new(AtomicBool::new(false));
  let registered_at_failure = Arc::new(Mutex::new(None));
  let (ended, ending) = oneshot::channel();
  let ended = Arc::new(Mutex::new(Some(ended)));
  let config = SessionConfig::default().validate().unwrap();
  let sessions = registry.clone();
  let authenticator = Arc::new(BreakingWrites(Arc::clone(&failing)));
  let endpoint = Endpoint::new(Config::new("/rt", &config), move |transport: Transport| -> Boxed {
    let registry = sessions.clone();
    let authenticator = Arc::clone(&authenticator);
    let ended = ended.lock().unwrap().take();
    Box::pin(async move {
      let ending = run(
        transport,
        &registry,
        &MethodRegistry::new(),
        &*authenticator,
        &Arc::new(AllowAll),
      )
      .await;
      if let Some(ended) = ended {
        let _ = ended.send(ending);
      }
    })
  });
  let (client, server) = tokio::io::duplex(4096);
  let server = FailingWrites {
    inner: server,
    failing,
    registry: registry.clone(),
    registered_at_failure: Arc::clone(&registered_at_failure),
  };
  tokio::spawn(async move { endpoint.serve_connection(server, None).await });
  let mut socket = open(client).await;
  socket.send(Message::binary(hello_frame())).await.unwrap();
  let ending = tokio::time::timeout(EXPECT_WITHIN, ending).await.unwrap().unwrap();
  assert_eq!(ending, Ending::TransportLost);
  assert_eq!(*registered_at_failure.lock().unwrap(), Some(0));
  assert!(registry.is_empty());
}

fn call_frame(stream: u8, method: &str, data: &[u8]) -> Vec<u8> {
  call_frame_shaped(stream, method, 0, data)
}

fn call_frame_shaped(stream: u8, method: &str, shape: u8, data: &[u8]) -> Vec<u8> {
  let mut header = vec![0xa2, 0x02, 0x60 + u8::try_from(method.len()).unwrap()];
  header.extend(method.as_bytes());
  header.extend([0x05, shape]);
  let mut payload = vec![u8::try_from(header.len()).unwrap()];
  payload.extend(header);
  payload.extend(data);
  let mut frame = vec![stream, 0x10, 0x00, 0x40, u8::try_from(payload.len()).unwrap()];
  frame.extend(payload);
  frame
}

struct Answer {
  stream: u64,
  frame_type: u8,
  header: BTreeMap<u64, Value>,
  data: Vec<u8>,
}

async fn next_answer(socket: &mut Socket) -> Answer {
  let Message::Binary(message) = tokio::time::timeout(EXPECT_WITHIN, socket.next())
    .await
    .expect("an answer")
    .unwrap()
    .unwrap()
  else {
    panic!("expected a binary message");
  };
  let mut rest = &message[..];
  let stream = varint(&mut rest);
  let frame_type = rest[0];
  rest = &rest[2..];
  let _payload = varint(&mut rest);
  let header_length = usize::try_from(varint(&mut rest)).unwrap();
  let header = if header_length == 0 {
    BTreeMap::new()
  } else {
    header_map(&rest[..header_length])
  };
  Answer {
    stream,
    frame_type,
    header,
    data: rest[header_length..].to_vec(),
  }
}

fn assert_refused(answer: &Answer, stream: u64, code: u8) {
  assert_eq!((answer.stream, answer.frame_type), (stream, 0x14));
  assert_eq!(answer.header[&6], Value::from(code));
  assert_eq!(answer.header.get(&45), Some(&Value::Bool(true)));
}

fn assert_error(answer: &Answer, stream: u64, code: u8) {
  assert_eq!((answer.stream, answer.frame_type), (stream, 0x14));
  assert_eq!(answer.header[&6], Value::from(code));
  assert_eq!(answer.header.get(&45), None);
}

fn chat() -> (Arc<MethodRegistry<Handler>>, Arc<AtomicUsize>) {
  let runs = Arc::new(AtomicUsize::new(0));
  let counted = Arc::clone(&runs);
  let mut methods = MethodRegistry::new();
  methods
    .register_server_method(
      "chat/send",
      MethodEntry::call(Handler::new(2, move |arguments: Vec<Arg>| {
        counted.fetch_add(1, Ordering::SeqCst);
        async move {
          let [Arg::Text(room), Arg::Text(text)] = arguments.as_slice() else {
            return Arg::Null;
          };
          Arg::Text(format!("{room}:{text}"))
        }
      })),
    )
    .unwrap();
  methods
    .register_server_method("chat/ping", MethodEntry::call(Handler::new(0, |_| async { Arg::Null })))
    .unwrap();
  methods
    .register_server_method("chat/typing", MethodEntry::notify(Handler::new(0, |_| async { Arg::Null })))
    .unwrap();
  methods
    .register_server_method(
      "chat/crash",
      MethodEntry::call(Handler::new(0, |_| async { panic!("the handler fails") })),
    )
    .unwrap();
  (Arc::new(methods), runs)
}

async fn serving(methods: Arc<MethodRegistry<Handler>>) -> Socket {
  serving_with(methods, SessionConfig::default()).await
}

async fn serving_with(methods: Arc<MethodRegistry<Handler>>, config: SessionConfig) -> Socket {
  let (address, _) = listen_serving(Registry::new(), methods, config, Arc::new(Permissive), Arc::new(AllowAll)).await;
  let mut socket = open(TcpStream::connect(address).await.unwrap()).await;
  established(&mut socket).await;
  socket
}

const GENERAL_HELLO: &[u8] = &[
  0x82, 0x67, b'g', b'e', b'n', b'e', b'r', b'a', b'l', 0x65, b'h', b'e', b'l', b'l', b'o',
];

#[tokio::test]
async fn answers_a_call_with_the_handler_result() {
  let (methods, runs) = chat();
  let mut socket = serving(methods).await;
  socket
    .send(Message::binary(call_frame(2, "chat/send", GENERAL_HELLO)))
    .await
    .unwrap();
  let answer = next_answer(&mut socket).await;
  assert_eq!((answer.stream, answer.frame_type), (2, 0x13));
  let expected = [&[0x6d][..], b"general:hello"].concat();
  assert_eq!(answer.data, expected);
  assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn refuses_a_wrong_argument_count_without_running_the_handler() {
  let (methods, runs) = chat();
  let mut socket = serving(methods).await;
  let one = [0x81, 0x61, b'a'];
  let three = [0x83, 0x61, b'a', 0x61, b'b', 0x61, b'c'];
  socket.send(Message::binary(call_frame(2, "chat/send", &one))).await.unwrap();
  assert_refused(&next_answer(&mut socket).await, 2, 3);
  socket
    .send(Message::binary(call_frame(4, "chat/send", &three)))
    .await
    .unwrap();
  assert_refused(&next_answer(&mut socket).await, 4, 3);
  assert_eq!(runs.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn answers_a_method_without_parameters_or_result_with_null() {
  let (methods, _) = chat();
  let mut socket = serving(methods).await;
  socket
    .send(Message::binary(call_frame(2, "chat/ping", &[0x80])))
    .await
    .unwrap();
  let answer = next_answer(&mut socket).await;
  assert_eq!((answer.stream, answer.frame_type, answer.data), (2, 0x13, vec![0xf6]));
}

#[tokio::test]
async fn refuses_arguments_that_are_not_an_array_or_do_not_decode() {
  let (methods, runs) = chat();
  let mut socket = serving(methods).await;
  socket
    .send(Message::binary(call_frame(2, "chat/send", &[0x61, b'x'])))
    .await
    .unwrap();
  assert_refused(&next_answer(&mut socket).await, 2, 3);
  socket
    .send(Message::binary(call_frame(4, "chat/send", &[0xff])))
    .await
    .unwrap();
  assert_refused(&next_answer(&mut socket).await, 4, 3);
  assert_eq!(runs.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn answers_sequential_calls_on_new_streams() {
  let (methods, runs) = chat();
  let mut socket = serving(methods).await;
  for stream in [2, 4] {
    socket
      .send(Message::binary(call_frame(stream, "chat/send", GENERAL_HELLO)))
      .await
      .unwrap();
    let answer = next_answer(&mut socket).await;
    assert_eq!((answer.stream, answer.frame_type), (u64::from(stream), 0x13));
  }
  assert_eq!(runs.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn refuses_an_unknown_method_and_a_notify_method_without_running_anything() {
  let (methods, _) = chat();
  let mut socket = serving(methods).await;
  socket
    .send(Message::binary(call_frame(2, "chat/nope", &[0x80])))
    .await
    .unwrap();
  assert_refused(&next_answer(&mut socket).await, 2, 12);
  socket
    .send(Message::binary(call_frame(4, "chat/typing", &[0x80])))
    .await
    .unwrap();
  assert_refused(&next_answer(&mut socket).await, 4, 3);
}

#[tokio::test]
async fn answers_internal_when_the_handler_panics_and_keeps_the_session() {
  let (methods, _) = chat();
  let mut socket = serving(methods).await;
  socket
    .send(Message::binary(call_frame(2, "chat/crash", &[0x80])))
    .await
    .unwrap();
  assert_error(&next_answer(&mut socket).await, 2, 13);
  socket
    .send(Message::binary(call_frame(4, "chat/ping", &[0x80])))
    .await
    .unwrap();
  assert_eq!(next_answer(&mut socket).await.frame_type, 0x13);
}

fn slow() -> (MethodRegistry<Handler>, Arc<Semaphore>, Arc<AtomicUsize>) {
  let gate = Arc::new(Semaphore::new(0));
  let runs = Arc::new(AtomicUsize::new(0));
  let (held, counted) = (Arc::clone(&gate), Arc::clone(&runs));
  let mut methods = MethodRegistry::new();
  methods
    .register_server_method(
      "chat/slow",
      MethodEntry::call(Handler::new(0, move |_| {
        let (gate, runs) = (Arc::clone(&held), Arc::clone(&counted));
        async move {
          runs.fetch_add(1, Ordering::SeqCst);
          gate.acquire().await.unwrap().forget();
          Arg::Null
        }
      })),
    )
    .unwrap();
  (methods, gate, runs)
}

#[tokio::test]
async fn answers_a_fast_call_while_a_slow_one_runs() {
  let (mut methods, gate, _) = slow();
  methods
    .register_server_method(
      "chat/fast",
      MethodEntry::call(Handler::new(0, |_| async { Arg::Text("fast".into()) })),
    )
    .unwrap();
  let mut socket = serving(Arc::new(methods)).await;
  socket
    .send(Message::binary(call_frame(2, "chat/slow", &[0x80])))
    .await
    .unwrap();
  socket
    .send(Message::binary(call_frame(4, "chat/fast", &[0x80])))
    .await
    .unwrap();
  assert_eq!(next_answer(&mut socket).await.stream, 4);
  gate.add_permits(1);
  assert_eq!(next_answer(&mut socket).await.stream, 2);
}

#[tokio::test]
async fn answers_internal_when_the_result_does_not_encode() {
  let mut methods = MethodRegistry::new();
  methods
    .register_server_method("chat/odd", MethodEntry::call(Handler::new(0, |_| async { Arg::Simple(25) })))
    .unwrap();
  let mut socket = serving(Arc::new(methods)).await;
  socket
    .send(Message::binary(call_frame(2, "chat/odd", &[0x80])))
    .await
    .unwrap();
  assert_error(&next_answer(&mut socket).await, 2, 13);
}

#[tokio::test]
async fn answers_internal_when_a_handler_panics_before_its_future_runs() {
  let mut methods = MethodRegistry::new();
  methods
    .register_server_method(
      "chat/early",
      MethodEntry::call(Handler::new(0, |_| -> std::future::Ready<Arg> {
        panic!("the handler fails before it returns a future")
      })),
    )
    .unwrap();
  methods
    .register_server_method("chat/ping", MethodEntry::call(Handler::new(0, |_| async { Arg::Null })))
    .unwrap();
  let mut socket = serving(Arc::new(methods)).await;
  socket
    .send(Message::binary(call_frame(2, "chat/early", &[0x80])))
    .await
    .unwrap();
  assert_error(&next_answer(&mut socket).await, 2, 13);
  socket
    .send(Message::binary(call_frame(4, "chat/ping", &[0x80])))
    .await
    .unwrap();
  assert_eq!(next_answer(&mut socket).await.frame_type, 0x13);
}

#[tokio::test]
async fn answers_calls_in_json_when_the_session_negotiated_it() {
  let (methods, _) = chat();
  let mut config = SessionConfig::default();
  config.codecs = vec![Codec::Json];
  let (address, _) = listen_serving(Registry::new(), methods, config, Arc::new(Permissive), Arc::new(AllowAll)).await;
  let mut socket = open(TcpStream::connect(address).await.unwrap()).await;
  socket.send(Message::binary(hello_frame_with(b"json"))).await.unwrap();
  next_welcome(&mut socket).await;
  socket
    .send(Message::binary(call_frame(2, "chat/send", br#"["general","hello"]"#)))
    .await
    .unwrap();
  let answer = next_answer(&mut socket).await;
  assert_eq!((answer.frame_type, answer.data), (0x13, br#""general:hello""#.to_vec()));
  socket.send(Message::binary(call_frame(4, "chat/ping", b"[]"))).await.unwrap();
  assert_eq!(next_answer(&mut socket).await.data, b"null".to_vec());
  socket
    .send(Message::binary(call_frame(6, "chat/send", br#""general""#)))
    .await
    .unwrap();
  assert_refused(&next_answer(&mut socket).await, 6, 3);
}

struct Dropped(Arc<AtomicBool>);

impl Drop for Dropped {
  fn drop(&mut self) {
    self.0.store(true, Ordering::SeqCst);
  }
}

fn forever() -> (MethodRegistry<Handler>, Arc<AtomicBool>, Arc<AtomicBool>) {
  let dropped = Arc::new(AtomicBool::new(false));
  let started = Arc::new(AtomicBool::new(false));
  let (flag, running) = (Arc::clone(&dropped), Arc::clone(&started));
  let mut methods = MethodRegistry::new();
  methods
    .register_server_method(
      "chat/forever",
      MethodEntry::call(Handler::new(0, move |_| {
        let guard = Dropped(Arc::clone(&flag));
        running.store(true, Ordering::SeqCst);
        async move {
          let _guard = guard;
          std::future::pending::<Arg>().await
        }
      })),
    )
    .unwrap();
  (methods, started, dropped)
}

#[tokio::test]
async fn drops_running_handlers_when_the_session_ends() {
  let (methods, started, dropped) = forever();
  let mut socket = serving(Arc::new(methods)).await;
  socket
    .send(Message::binary(call_frame(2, "chat/forever", &[0x80])))
    .await
    .unwrap();
  eventually(|| started.load(Ordering::SeqCst), "the handler starts").await;
  assert!(!dropped.load(Ordering::SeqCst));
  drop(socket);
  eventually(|| dropped.load(Ordering::SeqCst), "the handler is dropped with the session").await;
}

#[tokio::test]
async fn refuses_a_call_whose_shape_differs_from_the_method() {
  let mut methods = MethodRegistry::new();
  methods
    .register_server_method(
      "chat/history",
      MethodEntry::call(Handler::new(0, |_| async { Arg::Null })).with_shape(Shape::ServerStream),
    )
    .unwrap();
  let mut socket = serving(Arc::new(methods)).await;
  socket
    .send(Message::binary(call_frame(2, "chat/history", &[0x80])))
    .await
    .unwrap();
  assert_refused(&next_answer(&mut socket).await, 2, 3);
}

fn unary_call(method: &str, data: &'static [u8], meta: Option<Meta>) -> Call {
  Call {
    method: method.to_owned(),
    shape: 0,
    deadline: None,
    meta,
    data: Bytes::from_static(data),
  }
}

fn refusal(call: &Call, idempotency: bool) -> Option<Refusal> {
  let (methods, _) = chat();
  validate_call(call, &methods, Codec::Cbor, idempotency).err()
}

#[test]
fn stops_at_the_first_failing_check() {
  let garbage = &[0xff];
  assert_eq!(
    refusal(&unary_call("nope/nope", garbage, None), false),
    Some(Refusal::new(StatusCode::Unimplemented, "unknown method"))
  );
  let mut mismatched = unary_call("chat/send", garbage, None);
  mismatched.shape = 1;
  assert_eq!(
    refusal(&mismatched, false),
    Some(Refusal::new(StatusCode::InvalidArgument, "shape does not match the method"))
  );
  assert_eq!(
    refusal(&unary_call("chat/typing", garbage, None), false),
    Some(Refusal::new(StatusCode::InvalidArgument, "method takes NOTIFY, not CALL"))
  );
  assert_eq!(
    refusal(&unary_call("chat/send", garbage, None), false),
    Some(Refusal::new(StatusCode::InvalidArgument, "arguments do not decode"))
  );
  assert_eq!(
    refusal(&unary_call("chat/send", &[0x81, 0x61, b'a'], None), false),
    Some(Refusal::new(StatusCode::InvalidArgument, "expected 2 arguments, got 1"))
  );
  assert_eq!(refusal(&unary_call("chat/send", GENERAL_HELLO, None), false), None);
}

fn with_key(length: usize, bytes: bool) -> Option<Meta> {
  let value = if bytes {
    MetaValue::Bytes(Bytes::from(vec![b'k'; length]))
  } else {
    MetaValue::Text("k".repeat(length))
  };
  let name = MetaName::try_from("pavia-idempotency-key").unwrap();
  Some([(name, value)].into_iter().collect())
}

#[test]
fn bounds_the_idempotency_key_only_with_the_capability() {
  for bytes in [false, true] {
    let long = unary_call("chat/send", GENERAL_HELLO, with_key(129, bytes));
    assert_eq!(
      refusal(&long, true),
      Some(Refusal::new(StatusCode::InvalidArgument, "idempotency key above 128 bytes"))
    );
    assert_eq!(refusal(&long, false), None);
    assert_eq!(
      refusal(&unary_call("chat/send", GENERAL_HELLO, with_key(128, bytes)), true),
      None
    );
  }
  let other = MetaName::try_from("traceparent").unwrap();
  let meta = [(other, MetaValue::Text("t".repeat(200)))].into_iter().collect();
  assert_eq!(refusal(&unary_call("chat/send", GENERAL_HELLO, Some(meta)), true), None);
}

#[tokio::test]
async fn refuses_a_call_beyond_max_calls_before_looking_at_it() {
  let (methods, gate, _) = slow();
  let mut config = SessionConfig::default();
  config.max_calls = 1;
  let mut socket = serving_with(Arc::new(methods), config).await;
  socket
    .send(Message::binary(call_frame(2, "chat/slow", &[0x80])))
    .await
    .unwrap();
  socket
    .send(Message::binary(call_frame(4, "nope/nope", &[0xff])))
    .await
    .unwrap();
  assert_refused(&next_answer(&mut socket).await, 4, 8);
  gate.add_permits(1);
  assert_eq!(next_answer(&mut socket).await.stream, 2);
}

#[tokio::test]
async fn refuses_an_unknown_shape_after_the_method_lookup() {
  let (methods, runs) = chat();
  let mut socket = serving(methods).await;
  socket
    .send(Message::binary(call_frame_shaped(2, "nope/nope", 7, &[0x80])))
    .await
    .unwrap();
  assert_refused(&next_answer(&mut socket).await, 2, 12);
  socket
    .send(Message::binary(call_frame_shaped(4, "chat/send", 7, GENERAL_HELLO)))
    .await
    .unwrap();
  assert_refused(&next_answer(&mut socket).await, 4, 3);
  assert_eq!(runs.load(Ordering::SeqCst), 0);
}

#[derive(Clone)]
struct DenyAll;

impl Authorizer for DenyAll {
  async fn authorize(&self, _: &Principal, _: &MethodName) -> bool {
    false
  }
}

#[tokio::test]
async fn answers_a_denied_call_by_whether_the_session_is_anonymous() {
  for (authenticator, hello, code) in [
    (Recording::answering(Some(Ok(Principal::anonymous()))), hello_frame(), 16),
    (Recording::answering(Some(Ok(Principal::for_user("u-1")))), bearer_hello(), 7),
  ] {
    let (methods, runs) = chat();
    let (address, _) = listen_serving(
      Registry::new(),
      methods,
      SessionConfig::default(),
      authenticator,
      Arc::new(DenyAll),
    )
    .await;
    let mut socket = open(TcpStream::connect(address).await.unwrap()).await;
    socket.send(Message::binary(hello)).await.unwrap();
    next_welcome(&mut socket).await;
    socket
      .send(Message::binary(call_frame(2, "chat/send", GENERAL_HELLO)))
      .await
      .unwrap();
    assert_refused(&next_answer(&mut socket).await, 2, code);
    assert_eq!(runs.load(Ordering::SeqCst), 0, "{code}");
  }
}

fn put_varint(out: &mut Vec<u8>, value: usize) {
  match value {
    0..=63 => out.push(u8::try_from(value).unwrap()),
    64..=16_383 => out.extend((u16::try_from(value).unwrap() | 0x4000).to_be_bytes()),
    _ => out.extend((u32::try_from(value).unwrap() | 0x8000_0000).to_be_bytes()),
  }
}

fn call_with_header(stream: u8, header: &[u8], data: &[u8]) -> Vec<u8> {
  let mut payload = Vec::new();
  put_varint(&mut payload, header.len());
  payload.extend(header);
  payload.extend(data);
  let mut frame = vec![stream, 0x10, 0x00];
  put_varint(&mut frame, payload.len());
  frame.extend(payload);
  frame
}

#[tokio::test]
async fn runs_a_call_with_a_long_idempotency_key_when_idempotency_is_not_granted() {
  let (methods, runs) = chat();
  let mut socket = serving(methods).await;
  let mut header = vec![0xa3, 0x02, 0x69];
  header.extend(b"chat/send");
  header.extend([0x03, 0xa1, 0x75]);
  header.extend(b"pavia-idempotency-key");
  header.extend([0x78, 129]);
  header.extend([b'k'; 129]);
  header.extend([0x05, 0x00]);
  socket
    .send(Message::binary(call_with_header(2, &header, GENERAL_HELLO)))
    .await
    .unwrap();
  let answer = next_answer(&mut socket).await;
  assert_eq!((answer.stream, answer.frame_type), (2, 0x13));
  assert_eq!(runs.load(Ordering::SeqCst), 1);
}

type Asked = (Option<String>, String);

#[derive(Clone, Default)]
struct OnlyUserOne(Arc<Mutex<Vec<Asked>>>);

impl Authorizer for OnlyUserOne {
  async fn authorize(&self, principal: &Principal, method: &MethodName) -> bool {
    self
      .0
      .lock()
      .unwrap()
      .push((principal.user().map(str::to_owned), method.as_str().to_owned()));
    principal.user() == Some("u-1") && method.as_str() == "chat/send"
  }
}

#[tokio::test]
async fn hands_the_authorizer_the_session_principal_and_the_method() {
  let authorizer = OnlyUserOne::default();
  let (methods, runs) = chat();
  let authenticator = Recording::answering(Some(Ok(Principal::for_user("u-1"))));
  let (address, _) = listen_serving(
    Registry::new(),
    methods,
    SessionConfig::default(),
    authenticator,
    Arc::new(authorizer.clone()),
  )
  .await;
  let mut socket = open(TcpStream::connect(address).await.unwrap()).await;
  socket.send(Message::binary(bearer_hello())).await.unwrap();
  next_welcome(&mut socket).await;
  socket
    .send(Message::binary(call_frame(2, "chat/send", GENERAL_HELLO)))
    .await
    .unwrap();
  assert_eq!(next_answer(&mut socket).await.frame_type, 0x13);
  socket
    .send(Message::binary(call_frame(4, "chat/ping", &[0x80])))
    .await
    .unwrap();
  assert_refused(&next_answer(&mut socket).await, 4, 7);
  assert_eq!(runs.load(Ordering::SeqCst), 1);
  assert_eq!(
    *authorizer.0.lock().unwrap(),
    [
      (Some("u-1".to_owned()), "chat/send".to_owned()),
      (Some("u-1".to_owned()), "chat/ping".to_owned())
    ]
  );
}

#[test]
fn refuses_a_matching_streaming_method_until_streams_are_served() {
  let mut methods = MethodRegistry::new();
  methods
    .register_server_method(
      "chat/history",
      MethodEntry::call(Handler::new(0, |_| async { Arg::Null })).with_shape(Shape::ServerStream),
    )
    .unwrap();
  let mut call = unary_call("chat/history", &[0x80], None);
  call.shape = 1;
  assert_eq!(
    validate_call(&call, &methods, Codec::Cbor, false).err(),
    Some(Refusal::new(StatusCode::Unimplemented, "streaming calls are not served yet"))
  );
}

#[derive(Default)]
struct FirstCallOnly(AtomicUsize);

impl Authorizer for FirstCallOnly {
  async fn authorize(&self, _: &Principal, _: &MethodName) -> bool {
    self.0.fetch_add(1, Ordering::SeqCst) == 0
  }
}

#[tokio::test]
async fn shares_one_authorizer_between_the_calls_of_a_session() {
  let (methods, runs) = chat();
  let authorizer = Arc::new(FirstCallOnly::default());
  let (address, _) = listen_serving(
    Registry::new(),
    methods,
    SessionConfig::default(),
    Arc::new(Permissive),
    Arc::clone(&authorizer),
  )
  .await;
  let mut socket = open(TcpStream::connect(address).await.unwrap()).await;
  established(&mut socket).await;
  socket
    .send(Message::binary(call_frame(2, "chat/send", GENERAL_HELLO)))
    .await
    .unwrap();
  assert_eq!(next_answer(&mut socket).await.frame_type, 0x13);
  socket
    .send(Message::binary(call_frame(4, "chat/send", GENERAL_HELLO)))
    .await
    .unwrap();
  assert_refused(&next_answer(&mut socket).await, 4, 16);
  assert_eq!((runs.load(Ordering::SeqCst), authorizer.0.load(Ordering::SeqCst)), (1, 2));
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
  fn text(&self) -> String {
    String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
  }
}

impl io::Write for Captured {
  fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
    self.0.lock().unwrap().extend_from_slice(bytes);
    Ok(bytes.len())
  }

  fn flush(&mut self) -> io::Result<()> {
    Ok(())
  }
}

fn failing() -> Arc<MethodRegistry<Handler>> {
  let mut methods = MethodRegistry::new();
  methods
    .register_server_method(
      "users/get",
      MethodEntry::call(Handler::fallible(0, |_| async {
        let not_found = ErrorStatus::try_from(StatusCode::NotFound).unwrap();
        let detail = Arg::Map(vec![(Arg::Text("id".into()), Arg::Text("u1".into()))]);
        let reason = [(MetaName::try_from("x-reason").unwrap(), MetaValue::Text("quota".into()))];
        Err(
          StatusError::new(not_found, "no such user")
            .with_detail(detail)
            .with_meta(reason.into_iter().collect())
            .into(),
        )
      })),
    )
    .unwrap();
  methods
    .register_server_method(
      "users/save",
      MethodEntry::call(Handler::fallible(0, |_| async {
        Err(HandlerError::unexpected("db password=hunter2"))
      })),
    )
    .unwrap();
  methods
    .register_server_method(
      "users/load",
      MethodEntry::call(Handler::fallible(0, |_| async {
        Err(HandlerError::unexpected(LoadFailed(io::Error::other("disk full"))))
      })),
    )
    .unwrap();
  methods
    .register_server_method(
      "users/dump",
      MethodEntry::call(Handler::fallible(0, |_| async {
        Err(HandlerError::unexpected(format!("x{}", "é".repeat(600))))
      })),
    )
    .unwrap();
  methods
    .register_server_method(
      "users/odd",
      MethodEntry::call(Handler::fallible(0, |_| async {
        let invalid = ErrorStatus::try_from(StatusCode::InvalidArgument).unwrap();
        Err(StatusError::new(invalid, "odd detail").with_detail(Arg::Simple(25)).into())
      })),
    )
    .unwrap();
  Arc::new(methods)
}

#[derive(Debug, thiserror::Error)]
#[error("loading the user failed")]
struct LoadFailed(#[source] io::Error);

/// Captures this thread's log until the guard drops.
///
/// tracing-core asks only the current thread's subscriber for a new callsite's
/// interest while one dispatcher is registered, so a test without a subscriber
/// that reaches a callsite first would disable it for every test. The idle
/// dispatcher kept in `OTHER` makes it ask every registered subscriber instead.
fn capture_log() -> (Captured, DefaultGuard) {
  static OTHER: OnceLock<Dispatch> = OnceLock::new();
  OTHER.get_or_init(|| Dispatch::new(NoSubscriber::default()));
  let log = Captured::default();
  let writer = log.clone();
  let subscriber = tracing_subscriber::fmt()
    .with_ansi(false)
    .with_writer(move || writer.clone())
    .finish();
  (log, tracing::subscriber::set_default(subscriber))
}

async fn logged_answer(method: &str) -> (Answer, String) {
  let (log, _logging) = capture_log();
  let mut socket = serving(failing()).await;
  socket.send(Message::binary(call_frame(2, method, &[0x80]))).await.unwrap();
  let answer = next_answer(&mut socket).await;
  let logged = log.text();
  (answer, logged)
}

fn assert_hidden(answer: &Answer) {
  assert_eq!((answer.stream, answer.frame_type), (2, 0x14));
  assert_eq!(
    answer.header,
    BTreeMap::from([(6, Value::from(13)), (7, Value::Text("internal error".into()))])
  );
  assert!(answer.data.is_empty());
}

#[tokio::test]
async fn sends_a_deliberate_error_as_given_with_its_detail_and_meta() {
  let mut socket = serving(failing()).await;
  socket
    .send(Message::binary(call_frame(2, "users/get", &[0x80])))
    .await
    .unwrap();
  let answer = next_answer(&mut socket).await;
  assert_error(&answer, 2, 5);
  assert_eq!(answer.header[&7], Value::Text("no such user".into()));
  let meta = answer.header[&3].as_map().unwrap();
  assert_eq!(meta, &vec![(Value::Text("x-reason".into()), Value::Text("quota".into()))]);
  assert_eq!(answer.data, [0xa1, 0x62, b'i', b'd', 0x62, b'u', b'1']);
}

#[tokio::test]
async fn hides_an_unexpected_error_from_the_client_and_logs_it() {
  let (answer, logged) = logged_answer("users/save").await;
  assert_hidden(&answer);
  for expected in ["ERROR", "stream=2", "method=\"users/save\"", "db password=hunter2"] {
    assert!(logged.contains(expected), "{expected} in {logged}");
  }
}

#[tokio::test]
async fn logs_the_source_chain_of_an_unexpected_error() {
  let (answer, logged) = logged_answer("users/load").await;
  assert_hidden(&answer);
  assert!(logged.contains("loading the user failed"), "{logged}");
  assert!(logged.contains("disk full"), "{logged}");
}

#[tokio::test]
async fn answers_internal_when_a_deliberate_detail_does_not_encode() {
  let (answer, logged) = logged_answer("users/odd").await;
  assert_hidden(&answer);
  assert!(logged.contains("method=\"users/odd\""), "{logged}");
}

#[tokio::test]
async fn shows_an_unexpected_error_in_development_mode() {
  let mut config = SessionConfig::default();
  config.disclose_internal_errors = true;
  let mut socket = serving_with(failing(), config).await;
  socket
    .send(Message::binary(call_frame(2, "users/save", &[0x80])))
    .await
    .unwrap();
  let answer = next_answer(&mut socket).await;
  assert_eq!(answer.header[&6], Value::from(13));
  assert_eq!(answer.header[&7], Value::Text("db password=hunter2".into()));
}

#[tokio::test]
async fn cuts_a_long_disclosed_error_on_a_character_boundary() {
  let mut config = SessionConfig::default();
  config.disclose_internal_errors = true;
  let mut socket = serving_with(failing(), config).await;
  socket
    .send(Message::binary(call_frame(2, "users/dump", &[0x80])))
    .await
    .unwrap();
  let answer = next_answer(&mut socket).await;
  assert_eq!(answer.header[&6], Value::from(13));
  assert_eq!(answer.header[&7], Value::Text(format!("x{}", "é".repeat(511))));
}

#[tokio::test]
async fn sends_the_configured_generic_message_for_a_panic() {
  let (methods, _) = chat();
  let mut config = SessionConfig::default();
  config.internal_error_message = "something went wrong".into();
  let mut socket = serving_with(methods, config).await;
  socket
    .send(Message::binary(call_frame(2, "chat/crash", &[0x80])))
    .await
    .unwrap();
  let answer = next_answer(&mut socket).await;
  assert_eq!(answer.header[&7], Value::Text("something went wrong".into()));
}

#[tokio::test]
async fn runs_two_slow_calls_at_the_same_time() {
  let barrier = Arc::new(Barrier::new(2));
  let mut methods = MethodRegistry::new();
  methods
    .register_server_method(
      "chat/meet",
      MethodEntry::call(Handler::new(0, move |_| {
        let barrier = Arc::clone(&barrier);
        async move {
          barrier.wait().await;
          Arg::Null
        }
      })),
    )
    .unwrap();
  let mut socket = serving(Arc::new(methods)).await;
  for stream in [2, 4] {
    socket
      .send(Message::binary(call_frame(stream, "chat/meet", &[0x80])))
      .await
      .unwrap();
  }
  let mut answered = [next_answer(&mut socket).await, next_answer(&mut socket).await].map(|answer| {
    assert_eq!(answer.frame_type, 0x13);
    answer.stream
  });
  answered.sort_unstable();
  assert_eq!(answered, [2, 4]);
}

#[tokio::test]
async fn refuses_a_third_call_beyond_max_calls_two_and_admits_a_fourth_once_a_slot_frees() {
  let (methods, gate, runs) = slow();
  let mut config = SessionConfig::default();
  config.max_calls = 2;
  let mut socket = serving_with(Arc::new(methods), config).await;
  for stream in [2, 4, 6] {
    socket
      .send(Message::binary(call_frame(stream, "chat/slow", &[0x80])))
      .await
      .unwrap();
  }
  assert_refused(&next_answer(&mut socket).await, 6, 8);
  eventually(|| runs.load(Ordering::SeqCst) == 2, "both admitted calls to run").await;
  gate.add_permits(1);
  let first = next_answer(&mut socket).await;
  assert_eq!(first.frame_type, 0x13);
  socket
    .send(Message::binary(call_frame(8, "chat/slow", &[0x80])))
    .await
    .unwrap();
  eventually(|| runs.load(Ordering::SeqCst) == 3, "the call after the freed slot to run").await;
  gate.add_permits(2);
  let mut answered = vec![first.stream];
  for _ in 0..2 {
    let answer = next_answer(&mut socket).await;
    assert_eq!(answer.frame_type, 0x13);
    answered.push(answer.stream);
  }
  answered.sort_unstable();
  assert_eq!(answered, [2, 4, 8]);
  assert_eq!(runs.load(Ordering::SeqCst), 3);
}

fn cancel_frame(stream: u8, msg: Option<&str>) -> Vec<u8> {
  let header = msg.map_or_else(Vec::new, |msg| {
    let mut header = vec![0xa1, 0x07, 0x60 + u8::try_from(msg.len()).unwrap()];
    header.extend(msg.as_bytes());
    header
  });
  raw_frame_on(stream, 0x15, &header)
}

#[tokio::test]
async fn answers_cancelled_once_the_handler_of_a_cancelled_call_is_dropped() {
  for msg in [None, Some("user left")] {
    let (log, _logging) = capture_log();
    let (methods, started, dropped) = forever();
    let mut socket = serving(Arc::new(methods)).await;
    socket
      .send(Message::binary(call_frame(2, "chat/forever", &[0x80])))
      .await
      .unwrap();
    eventually(|| started.load(Ordering::SeqCst), "the handler starts").await;
    socket.send(Message::binary(cancel_frame(2, msg))).await.unwrap();
    let answer = next_answer(&mut socket).await;
    assert!(dropped.load(Ordering::SeqCst), "answered before the handler was dropped");
    assert_error(&answer, 2, 1);
    assert_eq!(answer.header[&7], Value::Text("cancelled".into()));
    let logged = log.text();
    assert!(!logged.contains("ERROR"), "{logged}");
  }
}

#[tokio::test]
async fn admits_a_call_after_a_cancelled_one_with_max_calls_one() {
  let (methods, gate, runs) = slow();
  let mut config = SessionConfig::default();
  config.max_calls = 1;
  let mut socket = serving_with(Arc::new(methods), config).await;
  socket
    .send(Message::binary(call_frame(2, "chat/slow", &[0x80])))
    .await
    .unwrap();
  eventually(|| runs.load(Ordering::SeqCst) == 1, "the first call to run").await;
  socket.send(Message::binary(cancel_frame(2, None))).await.unwrap();
  assert_error(&next_answer(&mut socket).await, 2, 1);
  socket
    .send(Message::binary(call_frame(4, "chat/slow", &[0x80])))
    .await
    .unwrap();
  eventually(|| runs.load(Ordering::SeqCst) == 2, "the call after the cancelled one to run").await;
  gate.add_permits(1);
  let answer = next_answer(&mut socket).await;
  assert_eq!((answer.stream, answer.frame_type), (4, 0x13));
}

#[tokio::test]
async fn answers_a_refused_call_cancelled_in_the_same_message_once() {
  let (methods, started, _) = forever();
  let mut socket = serving(Arc::new(methods)).await;
  let mut message = call_frame(2, "chat/missing", &[0x80]);
  message.extend(cancel_frame(2, None));
  socket.send(Message::binary(message)).await.unwrap();
  assert_refused(&next_answer(&mut socket).await, 2, 12);
  socket
    .send(Message::binary(call_frame(4, "chat/forever", &[0x80])))
    .await
    .unwrap();
  eventually(|| started.load(Ordering::SeqCst), "the next call to run").await;
  socket.send(Message::binary(cancel_frame(4, None))).await.unwrap();
  assert_error(&next_answer(&mut socket).await, 4, 1);
}

async fn cancelled_after_it_finished(outcome: Result<Arg, Failed>) -> Command {
  let methods = MethodRegistry::new();
  let mut calls = Calls {
    methods: &methods,
    authorizer: Arc::new(AllowAll),
    principal: Arc::new(Principal::anonymous()),
    disclosure: Disclosure {
      generic: "internal error".to_owned(),
      disclose: false,
    },
    running: JoinSet::new(),
    streams: HashMap::new(),
    tasks: HashMap::new(),
  };
  let task = calls.running.spawn(async move { outcome });
  let running = RunningCall {
    stream: 2,
    method: MethodName::parse("chat/done").unwrap(),
    cancelled: false,
  };
  calls.streams.insert(task.id(), running);
  calls.tasks.insert(2, task.clone());
  eventually(|| task.is_finished(), "the handler to finish").await;
  calls.cancel(2);
  let joined = calls.running.join_next_with_id().await.unwrap();
  let Input::Command(command) = calls.answered(joined, Codec::Cbor) else {
    panic!("expected a command");
  };
  assert!(calls.streams.is_empty() && calls.tasks.is_empty());
  command
}

#[tokio::test]
async fn answers_cancelled_for_a_handler_that_finished_before_the_cancel() {
  let (log, _logging) = capture_log();
  let cancelled = |command: &Command| {
    matches!(
      command,
      Command::Error {
        stream: 2,
        code: StatusCode::Cancelled,
        nx: false,
        ..
      }
    )
  };
  assert!(cancelled(&cancelled_after_it_finished(Ok(Arg::Null)).await));
  assert!(!log.text().contains("ERROR"));
  let failed = Failed::Handler(HandlerError::unexpected("disk full"));
  assert!(cancelled(&cancelled_after_it_finished(Err(failed)).await));
  assert!(log.text().contains("call failed unexpectedly"), "{}", log.text());
}
