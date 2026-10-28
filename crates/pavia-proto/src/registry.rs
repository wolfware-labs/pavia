//! The registries of spec §15: session close codes (§15.1), call status codes (§15.2),
//! unsubscribe reasons (§15.3) and presence kinds (§15.4), with their numeric wire values.

use thiserror::Error;

/// A session close code (§15.1), carried by REJECT, GOAWAY and CLOSE.
///
/// On WebSocket the transport close code is `4000 + code`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[repr(u16)]
pub enum CloseCode {
  /// Normal closure.
  Normal = 0,
  /// Malformed or disallowed frame.
  ProtocolError = 1,
  /// No common protocol version or codec.
  UnsupportedVersion = 2,
  /// Missing, invalid or expired credentials.
  Unauthenticated = 3,
  /// Authenticated but not allowed.
  Forbidden = 4,
  /// Nothing received for `idle` ms.
  IdleTimeout = 5,
  /// Outbound queue full too long.
  SlowConsumer = 6,
  /// Server draining.
  GoingAway = 7,
  /// Unexpected server failure.
  Internal = 8,
  /// Frame exceeds `max_frame`.
  FrameTooLarge = 9,
  /// Server capacity or a session limit exceeded.
  LimitExceeded = 10,
  /// Resumption not possible.
  ResumeFailed = 11,
  /// Handshake not completed in time (§7.1).
  HandshakeTimeout = 12,
  /// Strict contract policy rejected the client.
  ContractMismatch = 13,
}

/// A call status code (§15.2), carried by ERROR, REQUEST_ERROR and AUTH_RESULT.
///
/// Values and meanings follow the gRPC status codes.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[repr(u16)]
pub enum StatusCode {
  /// Success; never sent in ERROR.
  Ok = 0,
  /// Caller cancelled.
  Cancelled = 1,
  /// Unknown error.
  Unknown = 2,
  /// Bad arguments, shape mismatch or invalid channel name.
  InvalidArgument = 3,
  /// Deadline expired.
  DeadlineExceeded = 4,
  /// Application-level "not found".
  NotFound = 5,
  /// Duplicate subscription or application conflict.
  AlreadyExists = 6,
  /// Authorization failed.
  PermissionDenied = 7,
  /// A limit was hit: concurrency, subscriptions or sizes.
  ResourceExhausted = 8,
  /// Application precondition not met.
  FailedPrecondition = 9,
  /// Concurrency conflict.
  Aborted = 10,
  /// Application range error.
  OutOfRange = 11,
  /// Unknown method.
  Unimplemented = 12,
  /// Unexpected failure.
  Internal = 13,
  /// Draining server or transport loss.
  Unavailable = 14,
  /// Unrecoverable data loss.
  DataLoss = 15,
  /// Missing, invalid or expired credentials.
  Unauthenticated = 16,
}

/// Why the server ended a subscription (§15.3).
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[repr(u16)]
pub enum UnsubscribeReason {
  /// Normal end.
  Normal = 0,
  /// The subscriber lost permission.
  PermissionRevoked = 1,
  /// The subscriber should subscribe again.
  ResubscribeRequired = 2,
  /// The channel was closed.
  ChannelClosed = 3,
}

/// The kind of a presence event (§15.4).
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[repr(u16)]
pub enum PresenceKind {
  /// All current members.
  Snapshot = 0,
  /// Members that joined.
  Join = 1,
  /// Members that left.
  Leave = 2,
  /// Members whose `info` changed without leaving.
  Update = 3,
}

/// A number that is not registered in one of the §15 registries.
///
/// Each variant names the registry and carries the rejected number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum UnknownCode {
  /// A number outside the session close codes (§15.1).
  #[error("unknown close code {0}")]
  Close(u16),
  /// A number outside the call status codes (§15.2).
  #[error("unknown status code {0}")]
  Status(u16),
  /// A number outside the unsubscribe reasons (§15.3).
  #[error("unknown unsubscribe reason {0}")]
  UnsubscribeReason(u16),
  /// A number outside the presence kinds (§15.4).
  #[error("unknown presence kind {0}")]
  PresenceKind(u16),
}

impl CloseCode {
  /// Maps a received number to a code, never failing: an unknown number becomes
  /// [`CloseCode::Internal`], so codes added by later drafts do not break older receivers (§15).
  pub fn from_u16_lossy(value: u16) -> Self {
    Self::try_from(value).unwrap_or(Self::Internal)
  }
}

impl From<CloseCode> for u16 {
  fn from(code: CloseCode) -> Self {
    code as u16
  }
}

impl TryFrom<u16> for CloseCode {
  type Error = UnknownCode;

  fn try_from(value: u16) -> Result<Self, Self::Error> {
    match value {
      0 => Ok(Self::Normal),
      1 => Ok(Self::ProtocolError),
      2 => Ok(Self::UnsupportedVersion),
      3 => Ok(Self::Unauthenticated),
      4 => Ok(Self::Forbidden),
      5 => Ok(Self::IdleTimeout),
      6 => Ok(Self::SlowConsumer),
      7 => Ok(Self::GoingAway),
      8 => Ok(Self::Internal),
      9 => Ok(Self::FrameTooLarge),
      10 => Ok(Self::LimitExceeded),
      11 => Ok(Self::ResumeFailed),
      12 => Ok(Self::HandshakeTimeout),
      13 => Ok(Self::ContractMismatch),
      _ => Err(UnknownCode::Close(value)),
    }
  }
}

impl StatusCode {
  /// Maps a received number to a code, never failing: an unknown number becomes
  /// [`StatusCode::Unknown`], so codes added by later drafts do not break older receivers (§15).
  pub fn from_u16_lossy(value: u16) -> Self {
    Self::try_from(value).unwrap_or(Self::Unknown)
  }
}

impl From<StatusCode> for u16 {
  fn from(code: StatusCode) -> Self {
    code as u16
  }
}

impl TryFrom<u16> for StatusCode {
  type Error = UnknownCode;

  fn try_from(value: u16) -> Result<Self, Self::Error> {
    match value {
      0 => Ok(Self::Ok),
      1 => Ok(Self::Cancelled),
      2 => Ok(Self::Unknown),
      3 => Ok(Self::InvalidArgument),
      4 => Ok(Self::DeadlineExceeded),
      5 => Ok(Self::NotFound),
      6 => Ok(Self::AlreadyExists),
      7 => Ok(Self::PermissionDenied),
      8 => Ok(Self::ResourceExhausted),
      9 => Ok(Self::FailedPrecondition),
      10 => Ok(Self::Aborted),
      11 => Ok(Self::OutOfRange),
      12 => Ok(Self::Unimplemented),
      13 => Ok(Self::Internal),
      14 => Ok(Self::Unavailable),
      15 => Ok(Self::DataLoss),
      16 => Ok(Self::Unauthenticated),
      _ => Err(UnknownCode::Status(value)),
    }
  }
}

impl UnsubscribeReason {
  /// Maps a received number to a code, never failing: an unknown number becomes
  /// [`UnsubscribeReason::ResubscribeRequired`], so codes added by later drafts do not break older receivers (§15).
  pub fn from_u16_lossy(value: u16) -> Self {
    Self::try_from(value).unwrap_or(Self::ResubscribeRequired)
  }
}

impl From<UnsubscribeReason> for u16 {
  fn from(reason: UnsubscribeReason) -> Self {
    reason as u16
  }
}

impl TryFrom<u16> for UnsubscribeReason {
  type Error = UnknownCode;

  fn try_from(value: u16) -> Result<Self, Self::Error> {
    match value {
      0 => Ok(Self::Normal),
      1 => Ok(Self::PermissionRevoked),
      2 => Ok(Self::ResubscribeRequired),
      3 => Ok(Self::ChannelClosed),
      _ => Err(UnknownCode::UnsubscribeReason(value)),
    }
  }
}

impl From<PresenceKind> for u16 {
  fn from(kind: PresenceKind) -> Self {
    kind as u16
  }
}

impl TryFrom<u16> for PresenceKind {
  type Error = UnknownCode;

  fn try_from(value: u16) -> Result<Self, Self::Error> {
    match value {
      0 => Ok(Self::Snapshot),
      1 => Ok(Self::Join),
      2 => Ok(Self::Leave),
      3 => Ok(Self::Update),
      _ => Err(UnknownCode::PresenceKind(value)),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::fmt::Debug;

  const CLOSE_CODES: [(u16, CloseCode); 14] = [
    (0, CloseCode::Normal),
    (1, CloseCode::ProtocolError),
    (2, CloseCode::UnsupportedVersion),
    (3, CloseCode::Unauthenticated),
    (4, CloseCode::Forbidden),
    (5, CloseCode::IdleTimeout),
    (6, CloseCode::SlowConsumer),
    (7, CloseCode::GoingAway),
    (8, CloseCode::Internal),
    (9, CloseCode::FrameTooLarge),
    (10, CloseCode::LimitExceeded),
    (11, CloseCode::ResumeFailed),
    (12, CloseCode::HandshakeTimeout),
    (13, CloseCode::ContractMismatch),
  ];

  const STATUS_CODES: [(u16, StatusCode); 17] = [
    (0, StatusCode::Ok),
    (1, StatusCode::Cancelled),
    (2, StatusCode::Unknown),
    (3, StatusCode::InvalidArgument),
    (4, StatusCode::DeadlineExceeded),
    (5, StatusCode::NotFound),
    (6, StatusCode::AlreadyExists),
    (7, StatusCode::PermissionDenied),
    (8, StatusCode::ResourceExhausted),
    (9, StatusCode::FailedPrecondition),
    (10, StatusCode::Aborted),
    (11, StatusCode::OutOfRange),
    (12, StatusCode::Unimplemented),
    (13, StatusCode::Internal),
    (14, StatusCode::Unavailable),
    (15, StatusCode::DataLoss),
    (16, StatusCode::Unauthenticated),
  ];

  const UNSUBSCRIBE_REASONS: [(u16, UnsubscribeReason); 4] = [
    (0, UnsubscribeReason::Normal),
    (1, UnsubscribeReason::PermissionRevoked),
    (2, UnsubscribeReason::ResubscribeRequired),
    (3, UnsubscribeReason::ChannelClosed),
  ];

  const PRESENCE_KINDS: [(u16, PresenceKind); 4] = [
    (0, PresenceKind::Snapshot),
    (1, PresenceKind::Join),
    (2, PresenceKind::Leave),
    (3, PresenceKind::Update),
  ];

  fn registered<T: Copy>(table: &[(u16, T)], code: u16) -> Option<T> {
    table.iter().find(|(number, _)| *number == code).map(|&(_, value)| value)
  }

  fn assert_converts_to_numbers<T>(table: &[(u16, T)])
  where
    T: Copy + Debug,
    u16: From<T>,
  {
    for &(number, value) in table {
      assert_eq!(u16::from(value), number, "{value:?} should be {number}");
    }
  }

  fn assert_parses_strictly<T>(table: &[(u16, T)], unknown: fn(u16) -> UnknownCode)
  where
    T: Copy + Debug + PartialEq + TryFrom<u16, Error = UnknownCode>,
  {
    for code in 0..=u16::MAX {
      let expected = registered(table, code).ok_or(unknown(code));
      assert_eq!(T::try_from(code), expected, "code {code}");
    }
  }

  fn assert_parses_leniently<T>(table: &[(u16, T)], lossy: fn(u16) -> T, fallback: T)
  where
    T: Copy + Debug + PartialEq,
  {
    for code in 0..=u16::MAX {
      let expected = registered(table, code).unwrap_or(fallback);
      assert_eq!(lossy(code), expected, "code {code}");
    }
  }

  #[test]
  fn converts_close_codes_to_numbers() {
    assert_converts_to_numbers(&CLOSE_CODES);
  }

  #[test]
  fn converts_status_codes_to_numbers() {
    assert_converts_to_numbers(&STATUS_CODES);
  }

  #[test]
  fn converts_unsubscribe_reasons_to_numbers() {
    assert_converts_to_numbers(&UNSUBSCRIBE_REASONS);
  }

  #[test]
  fn converts_presence_kinds_to_numbers() {
    assert_converts_to_numbers(&PRESENCE_KINDS);
  }

  #[test]
  fn parses_close_codes_strictly() {
    assert_parses_strictly(&CLOSE_CODES, UnknownCode::Close);
  }

  #[test]
  fn parses_status_codes_strictly() {
    assert_parses_strictly(&STATUS_CODES, UnknownCode::Status);
  }

  #[test]
  fn parses_unsubscribe_reasons_strictly() {
    assert_parses_strictly(&UNSUBSCRIBE_REASONS, UnknownCode::UnsubscribeReason);
  }

  #[test]
  fn parses_presence_kinds_strictly() {
    assert_parses_strictly(&PRESENCE_KINDS, UnknownCode::PresenceKind);
  }

  #[test]
  fn rejects_the_first_unregistered_number() {
    assert_eq!(CloseCode::try_from(14), Err(UnknownCode::Close(14)));
    assert_eq!(StatusCode::try_from(17), Err(UnknownCode::Status(17)));
    assert_eq!(UnsubscribeReason::try_from(4), Err(UnknownCode::UnsubscribeReason(4)));
    assert_eq!(PresenceKind::try_from(4), Err(UnknownCode::PresenceKind(4)));
  }

  #[test]
  fn maps_unknown_close_codes_to_internal() {
    assert_parses_leniently(&CLOSE_CODES, CloseCode::from_u16_lossy, CloseCode::Internal);
  }

  #[test]
  fn maps_unknown_status_codes_to_unknown() {
    assert_parses_leniently(&STATUS_CODES, StatusCode::from_u16_lossy, StatusCode::Unknown);
  }

  #[test]
  fn maps_unknown_unsubscribe_reasons_to_resubscribe_required() {
    assert_parses_leniently(
      &UNSUBSCRIBE_REASONS,
      UnsubscribeReason::from_u16_lossy,
      UnsubscribeReason::ResubscribeRequired,
    );
  }

  #[test]
  fn keeps_unauthenticated_apart_in_each_registry() {
    assert_eq!(u16::from(CloseCode::Unauthenticated), 3);
    assert_eq!(u16::from(StatusCode::Unauthenticated), 16);
    assert_eq!(StatusCode::try_from(3), Ok(StatusCode::InvalidArgument));
    assert_eq!(CloseCode::try_from(16), Err(UnknownCode::Close(16)));
  }

  #[test]
  fn names_the_registry_in_error_messages() {
    assert_eq!(UnknownCode::Close(14).to_string(), "unknown close code 14");
    assert_eq!(UnknownCode::Status(17).to_string(), "unknown status code 17");
    assert_eq!(UnknownCode::UnsubscribeReason(4).to_string(), "unknown unsubscribe reason 4");
    assert_eq!(UnknownCode::PresenceKind(4).to_string(), "unknown presence kind 4");
  }
}
