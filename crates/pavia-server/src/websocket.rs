//! The WebSocket binding (§4.3): an HTTP/1.1 upgrade on the endpoint path that selects the
//! `pavia.1` subprotocol, then one task per transport that moves whole binary messages.
//!
//! The driver never looks inside a message. The session applies the `WsFrame` envelope (§5.4) to
//! what [`Transport::recv`] returns and hands [`Transport::send`] messages it already built.
//!
//! ```text
//!  GET {path}, Upgrade: websocket, Sec-WebSocket-Protocol: pavia.1  ──▶ 101, pavia.1 echoed
//!  GET {path}, upgrade without pavia.1                              ──▶ 400
//!  GET {path}, no upgrade                                           ──▶ 426, Upgrade: websocket
//!  any other path                                                   ──▶ 404
//! ```

use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::sync::Arc;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http_body_util::Empty;
use hyper::body::Incoming;
use hyper::header::{self, HeaderMap, HeaderName, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::upgrade::Upgraded;
use hyper::{Method, Request, Response, StatusCode, Version};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::protocol::frame::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{self, Message};

/// The WebSocket subprotocol of framing version 1 (§4.3).
pub const SUBPROTOCOL: &str = "pavia.1";

const WEBSOCKET_VERSION: &str = "13";

/// Where the endpoint listens.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Config {
  /// The endpoint path, for example `/rt` (§3).
  pub path: String,
}

impl Config {
  /// An endpoint on `path`.
  #[must_use]
  pub fn new(path: impl Into<String>) -> Self {
    Self { path: path.into() }
  }
}

/// A WebSocket endpoint: answers HTTP/1.1 requests on its path and hands every accepted
/// transport to `handler` on a task of its own.
#[derive(Debug)]
pub struct Endpoint<H> {
  config: Arc<Config>,
  handler: H,
}

impl<H: Clone> Clone for Endpoint<H> {
  fn clone(&self) -> Self {
    Self {
      config: Arc::clone(&self.config),
      handler: self.handler.clone(),
    }
  }
}

impl<H, F> Endpoint<H>
where
  H: Fn(Transport) -> F + Clone + Send + Sync + 'static,
  F: Future<Output = ()> + Send + 'static,
{
  /// An endpoint for `config` that runs `handler` once per accepted transport.
  pub fn new(config: Config, handler: H) -> Self {
    Self {
      config: Arc::new(config),
      handler,
    }
  }

  /// Accepts connections on `listener` until accepting fails, serving each on its own task.
  ///
  /// # Errors
  ///
  /// The error from [`TcpListener::accept`].
  pub async fn serve(self, listener: TcpListener) -> io::Result<()> {
    loop {
      let (stream, _) = listener.accept().await?;
      let endpoint = self.clone();
      tokio::spawn(async move {
        let _ = endpoint.serve_connection(stream).await;
      });
    }
  }

  /// Serves HTTP/1.1 on one connection: a TCP stream, a TLS stream, or an in-memory pipe.
  ///
  /// # Errors
  ///
  /// The error from hyper when the connection fails before the response is written.
  pub async fn serve_connection<I>(&self, io: I) -> Result<(), hyper::Error>
  where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
  {
    let endpoint = self.clone();
    let service = service_fn(move |request| {
      let endpoint = endpoint.clone();
      async move { Ok::<_, Infallible>(endpoint.respond(request)) }
    });
    http1::Builder::new()
      .serve_connection(TokioIo::new(io), service)
      .with_upgrades()
      .await
  }

  fn respond(&self, mut request: Request<Incoming>) -> Response<Empty<Bytes>> {
    let accept = match answer(&self.config, &request) {
      Ok(accept) => accept,
      Err(refusal) => return refusal,
    };
    let upgrade = hyper::upgrade::on(&mut request);
    let handler = self.handler.clone();
    tokio::spawn(async move {
      if let Ok(upgraded) = upgrade.await {
        let socket = WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None).await;
        handler(Transport { socket }).await;
      }
    });
    switching_protocols(&accept)
  }
}

/// Decides the response to `request`: `Ok` with the `Sec-WebSocket-Accept` value for an upgrade
/// that offers `pavia.1`, `Err` with the refusal otherwise. Nothing is upgraded before this runs.
fn answer<B>(config: &Config, request: &Request<B>) -> Result<String, Response<Empty<Bytes>>> {
  let headers = request.headers();
  if request.uri().path() != config.path {
    return Err(status(StatusCode::NOT_FOUND));
  }
  if !is_upgrade(request) {
    let mut response = status(StatusCode::UPGRADE_REQUIRED);
    response
      .headers_mut()
      .insert(header::UPGRADE, HeaderValue::from_static("websocket"));
    return Err(response);
  }
  if headers.get(header::SEC_WEBSOCKET_VERSION).map(HeaderValue::as_bytes) != Some(WEBSOCKET_VERSION.as_bytes()) {
    let mut response = status(StatusCode::UPGRADE_REQUIRED);
    response
      .headers_mut()
      .insert(header::SEC_WEBSOCKET_VERSION, HeaderValue::from_static(WEBSOCKET_VERSION));
    return Err(response);
  }
  let Some(key) = headers.get(header::SEC_WEBSOCKET_KEY).filter(|key| is_key(key.as_bytes())) else {
    return Err(status(StatusCode::BAD_REQUEST));
  };
  if !offers(headers, SUBPROTOCOL) {
    return Err(status(StatusCode::BAD_REQUEST));
  }
  Ok(derive_accept_key(key.as_bytes()))
}

fn is_upgrade<B>(request: &Request<B>) -> bool {
  request.method() == Method::GET
    && request.version() == Version::HTTP_11
    && has_token(request.headers(), &header::CONNECTION, "upgrade")
    && has_token(request.headers(), &header::UPGRADE, "websocket")
}

fn has_token(headers: &HeaderMap, name: &HeaderName, token: &str) -> bool {
  list(headers, name).any(|value| value.eq_ignore_ascii_case(token))
}

/// Whether the client lists `subprotocol` in `Sec-WebSocket-Protocol`, which may span several
/// header lines, each a comma-separated list (RFC 6455 §11.3.4). Subprotocol names are compared
/// exactly.
fn offers(headers: &HeaderMap, subprotocol: &str) -> bool {
  list(headers, &header::SEC_WEBSOCKET_PROTOCOL).any(|value| value == subprotocol)
}

fn list<'h>(headers: &'h HeaderMap, name: &HeaderName) -> impl Iterator<Item = &'h str> {
  headers
    .get_all(name)
    .iter()
    .filter_map(|value| value.to_str().ok())
    .flat_map(|value| value.split(','))
    .map(str::trim)
}

/// A `Sec-WebSocket-Key` is 16 bytes in base64: 22 characters and `==` (RFC 6455 §4.1).
fn is_key(key: &[u8]) -> bool {
  key.len() == 24
    && key.ends_with(b"==")
    && key[..22]
      .iter()
      .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
}

fn status(code: StatusCode) -> Response<Empty<Bytes>> {
  let mut response = Response::new(Empty::new());
  *response.status_mut() = code;
  response
}

fn switching_protocols(accept: &str) -> Response<Empty<Bytes>> {
  let mut response = status(StatusCode::SWITCHING_PROTOCOLS);
  let headers = response.headers_mut();
  headers.insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
  headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
  headers.insert(header::SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static(SUBPROTOCOL));
  if let Ok(accept) = HeaderValue::from_str(accept) {
    headers.insert(header::SEC_WEBSOCKET_ACCEPT, accept);
  }
  response
}

/// What [`Transport::recv`] read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Received {
  /// One whole binary message: one or more `WsFrame`s (§5.4).
  Binary(Bytes),
  /// A text message, which the session answers with PROTOCOL_ERROR (§4.3).
  Text,
}

/// Error from the WebSocket connection under a [`Transport`].
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct TransportError(#[from] tungstenite::Error);

/// One accepted WebSocket transport. Exactly one session runs on it (§4.1).
#[derive(Debug)]
pub struct Transport {
  socket: WebSocketStream<TokioIo<Upgraded>>,
}

impl Transport {
  /// The next data message, or `None` once the peer closed the WebSocket or the connection ended.
  /// WebSocket PING and PONG are answered and skipped here; they are not Pavia's (§7.4).
  ///
  /// # Errors
  ///
  /// A [`TransportError`] when the connection fails or the peer breaks RFC 6455.
  pub async fn recv(&mut self) -> Option<Result<Received, TransportError>> {
    loop {
      match self.socket.next().await? {
        Ok(Message::Binary(message)) => return Some(Ok(Received::Binary(message))),
        Ok(Message::Text(_)) => return Some(Ok(Received::Text)),
        Ok(Message::Close(_)) => return None,
        Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
        Err(error) => return Some(Err(error.into())),
      }
    }
  }

  /// Sends one binary message, built by the session with the `WsFrame` envelope applied.
  ///
  /// # Errors
  ///
  /// A [`TransportError`] when the connection fails or is already closed.
  pub async fn send(&mut self, message: Bytes) -> Result<(), TransportError> {
    Ok(self.socket.send(Message::Binary(message)).await?)
  }

  /// Closes the WebSocket with `code` and `reason`. A session close uses `4000 + code` (§4.3,
  /// §15.1).
  ///
  /// # Errors
  ///
  /// A [`TransportError`] when the close frame cannot be written.
  pub async fn close(mut self, code: u16, reason: &str) -> Result<(), TransportError> {
    let frame = CloseFrame {
      code: CloseCode::from(code),
      reason: reason.into(),
    };
    Ok(self.socket.close(Some(frame)).await?)
  }
}

#[cfg(test)]
mod tests;
