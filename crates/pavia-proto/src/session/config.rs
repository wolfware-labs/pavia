use std::time::Duration;

use thiserror::Error;

use crate::data::Codec;
use crate::frame::{Limits, ServerLimits};

/// Session settings chosen by the application. [`Config::validate`] turns them into the
/// [`ValidConfig`] a session is built from, so a WELCOME never advertises a value the spec
/// forbids.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Config {
  /// How long the handshake may take, from transport accept until WELCOME or REJECT is sent
  /// (§7.1). Default 10 s.
  pub handshake_timeout: Duration,
  /// The protocol major versions this server speaks. Default `[1]`.
  pub versions: Vec<u64>,
  /// The data-section codecs this server supports. Default `[Codec::Cbor]`; `json` waits for
  /// its data-section codec (#59).
  pub codecs: Vec<Codec>,
  /// WELCOME `hb`, the heartbeat interval (§7.4). Default 15 s.
  pub heartbeat: Duration,
  /// WELCOME `idle`, the idle timeout (§7.4). Default 45 s.
  pub idle_timeout: Duration,
  /// WELCOME `max_calls`: concurrent calls the client may open. Default 100.
  pub max_calls: u64,
  /// WELCOME `max_frame`: the largest frame payload this server accepts, in bytes, at least
  /// 64 KiB (§4.1). Default 1 MiB.
  pub max_frame: u64,
  /// WELCOME `max_subs`: concurrent channel subscriptions per session. Default 256.
  pub max_subs: u64,
  /// WELCOME `max_dgram`: the largest datagram payload, in bytes. Default 1,100.
  pub max_dgram: u64,
  /// WELCOME `window`: the initial per-stream WebSocket credit, in bytes, at least 64 KiB
  /// (§4.1). Default 256 KiB. It may be smaller than `max_frame` (§6.4).
  pub window: u64,
}

impl Default for Config {
  fn default() -> Self {
    Self {
      handshake_timeout: Duration::from_secs(10),
      versions: vec![1],
      codecs: vec![Codec::Cbor],
      heartbeat: Duration::from_secs(15),
      idle_timeout: Duration::from_secs(45),
      max_calls: 100,
      max_frame: 1 << 20,
      max_subs: 256,
      max_dgram: 1_100,
      window: 256 << 10,
    }
  }
}

impl Config {
  /// Checks every setting: no duration under 1 ms, no zero limit, no empty version or codec list, and
  /// `max_frame` and `window` at or above the 64 KiB floor (§4.1, §16).
  ///
  /// ```
  /// use pavia_proto::session::{Config, ConfigError};
  ///
  /// let mut config = Config::default();
  /// config.window = 65_535;
  /// assert_eq!(config.validate().err(), Some(ConfigError::BelowFloor("window")));
  /// ```
  ///
  /// # Errors
  ///
  /// The first [`ConfigError`] found, naming the setting.
  pub fn validate(self) -> Result<ValidConfig, ConfigError> {
    let durations = [
      ("handshake_timeout", self.handshake_timeout),
      ("heartbeat", self.heartbeat),
      ("idle_timeout", self.idle_timeout),
    ];
    if let Some((name, _)) = durations.iter().find(|(_, duration)| millis(*duration) == 0) {
      return Err(ConfigError::TooShort(name));
    }
    let counts = [
      ("max_calls", self.max_calls),
      ("max_subs", self.max_subs),
      ("max_dgram", self.max_dgram),
    ];
    if let Some((name, _)) = counts.iter().find(|(_, count)| *count == 0) {
      return Err(ConfigError::Zero(name));
    }
    let floored = [("max_frame", self.max_frame), ("window", self.window)];
    if let Some((name, _)) = floored.iter().find(|(_, bytes)| *bytes < Limits::FLOOR) {
      return Err(ConfigError::BelowFloor(name));
    }
    if self.versions.is_empty() {
      return Err(ConfigError::Empty("versions"));
    }
    if self.codecs.is_empty() {
      return Err(ConfigError::Empty("codecs"));
    }
    Ok(ValidConfig(self))
  }
}

/// A [`Config`] that passed [`Config::validate`]. Its default is the default [`Config`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidConfig(pub(super) Config);

impl ValidConfig {
  pub(super) fn limits(&self) -> ServerLimits {
    ServerLimits {
      max_calls: self.0.max_calls,
      max_frame: self.0.max_frame,
      max_subs: self.0.max_subs,
      max_dgram: self.0.max_dgram,
      window: Some(self.0.window),
    }
  }

  pub(super) fn heartbeat_ms(&self) -> u64 {
    millis(self.0.heartbeat)
  }

  pub(super) fn idle_ms(&self) -> u64 {
    millis(self.0.idle_timeout)
  }
}

/// A [`Config`] setting the spec does not allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ConfigError {
  /// A duration under 1 ms; WELCOME carries durations in whole milliseconds.
  #[error("{0} must be at least 1 ms")]
  TooShort(&'static str),
  /// A zero limit.
  #[error("{0} must not be zero")]
  Zero(&'static str),
  /// `max_frame` or `window` below 65,536 bytes.
  #[error("{0} must be at least 65536 bytes")]
  BelowFloor(&'static str),
  /// No protocol version or no codec.
  #[error("{0} must not be empty")]
  Empty(&'static str),
}

fn millis(duration: Duration) -> u64 {
  u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
