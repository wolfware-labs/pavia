//! Typed frames (roadmap C1.5): one type per entry of the frame type registry (spec §5.6), with
//! the header fields of §5.7 as typed members.
//!
//! Inside the crate, `Frame::from_raw` turns a raw frame from the wire codec into a typed one and
//! checks the fields that every frame of its type must carry, and `Frame::to_raw` goes back.
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

use crate::error::ProtocolError;

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
  Hello = 0x01,
  Welcome = 0x02,
  Reject = 0x03,
  Ping = 0x04,
  Pong = 0x05,
  Goaway = 0x06,
  Close = 0x07,
  AuthRefresh = 0x08,
  AuthResult = 0x09,
  Ack = 0x0A,
  AuthExpiring = 0x0B,
  RequestError = 0x0C,
  Call = 0x10,
  Item = 0x11,
  End = 0x12,
  Result = 0x13,
  Error = 0x14,
  Cancel = 0x15,
  Credit = 0x16,
  Notify = 0x20,
  LaneOpen = 0x21,
  Subscribe = 0x30,
  Subscribed = 0x31,
  Unsubscribe = 0x32,
  Unsubscribed = 0x33,
  Publication = 0x34,
  Presence = 0x35,
  Datagram = 0x40,
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

/// The `limits` map (key 21, §7.2). Every entry is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Limits {
  pub max_calls: Option<u64>,
  pub max_frame: Option<u64>,
  pub max_subs: Option<u64>,
  pub max_dgram: Option<u64>,
  pub window: Option<u64>,
}

/// The `auth` map (key 33, §7.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Auth {
  pub scheme: String,
  pub token: Token,
}

/// The `resume` map of HELLO (key 20, §7.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resume {
  pub token: Token,
  pub session: String,
  pub ack: AckMap,
  pub lost: Option<Vec<u64>>,
}

/// The `client` map of HELLO (key 34).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientInfo {
  pub name: Option<String>,
  pub version: Option<String>,
}

/// The `since` map of SUBSCRIBE (key 41, §10.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Since {
  pub epoch: String,
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
  pub versions: Vec<u64>,
  pub codecs: Vec<String>,
  pub caps: Option<Vec<String>>,
  pub limits: Option<Limits>,
  pub contract: Option<[u8; 32]>,
  pub auth: Option<Auth>,
  pub resume: Option<Resume>,
  pub client: Option<ClientInfo>,
}

/// WELCOME (§7.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Welcome {
  pub version: u64,
  pub codec: String,
  pub caps: Vec<String>,
  pub session: String,
  pub token: Option<Token>,
  pub limits: Limits,
  pub hb: u64,
  pub idle: u64,
  pub resume_window: Option<u64>,
  pub contract: Option<[u8; 32]>,
  pub user: Option<String>,
  pub resumed: Option<bool>,
  pub ack: Option<AckMap>,
  pub lost: Option<Vec<u64>>,
  pub transports: Option<Vec<String>>,
}

/// REJECT (§7.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reject {
  pub code: u64,
  pub msg: String,
  pub retry: Option<u64>,
}

/// GOAWAY (§7.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Goaway {
  pub code: u64,
  pub msg: String,
  pub url: Option<String>,
  pub retry: Option<u64>,
}

/// CLOSE (§7.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Close {
  pub code: u64,
  pub msg: Option<String>,
}

/// AUTH_REFRESH (§7.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRefresh {
  pub rid: u64,
  pub auth: Auth,
  pub sequencing: Sequencing,
}

/// AUTH_RESULT (§7.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthResult {
  pub rid: u64,
  pub code: u64,
  pub expires: Option<u64>,
  pub sequencing: Sequencing,
}

/// AUTH_EXPIRING (§7.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthExpiring {
  pub expires: u64,
  pub sequencing: Sequencing,
}

/// REQUEST_ERROR (§7.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestError {
  pub rid: u64,
  pub code: u64,
  pub msg: String,
  pub sequencing: Sequencing,
}

/// CALL (§8.2). The data section holds the arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
  pub method: String,
  pub shape: u64,
  pub deadline: Option<u64>,
  pub meta: Option<Meta>,
  pub data: Bytes,
}

/// ITEM (§8.3). The data section holds one stream element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
  pub meta: Option<Meta>,
  pub data: Bytes,
}

/// END (§8.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct End {
  pub meta: Option<Meta>,
}

/// RESULT (§8.4). The data section holds the return value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallResult {
  pub meta: Option<Meta>,
  pub data: Bytes,
}

/// ERROR (§8.4). The data section holds the typed error detail, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallError {
  pub code: u64,
  pub msg: String,
  pub nx: Option<bool>,
  pub meta: Option<Meta>,
  pub data: Bytes,
}

/// CANCEL (§8.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cancel {
  pub msg: Option<String>,
}

/// CREDIT (§6.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credit {
  pub credit: u64,
}

/// NOTIFY (§9.1). The data section holds the arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notify {
  pub method: String,
  pub meta: Option<Meta>,
  pub sequencing: Sequencing,
  pub data: Bytes,
}

/// LANE_OPEN (§9.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneOpen {
  pub lane: u64,
}

/// SUBSCRIBE (§10.2). The data section holds optional subscription parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscribe {
  pub rid: u64,
  pub channel: String,
  pub since: Option<Since>,
  pub presence: Option<bool>,
  pub meta: Option<Meta>,
  pub sequencing: Sequencing,
  pub data: Bytes,
}

/// SUBSCRIBED (§10.2). The data section holds the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscribed {
  pub rid: u64,
  pub channel: String,
  pub epoch: String,
  pub offset: u64,
  pub recovered: Option<bool>,
  pub sequencing: Sequencing,
  pub data: Bytes,
}

/// UNSUBSCRIBE (§10.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsubscribe {
  pub rid: u64,
  pub channel: String,
  pub sequencing: Sequencing,
}

/// UNSUBSCRIBED (§10.3): a reply with `rid`, or a server-initiated notice with `code` and `msg`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsubscribed {
  pub channel: String,
  pub rid: Option<u64>,
  pub code: Option<u64>,
  pub msg: Option<String>,
  pub sequencing: Sequencing,
}

/// PUBLICATION (§10.4). The data section holds the published value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
  pub channel: String,
  pub offset: u64,
  pub sequencing: Sequencing,
  pub data: Bytes,
}

/// PRESENCE (§10.6). The data section holds the entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presence {
  pub channel: String,
  pub kind: u64,
  pub sequencing: Sequencing,
  pub data: Bytes,
}

/// DATAGRAM on the WebSocket control stream (§11.4). The data section holds the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datagram {
  pub key: u64,
  pub seq: u64,
  pub data: Bytes,
}

/// DGRAM_BIND (§11.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DgramBind {
  pub key: u64,
  pub name: String,
  pub sequencing: Sequencing,
}

/// A frame with an extension type (`0x80` to `0xFF`), kept as received so the caller can drop it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extension {
  pub frame_type: u8,
  pub header: Bytes,
  pub data: Bytes,
}

/// One typed frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
  Hello(Hello),
  Welcome(Welcome),
  Reject(Reject),
  Ping { data: Bytes },
  Pong { data: Bytes },
  Goaway(Goaway),
  Close(Close),
  AuthRefresh(AuthRefresh),
  AuthResult(AuthResult),
  Ack { ack: AckMap },
  AuthExpiring(AuthExpiring),
  RequestError(RequestError),
  Call(Call),
  Item(Item),
  End(End),
  Result(CallResult),
  Error(CallError),
  Cancel(Cancel),
  Credit(Credit),
  Notify(Notify),
  LaneOpen(LaneOpen),
  Subscribe(Subscribe),
  Subscribed(Subscribed),
  Unsubscribe(Unsubscribe),
  Unsubscribed(Unsubscribed),
  Publication(Publication),
  Presence(Presence),
  Datagram(Datagram),
  DgramBind(DgramBind),
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
  /// A field is missing or has the wrong type.
  #[error(transparent)]
  Field(#[from] FieldError),
}

impl From<FrameError> for ProtocolError {
  fn from(error: FrameError) -> Self {
    match error {
      FrameError::UnknownType(_) => Self::FrameTypeNotAllowed,
      FrameError::MalformedHeader => Self::MalformedHeader,
      FrameError::Field(FieldError {
        frame_type,
        rid: Some(rid),
        ..
      }) if frame_type.is_control_request() => Self::InvalidControlRequestField { rid },
      FrameError::Field(_) => Self::InvalidHeaderField,
    }
  }
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
