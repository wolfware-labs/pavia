use std::net::SocketAddr;
use std::time::Duration;

use pavia_proto::session::Config as SessionConfig;
use pavia_server::auth::Permissive;
use pavia_server::session::run;
use pavia_server::websocket::{Config, Endpoint, Transport};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const CLIENTS: usize = 16;
const ANNOUNCED: u64 = 4 << 20;
const PEAK_BOUND: usize = 16 << 20;

async fn listen() -> SocketAddr {
  let mut config = SessionConfig::default();
  config.max_frame = 65_536;
  let config = config.validate().unwrap();
  let endpoint = Endpoint::new(Config::new("/rt", &config), move |transport: Transport| {
    let config = config.clone();
    async move {
      let (_commands, receiver) = mpsc::channel(1);
      run(transport, receiver, &config, &Permissive).await;
    }
  });
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = listener.local_addr().unwrap();
  tokio::spawn(endpoint.serve(listener));
  address
}

async fn upgraded(address: SocketAddr) -> TcpStream {
  let mut stream = TcpStream::connect(address).await.unwrap();
  let request = concat!(
    "GET /rt HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n",
    "Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
    "Sec-WebSocket-Protocol: pavia.1\r\n\r\n",
  );
  stream.write_all(request.as_bytes()).await.unwrap();
  let mut head = Vec::new();
  while !head.ends_with(b"\r\n\r\n") {
    head.push(stream.read_u8().await.unwrap());
  }
  assert!(head.starts_with(b"HTTP/1.1 101"), "{}", String::from_utf8_lossy(&head));
  stream
}

fn announce(length: u64) -> Vec<u8> {
  let mut header = vec![0x82, 0xff];
  header.extend(length.to_be_bytes());
  header.extend([0x00; 4]);
  header.extend([0x00; 16]);
  header
}

#[test]
fn keeps_memory_flat_under_a_flood_of_oversized_announcements() {
  let _profiler = dhat::Profiler::builder().testing().build();
  let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
  runtime.block_on(async {
    let address = listen().await;
    let mut streams = Vec::new();
    for _ in 0..CLIENTS {
      streams.push(upgraded(address).await);
    }
    let before = dhat::HeapStats::get().curr_bytes;
    for stream in &mut streams {
      stream.write_all(&announce(ANNOUNCED)).await.unwrap();
    }
    for stream in &mut streams {
      let mut rest = Vec::new();
      let read = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut rest)).await;
      assert!(read.is_ok(), "the server waited for the announced payload");
      let close_4001 = rest.windows(4).any(|window| window[0] == 0x88 && window[2..] == [0x0f, 0xa1]);
      assert!(close_4001, "{rest:02x?}");
    }
    let peak = dhat::HeapStats::get().max_bytes.saturating_sub(before);
    assert!(peak < PEAK_BOUND, "peak heap grew by {peak} bytes");
  });
}
