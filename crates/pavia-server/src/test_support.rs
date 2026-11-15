use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use futures_util::SinkExt;
use pavia_proto::frame::Limits;
use pavia_proto::session::Config as SessionConfig;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

use crate::websocket::{Endpoint, Transport};

pub(crate) const EXPECT_WITHIN: Duration = Duration::from_secs(5);

pub(crate) async fn serve_on_loopback<H, F>(endpoint: Endpoint<H>) -> SocketAddr
where
  H: Fn(Transport) -> F + Clone + Send + Sync + 'static,
  F: Future<Output = ()> + Send + 'static,
{
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  tokio::spawn(endpoint.serve(listener));
  address
}

pub(crate) fn floor_frame() -> SessionConfig {
  let mut config = SessionConfig::default();
  config.max_frame = Limits::FLOOR;
  config
}

pub(crate) fn has_ws_close(bytes: &[u8], code: u16) -> bool {
  let [high, low] = code.to_be_bytes();
  bytes.windows(4).any(|window| window[0] == 0x88 && window[2..] == [high, low])
}

pub(crate) async fn send<S: AsyncRead + AsyncWrite + Unpin>(socket: &mut WebSocketStream<S>, message: impl Into<Bytes>) {
  socket.send(Message::binary(message)).await.unwrap();
}
