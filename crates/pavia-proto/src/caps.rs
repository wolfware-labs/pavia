//! Capabilities (§7.10): which optional features a session may use.
//!
//! The server grants the requested capabilities it implements, plus `calls`, which v1 always
//! grants. [`IMPLEMENTED`] is the only source of "implements": a capability joins it in the same
//! change that implements its feature, so a grant can never name missing code. After WELCOME,
//! [`check`] gates every inbound frame against the granted set.

use std::collections::BTreeSet;

use crate::error::ProtocolError;
use crate::frame::{Frame, Sequencing};
use crate::stream::Side;

/// One capability name from §7.10.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Capability {
  /// `calls`: calls and notifications.
  Calls,
  /// `server-calls`: the server may call client methods.
  ServerCalls,
  /// `streams`: stream shapes 1 to 3.
  Streams,
  /// `resume`: session resumption (§7.8).
  Resume,
  /// `channels`: channels (§10).
  Channels,
  /// `history`: channel history and recovery (§10.5).
  History,
  /// `presence`: presence (§10.6).
  Presence,
  /// `datagrams`: datagrams (§11).
  Datagrams,
  /// `lanes`: lanes (§9.2).
  Lanes,
  /// `idempotency`: the server honors `pavia-idempotency-key` (§8.7).
  Idempotency,
}

impl Capability {
  /// Every capability §7.10 defines, in its order.
  pub const ALL: [Self; 10] = [
    Self::Calls,
    Self::ServerCalls,
    Self::Streams,
    Self::Resume,
    Self::Channels,
    Self::History,
    Self::Presence,
    Self::Datagrams,
    Self::Lanes,
    Self::Idempotency,
  ];

  /// The name used in HELLO and WELCOME `caps`.
  #[must_use]
  pub const fn name(self) -> &'static str {
    match self {
      Self::Calls => "calls",
      Self::ServerCalls => "server-calls",
      Self::Streams => "streams",
      Self::Resume => "resume",
      Self::Channels => "channels",
      Self::History => "history",
      Self::Presence => "presence",
      Self::Datagrams => "datagrams",
      Self::Lanes => "lanes",
      Self::Idempotency => "idempotency",
    }
  }

  /// The capability with this name, or `None` for a name §7.10 does not define.
  #[must_use]
  pub fn from_name(name: &str) -> Option<Self> {
    Self::ALL.into_iter().find(|capability| capability.name() == name)
  }
}

/// The capabilities this build implements. Only these can be granted.
pub const IMPLEMENTED: &[Capability] = &[Capability::Calls];

/// A set of capabilities, kept in §7.10 order so it serializes the same way every time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caps(BTreeSet<Capability>);

impl Caps {
  /// The grant for a HELLO `caps` list (`None` when HELLO has no `caps`): `calls`, plus every
  /// requested capability in [`IMPLEMENTED`]. Unknown names are ignored.
  ///
  /// ```
  /// use pavia_proto::caps::Caps;
  ///
  /// let granted = Caps::grant(Some(&["resume", "teleport"]));
  /// assert_eq!(granted.names().collect::<Vec<_>>(), ["calls"]);
  /// ```
  #[must_use]
  pub fn grant<S: AsRef<str>>(requested: Option<&[S]>) -> Self {
    Self::grant_from(requested, IMPLEMENTED)
  }

  fn grant_from<S: AsRef<str>>(requested: Option<&[S]>, implemented: &[Capability]) -> Self {
    let requested = requested
      .unwrap_or_default()
      .iter()
      .filter_map(|name| Capability::from_name(name.as_ref()))
      .filter(|capability| implemented.contains(capability));
    Self(std::iter::once(Capability::Calls).chain(requested).collect())
  }

  /// Whether `capability` is in the set.
  #[must_use]
  pub fn contains(&self, capability: Capability) -> bool {
    self.0.contains(&capability)
  }

  /// The names, in §7.10 order, for WELCOME `caps`.
  pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
    self.0.iter().map(|capability| capability.name())
  }
}

/// Checks one frame received from `sender` against the granted set. A frame that uses a feature
/// outside it is [`ProtocolError::CapabilityNotGranted`]. Metadata is never gated, so a
/// `pavia-idempotency-key` without `idempotency` is ordinary metadata (§8.7).
///
/// # Errors
///
/// [`ProtocolError::CapabilityNotGranted`] for the first capability the frame needs and
/// `granted` lacks.
pub fn check(granted: &Caps, frame: &Frame, sender: Side) -> Result<(), ProtocolError> {
  match needed(frame, sender)
    .into_iter()
    .flatten()
    .find(|&capability| !granted.contains(capability))
  {
    Some(_) => Err(ProtocolError::CapabilityNotGranted),
    None => Ok(()),
  }
}

fn needed(frame: &Frame, sender: Side) -> [Option<Capability>; 4] {
  let resume = |sequencing: &Sequencing| (sequencing.seq.is_some() || sequencing.ack.is_some()).then_some(Capability::Resume);
  let one = |capability| [Some(capability), None, None, None];
  let sequenced = |capability, sequencing| [capability, resume(sequencing), None, None];
  match frame {
    Frame::Call(call) => [
      Some(Capability::Calls),
      matches!(call.shape, 1..=3).then_some(Capability::Streams),
      (sender == Side::Server).then_some(Capability::ServerCalls),
      None,
    ],
    Frame::Item(_) | Frame::End(_) | Frame::Result(_) | Frame::Error(_) | Frame::Cancel(_) => one(Capability::Calls),
    Frame::Notify(notify) => sequenced(Some(Capability::Calls), &notify.sequencing),
    Frame::Ack { .. } => one(Capability::Resume),
    Frame::Subscribe(subscribe) => [
      Some(Capability::Channels),
      subscribe.since.as_ref().map(|_| Capability::History),
      subscribe.presence.map(|_| Capability::Presence),
      resume(&subscribe.sequencing),
    ],
    Frame::Subscribed(subscribed) => sequenced(Some(Capability::Channels), &subscribed.sequencing),
    Frame::Unsubscribe(unsubscribe) => sequenced(Some(Capability::Channels), &unsubscribe.sequencing),
    Frame::Unsubscribed(unsubscribed) => sequenced(Some(Capability::Channels), &unsubscribed.sequencing),
    Frame::Publication(publication) => sequenced(Some(Capability::Channels), &publication.sequencing),
    Frame::Presence(presence) => sequenced(Some(Capability::Presence), &presence.sequencing),
    Frame::Datagram(_) => one(Capability::Datagrams),
    Frame::DgramBind(bind) => sequenced(Some(Capability::Datagrams), &bind.sequencing),
    Frame::LaneOpen(_) => one(Capability::Lanes),
    Frame::AuthRefresh(refresh) => sequenced(None, &refresh.sequencing),
    Frame::AuthResult(result) => sequenced(None, &result.sequencing),
    Frame::AuthExpiring(expiring) => sequenced(None, &expiring.sequencing),
    Frame::RequestError(error) => sequenced(None, &error.sequencing),
    Frame::Hello(_)
    | Frame::Welcome(_)
    | Frame::Reject(_)
    | Frame::Ping { .. }
    | Frame::Pong { .. }
    | Frame::Goaway(_)
    | Frame::Close(_)
    | Frame::Credit(_)
    | Frame::Extension(_) => [None; 4],
  }
}

#[cfg(test)]
mod tests;
