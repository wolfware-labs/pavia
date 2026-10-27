//! The error model (roadmap C1.12): every rejection in spec §5, §6 and §8.10 as one error type, and
//! the outcome each one calls for: end the session with a close code, fail one call or request
//! with a status code, or ignore the frame.

use crate::registry::{CloseCode, StatusCode};
use thiserror::Error;

/// A frame or data section the receiver rejects; [`ProtocolError::outcome`] says what happens next.
#[derive(Debug, Error)]
pub enum ProtocolError {
  /// A frame longer than the receiver's `max_frame` (close code FRAME_TOO_LARGE, §15.1).
  #[error("frame too large")]
  FrameTooLarge,
  /// A WebSocket message that does not parse as frames, such as a truncated frame (§5.4).
  #[error("malformed WebSocket message")]
  MalformedMessage,
  /// A WebTransport stream that ends (FIN) in the middle of a frame (§5.3).
  #[error("stream ended in the middle of a frame")]
  StreamEndedMidFrame,
  /// Bytes after the last complete frame of a WebSocket message (§5.4).
  #[error("bytes left over after the last frame in a message")]
  LeftoverBytes,
  /// A header section that is not a well-formed map (§5.4.1).
  #[error("malformed header section")]
  MalformedHeader,
  /// An unknown must-understand type (`0x00` to `0x7F`), or a known type where it is not allowed (§5.6).
  #[error("frame type not allowed here")]
  FrameTypeNotAllowed,
  /// A stream ID that breaks the opening order (§6.1): an opening frame at or below the
  /// initiator's highest opened ID, or a non-opening frame above it. Gaps between IDs are allowed.
  #[error("stream id out of order")]
  StreamIdOutOfOrder,
  /// A frame on a half that already sent its terminal frame (§6.3).
  #[error("frame after the terminal frame of its half")]
  FrameAfterTerminalFrame,
  /// An ITEM the sender had no credit for (§6.4).
  #[error("ITEM beyond the credit window")]
  ItemBeyondCreditWindow,
  /// More stream opens and CANCELs than the per-session rate allows (§6.5).
  #[error("stream open or CANCEL rate exceeded")]
  StreamRateExceeded,
  /// An unknown extension type (`0x80` to `0xFF`), skipped by its length (§5.6).
  #[error("unknown extension frame type")]
  UnknownExtensionFrameType,
  /// A non-opening frame for a stream at or below the highest opened ID that is no longer open (§6.1).
  #[error("frame on a closed stream")]
  FrameOnClosedStream,
  /// A frame that arrives after the call completed locally.
  #[error("frame after the call completed")]
  FrameAfterCallCompleted,
  /// A CALL received beyond the `max_calls` this side advertised (§6.5).
  #[error("CALL above max_calls")]
  CallAboveMaxCalls,
  /// A call this side was about to start beyond the peer's `max_calls` (§6.5). Nothing is sent.
  #[error("local CALL above the peer's max_calls")]
  LocalCallAboveMaxCalls,
  /// An ITEM this side was about to send that exceeds the peer's `max_frame`.
  #[error("ITEM larger than max_frame")]
  LocalItemTooLarge,
  /// A missing or ill-typed header field in a control request this side received: AUTH_REFRESH,
  /// SUBSCRIBE or UNSUBSCRIBE (§7.9). `rid` is the request's ID, answered with REQUEST_ERROR.
  #[error("invalid control request field")]
  InvalidControlRequestField { rid: u64 },
  /// A missing or ill-typed header field in any other frame, including a reply that carries a
  /// `rid`. Nothing can be answered, so the session ends.
  #[error("invalid header field")]
  InvalidHeaderField,
  /// A data section that does not decode against its contract type (§8.10).
  #[error("undecodable data section")]
  UndecodableData(DataSection),
}

/// Which data section failed to decode, and on which side for ITEMs (§8.10).
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum DataSection {
  /// CALL arguments, decoded by the callee before the handler runs (§8.2).
  CallArguments,
  /// An ITEM. `at_caller` is true when the caller decoded it and false when the callee did.
  Item { at_caller: bool },
  /// A RESULT, decoded by the caller.
  Result,
  /// The detail of an ERROR, decoded by the caller.
  ErrorDetail,
  /// A channel PUBLICATION (§10).
  Publication,
  /// One entry of a PRESENCE frame.
  PresenceEntry,
  /// The snapshot in a SUBSCRIBED frame.
  SubscribedSnapshot,
  /// The value of a DATAGRAM (§11).
  DatagramValue,
  /// NOTIFY arguments (§9.1).
  NotifyArguments,
}

/// A call status code other than [`StatusCode::Ok`]: the status of an error outcome (§8.4, §15.2).
///
/// ```
/// use pavia_proto::{error::ErrorStatus, registry::StatusCode};
///
/// assert!(ErrorStatus::try_from(StatusCode::Internal).is_ok());
/// assert!(ErrorStatus::try_from(StatusCode::Ok).is_err());
/// ```
///
/// Wrapping a status directly, skipping the check, does not compile:
///
/// ```compile_fail,E0603
/// use pavia_proto::{error::ErrorStatus, registry::StatusCode};
///
/// let status = ErrorStatus(StatusCode::Ok);
/// ```
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ErrorStatus(StatusCode);

/// What the session driver does about a [`ProtocolError`].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Outcome {
  /// Close the session with this code.
  SessionFatal(CloseCode),
  /// Answer the call with ERROR. `nx` is true when the handler never ran (§8.4).
  PerCall {
    /// The ERROR status.
    status: ErrorStatus,
    /// Whether the ERROR carries `nx: true`.
    nx: bool,
  },
  /// Answer the control request `rid` with REQUEST_ERROR (§7.9).
  PerRequest {
    /// The REQUEST_ERROR status.
    status: ErrorStatus,
    /// The ID of the request being answered.
    rid: u64,
  },
  /// Complete the call locally with this status, without sending ERROR.
  Local {
    /// The status the local caller sees.
    status: ErrorStatus,
    /// Whether to send CANCEL because the call is still open.
    cancel: bool,
    /// Whether the call never reached the peer, so no handler ran and a retry is safe (§8.4).
    nx: bool,
  },
  /// Ignore the frame. `event` is true when the application gets a local error event (§8.10).
  Dropped {
    /// Whether to surface a local error event.
    event: bool,
  },
}

/// The error for [`ErrorStatus::try_from`] given [`StatusCode::Ok`].
#[derive(Debug, Error, Copy, Clone, Eq, PartialEq)]
#[error("status OK is not an error status")]
pub struct OkIsNotAnError;

impl TryFrom<StatusCode> for ErrorStatus {
  type Error = OkIsNotAnError;

  fn try_from(value: StatusCode) -> Result<Self, Self::Error> {
    if value == StatusCode::Ok {
      Err(OkIsNotAnError)
    } else {
      Ok(Self(value))
    }
  }
}

impl From<ErrorStatus> for StatusCode {
  fn from(value: ErrorStatus) -> Self {
    value.0
  }
}

impl ProtocolError {
  /// The outcome the spec prescribes for this error. Classifying only; the driver acts on it.
  pub fn outcome(&self) -> Outcome {
    match self {
      Self::FrameTooLarge => Outcome::SessionFatal(CloseCode::FrameTooLarge),
      Self::MalformedMessage
      | Self::StreamEndedMidFrame
      | Self::LeftoverBytes
      | Self::MalformedHeader
      | Self::FrameTypeNotAllowed
      | Self::StreamIdOutOfOrder
      | Self::FrameAfterTerminalFrame
      | Self::ItemBeyondCreditWindow => Outcome::SessionFatal(CloseCode::ProtocolError),
      Self::StreamRateExceeded => Outcome::SessionFatal(CloseCode::LimitExceeded),
      Self::UnknownExtensionFrameType | Self::FrameOnClosedStream | Self::FrameAfterCallCompleted => {
        Outcome::Dropped { event: false }
      }
      Self::CallAboveMaxCalls => Outcome::PerCall {
        status: ErrorStatus(StatusCode::ResourceExhausted),
        nx: true,
      },
      Self::LocalCallAboveMaxCalls => Outcome::Local {
        status: ErrorStatus(StatusCode::ResourceExhausted),
        cancel: false,
        nx: true,
      },
      Self::LocalItemTooLarge => Outcome::Local {
        status: ErrorStatus(StatusCode::ResourceExhausted),
        cancel: false,
        nx: false,
      },
      Self::InvalidControlRequestField { rid } => Outcome::PerRequest {
        status: ErrorStatus(StatusCode::InvalidArgument),
        rid: *rid,
      },
      Self::InvalidHeaderField => Outcome::SessionFatal(CloseCode::ProtocolError),
      Self::UndecodableData(section) => match section {
        DataSection::CallArguments => Outcome::PerCall {
          status: ErrorStatus(StatusCode::InvalidArgument),
          nx: true,
        },
        DataSection::Item { at_caller: false } => Outcome::PerCall {
          status: ErrorStatus(StatusCode::InvalidArgument),
          nx: false,
        },
        DataSection::Item { at_caller: true } => Outcome::Local {
          status: ErrorStatus(StatusCode::Internal),
          cancel: true,
          nx: false,
        },
        DataSection::Result | DataSection::ErrorDetail => Outcome::Local {
          status: ErrorStatus(StatusCode::Internal),
          cancel: false,
          nx: false,
        },
        DataSection::Publication
        | DataSection::PresenceEntry
        | DataSection::SubscribedSnapshot
        | DataSection::DatagramValue => Outcome::Dropped { event: true },
        DataSection::NotifyArguments => Outcome::Dropped { event: false },
      },
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn ends_the_session_on_framing_and_stream_violations() {
    let cases = [
      (ProtocolError::FrameTooLarge, CloseCode::FrameTooLarge),
      (ProtocolError::MalformedMessage, CloseCode::ProtocolError),
      (ProtocolError::StreamEndedMidFrame, CloseCode::ProtocolError),
      (ProtocolError::LeftoverBytes, CloseCode::ProtocolError),
      (ProtocolError::MalformedHeader, CloseCode::ProtocolError),
      (ProtocolError::FrameTypeNotAllowed, CloseCode::ProtocolError),
      (ProtocolError::StreamIdOutOfOrder, CloseCode::ProtocolError),
      (ProtocolError::FrameAfterTerminalFrame, CloseCode::ProtocolError),
      (ProtocolError::ItemBeyondCreditWindow, CloseCode::ProtocolError),
      (ProtocolError::StreamRateExceeded, CloseCode::LimitExceeded),
    ];
    for (error, code) in cases {
      assert_eq!(error.outcome(), Outcome::SessionFatal(code), "{error:?}");
    }
  }

  #[test]
  fn drops_frames_the_peer_may_send_without_an_event() {
    let cases = [
      ProtocolError::UnknownExtensionFrameType,
      ProtocolError::FrameOnClosedStream,
      ProtocolError::FrameAfterCallCompleted,
    ];
    for error in cases {
      assert_eq!(error.outcome(), Outcome::Dropped { event: false }, "{error:?}");
    }
  }

  #[test]
  fn ignores_unknown_extension_types_but_not_required_ones() {
    assert_eq!(
      ProtocolError::UnknownExtensionFrameType.outcome(),
      Outcome::Dropped { event: false }
    );
    assert_eq!(
      ProtocolError::FrameTypeNotAllowed.outcome(),
      Outcome::SessionFatal(CloseCode::ProtocolError)
    );
  }

  #[test]
  fn fails_a_call_above_max_calls_without_running_it() {
    assert_eq!(
      ProtocolError::CallAboveMaxCalls.outcome(),
      Outcome::PerCall {
        status: ErrorStatus(StatusCode::ResourceExhausted),
        nx: true
      }
    );
  }

  #[test]
  fn fails_a_local_call_above_the_peer_max_calls_as_never_sent() {
    assert_eq!(
      ProtocolError::LocalCallAboveMaxCalls.outcome(),
      Outcome::Local {
        status: ErrorStatus(StatusCode::ResourceExhausted),
        cancel: false,
        nx: true
      }
    );
  }

  #[test]
  fn reports_an_oversized_local_item_without_cancelling() {
    assert_eq!(
      ProtocolError::LocalItemTooLarge.outcome(),
      Outcome::Local {
        status: ErrorStatus(StatusCode::ResourceExhausted),
        cancel: false,
        nx: false
      }
    );
  }

  #[test]
  fn answers_a_bad_field_in_a_control_request_with_request_error() {
    let subscribe = ProtocolError::InvalidControlRequestField { rid: 7 };
    assert_eq!(
      subscribe.outcome(),
      Outcome::PerRequest {
        status: ErrorStatus(StatusCode::InvalidArgument),
        rid: 7
      }
    );
  }

  #[test]
  fn ends_the_session_on_a_bad_field_outside_a_control_request() {
    let error = ProtocolError::InvalidHeaderField;
    assert_eq!(error.outcome(), Outcome::SessionFatal(CloseCode::ProtocolError));
  }

  fn undecodable(section: DataSection) -> Outcome {
    ProtocolError::UndecodableData(section).outcome()
  }

  #[test]
  fn fails_a_call_whose_arguments_do_not_decode_without_running_it() {
    assert_eq!(
      undecodable(DataSection::CallArguments),
      Outcome::PerCall {
        status: ErrorStatus(StatusCode::InvalidArgument),
        nx: true
      }
    );
  }

  #[test]
  fn fails_a_running_call_on_an_undecodable_item_at_the_callee() {
    assert_eq!(
      undecodable(DataSection::Item { at_caller: false }),
      Outcome::PerCall {
        status: ErrorStatus(StatusCode::InvalidArgument),
        nx: false
      }
    );
  }

  #[test]
  fn completes_the_call_locally_without_cancel_after_a_terminal_frame() {
    for section in [DataSection::Result, DataSection::ErrorDetail] {
      assert_eq!(
        undecodable(section),
        Outcome::Local {
          status: ErrorStatus(StatusCode::Internal),
          cancel: false,
          nx: false
        },
        "{section:?}"
      );
    }
  }

  #[test]
  fn cancels_an_open_stream_on_an_undecodable_item_at_the_caller() {
    assert_eq!(
      undecodable(DataSection::Item { at_caller: true }),
      Outcome::Local {
        status: ErrorStatus(StatusCode::Internal),
        cancel: true,
        nx: false
      }
    );
  }

  #[test]
  fn drops_undecodable_channel_and_datagram_data_with_an_event() {
    let sections = [
      DataSection::Publication,
      DataSection::PresenceEntry,
      DataSection::SubscribedSnapshot,
      DataSection::DatagramValue,
    ];
    for section in sections {
      assert_eq!(undecodable(section), Outcome::Dropped { event: true }, "{section:?}");
    }
  }

  #[test]
  fn drops_undecodable_notify_arguments_without_an_event() {
    assert_eq!(undecodable(DataSection::NotifyArguments), Outcome::Dropped { event: false });
  }

  #[test]
  fn never_ends_the_session_on_undecodable_data() {
    let sections = [
      DataSection::CallArguments,
      DataSection::Item { at_caller: false },
      DataSection::Item { at_caller: true },
      DataSection::Result,
      DataSection::ErrorDetail,
      DataSection::Publication,
      DataSection::PresenceEntry,
      DataSection::SubscribedSnapshot,
      DataSection::DatagramValue,
      DataSection::NotifyArguments,
    ];
    for section in sections {
      let outcome = undecodable(section);
      assert!(!matches!(outcome, Outcome::SessionFatal(_)), "{section:?} gave {outcome:?}");
    }
  }

  #[test]
  fn rejects_ok_as_an_error_status() {
    assert!(matches!(ErrorStatus::try_from(StatusCode::Ok), Err(OkIsNotAnError)));
  }

  #[test]
  fn explains_why_ok_is_rejected() {
    assert_eq!(OkIsNotAnError.to_string(), "status OK is not an error status");
  }

  #[test]
  fn gives_back_the_wrapped_status() {
    for code in 1..=16 {
      let status = StatusCode::try_from(code).expect("codes 1 to 16 are registered");
      let wrapped = ErrorStatus::try_from(status).expect("only OK is rejected");
      assert_eq!(StatusCode::from(wrapped), status);
    }
  }

  #[test]
  fn accepts_every_other_status_code() {
    for code in 1..=16 {
      let status = StatusCode::try_from(code).expect("codes 1 to 16 are registered");
      assert!(ErrorStatus::try_from(status).is_ok(), "{status:?} should be an error status");
    }
  }
}
