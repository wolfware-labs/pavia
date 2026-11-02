//! The session state machine (§7), driven as sans-IO: the driver feeds [`Input`]s with
//! [`Session::handle_input`] and then drains [`Session::poll_output`] until it returns `None`.
//!
//! So far it is the server side through the handshake: a validated HELLO with its version, codec
//! and capabilities chosen (§6.2, §7.1, §7.2, §7.10), authentication by the driver's
//! authenticator (§7.5), WELCOME, the capability gate on every later frame, the heartbeat
//! (§7.4), and every way a session ends (§7.3, §7.7, §13.4). Time is an input: the session is
//! built with the accept time, every [`Session::handle_input`] call takes `now`, and
//! [`Session::poll_timeout`] gives the handshake deadline, then the next heartbeat PING.
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
//!  any                       CLOSE received, TransportLost    ▶ Ended
//! ```
//!
//! A validated HELLO yields [`Output::Authenticate`]. The driver runs the authenticator and feeds
//! its answer back as [`Input::Authenticated`] or [`Input::AuthFailed`]; the handshake timer keeps
//! running meanwhile, so a slow authenticator ends in HANDSHAKE_TIMEOUT (§7.1).
//!
//! Every ending yields exactly one [`Output::Ended`], and after it the session ignores all input.
//! A text message is a PROTOCOL_ERROR in every state (§4.3). Frames the client pipelines before
//! WELCOME are dropped until the pre-authentication buffer exists (#40). After WELCOME every frame
//! passes [`caps::check`] and is otherwise ignored until its feature lands.
//! PING, PONG and CLOSE skip ahead of other queued outputs, in the order they were made (§6.6,
//! §13.5). Sending CLOSE drops what is still queued behind it, since nothing follows CLOSE on the
//! transport (§7.7).
//! Only the WebSocket binding exists so far: one [`Output::Transmit`] is one binary message.

use std::collections::VecDeque;
use std::time::Instant;

use bytes::{Bytes, BytesMut};

use crate::caps::{self, Capability, Caps};
use crate::data::Codec;
use crate::error::{Outcome, ProtocolError};
use crate::frame::{Auth, Close, Frame, Heartbeat, Hello, Reject, Welcome};
use crate::negotiate;
use crate::registry::CloseCode;
use crate::session_id::SessionId;
use crate::stream::Side;
use crate::wire::envelope::{self, MAX_FRAME_FLOOR};
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
  /// Close the transport. On WebSocket the close code is `4000 + code` and the reason is cut
  /// to 123 bytes on a character boundary (§4.3, §7.7).
  CloseTransport {
    /// The session close code.
    code: CloseCode,
    /// The close reason.
    reason: String,
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
  },
  Ended,
}

/// One server session.
#[derive(Debug)]
pub struct Session {
  local: Side,
  now: Instant,
  config: ValidConfig,
  max_frame: u64,
  limits: DecodeLimits,
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
      max_frame: MAX_FRAME_FLOOR,
      limits: DecodeLimits::default(),
      state: State::AwaitHello {
        deadline: now + config.0.handshake_timeout,
      },
      outputs: VecDeque::new(),
      urgent: 0,
    }
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
  pub const fn poll_timeout(&self) -> Option<Instant> {
    match self.state {
      State::AwaitHello { deadline } | State::Handshaking { deadline, .. } | State::Established { ping_at: deadline, .. } => {
        Some(deadline)
      }
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
    let due = self.poll_timeout().is_some_and(|deadline| now >= deadline);
    let expired = due && matches!(self.state, State::AwaitHello { .. } | State::Handshaking { .. });
    match input {
      Input::TransportLost => self.end(Ending::TransportLost),
      Input::TextMessage => self.fail(&ProtocolError::TextMessage),
      Input::Command(Command::Close { code, msg }) => self.close(code, msg),
      Input::Message(_) | Input::Timeout | Input::Authenticated { .. } | Input::AuthFailed(_) if expired => {
        self.close(CloseCode::HandshakeTimeout, None);
      }
      Input::Message(message) => self.receive(message),
      Input::Authenticated { session, user } => self.welcome(&session, user),
      Input::AuthFailed(failure) => {
        if let State::Handshaking { .. } = self.state {
          let (code, msg) = failure.reject();
          self.reject(code, msg);
        }
      }
      Input::Timeout if due => self.transmit(&Frame::Ping {
        data: Heartbeat::default(),
      }),
      Input::Timeout => {}
    }
  }

  /// The next output, or `None` when there is nothing to do until the next input.
  pub fn poll_output(&mut self) -> Option<Output> {
    self.urgent = self.urgent.saturating_sub(1);
    self.outputs.pop_front()
  }

  fn receive(&mut self, message: Bytes) {
    let frames = match envelope::decode_message(message, self.max_frame) {
      Ok(frames) => frames,
      Err(error) => return self.fail(&error),
    };
    for ws_frame in frames {
      let frame = match Frame::from_raw(&ws_frame.frame, &self.limits) {
        Ok(frame) => frame,
        Err(error) => return self.fail(&error.into()),
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
    };
    self.max_frame = self.config.0.max_frame;
    self.transmit(&welcome);
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
      code: u64::from(u16::from(code)),
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
      code: u64::from(u16::from(code)),
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

  fn transmit(&mut self, frame: &Frame) {
    let mut message = BytesMut::new();
    varint::encode(CONTROL_STREAM, &mut message).expect("stream 0 fits a varint");
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

#[cfg(test)]
mod tests;
