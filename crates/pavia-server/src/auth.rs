//! Authentication (§7.5): the hook that turns HELLO `auth` and the upgrade request into a
//! principal before WELCOME.
//!
//! The application implements [`Authenticator`]. [`session::run`](crate::session::run) calls it
//! once per session, after HELLO is validated, while the handshake timer keeps running (§7.1). A
//! refusal is answered with REJECT UNAUTHENTICATED or FORBIDDEN and the transport closes.
//!
//! Credentials are secrets (§14): the HELLO token prints as redacted, and [`RequestContext`]
//! prints header names but no values.

use std::fmt;
use std::future::{self, Future};
use std::net::SocketAddr;

use hyper::HeaderMap;
use pavia_proto::frame::Auth;
pub use pavia_proto::session::AuthFailure;

/// Decides who a client is.
pub trait Authenticator: Send + Sync {
  /// The credential schemes this authenticator accepts, such as `bearer`. A later AUTH_REFRESH
  /// with another scheme is malformed (§7.5).
  fn schemes(&self) -> &[String];

  /// The principal for HELLO `auth` (`None` when HELLO has none) and the upgrade request, or
  /// why the client is refused.
  fn authenticate(
    &self,
    auth: Option<&Auth>,
    context: &RequestContext,
  ) -> impl Future<Output = Result<Principal, AuthFailure>> + Send;
}

/// Accepts every client as an anonymous session and ignores credentials. For examples and tests
/// only: a server with it has no authentication at all.
#[derive(Debug, Clone, Copy, Default)]
pub struct Permissive;

impl Authenticator for Permissive {
  fn schemes(&self) -> &[String] {
    &[]
  }

  fn authenticate(&self, _: Option<&Auth>, _: &RequestContext) -> impl Future<Output = Result<Principal, AuthFailure>> + Send {
    future::ready(Ok(Principal::anonymous()))
  }
}

/// The authenticated identity of a session, fixed for its life (§7.5).
///
/// Two principals are equal when their stable IDs are, which by default is the user ID. A
/// principal without a user ID makes an anonymous session (§2), and an anonymous principal never
/// equals one with a user ID.
///
/// ```
/// use pavia_server::auth::Principal;
///
/// assert_eq!(Principal::for_user("u-42"), Principal::for_user("u-42"));
/// assert_ne!(Principal::for_user("u-42"), Principal::anonymous());
/// assert_eq!(Principal::for_user("u-42").with_id("key-7"), Principal::for_user("u-9").with_id("key-7"));
/// ```
#[derive(Debug, Clone)]
pub struct Principal {
  id: Option<String>,
  user: Option<String>,
}

impl Principal {
  /// A principal with no user ID, for an anonymous session.
  #[must_use]
  pub const fn anonymous() -> Self {
    Self { id: None, user: None }
  }

  /// The principal of user `user`, whose stable ID is the user ID.
  #[must_use]
  pub fn for_user(user: impl Into<String>) -> Self {
    let user = user.into();
    Self {
      id: Some(user.clone()),
      user: Some(user),
    }
  }

  /// The same principal with `id` as its stable ID instead of the user ID.
  #[must_use]
  pub fn with_id(self, id: impl Into<String>) -> Self {
    Self {
      id: Some(id.into()),
      ..self
    }
  }

  /// The stable ID that principal equality compares.
  #[must_use]
  pub fn id(&self) -> Option<&str> {
    self.id.as_deref()
  }

  /// The user ID, which targeting by user uses (§13.3). `None` for an anonymous session.
  #[must_use]
  pub fn user(&self) -> Option<&str> {
    self.user.as_deref()
  }

  /// Whether the session is anonymous: the principal has no user ID (§2).
  #[must_use]
  pub const fn is_anonymous(&self) -> bool {
    self.user.is_none()
  }
}

impl PartialEq for Principal {
  fn eq(&self, other: &Self) -> bool {
    self.id == other.id && self.is_anonymous() == other.is_anonymous()
  }
}

impl Eq for Principal {}

/// The WebSocket upgrade request a session arrived on: its headers, cookies and peer address.
#[derive(Clone)]
pub struct RequestContext {
  headers: HeaderMap,
  peer: Option<SocketAddr>,
}

impl RequestContext {
  pub(crate) const fn new(headers: HeaderMap, peer: Option<SocketAddr>) -> Self {
    Self { headers, peer }
  }

  /// The first value of header `name`, compared without case.
  #[must_use]
  pub fn header(&self, name: &str) -> Option<&[u8]> {
    self.headers.get(name).map(|value| value.as_bytes())
  }

  /// The value of cookie `name`, from every `Cookie` header of the request.
  #[must_use]
  pub fn cookie(&self, name: &str) -> Option<&str> {
    self
      .headers
      .get_all(hyper::header::COOKIE)
      .iter()
      .filter_map(|value| value.to_str().ok())
      .flat_map(|value| value.split(';'))
      .filter_map(|pair| pair.trim().split_once('='))
      .find_map(|(cookie, value)| (cookie == name).then_some(value))
  }

  /// The address of the peer that opened the connection, when the transport has one.
  #[must_use]
  pub const fn peer(&self) -> Option<SocketAddr> {
    self.peer
  }
}

impl fmt::Debug for RequestContext {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("RequestContext")
      .field("headers", &self.headers.keys().collect::<Vec<_>>())
      .field("peer", &self.peer)
      .finish()
  }
}

#[cfg(test)]
mod tests;
