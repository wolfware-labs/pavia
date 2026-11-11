//! The session registry (§13.3, §13.4): every established session on this server, by session ID.
//!
//! [`session::run`](crate::session::run) registers a session when the session reports it
//! established, right before WELCOME is queued, and the session leaves the registry when `run`
//! returns, whatever ended it. Before WELCOME a session is not reachable from here (§14), so a
//! command sent to it through the registry can never arrive ahead of WELCOME.
//!
//! ```text
//!  HELLO ─▶ authenticator ─▶ Established: register ─▶ WELCOME ─▶ … ─▶ run returns: removed
//!                          ╰─▶ REJECT or close: never registered
//! ```
//!
//! Group joins staged by the on-connect hook will commit under the same lock as the registration
//! (§13.4, #100, #104), so no observer sees one without the other.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use pavia_proto::session::Command;
use pavia_proto::session_id::SessionId;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::SendError;

/// A handle to one running session: commands sent through it reach the session's task in order.
#[derive(Debug, Clone)]
pub struct Handle(mpsc::Sender<Command>);

impl Handle {
  pub(crate) const fn new(commands: mpsc::Sender<Command>) -> Self {
    Self(commands)
  }

  /// Sends `command` to the session, waiting while its command queue is full.
  ///
  /// # Errors
  ///
  /// Gives the command back when the session has ended.
  pub async fn send(&self, command: Command) -> Result<(), SendError<Command>> {
    self.0.send(command).await
  }
}

/// Every established session on this server. Cloning it gives another view of the same sessions.
#[derive(Debug, Clone, Default)]
pub struct Registry {
  sessions: Arc<Mutex<HashMap<SessionId, Handle>>>,
}

impl Registry {
  /// An empty registry.
  #[must_use]
  pub fn new() -> Self {
    Self::default()
  }

  /// The number of established sessions.
  #[must_use]
  pub fn len(&self) -> usize {
    self.lock().len()
  }

  /// Whether no session is established.
  #[must_use]
  pub fn is_empty(&self) -> bool {
    self.lock().is_empty()
  }

  /// The handle of the established session `id`, if it is still running.
  #[must_use]
  pub fn get(&self, id: &SessionId) -> Option<Handle> {
    self.lock().get(id).cloned()
  }

  /// Registers the session `id`, reachable through `handle`, until the returned guard is dropped.
  pub(crate) fn register(&self, id: SessionId, handle: Handle) -> Registration {
    self.lock().insert(id.clone(), handle.clone());
    Registration {
      registry: self.clone(),
      id,
      handle,
    }
  }

  fn lock(&self) -> MutexGuard<'_, HashMap<SessionId, Handle>> {
    self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
  }
}

/// A registered session. Dropping it removes the session, once, and only the entry it made: a
/// later registration under the same ID stays.
#[derive(Debug)]
pub(crate) struct Registration {
  registry: Registry,
  id: SessionId,
  handle: Handle,
}

impl Drop for Registration {
  fn drop(&mut self) {
    let mut sessions = self.registry.lock();
    if sessions
      .get(&self.id)
      .is_some_and(|entry| entry.0.same_channel(&self.handle.0))
    {
      sessions.remove(&self.id);
    }
  }
}

#[cfg(test)]
mod tests;
