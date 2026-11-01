use std::net::SocketAddr;
use std::pin::Pin;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use hyper::StatusCode;
use hyper::header::{self, HeaderValue};
use pavia_proto::registry::CloseCode;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Response;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode as WsCloseCode;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{WebSocketStream, client_async};

use super::{Config, Endpoint, Received, SUBPROTOCOL, Transport, is_key};

const PATH: &str = "/rt";

async fn session(mut transport: Transport) {
  while let Some(Ok(received)) = transport.recv().await {
    let sent = match received {
      Received::Binary(message) if message == "close" => {
        let _ = transport.close(CloseCode::ProtocolError, "bye").await;
        return;
      }
      Received::Binary(message) if message == "long" => {
        let _ = transport.close(CloseCode::Internal, &"é".repeat(100)).await;
        return;
      }
      Received::Binary(message) => transport.send(message).await,
      Received::Text => transport.send(Bytes::from_static(b"text")).await,
    };
    if sent.is_err() {
      return;
    }
  }
}

type Session = Pin<Box<dyn Future<Output = ()> + Send>>;

fn endpoint() -> Endpoint<fn(Transport) -> Session> {
  Endpoint::new(Config::new(PATH), |transport| Box::pin(session(transport)))
}

async fn listen() -> SocketAddr {
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  tokio::spawn(endpoint().serve(listener));
  address
}

async fn upgrade<S>(io: S, path: &str, offers: &[&str]) -> Result<(WebSocketStream<S>, Response), tungstenite::Error>
where
  S: AsyncRead + AsyncWrite + Unpin,
{
  let mut request = format!("ws://localhost{path}").into_client_request().unwrap();
  for offer in offers {
    request
      .headers_mut()
      .append(header::SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_str(offer).unwrap());
  }
  client_async(request, io).await
}

async fn upgrade_tcp(offers: &[&str]) -> Result<(WebSocketStream<TcpStream>, Response), tungstenite::Error> {
  let stream = TcpStream::connect(listen().await).await.unwrap();
  upgrade(stream, PATH, offers).await
}

fn refused(result: Result<(WebSocketStream<TcpStream>, Response), tungstenite::Error>) -> StatusCode {
  match result {
    Err(tungstenite::Error::Http(response)) => response.status(),
    Err(error) => panic!("expected an HTTP refusal, got {error}"),
    Ok((_, response)) => panic!("expected a refusal, got {}", response.status()),
  }
}

fn selected(response: &Response) -> Vec<&str> {
  response
    .headers()
    .get_all(header::SEC_WEBSOCKET_PROTOCOL)
    .iter()
    .map(|value| value.to_str().unwrap())
    .collect()
}

async fn raw(request: &str) -> String {
  let mut stream = TcpStream::connect(listen().await).await.unwrap();
  head(&mut stream, request).await
}

async fn head(stream: &mut TcpStream, request: &str) -> String {
  stream.write_all(request.as_bytes()).await.unwrap();
  let mut response = Vec::new();
  while !response.ends_with(b"\r\n\r\n") {
    response.push(stream.read_u8().await.unwrap());
  }
  String::from_utf8(response).unwrap()
}

#[tokio::test]
async fn selects_pavia_1() {
  let (_, response) = upgrade_tcp(&[SUBPROTOCOL]).await.unwrap();
  assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
  assert_eq!(selected(&response), [SUBPROTOCOL]);
  assert!(response.headers().get(header::SEC_WEBSOCKET_EXTENSIONS).is_none());
}

#[tokio::test]
async fn selects_pavia_1_from_a_list() {
  let (_, response) = upgrade_tcp(&["pavia.2, pavia.1"]).await.unwrap();
  assert_eq!(selected(&response), [SUBPROTOCOL]);
}

#[tokio::test]
async fn selects_pavia_1_from_several_header_lines() {
  let response = raw(concat!(
    "GET /rt HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n",
    "Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
    "Sec-WebSocket-Protocol: pavia.2\r\nSec-WebSocket-Protocol: pavia.3, pavia.1\r\n\r\n",
  ))
  .await;
  assert!(response.starts_with("HTTP/1.1 101 Switching Protocols\r\n"), "{response}");
  let lower = response.to_ascii_lowercase();
  assert!(lower.contains("\r\nsec-websocket-protocol: pavia.1\r\n"), "{response}");
  assert!(
    lower.contains("\r\nsec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo=\r\n"),
    "{response}"
  );
}

#[tokio::test]
async fn refuses_an_upgrade_without_a_subprotocol() {
  assert_eq!(refused(upgrade_tcp(&[]).await), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn refuses_an_upgrade_offering_only_other_subprotocols() {
  assert_eq!(refused(upgrade_tcp(&["pavia.2"]).await), StatusCode::BAD_REQUEST);
  assert_eq!(refused(upgrade_tcp(&["Pavia.1, pavia.10"]).await), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn refuses_another_path() {
  let stream = TcpStream::connect(listen().await).await.unwrap();
  assert_eq!(
    refused(upgrade(stream, "/other", &[SUBPROTOCOL]).await),
    StatusCode::NOT_FOUND
  );
}

#[tokio::test]
async fn answers_a_plain_get_with_426() {
  let response = raw("GET /rt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").await;
  assert!(response.starts_with("HTTP/1.1 426 Upgrade Required\r\n"), "{response}");
  assert!(
    response.to_ascii_lowercase().contains("\r\nupgrade: websocket\r\n"),
    "{response}"
  );
}

#[tokio::test]
async fn answers_another_websocket_version_with_426() {
  let response = raw(concat!(
    "GET /rt HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n",
    "Sec-WebSocket-Version: 8\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
    "Sec-WebSocket-Protocol: pavia.1\r\n\r\n",
  ))
  .await;
  assert!(response.starts_with("HTTP/1.1 426 Upgrade Required\r\n"), "{response}");
  assert!(
    response.to_ascii_lowercase().contains("\r\nsec-websocket-version: 13\r\n"),
    "{response}"
  );
}

#[tokio::test]
async fn refuses_a_malformed_key() {
  let response = raw(concat!(
    "GET /rt HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n",
    "Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: short\r\n",
    "Sec-WebSocket-Protocol: pavia.1\r\n\r\n",
  ))
  .await;
  assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{response}");
}

#[tokio::test]
async fn moves_binary_messages_both_ways() {
  let (mut socket, _) = upgrade_tcp(&[SUBPROTOCOL]).await.unwrap();
  socket.send(Message::binary(&b"\x00\x04\x00\x00\x00"[..])).await.unwrap();
  socket.send(Message::Ping(Bytes::new())).await.unwrap();
  let mut received = vec![socket.next().await.unwrap().unwrap(), socket.next().await.unwrap().unwrap()];
  received.sort_by_key(Message::is_pong);
  assert_eq!(
    received,
    [Message::binary(&b"\x00\x04\x00\x00\x00"[..]), Message::Pong(Bytes::new())]
  );
}

#[tokio::test]
async fn reports_text_messages_to_the_session() {
  let (mut socket, _) = upgrade_tcp(&[SUBPROTOCOL]).await.unwrap();
  socket.send(Message::text("hello")).await.unwrap();
  assert_eq!(socket.next().await.unwrap().unwrap(), Message::binary(&b"text"[..]));
}

#[tokio::test]
async fn closes_with_the_given_code() {
  let (mut socket, _) = upgrade_tcp(&[SUBPROTOCOL]).await.unwrap();
  socket.send(Message::binary(&b"close"[..])).await.unwrap();
  let Message::Close(Some(frame)) = socket.next().await.unwrap().unwrap() else {
    panic!("expected a close frame");
  };
  assert_eq!(frame.code, WsCloseCode::from(4001));
  assert_eq!(frame.reason, "bye");
}

#[tokio::test]
async fn truncates_the_close_reason_on_a_character_boundary() {
  let (mut socket, _) = upgrade_tcp(&[SUBPROTOCOL]).await.unwrap();
  socket.send(Message::binary(&b"long"[..])).await.unwrap();
  let Message::Close(Some(frame)) = socket.next().await.unwrap().unwrap() else {
    panic!("expected a close frame");
  };
  assert_eq!(frame.code, WsCloseCode::from(4008));
  assert_eq!(frame.reason, "é".repeat(61));
}

#[tokio::test]
async fn answers_a_ping_on_its_own() {
  let (mut socket, _) = upgrade_tcp(&[SUBPROTOCOL]).await.unwrap();
  socket.send(Message::Ping(Bytes::from_static(b"p"))).await.unwrap();
  assert_eq!(socket.next().await.unwrap().unwrap(), Message::Pong(Bytes::from_static(b"p")));
}

#[tokio::test]
async fn serves_an_in_memory_connection() {
  let (client, server) = tokio::io::duplex(65_536);
  tokio::spawn(async move { endpoint().serve_connection(server).await });
  let (mut socket, response) = upgrade(client, PATH, &[SUBPROTOCOL]).await.unwrap();
  assert_eq!(selected(&response), [SUBPROTOCOL]);
  socket.send(Message::binary(&b"\x00\x04\x00\x00\x00"[..])).await.unwrap();
  assert_eq!(
    socket.next().await.unwrap().unwrap(),
    Message::binary(&b"\x00\x04\x00\x00\x00"[..])
  );
}

#[test]
fn checks_the_key_shape() {
  assert!(is_key(b"dGhlIHNhbXBsZSBub25jZQ=="));
  assert!(!is_key(b"dGhlIHNhbXBsZSBub25jZQ="));
  assert!(!is_key(b"dGhlIHNhbXBsZSBub25jZ$=="));
  assert!(!is_key(b"dGhlIHNhbXBsZSBub25jZQ.="));
}

async fn first_recv(client: impl AsyncFnOnce(SocketAddr)) -> Option<bool> {
  let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
  let endpoint = Endpoint::new(Config::new(PATH), move |mut transport: Transport| {
    let sender = sender.clone();
    async move {
      let _ = sender.send(transport.recv().await.map(|received| received.is_ok())).await;
    }
  });
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  tokio::spawn(endpoint.serve(listener));
  client(address).await;
  receiver.recv().await.unwrap()
}

#[tokio::test]
async fn ends_when_the_peer_closes() {
  let received = first_recv(async |address| {
    let stream = TcpStream::connect(address).await.unwrap();
    let (mut socket, _) = upgrade(stream, PATH, &[SUBPROTOCOL]).await.unwrap();
    socket.close(None).await.unwrap();
    assert!(matches!(socket.next().await, Some(Ok(Message::Close(_)))));
  })
  .await;
  assert_eq!(received, None);
}

#[tokio::test]
async fn reports_a_broken_frame_as_an_error() {
  let received = first_recv(async |address| {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let response = head(
      &mut stream,
      concat!(
        "GET /rt HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n",
        "Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
        "Sec-WebSocket-Protocol: pavia.1\r\n\r\n",
      ),
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 101 "), "{response}");
    stream.write_all(&[0x82, 0x01, 0x00]).await.unwrap();
  })
  .await;
  assert_eq!(received, Some(false));
}
