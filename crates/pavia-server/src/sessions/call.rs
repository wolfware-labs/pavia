//! What a server call takes and how it fails (§8.4, §8.6, §8.8).

use std::fmt;
use std::time::Duration;

use pavia_proto::data::Value;
use pavia_proto::registry::StatusCode;
use pavia_proto::session::CallRefused;

/// How a server call is made. The default has no deadline.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CallOptions {
  /// How long the call may take, from when it is made; sent to the client in the CALL (§8.6).
  pub deadline: Option<Duration>,
}

impl CallOptions {
  /// Sets [`deadline`](Self::deadline).
  #[must_use]
  pub const fn with_deadline(mut self, deadline: Duration) -> Self {
    self.deadline = Some(deadline);
    self
  }
}

/// How a server call failed: the client's ERROR, or a local failure (§8.4).
#[derive(Debug, Clone, PartialEq)]
pub struct CallError {
  code: StatusCode,
  msg: String,
  nx: bool,
  detail: Option<Value>,
}

impl CallError {
  pub(crate) fn new(code: StatusCode, msg: impl Into<String>, nx: bool, detail: Option<Value>) -> Self {
    Self {
      code,
      msg: msg.into(),
      nx,
      detail,
    }
  }

  pub(crate) fn refused(refused: CallRefused) -> Self {
    Self::new(refused.code(), refused.msg(), true, None)
  }

  pub(crate) fn ended() -> Self {
    Self::refused(CallRefused::NotEstablished)
  }

  pub(crate) fn unavailable() -> Self {
    Self::new(StatusCode::Unavailable, "the session ended before the answer", false, None)
  }

  pub(crate) fn deadline_exceeded() -> Self {
    Self::new(
      StatusCode::DeadlineExceeded,
      "the deadline passed before the answer",
      false,
      None,
    )
  }

  /// The status code.
  #[must_use]
  pub const fn code(&self) -> StatusCode {
    self.code
  }

  /// The message.
  #[must_use]
  pub fn msg(&self) -> &str {
    &self.msg
  }

  /// `nx`: the client's handler never ran. Every local failure that came before the CALL was sent
  /// sets it (§8.4).
  #[must_use]
  pub const fn nx(&self) -> bool {
    self.nx
  }

  /// The typed error detail of the client's ERROR, decoded, if it carried one.
  #[must_use]
  pub const fn detail(&self) -> Option<&Value> {
    self.detail.as_ref()
  }
}

impl fmt::Display for CallError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(formatter, "{:?}: {}", self.code, self.msg)
  }
}

impl std::error::Error for CallError {}
