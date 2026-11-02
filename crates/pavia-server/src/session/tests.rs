use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ciborium::Value;
use futures_util::{SinkExt, StreamExt};
use hyper::header::{self, HeaderValue};
use pavia_proto::frame::Auth;
use pavia_proto::registry::CloseCode;
use pavia_proto::session::{Command, Config as SessionConfig, Ending};
use pavia_proto::stream::Side;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode as WsCloseCode;
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};
use tokio_tungstenite::{WebSocketStream, client_async};

use super::run;
use crate::auth::{AuthFailure, Authenticator, Permissive, Principal, RequestContext};
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

async fn listen(command: Option<Command>, config: SessionConfig) -> (SocketAddr, oneshot::Receiver<Ending>) {
  listen_with(command, config, Arc::new(Permissive)).await
}

async fn listen_with<A: Authenticator + 'static>(
  command: Option<Command>,
  config: SessionConfig,
  authenticator: Arc<A>,
) -> (SocketAddr, oneshot::Receiver<Ending>) {
  let config = config.validate().unwrap();
  let (ended, ending) = oneshot::channel();
  let ended = Arc::new(Mutex::new(Some(ended)));
  let endpoint = Endpoint::new(Config::new("/rt"), move |transport: Transport| {
    let ended = ended.lock().unwrap().take();
    let command = command.clone();
    let config = config.clone();
    let authenticator = Arc::clone(&authenticator);
    async move {
      let (commands, receiver) = mpsc::channel(1);
      if let Some(command) = command {
        commands.send(command).await.unwrap();
      }
      drop(commands);
      let ending = run(transport, receiver, &config, &*authenticator).await;
      if let Some(ended) = ended {
        let _ = ended.send(ending);
      }
    }
  });
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  tokio::spawn(endpoint.serve(listener));
  (address, ending)
}

async fn connect_with(command: Option<Command>, config: SessionConfig) -> (Socket, oneshot::Receiver<Ending>) {
  let (address, ending) = listen(command, config).await;
  let mut request = "ws://localhost/rt".into_client_request().unwrap();
  request
    .headers_mut()
    .insert(header::SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static(SUBPROTOCOL));
  let (socket, _) = client_async(request, TcpStream::connect(address).await.unwrap())
    .await
    .unwrap();
  (socket, ending)
}

async fn connect(command: Option<Command>) -> (Socket, oneshot::Receiver<Ending>) {
  connect_with(command, SessionConfig::default()).await
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

#[tokio::test]
async fn sends_close_then_closes_with_every_code() {
  for code in CODES {
    let (mut socket, ending) = connect(Some(Command::Close { code, msg: None })).await;
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
  let (mut socket, _) = connect(Some(Command::Close {
    code: CloseCode::GoingAway,
    msg: Some(msg),
  }))
  .await;
  let (_, _, reason) = closed(&mut socket).await;
  assert_eq!(reason, "€".repeat(41));
}

#[tokio::test]
async fn mirrors_a_client_close() {
  let (mut socket, ending) = connect(None).await;
  socket.send(Message::binary(hello_frame())).await.unwrap();
  socket.send(Message::binary(close_frame(0))).await.unwrap();
  let (binary, ws_code, _) = closed(&mut socket).await;
  assert_eq!(binary.iter().map(|message| welcome(message).len()).collect::<Vec<_>>(), [7]);
  assert_eq!(ws_code, WsCloseCode::from(4000));
  assert_eq!(
    ending.await.unwrap(),
    Ending::Closed {
      code: CloseCode::Normal,
      msg: None,
      by: Side::Client
    }
  );
}

#[tokio::test]
async fn records_an_unknown_client_code_as_internal() {
  let (mut socket, ending) = connect(None).await;
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
  let (socket, ending) = connect(None).await;
  drop(socket);
  assert_eq!(ending.await.unwrap(), Ending::TransportLost);
}

#[tokio::test]
async fn ends_on_a_websocket_close_without_close_frame() {
  let (mut socket, ending) = connect(None).await;
  socket.close(None).await.unwrap();
  assert_eq!(ending.await.unwrap(), Ending::TransportLost);
}

#[tokio::test]
async fn closes_with_protocol_error_on_a_malformed_message() {
  let (mut socket, ending) = connect(None).await;
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
  let (mut socket, ending) = connect(None).await;
  socket.send(Message::binary(hello_frame())).await.unwrap();
  socket.send(Message::binary(close_frame(0))).await.unwrap();
  let (binary, ws_code, _) = closed(&mut socket).await;
  assert_eq!(binary.iter().map(|message| welcome(message).len()).collect::<Vec<_>>(), [7]);
  assert_eq!(ws_code, WsCloseCode::from(4000));
  assert!(matches!(
    ending.await.unwrap(),
    Ending::Closed {
      code: CloseCode::Normal,
      by: Side::Client,
      ..
    }
  ));
}

#[tokio::test]
async fn closes_with_protocol_error_on_a_text_message() {
  for text in ["hello", ""] {
    let (mut socket, ending) = connect(None).await;
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
  let (mut socket, ending) = connect_with(None, short_timeout()).await;
  let (binary, ws_code, _) = closed(&mut socket).await;
  assert!(started.elapsed() >= Duration::from_millis(200));
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

#[tokio::test]
async fn sends_welcome_and_stops_the_handshake_timer() {
  let (mut socket, ending) = connect_with(None, short_timeout()).await;
  socket.send(Message::binary(hello_frame())).await.unwrap();
  let Message::Binary(message) = socket.next().await.unwrap().unwrap() else {
    panic!("expected WELCOME");
  };
  let welcome = welcome(&message);
  let keys: Vec<u64> = welcome.keys().copied().collect();
  assert_eq!(keys, [15, 17, 18, 19, 21, 22, 23]);
  assert_eq!(welcome[&15], Value::Integer(1.into()));
  assert_eq!(welcome[&17], Value::Text("cbor".into()));
  assert_eq!(welcome[&18], Value::Array(vec![Value::Text("calls".into())]));
  let Value::Text(id) = &welcome[&19] else {
    panic!("{welcome:?}");
  };
  assert!(pavia_proto::session_id::SessionId::parse(id).is_ok(), "{id}");
  assert!(tokio::time::timeout(Duration::from_millis(400), socket.next()).await.is_err());
  socket.send(Message::binary(close_frame(0))).await.unwrap();
  let (binary, ws_code, _) = closed(&mut socket).await;
  assert!(binary.is_empty());
  assert_eq!(ws_code, WsCloseCode::from(4000));
  assert!(matches!(ending.await.unwrap(), Ending::Closed { by: Side::Client, .. }));
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
  let (address, ending) = listen(None, short_timeout()).await;
  let mut stream = TcpStream::connect(address).await.unwrap();
  let request = concat!(
    "GET /rt HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n",
    "Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
    "Sec-WebSocket-Protocol: pavia.1\r\n\r\n",
  );
  stream.write_all(request.as_bytes()).await.unwrap();
  let head = tokio::time::timeout(Duration::from_secs(5), async {
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
  let read = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut rest)).await;
  assert!(read.is_ok(), "the server never closed the connection");
  let close_4012 = rest.windows(4).any(|window| window[0] == 0x88 && window[2..] == [0x0f, 0xac]);
  assert!(close_4012, "{rest:02x?}");
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
  let (mut socket, ending) = connect(None).await;
  socket.send(Message::binary(&b"\x00\x04\x00\x01\x00"[..])).await.unwrap();
  let (binary, ws_code, _) = closed(&mut socket).await;
  assert_eq!(binary, [close_frame(1)]);
  assert_eq!(ws_code, WsCloseCode::from(4001));
  assert!(matches!(
    ending.await.unwrap(),
    Ending::Closed {
      code: CloseCode::ProtocolError,
      ..
    }
  ));
}

fn raw_frame(frame_type: u8, header: &[u8]) -> Vec<u8> {
  let mut frame = vec![0x00, frame_type, 0x00, header.len() as u8 + 1, header.len() as u8];
  frame.extend(header);
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
    let (mut socket, ending) = connect(None).await;
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
  let (mut socket, ending) = connect(None).await;
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

async fn connect_to(authenticator: Arc<Recording>, config: SessionConfig) -> (Socket, SocketAddr, oneshot::Receiver<Ending>) {
  let (address, ending) = listen_with(None, config, authenticator).await;
  let mut request = "ws://localhost/rt".into_client_request().unwrap();
  let headers = request.headers_mut();
  headers.insert(header::SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static(SUBPROTOCOL));
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
    let (mut socket, _, ending) = connect_to(Recording::answering(Some(Err(failure))), SessionConfig::default()).await;
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
  let (mut socket, local, ending) = connect_to(Arc::clone(&authenticator), SessionConfig::default()).await;
  socket.send(Message::binary(bearer_hello())).await.unwrap();
  socket.send(Message::binary(close_frame(0))).await.unwrap();
  let (binary, ws_code, _) = closed(&mut socket).await;
  let [welcome] = binary.as_slice() else {
    panic!("{binary:02x?}");
  };
  assert_eq!(self::welcome(welcome).get(&36), Some(&Value::Text("u-42".into())));
  assert_eq!(ws_code, WsCloseCode::from(4000));
  assert!(matches!(ending.await.unwrap(), Ending::Closed { by: Side::Client, .. }));
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
  let (mut socket, _, ending) = connect_to(Arc::clone(&authenticator), SessionConfig::default()).await;
  socket.send(Message::binary(bearer_hello())).await.unwrap();
  socket.send(Message::binary(close_frame(0))).await.unwrap();
  let (_, ws_code, _) = closed(&mut socket).await;
  assert_eq!(ws_code, WsCloseCode::from(4000));
  assert!(matches!(ending.await.unwrap(), Ending::Closed { by: Side::Client, .. }));
  assert_eq!(authenticator.seen().len(), 1);
}

#[tokio::test]
async fn closes_a_slow_authenticator_with_handshake_timeout() {
  let (mut socket, _, ending) = connect_to(Recording::answering(None), short_timeout()).await;
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
