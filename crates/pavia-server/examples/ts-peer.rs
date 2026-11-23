//! A server for the TypeScript client's integration test (`clients/typescript`, the
//! `ts-integration` CI job): a WebSocket endpoint at `/rt` with no methods and a short heartbeat
//! (`hb` 200 ms, `idle` 600 ms), so a session that stays open for a second has lived on PING and
//! PONG alone.
//!
//! It listens on `PAVIA_ADDR` (default `127.0.0.1:0`) and prints `listening ws://<addr>/rt`
//! once it accepts connections.
//!
//! ```sh
//! cargo run -p pavia-server --example ts-peer
//! ```

use std::sync::Arc;
use std::time::Duration;

use pavia_proto::session::Config as SessionConfig;
use pavia_server::auth::{AllowAll, Permissive};
use pavia_server::methods::MethodRegistry;
use pavia_server::session;
use pavia_server::sessions::Registry;
use pavia_server::websocket::{Config, Endpoint, Transport};
use tokio::net::TcpListener;

const HEARTBEAT: Duration = Duration::from_millis(200);
const IDLE: Duration = Duration::from_millis(600);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
  let mut config = SessionConfig::default();
  config.heartbeat = HEARTBEAT;
  config.idle_timeout = IDLE;
  let config = config.validate()?;
  let methods = Arc::new(MethodRegistry::new());
  let sessions = Arc::new(Registry::new());
  let endpoint = Endpoint::new(Config::new("/rt", &config), move |transport: Transport| {
    let methods = Arc::clone(&methods);
    let sessions = Arc::clone(&sessions);
    async move {
      session::run(transport, &sessions, &methods, &Permissive, &Arc::new(AllowAll)).await;
    }
  });
  let addr = std::env::var("PAVIA_ADDR").unwrap_or_else(|_| "127.0.0.1:0".to_owned());
  let listener = TcpListener::bind(&addr).await?;
  println!("listening ws://{}/rt", listener.local_addr()?);
  endpoint.serve(listener).await?;
  Ok(())
}
