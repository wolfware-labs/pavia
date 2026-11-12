//! Heap use of the server under floods of oversized WebSocket frames, of WebSocket messages
//! fragmented past `2 × max_frame`, and of Pavia Lengths above `max_frame` (§4.3, §5.2, §14).
//! The test replaces the global allocator with a counting one, so it runs as a test binary of its
//! own.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use pavia_proto::frame::Limits;
use pavia_proto::session::Config as SessionConfig;
use pavia_server::auth::{AllowAll, Permissive};
use pavia_server::methods::MethodRegistry;
use pavia_server::session::run;
use pavia_server::sessions::Registry;
use pavia_server::websocket::{Config, Endpoint, Transport};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

const CLIENTS: usize = 16;
const ANNOUNCED: u64 = 4 << 20;
const PEAK_BOUND: usize = 16 << 20;

async fn listen() -> SocketAddr {
  let mut config = SessionConfig::default();
  config.max_frame = Limits::FLOOR;
  let config = config.validate().unwrap();
  let registry = Registry::new();
  let endpoint = Endpoint::new(Config::new("/rt", &config), move |transport: Transport| {
    let registry = registry.clone();
    async move {
      run(transport, &registry, &MethodRegistry::new(), &Permissive, &Arc::new(AllowAll)).await;
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

fn header(first: u8, length: u64) -> Vec<u8> {
  let mut header = vec![first];
  match length {
    0..=125 => header.push(0x80 | u8::try_from(length).unwrap()),
    126..=65_535 => {
      header.push(0xfe);
      header.extend(u16::try_from(length).unwrap().to_be_bytes());
    }
    _ => {
      header.push(0xff);
      header.extend(length.to_be_bytes());
    }
  }
  header.extend([0x00; 4]);
  header
}

fn frame(first: u8, payload: &[u8]) -> Vec<u8> {
  let mut frame = header(first, payload.len() as u64);
  frame.extend(payload);
  frame
}

async fn flood(address: SocketAddr, writes: &[(&[u8], usize)], code: u16) {
  let mut streams = Vec::new();
  for _ in 0..CLIENTS {
    streams.push(upgraded(address).await);
  }
  for stream in &mut streams {
    'writes: for (bytes, times) in writes {
      for _ in 0..*times {
        if stream.write_all(bytes).await.is_err() {
          break 'writes;
        }
      }
    }
  }
  for stream in &mut streams {
    let mut rest = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut rest)).await;
    assert!(read.is_ok(), "the server kept waiting for {code}");
    let [high, low] = code.to_be_bytes();
    let closed = rest.windows(4).any(|window| window[0] == 0x88 && window[2..] == [high, low]);
    assert!(closed, "expected {code}: {rest:02x?}");
  }
}

#[test]
fn keeps_memory_flat_under_floods_of_oversized_frames_and_messages() {
  let _profiler = dhat::Profiler::builder().testing().build();
  let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
  runtime.block_on(async {
    let address = listen().await;
    let before = dhat::HeapStats::get().curr_bytes;
    let mut announcement = header(0x82, ANNOUNCED);
    announcement.extend([0x00; 16]);
    flood(address, &[(&announcement, 1)], 4001).await;
    let fragment = vec![0x00; 65_536];
    let fragments = ANNOUNCED as usize / fragment.len();
    let unfinished = [(&frame(0x02, &fragment)[..], 1), (&frame(0x00, &fragment)[..], fragments - 1)];
    flood(address, &unfinished, 4001).await;
    let prefix_of_65537 = frame(0x82, &[0x00, 0x04, 0x00, 0x80, 0x01, 0x00, 0x01]);
    flood(address, &[(&prefix_of_65537, 1)], 4009).await;
    let peak = dhat::HeapStats::get().max_bytes.saturating_sub(before);
    assert!(peak < PEAK_BOUND, "peak heap grew by {peak} bytes");
  });
}
