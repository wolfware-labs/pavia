use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpListener;

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

pub(crate) fn has_ws_close(bytes: &[u8], code: u16) -> bool {
  let [high, low] = code.to_be_bytes();
  bytes.windows(4).any(|window| window[0] == 0x88 && window[2..] == [high, low])
}
