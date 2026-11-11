use std::collections::BTreeMap;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use ciborium::Value;
use futures_util::{SinkExt, StreamExt};
use hyper::header::{self, HeaderValue};
use pavia_proto::frame::Auth;
use pavia_proto::registry::CloseCode;
use pavia_proto::session::{Command, Config as SessionConfig, Ending, Input};
use pavia_proto::session_id::SessionId;
use pavia_proto::stream::Side;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::protocol::frame::Frame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode as WsCloseCode;
use tokio_tungstenite::tungstenite::protocol::frame::coding::{Data, OpCode};
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};
use tokio_tungstenite::{WebSocketStream, client_async};

use super::{authenticated, run};
use crate::auth::{AuthFailure, Authenticator, Permissive, Principal, RequestContext};
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
  let config = config.validate().unwrap();
  let (ended, ending) = oneshot::channel();
  let ended = Arc::new(Mutex::new(Some(ended)));
  let endpoint = Endpoint::new(Config::new("/rt", &config), move |transport: Transport| {
    let ended = ended.lock().unwrap().take();
    let registry = registry.clone();
    let authenticator = Arc::clone(&authenticator);
    async move {
      let ending = run(transport, &registry, &*authenticator).await;
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
  let header = [0xa2, 0x0e, 0x81, 0x01, 0x10, 0x81, 0x64, b'c', b'b', b'o', b'r'];
  let mut frame = vec![0x00, 0x01, 0x00, header.len() as u8 + 1, header.len() as u8];
  frame.extend(header);
  frame
}

fn close_frame(code: u8) -> Vec<u8> {
  let mut header = vec![0xa1, 0x06];
  if code >= 24 {
    header.push(0x18);
  }
  header.push(code);
  let mut frame = vec![0x00, 0x07, 0x00, header.len() as u8 + 1, header.len() as u8];
  frame.extend(header);
  frame
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
  let header: Value = ciborium::from_reader(&rest[..header as usize]).unwrap();
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
  let mut frame = vec![0x00, frame_type, 0x00, header.len() as u8 + 1, header.len() as u8];
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
      let _ = ended.send(run(transport, &registry, &Permissive).await);
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
      let ending = run(transport, &registry, &*authenticator).await;
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
