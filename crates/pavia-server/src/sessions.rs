//! The session registry (§13.3, §13.4): every established session on this server, by session ID.
//!
//! [`session::run`](crate::session::run) registers a session once the WELCOME that establishes
//! it is written, and the session leaves the registry when `run` returns, whatever ended it.
//! Before WELCOME a session is not reachable from here (§14), so a command sent to it through the
//! registry can never arrive ahead of WELCOME. [`Registry::call`] and [`Handle::call`] call a
//! method of a session's client (§8.8).
//!
//! ```text
//!  HELLO ─▶ authenticator ─▶ WELCOME written: register ─▶ … ─▶ run returns: removed
//!                          ╰─▶ REJECT or close: never registered
//! ```
//!
//! Group joins staged by the on-connect hook will commit under the same lock as the registration
//! (§13.4, #100, #104), so no observer sees one without the other.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use pavia_proto::data::Value;
use pavia_proto::session::Command;
use pavia_proto::session_id::SessionId;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, timeout_at};

use crate::methods::MethodName;

mod call;

pub use call::{CallError, CallOptions};

const QUEUE: usize = 32;

/// A handle to one running session: commands sent through it reach the session's task in order,
/// and [`call`](Self::call) calls a method of its client.
#[derive(Debug, Clone)]
pub struct Handle {
  commands: mpsc::Sender<Command>,
  calls: mpsc::Sender<Calling>,
  abandoned: mpsc::UnboundedSender<u64>,
  tokens: Arc<AtomicU64>,
}

/// The receiving side of a [`Handle`], read by the session's task.
#[derive(Debug)]
pub(crate) struct Inbox {
  pub(crate) commands: mpsc::Receiver<Command>,
  pub(crate) calls: mpsc::Receiver<Calling>,
  pub(crate) abandoned: mpsc::UnboundedReceiver<u64>,
}

impl Inbox {
  /// Refuses everything sent from now on; what is already queued can still be read.
  pub(crate) fn close(&mut self) {
    self.commands.close();
    self.calls.close();
    self.abandoned.close();
  }

  /// Fails the calls still queued, which were never sent, as calls to an ended session.
  pub(crate) fn refuse_queued_calls(&mut self) {
    while let Ok(calling) = self.calls.try_recv() {
      let _ = calling.reply.send(Err(CallError::ended()));
    }
  }
}

/// A server call on its way to the session's task.
#[derive(Debug)]
pub(crate) struct Calling {
  pub(crate) token: u64,
  pub(crate) method: MethodName,
  pub(crate) arguments: Vec<Value>,
  pub(crate) deadline: Option<Instant>,
  pub(crate) reply: oneshot::Sender<Result<Value, CallError>>,
}

impl Handle {
  /// A handle and the inbox its session's task reads.
  pub(crate) fn channel() -> (Self, Inbox) {
    let (commands, command_inbox) = mpsc::channel(QUEUE);
    let (calls, call_inbox) = mpsc::channel(QUEUE);
    let (abandoned, abandoned_inbox) = mpsc::unbounded_channel();
    let handle = Self {
      commands,
      calls,
      abandoned,
      tokens: Arc::default(),
    };
    let inbox = Inbox {
      commands: command_inbox,
      calls: call_inbox,
      abandoned: abandoned_inbox,
    };
    (handle, inbox)
  }

  /// Sends `command` to the session, waiting while its command queue is full.
  ///
  /// # Errors
  ///
  /// Gives the command back when the session has ended.
  pub async fn send(&self, command: Command) -> Result<(), SendError<Command>> {
    self.commands.send(command).await
  }

  /// Calls the client method `method` of this session with `arguments`, encoded as one array in
  /// the session codec, and returns its RESULT decoded (§8.8). The client must have been granted
  /// `server-calls`, and at most its HELLO `max_calls` server calls are open at once (§6.5).
  ///
  /// With a [`deadline`](CallOptions::deadline) the CALL carries it, and once it passes the call
  /// is cancelled with CANCEL and fails with DEADLINE_EXCEEDED (§8.6). Dropping the returned
  /// future before the answer cancels the call the same way, so it never holds one of the
  /// client's `max_calls` slots for nothing.
  ///
  /// # Errors
  ///
  /// The client's ERROR, as given. Locally, with [`nx`](CallError::nx) set since nothing was
  /// sent: NOT_FOUND once the session has ended, FAILED_PRECONDITION without `server-calls`,
  /// RESOURCE_EXHAUSTED at the client's `max_calls` or for a CALL above its `max_frame`, and
  /// INVALID_ARGUMENT for arguments the session codec cannot encode. Without `nx`: UNAVAILABLE
  /// when the session ends before the answer, DEADLINE_EXCEEDED, and INTERNAL for a RESULT or an
  /// ERROR detail that does not decode (§8.10).
  pub async fn call(&self, method: &MethodName, arguments: Vec<Value>, options: CallOptions) -> Result<Value, CallError> {
    let deadline = options.deadline.and_then(|deadline| Instant::now().checked_add(deadline));
    let (reply, answer) = oneshot::channel();
    let token = self.tokens.fetch_add(1, Ordering::Relaxed);
    let calling = Calling {
      token,
      method: method.clone(),
      arguments,
      deadline,
      reply,
    };
    let queued = match deadline {
      Some(deadline) => timeout_at(deadline, self.calls.send(calling))
        .await
        .map_err(|_| CallError::deadline_exceeded(true))?,
      None => self.calls.send(calling).await,
    };
    if queued.is_err() {
      return Err(CallError::ended());
    }
    let mut pending = Pending {
      abandoned: &self.abandoned,
      token: Some(token),
      answer,
    };
    let answered = match deadline {
      Some(deadline) => timeout_at(deadline, &mut pending.answer)
        .await
        .map_err(|_| CallError::deadline_exceeded(false))?,
      None => (&mut pending.answer).await,
    };
    pending.token = None;
    answered.unwrap_or_else(|_| Err(CallError::unavailable()))
  }
}

/// A queued call the caller still waits for. Dropped early, it closes the answer before it tells
/// the session task to abandon the call, so a task that reads the abandon before the call itself
/// finds the answer closed and never sends the CALL.
struct Pending<'a> {
  abandoned: &'a mpsc::UnboundedSender<u64>,
  token: Option<u64>,
  answer: oneshot::Receiver<Result<Value, CallError>>,
}

impl Drop for Pending<'_> {
  fn drop(&mut self) {
    if let Some(token) = self.token {
      self.answer.close();
      let _ = self.abandoned.send(token);
    }
  }
}

/// Every established session on this server. Cloning it gives another view of the same sessions.
#[derive(Debug, Clone, Default)]
pub struct Registry {
  inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
  sessions: HashMap<SessionId, Entry>,
  registrations: u64,
}

#[derive(Debug)]
struct Entry {
  handle: Handle,
  registration: u64,
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
    self.lock().sessions.len()
  }

  /// Whether no session is established.
  #[must_use]
  pub fn is_empty(&self) -> bool {
    self.lock().sessions.is_empty()
  }

  /// The handle of the established session `id`, if it is still running.
  #[must_use]
  pub fn get(&self, id: &SessionId) -> Option<Handle> {
    self.lock().sessions.get(id).map(|entry| entry.handle.clone())
  }

  /// Calls the client method `method` of the established session `id`, as
  /// [`Handle::call`] does.
  ///
  /// # Errors
  ///
  /// NOT_FOUND with [`nx`](CallError::nx) when no session `id` is running (§8.8), and the errors
  /// of [`Handle::call`].
  pub async fn call(
    &self,
    id: &SessionId,
    method: &MethodName,
    arguments: Vec<Value>,
    options: CallOptions,
  ) -> Result<Value, CallError> {
    let handle = self.get(id).ok_or_else(CallError::ended)?;
    handle.call(method, arguments, options).await
  }

  /// Registers the session `id`, reachable through `handle`, until the returned guard is dropped.
  pub(crate) fn register(&self, id: SessionId, handle: Handle) -> Registration {
    let mut inner = self.lock();
    inner.registrations += 1;
    let registration = inner.registrations;
    inner.sessions.insert(id.clone(), Entry { handle, registration });
    Registration {
      registry: self.clone(),
      id,
      registration,
    }
  }

  fn lock(&self) -> MutexGuard<'_, Inner> {
    self.inner.lock().unwrap_or_else(PoisonError::into_inner)
  }
}

/// A registered session. Dropping it removes the session, once, and only the entry this
/// registration made: a later registration under the same ID stays, even with the same handle.
#[derive(Debug)]
#[must_use = "dropping the registration removes the session"]
pub(crate) struct Registration {
  registry: Registry,
  id: SessionId,
  registration: u64,
}

impl Drop for Registration {
  fn drop(&mut self) {
    let mut inner = self.registry.lock();
    if inner
      .sessions
      .get(&self.id)
      .is_some_and(|entry| entry.registration == self.registration)
    {
      inner.sessions.remove(&self.id);
    }
  }
}

#[cfg(test)]
mod tests;
