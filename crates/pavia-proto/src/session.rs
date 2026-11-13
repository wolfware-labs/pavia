//! The session state machine (§7), driven as sans-IO: the driver feeds [`Input`]s with
//! [`Session::handle_input`] and then drains [`Session::poll_output`] until it returns `None`.
//!
//! So far it is the server side through the handshake: a validated HELLO with its version, codec
//! and capabilities chosen (§6.2, §7.1, §7.2, §7.10), authentication by the driver's
//! authenticator (§7.5), WELCOME, the capability gate on every later frame, the heartbeat
//! (§7.4), and every way a session ends (§7.3, §7.7, §13.4). Time is an input: the session is
//! built with the accept time, every [`Session::handle_input`] call takes `now`, and
//! [`Session::poll_timeout`] gives the handshake deadline, then the earlier of the next heartbeat
//! PING and the idle deadline.
//!
//! ```text
//!  AwaitHello ──HELLO──▶ Handshaking ──Authenticated: WELCOME──▶ Established
//!
//!  AwaitHello                no common version or codec       ▶ REJECT UNSUPPORTED_VERSION
//!  AwaitHello                resume without `resume`          ▶ REJECT RESUME_FAILED
//!  AwaitHello                a first frame that is not HELLO  ▶ CLOSE PROTOCOL_ERROR
//!  Handshaking               AuthFailed                       ▶ REJECT UNAUTHENTICATED or FORBIDDEN
//!  AwaitHello, Handshaking   deadline                         ▶ CLOSE HANDSHAKE_TIMEOUT
//!  Handshaking, Established  second HELLO                     ▶ CLOSE PROTOCOL_ERROR
//!  Established               a capability not granted         ▶ CLOSE PROTOCOL_ERROR
//!  Established               PING with up to 8 bytes          ▶ PONG with the same bytes
//!  Established               hb with nothing sent             ▶ PING
//!  Established               idle with nothing received       ▶ CLOSE IDLE_TIMEOUT
//!  any                       CLOSE received, TransportLost    ▶ Ended
//! ```
//!
//! A validated HELLO yields [`Output::Authenticate`]. The driver runs the authenticator and feeds
//! its answer back as [`Input::Authenticated`] or [`Input::AuthFailed`]; the handshake timer keeps
//! running meanwhile, so a slow authenticator ends in HANDSHAKE_TIMEOUT (§7.1). Right before
//! WELCOME it yields [`Output::Established`] with the new session ID; the driver makes the session
//! reachable once that WELCOME is written, never before (§13.4, §14).
//!
//! After WELCOME every received message, on any stream, moves the idle deadline to `now + idle`;
//! a timeout at or past it closes with IDLE_TIMEOUT (§7.4). A message that reaches the session
//! past the deadline is still read and moves it, since the peer is alive and the delay was the
//! driver's. Sending does not move it: that is the heartbeat timer. A message at or past the
//! next heartbeat also sends the due PING, so a peer that keeps the driver busy with messages
//! that need no answer still gets one every `hb` (§7.4).
//!
//! Every ending yields exactly one [`Output::Ended`], and after it the session ignores all input.
//! Every message, HELLO and frames pipelined behind it included, is checked against the
//! configured `max_frame`: a Length above it closes with FRAME_TOO_LARGE before any payload byte
//! is read (§5.2), and a message above [`ValidConfig::max_message`] with PROTOCOL_ERROR (§4.3).
//! A text message is a PROTOCOL_ERROR in every state (§4.3). Frames the client pipelines before
//! WELCOME are dropped until the pre-authentication buffer exists (#40). After WELCOME every frame
//! passes [`caps::check`]. A CALL on a new client stream becomes [`Output::Call`], and the
//! driver's [`Command::Result`] or [`Command::Error`] answers it; every frame on the stream goes
//! through the call state machine, and the stream is freed once the call is complete (§6.1,
//! §6.3). An answer above the client's `max_frame` (§5.2) or the default header limit (§5.4.1)
//! is sent as RESOURCE_EXHAUSTED instead. A CALL beyond `max_calls` open calls is answered
//! RESOURCE_EXHAUSTED with `nx` before its header is read, the first of the callee checks of
//! §8.2; the driver makes the rest. CANCEL on a call still waiting for its answer yields
//! [`Output::Cancelled`]; the driver answers ERROR CANCELLED once the handler has stopped, and
//! that answer frees the call's slot (§8.5). Frames on the control stream other than these are
//! otherwise ignored until their feature lands.
//! PING, PONG and CLOSE skip ahead of other queued outputs, in the order they were made (§6.6,
//! §13.5), but never ahead of WELCOME, the server's first frame (§6.2). Sending CLOSE drops what is
//! still queued behind it, since nothing follows CLOSE on the transport (§7.7).
//! Only the WebSocket binding exists so far: one [`Output::Transmit`] is one binary message.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use bytes::{Bytes, BytesMut};

use crate::caps::{self, Capability, Caps};
use crate::data::Codec;
use crate::error::{Outcome, ProtocolError};
use crate::frame::{Auth, Call, CallError, CallResult, Close, Frame, FrameType, Heartbeat, Hello, Meta, Reject, Welcome};
use crate::negotiate;
use crate::registry::{CloseCode, StatusCode};
use crate::session_id::SessionId;
use crate::stream::call::{CallState, Step};
use crate::stream::ids::{self, Admitted, StreamIds};
use crate::stream::{Role, Shape, Side};
use crate::wire::envelope;
use crate::wire::header::DecodeLimits;
use crate::wire::varint;

mod config;

pub use config::{Config, ConfigError, ValidConfig};

const CONTROL_STREAM: u64 = 0;

/// What the driver tells the session.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Input {
  /// One whole binary WebSocket message.
  Message(Bytes),
  /// A text WebSocket message, which closes the session with PROTOCOL_ERROR at any point (§4.3).
  TextMessage,
  /// A WebSocket message above `2 × max_frame` that the transport refused before buffering it,
  /// which closes the session with PROTOCOL_ERROR (§4.3).
  MessageTooLarge,
  /// The transport closed or failed without CLOSE.
  TransportLost,
  /// The time from [`Session::poll_timeout`] has come.
  Timeout,
  /// The authenticator accepted the client after [`Output::Authenticate`]: the session sends
  /// WELCOME and the handshake timer stops (§7.2).
  Authenticated {
    /// The session ID the driver assigned, WELCOME `session`.
    session: SessionId,
    /// The principal's user ID, WELCOME `user`; `None` for an anonymous session (§2).
    user: Option<String>,
  },
  /// The authenticator refused the client after [`Output::Authenticate`].
  AuthFailed(AuthFailure),
  /// A request from the application.
  Command(Command),
}

/// A request from the application.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Command {
  /// End the session: send CLOSE with `code` and `msg`, then close the transport (§7.7).
  Close {
    /// The close code.
    code: CloseCode,
    /// The CLOSE `msg`, also the transport close reason.
    msg: Option<String>,
  },
  /// Answer the call on `stream` with RESULT carrying `data`, the return value already encoded in
  /// the session codec (§8.4). Ignored once the call has its answer or has gone.
  Result {
    /// The call stream, from [`Output::Call`].
    stream: u64,
    /// The data section.
    data: Bytes,
  },
  /// Answer the call on `stream` with ERROR (§8.4). Ignored once the call has its answer or has
  /// gone. ERROR never carries OK, so [`StatusCode::Ok`] is sent as INTERNAL.
  Error {
    /// The call stream, from [`Output::Call`].
    stream: u64,
    /// The status code.
    code: StatusCode,
    /// The message.
    msg: String,
    /// `nx`: the handler never ran.
    nx: bool,
    /// `meta` trailers (§8.7).
    meta: Option<Meta>,
    /// The typed error detail, already encoded in the session codec, or empty (§8.4).
    detail: Bytes,
  },
}

/// What the session asks the driver to do, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Output {
  /// Send one binary WebSocket message, already in the `WsFrame` envelope (§5.4).
  Transmit(Bytes),
  /// Run the authenticator on HELLO `auth` (`None` when HELLO has none) and the upgrade request,
  /// then feed its answer back (§7.5). Emitted once, after HELLO is validated and negotiated. The
  /// credential moves out of the session here, so [`Session::hello`] no longer holds it.
  Authenticate {
    /// HELLO `auth`.
    auth: Option<Auth>,
  },
  /// The session is established under `session`; the next output is the WELCOME that says so.
  /// The driver makes the session reachable once that WELCOME is written, never before: nothing
  /// sent to the session ahead of WELCOME may reach it (§13.4, §14).
  Established {
    /// The session ID WELCOME carries.
    session: SessionId,
  },
  /// Close the transport. On WebSocket the close code is `4000 + code` and the reason is cut
  /// to 123 bytes on a character boundary (§4.3, §7.7).
  CloseTransport {
    /// The session close code.
    code: CloseCode,
    /// The close reason.
    reason: String,
  },
  /// The client called a server method on `stream` (§8.2). The driver checks it from step 2 of
  /// §8.2 on, runs it and answers with [`Command::Result`] or [`Command::Error`] for the same
  /// stream. Only established sessions emit it, one per call stream, and only while fewer than
  /// `max_calls` calls are open: the session answers the rest RESOURCE_EXHAUSTED with `nx`
  /// itself, without reading their header (§6.5, §8.2 step 1). `call.shape` is the value the
  /// client sent, known or not. A shape the session does not know is tracked as a client stream,
  /// so ITEMs and END the client sent behind it are taken as frames of that call, until the
  /// driver refuses it at step 3.
  Call {
    /// The call stream.
    stream: u64,
    /// The CALL frame: method, shape, deadline, meta and the encoded arguments.
    call: Call,
  },
  /// The client cancelled the call on `stream` before its answer (§8.5). The driver stops the
  /// call's handler and, once it has stopped, answers [`Command::Error`] with
  /// [`StatusCode::Cancelled`] in place of whatever the handler returned. The call keeps its
  /// `max_calls` slot until that answer, so a handler still stopping is counted.
  Cancelled {
    /// The call stream, from [`Output::Call`].
    stream: u64,
  },
  /// The session is over. Emitted once, as the last output.
  Ended(Ending),
}

/// Why the authenticator refused a client (§7.5). The session answers with REJECT and the
/// matching code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthFailure {
  /// Missing, invalid or expired credentials: UNAUTHENTICATED.
  Unauthenticated,
  /// Credentials for a principal that may not connect: FORBIDDEN.
  Forbidden,
}

impl AuthFailure {
  const fn reject(self) -> (CloseCode, &'static str) {
    match self {
      Self::Unauthenticated => (CloseCode::Unauthenticated, "unauthenticated"),
      Self::Forbidden => (CloseCode::Forbidden, "forbidden"),
    }
  }
}

/// How a session ended (§13.4).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Ending {
  /// CLOSE was sent or received.
  Closed {
    /// The close code. A received code outside §15.1 is [`CloseCode::Internal`] (§15).
    code: CloseCode,
    /// The CLOSE `msg`.
    msg: Option<String>,
    /// The side that sent CLOSE.
    by: Side,
  },
  /// The server sent REJECT instead of WELCOME (§7.3).
  Rejected {
    /// The REJECT code.
    code: CloseCode,
    /// The REJECT `msg`.
    msg: String,
  },
  /// The transport closed or failed without CLOSE. Without resumption this ends the session.
  TransportLost,
}

/// The version and codec chosen from a HELLO (§7.2), announced in WELCOME.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Negotiated {
  /// The protocol major version.
  pub version: u64,
  /// The data-section codec.
  pub codec: Codec,
  /// The granted capabilities (§7.10), fixed for the life of the session.
  pub caps: Caps,
}

#[derive(Debug)]
enum State {
  AwaitHello {
    deadline: Instant,
  },
  Handshaking {
    deadline: Instant,
    hello: Box<Hello>,
    negotiated: Negotiated,
  },
  Established {
    hello: Box<Hello>,
    negotiated: Negotiated,
    ping_at: Instant,
    idle_at: Instant,
  },
  Ended,
}

#[derive(Debug)]
struct Calls {
  ids: StreamIds,
  open: HashMap<u64, CallState>,
}

impl Calls {
  fn settle(&mut self, stream: u64) {
    if self.open.get(&stream).is_some_and(CallState::is_complete) {
      self.open.remove(&stream);
      self.ids.close(stream);
    }
  }
}

/// One server session.
#[derive(Debug)]
pub struct Session {
  local: Side,
  now: Instant,
  config: ValidConfig,
  limits: DecodeLimits,
  calls: Calls,
  state: State,
  outputs: VecDeque<Output>,
  urgent: usize,
}

impl Session {
  /// A server session on a WebSocket transport accepted at `now`. The handshake timer starts
  /// here, not at the first byte (§7.1).
  #[must_use]
  pub fn server(config: &ValidConfig, now: Instant) -> Self {
    Self {
      local: Side::Server,
      now,
      config: config.clone(),
      limits: DecodeLimits::default(),
      calls: Calls {
        ids: StreamIds::new(Side::Server),
        open: HashMap::new(),
      },
      state: State::AwaitHello {
        deadline: now + config.0.handshake_timeout,
      },
      outputs: VecDeque::new(),
      urgent: 0,
    }
  }

  /// How many calls are open: opened by CALL and not complete yet (§6.3).
  #[must_use]
  pub fn open_calls(&self) -> usize {
    self.calls.open.len()
  }

  /// Whether the session has ended. Every later input is ignored.
  #[must_use]
  pub const fn is_ended(&self) -> bool {
    matches!(self.state, State::Ended)
  }

  /// The client's HELLO, once it has arrived and passed validation.
  #[must_use]
  pub fn hello(&self) -> Option<&Hello> {
    match &self.state {
      State::Handshaking { hello, .. } | State::Established { hello, .. } => Some(hello),
      State::AwaitHello { .. } | State::Ended => None,
    }
  }

  /// The version, codec and capabilities chosen from the client's HELLO.
  #[must_use]
  pub const fn negotiated(&self) -> Option<&Negotiated> {
    match &self.state {
      State::Handshaking { negotiated, .. } | State::Established { negotiated, .. } => Some(negotiated),
      State::AwaitHello { .. } | State::Ended => None,
    }
  }

  /// When the driver must feed [`Input::Timeout`], or `None` when no timer is running.
  #[must_use]
  pub fn poll_timeout(&self) -> Option<Instant> {
    match self.state {
      State::AwaitHello { deadline } | State::Handshaking { deadline, .. } => Some(deadline),
      State::Established { ping_at, idle_at, .. } => Some(ping_at.min(idle_at)),
      State::Ended => None,
    }
  }

  /// Applies one input that arrived at `now`. Drain [`poll_output`](Self::poll_output) and read
  /// [`poll_timeout`](Self::poll_timeout) afterwards.
  pub fn handle_input(&mut self, input: Input, now: Instant) {
    if self.is_ended() {
      return;
    }
    self.now = now;
    let expired = matches!(self.state, State::AwaitHello { deadline } | State::Handshaking { deadline, .. } if now >= deadline);
    let idle = matches!(self.state, State::Established { idle_at, .. } if now >= idle_at);
    match input {
      Input::TransportLost => self.end(Ending::TransportLost),
      Input::TextMessage => self.fail(&ProtocolError::TextMessage),
      Input::MessageTooLarge => self.fail(&ProtocolError::MessageTooLarge),
      Input::Command(Command::Close { code, msg }) => self.close(code, msg),
      Input::Command(Command::Result { stream, data }) => {
        self.answer(stream, &Frame::Result(CallResult { meta: None, data }));
      }
      Input::Command(Command::Error {
        stream,
        code,
        msg,
        nx,
        meta,
        detail,
      }) => {
        let error = CallError {
          meta,
          data: detail,
          ..call_error(code, msg, nx)
        };
        self.answer(stream, &Frame::Error(error));
      }
      Input::Message(_) | Input::Timeout | Input::Authenticated { .. } | Input::AuthFailed(_) if expired => {
        self.close(CloseCode::HandshakeTimeout, None);
      }
      Input::Timeout if idle => self.close(CloseCode::IdleTimeout, None),
      Input::Message(message) => {
        self.receive(message);
        self.send_due_ping();
      }
      Input::Authenticated { session, user } => self.welcome(&session, user),
      Input::AuthFailed(failure) => {
        if let State::Handshaking { .. } = self.state {
          let (code, msg) = failure.reject();
          self.reject(code, msg);
        }
      }
      Input::Timeout => self.send_due_ping(),
    }
  }

  /// The next output, or `None` when there is nothing to do until the next input.
  pub fn poll_output(&mut self) -> Option<Output> {
    self.urgent = self.urgent.saturating_sub(1);
    self.outputs.pop_front()
  }

  fn send_due_ping(&mut self) {
    if matches!(self.state, State::Established { ping_at, .. } if self.now >= ping_at) {
      self.transmit(&Frame::Ping {
        data: Heartbeat::default(),
      });
    }
  }

  fn receive(&mut self, message: Bytes) {
    if let State::Established { idle_at, .. } = &mut self.state {
      *idle_at = self.now + self.config.0.idle_timeout;
    }
    let frames = match envelope::decode_message(message, self.config.0.max_frame) {
      Ok(frames) => frames,
      Err(error) => return self.fail(&error),
    };
    for ws_frame in frames {
      if ws_frame.frame.frame_type == u8::from(FrameType::Call)
        && ws_frame.stream_id != CONTROL_STREAM
        && matches!(self.state, State::Established { .. })
        && self.calls_full()
      {
        self.refuse_beyond_max_calls(ws_frame.stream_id);
        if self.is_ended() {
          return;
        }
        continue;
      }
      let frame = match Frame::from_raw(&ws_frame.frame, &self.limits) {
        Ok(frame) => frame,
        Err(error) => {
          let error = ProtocolError::from(error);
          let fatal = matches!(error.outcome(), Outcome::SessionFatal(_));
          if !fatal && matches!(self.state, State::AwaitHello { .. }) {
            return self.fail(&ProtocolError::FrameTypeNotAllowed);
          }
          self.fail(&error);
          if self.is_ended() {
            return;
          }
          continue;
        }
      };
      let on_control = ws_frame.stream_id == CONTROL_STREAM;
      match (&self.state, frame) {
        (State::AwaitHello { deadline }, Frame::Hello(mut hello)) if on_control => {
          if hello.versions.is_empty() || hello.codecs.is_empty() {
            return self.fail(&ProtocolError::InvalidHeaderField);
          }
          let caps = Caps::grant(hello.caps.as_deref());
          if hello.resume.is_some() && !caps.contains(Capability::Resume) {
            return self.reject(CloseCode::ResumeFailed, "unknown session");
          }
          let version = negotiate::version(&hello.versions, &self.config.0.versions);
          let codec = negotiate::codec(&hello.codecs, &self.config.0.codecs);
          let (Some(version), Some(codec)) = (version, codec) else {
            return self.reject(CloseCode::UnsupportedVersion, "no common protocol version or codec");
          };
          let auth = hello.auth.take();
          self.state = State::Handshaking {
            deadline: *deadline,
            hello: Box::new(hello),
            negotiated: Negotiated { version, codec, caps },
          };
          self.outputs.push_back(Output::Authenticate { auth });
        }
        (State::Handshaking { .. } | State::Established { .. }, Frame::Close(close)) if on_control => {
          return self.peer_closed(close);
        }
        (State::Established { .. }, Frame::Ping { data }) if on_control => self.transmit(&Frame::Pong { data }),
        (State::Established { .. }, Frame::Pong { .. }) if on_control => {}
        (State::AwaitHello { .. }, _)
        | (State::Handshaking { .. } | State::Established { .. }, Frame::Hello(_) | Frame::Close(_))
        | (State::Established { .. }, Frame::Ping { .. } | Frame::Pong { .. }) => {
          return self.fail(&ProtocolError::FrameTypeNotAllowed);
        }
        (State::Handshaking { .. }, _) => {}
        (State::Established { negotiated, .. }, frame) => {
          if let Err(error) = caps::check(&negotiated.caps, &frame, self.local.peer()) {
            return self.fail(&error);
          }
          if !on_control {
            self.on_call_stream(ws_frame.stream_id, frame);
            if self.is_ended() {
              return;
            }
          }
        }
        (State::Ended, _) => return,
      }
    }
  }

  fn welcome(&mut self, session: &SessionId, user: Option<String>) {
    let (hello, negotiated) = match std::mem::replace(&mut self.state, State::Ended) {
      State::Handshaking { hello, negotiated, .. } => (hello, negotiated),
      other => {
        self.state = other;
        return;
      }
    };
    let welcome = Frame::Welcome(Welcome {
      version: negotiated.version,
      codec: negotiated.codec.name().to_owned(),
      caps: negotiated.caps.names().map(str::to_owned).collect(),
      session: session.as_str().to_owned(),
      token: None,
      limits: self.config.limits(),
      hb: self.config.heartbeat_ms(),
      idle: self.config.idle_ms(),
      resume_window: None,
      contract: None,
      user,
      resumed: None,
      ack: None,
      lost: None,
      transports: None,
    });
    self.state = State::Established {
      hello,
      negotiated,
      ping_at: self.now,
      idle_at: self.now + self.config.0.idle_timeout,
    };
    self.outputs.push_back(Output::Established {
      session: session.clone(),
    });
    self.transmit(&welcome);
    self.urgent = self.outputs.len();
  }

  fn fail(&mut self, error: &ProtocolError) {
    if let Outcome::SessionFatal(code) = error.outcome() {
      self.close(code, None);
    }
  }

  fn peer_closed(&mut self, close: Close) {
    let code = CloseCode::from_u16_lossy(u16::try_from(close.code).unwrap_or(u16::MAX));
    self.outputs.push_back(Output::CloseTransport {
      code,
      reason: close.msg.clone().unwrap_or_default(),
    });
    self.end(Ending::Closed {
      code,
      msg: close.msg,
      by: self.local.peer(),
    });
  }

  fn close(&mut self, code: CloseCode, msg: Option<String>) {
    self.outputs.truncate(self.urgent);
    let close = Frame::Close(Close {
      code: u64::from(code),
      msg: msg.clone(),
    });
    self.transmit(&close);
    self.outputs.push_back(Output::CloseTransport {
      code,
      reason: msg.clone().unwrap_or_default(),
    });
    self.end(Ending::Closed {
      code,
      msg,
      by: self.local,
    });
  }

  fn reject(&mut self, code: CloseCode, msg: &str) {
    let reject = Frame::Reject(Reject {
      code: u64::from(code),
      msg: msg.to_owned(),
      retry: None,
    });
    self.transmit(&reject);
    self.outputs.push_back(Output::CloseTransport {
      code,
      reason: msg.to_owned(),
    });
    self.end(Ending::Rejected {
      code,
      msg: msg.to_owned(),
    });
  }

  fn on_call_stream(&mut self, stream: u64, frame: Frame) {
    let Some(frame_type) = frame.frame_type() else {
      return;
    };
    match (self.calls.ids.receive(stream, ids::opens_stream(frame_type)), frame) {
      (Err(error), _) => self.fail(&error),
      (Ok(Admitted::Opened), Frame::Call(call)) => self.open_call(stream, call),
      (Ok(_), _) => {
        let step = self
          .calls
          .open
          .get_mut(&stream)
          .map_or(Ok(Step::Ignored), |call| call.on_frame(Role::Caller, frame_type));
        self.calls.settle(stream);
        match step {
          Ok(Step::Accepted) if frame_type == FrameType::Cancel && self.calls.open.contains_key(&stream) => {
            self.outputs.push_back(Output::Cancelled { stream });
          }
          Ok(_) => {}
          Err(error) => self.fail(&error),
        }
      }
    }
  }

  fn open_call(&mut self, stream: u64, call: Call) {
    let shape = Shape::try_from(call.shape).unwrap_or(Shape::ClientStream);
    self.calls.open.insert(stream, CallState::new(shape, Role::Callee));
    self.outputs.push_back(Output::Call { stream, call });
  }

  fn calls_full(&self) -> bool {
    u64::try_from(self.calls.open.len()).map_or(true, |open| open >= self.config.0.max_calls)
  }

  fn refuse_beyond_max_calls(&mut self, stream: u64) {
    match self.calls.ids.receive(stream, true) {
      Err(error) => self.fail(&error),
      Ok(_) => {
        self
          .calls
          .open
          .insert(stream, CallState::new(Shape::ClientStream, Role::Callee));
        let refusal = call_error(StatusCode::ResourceExhausted, "too many open calls".to_owned(), true);
        self.answer(stream, &Frame::Error(refusal));
      }
    }
  }

  fn answer(&mut self, stream: u64, frame: &Frame) {
    let too_large;
    let fits =
      frame.payload_length().is_ok_and(|length| length <= self.peer_max_frame()) && frame.header_length() <= peer_max_header();
    let frame = if fits {
      frame
    } else {
      too_large = Frame::Error(call_error(
        StatusCode::ResourceExhausted,
        "answer above the client's max_frame or header limit".to_owned(),
        false,
      ));
      &too_large
    };
    if let (Some(call), Some(frame_type)) = (self.calls.open.get_mut(&stream), frame.frame_type())
      && call.on_frame(Role::Callee, frame_type).is_ok()
    {
      self.calls.settle(stream);
      self.transmit_on(stream, frame);
    }
  }

  fn peer_max_frame(&self) -> u64 {
    self
      .hello()
      .and_then(|hello| hello.limits.as_ref())
      .and_then(|limits| limits.max_frame)
      .unwrap_or(config::DEFAULT_MAX_FRAME)
  }

  fn transmit(&mut self, frame: &Frame) {
    self.transmit_on(CONTROL_STREAM, frame);
  }

  fn transmit_on(&mut self, stream: u64, frame: &Frame) {
    let mut message = BytesMut::new();
    varint::encode(stream, &mut message).expect("stream IDs come from varints, so they fit one");
    if frame.encode(&mut message).is_ok() {
      let output = Output::Transmit(message.freeze());
      if matches!(frame, Frame::Ping { .. } | Frame::Pong { .. } | Frame::Close(_)) {
        self.outputs.insert(self.urgent, output);
        self.urgent += 1;
      } else {
        self.outputs.push_back(output);
      }
      if let State::Established { ping_at, .. } = &mut self.state {
        *ping_at = self.now + self.config.0.heartbeat;
      }
    }
  }

  fn end(&mut self, ending: Ending) {
    self.state = State::Ended;
    self.outputs.push_back(Output::Ended(ending));
  }
}

/// The client's header limit: HELLO does not carry one, so it is the §5.4.1 default, whatever
/// limit this session reads headers with.
fn peer_max_header() -> usize {
  DecodeLimits::default().max_size
}

fn call_error(code: StatusCode, msg: String, nx: bool) -> CallError {
  let code = if code == StatusCode::Ok { StatusCode::Internal } else { code };
  CallError {
    code: u64::from(code),
    msg,
    nx: nx.then_some(true),
    meta: None,
    data: Bytes::new(),
  }
}

#[cfg(test)]
mod tests;
