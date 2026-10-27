//! Typed frames (roadmap C1.5): one type per entry of the frame type registry (spec §5.6), with
//! the header fields of §5.7 as typed members.
//!
//! Inside the crate, `Frame::from_raw` turns a raw frame from the wire codec into a typed one and
//! checks the fields that every frame of its type must carry, and `Frame::encode` writes one
//! straight to bytes without allocating.
//! Fields that only some sessions require, such as `seq` when resumption is granted, are optional
//! here and checked by the session.
//!
//! Data sections stay raw bytes, because their meaning depends on the session codec. Flags are not
//! part of the model: every bit is reserved in v1, so a received value is ignored and every typed
//! frame is written with flags 0 (§5.2).

#[cfg_attr(not(test), expect(dead_code, reason = "used by the session from #24 onwards"))]
mod fields;
#[cfg_attr(not(test), expect(dead_code, reason = "used by the session from #24 onwards"))]
mod raw;

use std::collections::BTreeMap;

use bytes::Bytes;
use thiserror::Error;

pub use self::fields::Problem as FieldProblem;

/// The flag bits reserved in v1: all of them (§5.2). Senders write 0 and receivers ignore them.
pub const RESERVED_FLAGS: u8 = 0xFF;

/// Header keys (§5.7).
pub mod key {
  /// `rid`, the control request ID.
  pub const RID: u64 = 1;
  /// `method`.
  pub const METHOD: u64 = 2;
  /// `meta`.
  pub const META: u64 = 3;
  /// `deadline`, in ms.
  pub const DEADLINE: u64 = 4;
  /// `shape`.
  pub const SHAPE: u64 = 5;
  /// `code`.
  pub const CODE: u64 = 6;
  /// `msg`.
  pub const MSG: u64 = 7;
  /// `lane`.
  pub const LANE: u64 = 8;
  /// `channel`.
  pub const CHANNEL: u64 = 9;
  /// `epoch`.
  pub const EPOCH: u64 = 10;
  /// `offset`.
  pub const OFFSET: u64 = 11;
  /// `seq`.
  pub const SEQ: u64 = 12;
  /// `token`.
  pub const TOKEN: u64 = 13;
  /// `versions`.
  pub const VERSIONS: u64 = 14;
  /// `version`.
  pub const VERSION: u64 = 15;
  /// `codecs`.
  pub const CODECS: u64 = 16;
  /// `codec`.
  pub const CODEC: u64 = 17;
  /// `caps`.
  pub const CAPS: u64 = 18;
  /// `session`.
  pub const SESSION: u64 = 19;
  /// `resume`.
  pub const RESUME: u64 = 20;
  /// `limits`.
  pub const LIMITS: u64 = 21;
  /// `hb`, in ms.
  pub const HB: u64 = 22;
  /// `idle`, in ms.
  pub const IDLE: u64 = 23;
  /// `contract`.
  pub const CONTRACT: u64 = 24;
  /// `credit`.
  pub const CREDIT: u64 = 25;
  /// `recovered`.
  pub const RECOVERED: u64 = 26;
  /// `presence`.
  pub const PRESENCE: u64 = 27;
  /// `kind`.
  pub const KIND: u64 = 28;
  /// `key`.
  pub const KEY: u64 = 29;
  /// `name`.
  pub const NAME: u64 = 30;
  /// `retry`, in ms.
  pub const RETRY: u64 = 31;
  /// `url`.
  pub const URL: u64 = 32;
  /// `auth`.
  pub const AUTH: u64 = 33;
  /// `client`.
  pub const CLIENT: u64 = 34;
  /// `scheme`, inside `auth`.
  pub const SCHEME: u64 = 35;
  /// `user`.
  pub const USER: u64 = 36;
  /// `resume_window`, in ms.
  pub const RESUME_WINDOW: u64 = 37;
  /// `expires`, in ms.
  pub const EXPIRES: u64 = 38;
  /// `window`, inside `limits`.
  pub const WINDOW: u64 = 40;
  /// `since`.
  pub const SINCE: u64 = 41;
  /// `resumed`.
  pub const RESUMED: u64 = 43;
  /// `ack`.
  pub const ACK: u64 = 44;
  /// `nx`.
  pub const NX: u64 = 45;
  /// `transports`.
  pub const TRANSPORTS: u64 = 46;
  /// `max_calls`, inside `limits`.
  pub const MAX_CALLS: u64 = 48;
  /// `max_frame`, inside `limits`.
  pub const MAX_FRAME: u64 = 49;
  /// `max_subs`, inside `limits`.
  pub const MAX_SUBS: u64 = 50;
  /// `max_dgram`, inside `limits`.
  pub const MAX_DGRAM: u64 = 51;
  /// `version` as text, inside `client`.
  pub const CLIENT_VERSION: u64 = 52;
  /// `lost`.
  pub const LOST: u64 = 53;
}

/// A registered frame type (§5.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum FrameType {
  /// HELLO (0x01).
  Hello = 0x01,
  /// WELCOME (0x02).
  Welcome = 0x02,
  /// REJECT (0x03).
  Reject = 0x03,
  /// PING (0x04).
  Ping = 0x04,
  /// PONG (0x05).
  Pong = 0x05,
  /// GOAWAY (0x06).
  Goaway = 0x06,
  /// CLOSE (0x07).
  Close = 0x07,
  /// AUTH_REFRESH (0x08).
  AuthRefresh = 0x08,
  /// AUTH_RESULT (0x09).
  AuthResult = 0x09,
  /// ACK (0x0A).
  Ack = 0x0A,
  /// AUTH_EXPIRING (0x0B).
  AuthExpiring = 0x0B,
  /// REQUEST_ERROR (0x0C).
  RequestError = 0x0C,
  /// CALL (0x10).
  Call = 0x10,
  /// ITEM (0x11).
  Item = 0x11,
  /// END (0x12).
  End = 0x12,
  /// RESULT (0x13).
  Result = 0x13,
  /// ERROR (0x14).
  Error = 0x14,
  /// CANCEL (0x15).
  Cancel = 0x15,
  /// CREDIT (0x16).
  Credit = 0x16,
  /// NOTIFY (0x20).
  Notify = 0x20,
  /// LANE_OPEN (0x21).
  LaneOpen = 0x21,
  /// SUBSCRIBE (0x30).
  Subscribe = 0x30,
  /// SUBSCRIBED (0x31).
  Subscribed = 0x31,
  /// UNSUBSCRIBE (0x32).
  Unsubscribe = 0x32,
  /// UNSUBSCRIBED (0x33).
  Unsubscribed = 0x33,
  /// PUBLICATION (0x34).
  Publication = 0x34,
  /// PRESENCE (0x35).
  Presence = 0x35,
  /// DATAGRAM (0x40).
  Datagram = 0x40,
  /// DGRAM_BIND (0x41).
  DgramBind = 0x41,
}

impl FrameType {
  /// Every registered type, in registry order.
  pub const ALL: [FrameType; 29] = [
    Self::Hello,
    Self::Welcome,
    Self::Reject,
    Self::Ping,
    Self::Pong,
    Self::Goaway,
    Self::Close,
    Self::AuthRefresh,
    Self::AuthResult,
    Self::Ack,
    Self::AuthExpiring,
    Self::RequestError,
    Self::Call,
    Self::Item,
    Self::End,
    Self::Result,
    Self::Error,
    Self::Cancel,
    Self::Credit,
    Self::Notify,
    Self::LaneOpen,
    Self::Subscribe,
    Self::Subscribed,
    Self::Unsubscribe,
    Self::Unsubscribed,
    Self::Publication,
    Self::Presence,
    Self::Datagram,
    Self::DgramBind,
  ];

  /// Whether frames of this type have a data section. The others must send it empty.
  pub fn carries_data(self) -> bool {
    matches!(
      self,
      Self::Ping
        | Self::Pong
        | Self::Call
        | Self::Item
        | Self::Result
        | Self::Error
        | Self::Notify
        | Self::Subscribe
        | Self::Subscribed
        | Self::Publication
        | Self::Presence
        | Self::Datagram
    )
  }

  /// Whether this is a control request that a REQUEST_ERROR can answer (§7.9).
  pub fn is_control_request(self) -> bool {
    matches!(self, Self::AuthRefresh | Self::Subscribe | Self::Unsubscribe)
  }
}

impl From<FrameType> for u8 {
  fn from(frame_type: FrameType) -> Self {
    frame_type as u8
  }
}

impl TryFrom<u8> for FrameType {
  type Error = u8;

  fn try_from(value: u8) -> Result<Self, u8> {
    Self::ALL
      .into_iter()
      .find(|frame_type| u8::from(*frame_type) == value)
      .ok_or(value)
  }
}

/// A lane-to-seq map (key 44): the highest contiguous seq received per lane (§7.8).
pub type AckMap = BTreeMap<u64, u64>;

/// Call metadata (key 3): text keys to text or bytes (§8.7).
pub type Meta = BTreeMap<String, MetaValue>;

/// One metadata value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetaValue {
  /// A text value.
  Text(String),
  /// A byte string value.
  Bytes(Bytes),
}

/// A credential or resume token (key 13): bytes or text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
  /// A byte string token.
  Bytes(Bytes),
  /// A text token.
  Text(String),
}

/// The data section of a PING or PONG: at most 8 opaque bytes (§7.4).
///
/// ```
/// use bytes::Bytes;
/// use pavia_proto::frame::Heartbeat;
///
/// assert!(Heartbeat::try_from(Bytes::from_static(b"12345678")).is_ok());
/// assert!(Heartbeat::try_from(Bytes::from_static(b"123456789")).is_err());
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Heartbeat(Bytes);

impl Heartbeat {
  /// The longest data section a PING or PONG may carry.
  pub const MAX_LEN: usize = 8;

  /// The opaque bytes.
  pub fn as_bytes(&self) -> &Bytes {
    &self.0
  }
}

/// The error for [`Heartbeat::try_from`] given more than [`Heartbeat::MAX_LEN`] bytes.
#[derive(Debug, Error, Copy, Clone, Eq, PartialEq)]
#[error("heartbeat data section longer than 8 bytes")]
pub struct HeartbeatTooLarge;

impl TryFrom<Bytes> for Heartbeat {
  type Error = HeartbeatTooLarge;

  fn try_from(value: Bytes) -> Result<Self, Self::Error> {
    if value.len() > Self::MAX_LEN {
      Err(HeartbeatTooLarge)
    } else {
      Ok(Self(value))
    }
  }
}

impl From<Heartbeat> for Bytes {
  fn from(value: Heartbeat) -> Self {
    value.0
  }
}

/// The `limits` map of HELLO (key 21, §7.1). Every entry is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Limits {
  /// `max_calls` (key 48).
  pub max_calls: Option<u64>,
  /// `max_frame` (key 49), in bytes.
  pub max_frame: Option<u64>,
  /// `max_subs` (key 50).
  pub max_subs: Option<u64>,
  /// `max_dgram` (key 51), in bytes.
  pub max_dgram: Option<u64>,
  /// `window` (key 40), the initial per-stream WebSocket credit in bytes.
  pub window: Option<u64>,
}

/// The `limits` map of WELCOME (key 21, §7.2). Only `window` is optional, because it applies to
/// WebSocket alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerLimits {
  /// `max_calls` (key 48): concurrent call streams the client may open.
  pub max_calls: u64,
  /// `max_frame` (key 49), in bytes.
  pub max_frame: u64,
  /// `max_subs` (key 50).
  pub max_subs: u64,
  /// `max_dgram` (key 51), in bytes.
  pub max_dgram: u64,
  /// `window` (key 40), the initial per-stream WebSocket credit in bytes.
  pub window: Option<u64>,
}

/// The `auth` map (key 33, §7.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Auth {
  /// `scheme` (key 35), such as `"bearer"`.
  pub scheme: String,
  /// `token` (key 13).
  pub token: Token,
}

/// The `resume` map of HELLO (key 20, §7.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resume {
  /// `token` (key 13).
  pub token: Token,
  /// `session` (key 19), the session ID.
  pub session: String,
  /// `ack` (key 44).
  pub ack: AckMap,
  /// `lost` (key 53): lanes with a gap.
  pub lost: Option<Vec<u64>>,
}

/// The `client` map of HELLO (key 34).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientInfo {
  /// `name` (key 30).
  pub name: Option<String>,
  /// `version` (key 52), the client library version.
  pub version: Option<String>,
}

/// The `since` map of SUBSCRIBE (key 41, §10.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Since {
  /// `epoch` (key 10).
  pub epoch: String,
  /// `offset` (key 11).
  pub offset: u64,
}

/// `seq` and a piggybacked `ack`, carried by sequenced frames (§7.8).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sequencing {
  /// The frame's seq, which the session requires when `resume` is granted.
  pub seq: Option<u64>,
  /// An acknowledgment piggybacked on this frame.
  pub ack: Option<AckMap>,
}

/// HELLO (§7.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
  /// `versions` (key 14): protocol major versions, in preference order.
  pub versions: Vec<u64>,
  /// `codecs` (key 16), in preference order.
  pub codecs: Vec<String>,
  /// `caps` (key 18): capabilities requested or granted.
  pub caps: Option<Vec<String>>,
  /// `limits` (key 21).
  pub limits: Option<Limits>,
  /// `contract` (key 24), the SHA-256 manifest fingerprint.
  pub contract: Option<[u8; 32]>,
  /// `auth` (key 33).
  pub auth: Option<Auth>,
  /// `resume` (key 20).
  pub resume: Option<Resume>,
  /// `client` (key 34).
  pub client: Option<ClientInfo>,
}

/// WELCOME (§7.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Welcome {
  /// `version` (key 15).
  pub version: u64,
  /// `codec` (key 17).
  pub codec: String,
  /// `caps` (key 18): capabilities requested or granted.
  pub caps: Vec<String>,
  /// `session` (key 19), the session ID.
  pub session: String,
  /// `token` (key 13).
  pub token: Option<Token>,
  /// `limits` (key 21).
  pub limits: ServerLimits,
  /// `hb` (key 22), the heartbeat interval in ms.
  pub hb: u64,
  /// `idle` (key 23), the idle timeout in ms.
  pub idle: u64,
  /// `resume_window` (key 37), in ms.
  pub resume_window: Option<u64>,
  /// `contract` (key 24), the SHA-256 manifest fingerprint.
  pub contract: Option<[u8; 32]>,
  /// `user` (key 36), the authenticated user ID.
  pub user: Option<String>,
  /// `resumed` (key 43).
  pub resumed: Option<bool>,
  /// `ack` (key 44).
  pub ack: Option<AckMap>,
  /// `lost` (key 53): lanes with a gap.
  pub lost: Option<Vec<u64>>,
  /// `transports` (key 46): bindings offered on this endpoint.
  pub transports: Option<Vec<String>>,
}

/// REJECT (§7.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reject {
  /// `code` (key 6).
  pub code: u64,
  /// `msg` (key 7).
  pub msg: String,
  /// `retry` (key 31), the minimum delay in ms before reconnecting.
  pub retry: Option<u64>,
}

/// GOAWAY (§7.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Goaway {
  /// `code` (key 6).
  pub code: u64,
  /// `msg` (key 7).
  pub msg: String,
  /// `url` (key 32), the endpoint to reconnect to.
  pub url: Option<String>,
  /// `retry` (key 31), the minimum delay in ms before reconnecting.
  pub retry: Option<u64>,
}

/// CLOSE (§7.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Close {
  /// `code` (key 6).
  pub code: u64,
  /// `msg` (key 7).
  pub msg: Option<String>,
}

/// AUTH_REFRESH (§7.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRefresh {
  /// `rid` (key 1), the control request ID.
  pub rid: u64,
  /// `auth` (key 33).
  pub auth: Auth,
  /// `seq` and a piggybacked `ack` (§7.8).
  pub sequencing: Sequencing,
}

/// AUTH_RESULT (§7.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthResult {
  /// `rid` (key 1), the control request ID.
  pub rid: u64,
  /// `code` (key 6).
  pub code: u64,
  /// `expires` (key 38), ms until the credential expires.
  pub expires: Option<u64>,
  /// `seq` and a piggybacked `ack` (§7.8).
  pub sequencing: Sequencing,
}

/// AUTH_EXPIRING (§7.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthExpiring {
  /// `expires` (key 38), ms until the credential expires.
  pub expires: u64,
  /// `seq` and a piggybacked `ack` (§7.8).
  pub sequencing: Sequencing,
}

/// REQUEST_ERROR (§7.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestError {
  /// `rid` (key 1), the control request ID.
  pub rid: u64,
  /// `code` (key 6).
  pub code: u64,
  /// `msg` (key 7).
  pub msg: String,
  /// `seq` and a piggybacked `ack` (§7.8).
  pub sequencing: Sequencing,
}

/// CALL (§8.2). The data section holds the arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
  /// `method` (key 2).
  pub method: String,
  /// `shape` (key 5).
  pub shape: u64,
  /// `deadline` (key 4), in ms from the moment the callee receives the CALL.
  pub deadline: Option<u64>,
  /// `meta` (key 3).
  pub meta: Option<Meta>,
  /// The data section.
  pub data: Bytes,
}

/// ITEM (§8.3). The data section holds one stream element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
  /// `meta` (key 3).
  pub meta: Option<Meta>,
  /// The data section.
  pub data: Bytes,
}

/// END (§8.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct End {
  /// `meta` (key 3).
  pub meta: Option<Meta>,
}

/// RESULT (§8.4). The data section holds the return value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallResult {
  /// `meta` (key 3).
  pub meta: Option<Meta>,
  /// The data section.
  pub data: Bytes,
}

/// ERROR (§8.4). The data section holds the typed error detail, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallError {
  /// `code` (key 6).
  pub code: u64,
  /// `msg` (key 7).
  pub msg: String,
  /// `nx` (key 45): the handler never ran.
  pub nx: Option<bool>,
  /// `meta` (key 3).
  pub meta: Option<Meta>,
  /// The data section.
  pub data: Bytes,
}

/// CANCEL (§8.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cancel {
  /// `msg` (key 7).
  pub msg: Option<String>,
}

/// CREDIT (§6.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credit {
  /// `credit` (key 25), the window increment in bytes.
  pub credit: u64,
}

/// NOTIFY (§9.1). The data section holds the arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notify {
  /// `method` (key 2).
  pub method: String,
  /// `meta` (key 3).
  pub meta: Option<Meta>,
  /// `seq` and a piggybacked `ack` (§7.8).
  pub sequencing: Sequencing,
  /// The data section.
  pub data: Bytes,
}

/// LANE_OPEN (§9.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneOpen {
  /// `lane` (key 8).
  pub lane: u64,
}

/// SUBSCRIBE (§10.2). The data section holds optional subscription parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscribe {
  /// `rid` (key 1), the control request ID.
  pub rid: u64,
  /// `channel` (key 9).
  pub channel: String,
  /// `since` (key 41).
  pub since: Option<Since>,
  /// `presence` (key 27).
  pub presence: Option<bool>,
  /// `meta` (key 3).
  pub meta: Option<Meta>,
  /// `seq` and a piggybacked `ack` (§7.8).
  pub sequencing: Sequencing,
  /// The data section.
  pub data: Bytes,
}

/// SUBSCRIBED (§10.2). The data section holds the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscribed {
  /// `rid` (key 1), the control request ID.
  pub rid: u64,
  /// `channel` (key 9).
  pub channel: String,
  /// `epoch` (key 10).
  pub epoch: String,
  /// `offset` (key 11).
  pub offset: u64,
  /// `recovered` (key 26).
  pub recovered: Option<bool>,
  /// `seq` and a piggybacked `ack` (§7.8).
  pub sequencing: Sequencing,
  /// The data section.
  pub data: Bytes,
}

/// UNSUBSCRIBE (§10.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsubscribe {
  /// `rid` (key 1), the control request ID.
  pub rid: u64,
  /// `channel` (key 9).
  pub channel: String,
  /// `seq` and a piggybacked `ack` (§7.8).
  pub sequencing: Sequencing,
}

/// UNSUBSCRIBED (§10.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsubscribed {
  /// `channel` (key 9).
  pub channel: String,
  /// Which of the two forms this is.
  pub form: UnsubscribedForm,
  /// `seq` and a piggybacked `ack` (§7.8).
  pub sequencing: Sequencing,
}

/// The two forms of UNSUBSCRIBED (§10.3). Each forbids the other's fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnsubscribedForm {
  /// The reply to an UNSUBSCRIBE: `{1: rid, 9: channel}`.
  Reply {
    /// `rid` (key 1) of the UNSUBSCRIBE being answered.
    rid: u64,
  },
  /// A server-initiated notice: `{9: channel, 6: code, 7: msg}`, with an unsubscribe reason
  /// (§15.3) as the code.
  Notice {
    /// `code` (key 6), an unsubscribe reason.
    code: u64,
    /// `msg` (key 7).
    msg: String,
  },
}

/// PUBLICATION (§10.4). The data section holds the published value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
  /// `channel` (key 9).
  pub channel: String,
  /// `offset` (key 11).
  pub offset: u64,
  /// `seq` and a piggybacked `ack` (§7.8).
  pub sequencing: Sequencing,
  /// The data section.
  pub data: Bytes,
}

/// PRESENCE (§10.6). The data section holds the entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presence {
  /// `channel` (key 9).
  pub channel: String,
  /// `kind` (key 28): 0 snapshot, 1 join, 2 leave, 3 update.
  pub kind: u64,
  /// `seq` and a piggybacked `ack` (§7.8).
  pub sequencing: Sequencing,
  /// The data section.
  pub data: Bytes,
}

/// DATAGRAM on the WebSocket control stream (§11.4). The data section holds the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datagram {
  /// `key` (key 29), the datagram key.
  pub key: u64,
  /// `seq` (key 12).
  pub seq: u64,
  /// The data section.
  pub data: Bytes,
}

/// DGRAM_BIND (§11.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DgramBind {
  /// `key` (key 29), the datagram key.
  pub key: u64,
  /// `name` (key 30).
  pub name: String,
  /// `seq` and a piggybacked `ack` (§7.8).
  pub sequencing: Sequencing,
}

/// A frame with an extension type (`0x80` to `0xFF`), kept as received so the caller can drop it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extension {
  /// The frame type byte.
  pub frame_type: u8,
  /// The header section, unparsed.
  pub header: Bytes,
  /// The data section.
  pub data: Bytes,
}

/// One typed frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
  /// HELLO.
  Hello(Hello),
  /// WELCOME.
  Welcome(Welcome),
  /// REJECT.
  Reject(Reject),
  /// PING (§7.4).
  Ping {
    /// The data section, echoed by PONG.
    data: Heartbeat,
  },
  /// PONG (§7.4).
  Pong {
    /// The data section of the PING being answered.
    data: Heartbeat,
  },
  /// GOAWAY.
  Goaway(Goaway),
  /// CLOSE.
  Close(Close),
  /// AUTH_REFRESH.
  AuthRefresh(AuthRefresh),
  /// AUTH_RESULT.
  AuthResult(AuthResult),
  /// ACK, with the `ack` map (key 44, §7.8).
  Ack {
    /// `ack` (key 44).
    ack: AckMap,
  },
  /// AUTH_EXPIRING.
  AuthExpiring(AuthExpiring),
  /// REQUEST_ERROR.
  RequestError(RequestError),
  /// CALL.
  Call(Call),
  /// ITEM.
  Item(Item),
  /// END.
  End(End),
  /// RESULT.
  Result(CallResult),
  /// ERROR.
  Error(CallError),
  /// CANCEL.
  Cancel(Cancel),
  /// CREDIT.
  Credit(Credit),
  /// NOTIFY.
  Notify(Notify),
  /// LANE_OPEN.
  LaneOpen(LaneOpen),
  /// SUBSCRIBE.
  Subscribe(Subscribe),
  /// SUBSCRIBED.
  Subscribed(Subscribed),
  /// UNSUBSCRIBE.
  Unsubscribe(Unsubscribe),
  /// UNSUBSCRIBED.
  Unsubscribed(Unsubscribed),
  /// PUBLICATION.
  Publication(Publication),
  /// PRESENCE.
  Presence(Presence),
  /// DATAGRAM.
  Datagram(Datagram),
  /// DGRAM_BIND.
  DgramBind(DgramBind),
  /// A frame with an extension type, to be dropped by the caller.
  Extension(Extension),
}

/// A header field that is missing or has the wrong type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("{problem:?} field {key} in {frame_type:?}")]
pub struct FieldError {
  /// The frame the field belongs to.
  pub frame_type: FrameType,
  /// The header key. For a bad entry inside a nested map, the key of that map.
  pub key: u64,
  /// What is wrong.
  pub problem: FieldProblem,
  /// The frame's `rid`, when it decoded, so a control request can be answered (§7.9).
  pub rid: Option<u64>,
}

/// Error returned when a raw frame does not make a valid typed frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum FrameError {
  /// A must-understand type (`0x00` to `0x7F`) that is not in the registry (§5.6).
  #[error("unknown must-understand frame type {0:#04x}")]
  UnknownType(u8),
  /// The header section is not valid deterministic CBOR (§5.4.1).
  #[error("malformed header section")]
  MalformedHeader,
  /// A non-empty data section on a frame type without one, such as END (§8.3).
  #[error("data section on a frame type without one")]
  UnexpectedData,
  /// A PING or PONG data section longer than 8 bytes (§7.4).
  #[error(transparent)]
  HeartbeatTooLarge(#[from] HeartbeatTooLarge),
  /// A field is missing or has the wrong type.
  #[error(transparent)]
  Field(#[from] FieldError),
}

impl Frame {
  /// The registered type, or `None` for an extension frame.
  pub fn frame_type(&self) -> Option<FrameType> {
    Some(match self {
      Self::Hello(_) => FrameType::Hello,
      Self::Welcome(_) => FrameType::Welcome,
      Self::Reject(_) => FrameType::Reject,
      Self::Ping { .. } => FrameType::Ping,
      Self::Pong { .. } => FrameType::Pong,
      Self::Goaway(_) => FrameType::Goaway,
      Self::Close(_) => FrameType::Close,
      Self::AuthRefresh(_) => FrameType::AuthRefresh,
      Self::AuthResult(_) => FrameType::AuthResult,
      Self::Ack { .. } => FrameType::Ack,
      Self::AuthExpiring(_) => FrameType::AuthExpiring,
      Self::RequestError(_) => FrameType::RequestError,
      Self::Call(_) => FrameType::Call,
      Self::Item(_) => FrameType::Item,
      Self::End(_) => FrameType::End,
      Self::Result(_) => FrameType::Result,
      Self::Error(_) => FrameType::Error,
      Self::Cancel(_) => FrameType::Cancel,
      Self::Credit(_) => FrameType::Credit,
      Self::Notify(_) => FrameType::Notify,
      Self::LaneOpen(_) => FrameType::LaneOpen,
      Self::Subscribe(_) => FrameType::Subscribe,
      Self::Subscribed(_) => FrameType::Subscribed,
      Self::Unsubscribe(_) => FrameType::Unsubscribe,
      Self::Unsubscribed(_) => FrameType::Unsubscribed,
      Self::Publication(_) => FrameType::Publication,
      Self::Presence(_) => FrameType::Presence,
      Self::Datagram(_) => FrameType::Datagram,
      Self::DgramBind(_) => FrameType::DgramBind,
      Self::Extension(_) => return None,
    })
  }
}

#[cfg(test)]
mod tests;
