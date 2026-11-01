use futures_util::{SinkExt, StreamExt};
use hyper::header::{self, HeaderValue};
use pavia_proto::registry::CloseCode;
use pavia_proto::session::{Command, Ending};
use pavia_proto::stream::Side;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode as WsCloseCode;
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};
use tokio_tungstenite::{WebSocketStream, client_async};

use super::run;
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

async fn connect(command: Option<Command>) -> (Socket, oneshot::Receiver<Ending>) {
  let (ended, ending) = oneshot::channel();
  let ended = std::sync::Arc::new(std::sync::Mutex::new(Some(ended)));
  let endpoint = Endpoint::new(Config::new("/rt"), move |transport: Transport| {
    let ended = ended.lock().unwrap().take();
    let command = command.clone();
    async move {
      let (commands, receiver) = mpsc::channel(1);
      if let Some(command) = command {
        commands.send(command).await.unwrap();
      }
      let ending = run(transport, receiver).await;
      drop(commands);
      if let Some(ended) = ended {
        let _ = ended.send(ending);
      }
    }
  });
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  tokio::spawn(endpoint.serve(listener));
  let mut request = "ws://localhost/rt".into_client_request().unwrap();
  request
    .headers_mut()
    .insert(header::SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static(SUBPROTOCOL));
  let (socket, _) = client_async(request, TcpStream::connect(address).await.unwrap())
    .await
    .unwrap();
  (socket, ending)
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
  socket.send(Message::binary(close_frame(0))).await.unwrap();
  let (binary, ws_code, _) = closed(&mut socket).await;
  assert!(binary.is_empty());
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
