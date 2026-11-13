//! Lane 0, the control stream: the client's notifications, run one at a time in delivery order
//! (§9.1, §9.2).
//!
//! A NOTIFY is checked when it arrives and dropped with a log line when it names no `notify`
//! method or its arguments do not fit. The rest wait in order; each runs the authorizer and then
//! the handler, and the next starts only once that has completed. Nothing is ever sent back.

use std::collections::VecDeque;
use std::error::Error;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use pavia_proto::data::Codec;
use pavia_proto::frame::Notify;
use pavia_proto::session_id::SessionId;
use tokio::task::{JoinError, JoinSet};

use super::arguments;
use crate::auth::{Authorizer, Principal};
use crate::methods::{Handler, HandlerError, Kind, MethodName, MethodRegistry};

type Notifying = Pin<Box<dyn Future<Output = Notified> + Send>>;

pub(super) enum Notified {
  Done,
  Denied,
  Failed(HandlerError),
}

#[derive(Default)]
pub(super) struct Lane {
  session: Option<SessionId>,
  waiting: VecDeque<(MethodName, Notifying)>,
  running: JoinSet<Notified>,
  current: Option<MethodName>,
}

impl Lane {
  pub(super) fn established(&mut self, session: SessionId) {
    self.session = Some(session);
  }

  pub(super) fn receive<Z: Authorizer + 'static>(
    &mut self,
    notify: &Notify,
    methods: &MethodRegistry<Handler>,
    codec: Codec,
    authorizer: &Arc<Z>,
    principal: &Arc<Principal>,
  ) {
    let session = self.session.as_ref().map(SessionId::as_str);
    let Some((name, method)) = methods
      .server_entry(&notify.method)
      .filter(|(_, method)| matches!(method.kind, Kind::Notify))
    else {
      tracing::warn!(session, method = notify.method, "notification to an unknown method dropped");
      return;
    };
    let arguments = match arguments(&notify.data, codec, &method.handler) {
      Ok(arguments) => arguments,
      Err(refusal) => {
        tracing::warn!(
          session,
          method = name.as_str(),
          reason = refusal.msg,
          "notification with bad arguments dropped"
        );
        return;
      }
    };
    let (method_name, handler) = (name.clone(), method.handler.clone());
    let (authorizer, principal) = (Arc::clone(authorizer), Arc::clone(principal));
    let notifying = Box::pin(async move {
      if !authorizer.authorize(&principal, &method_name).await {
        return Notified::Denied;
      }
      handler.run(arguments).await.map_or_else(Notified::Failed, |_| Notified::Done)
    });
    self.waiting.push_back((name.clone(), notifying));
    self.start_next();
  }

  pub(super) fn is_running(&self) -> bool {
    !self.running.is_empty()
  }

  pub(super) async fn join(&mut self) -> Option<Result<Notified, JoinError>> {
    self.running.join_next().await
  }

  pub(super) fn finished(&mut self, joined: Result<Notified, JoinError>) {
    let method = self.current.take().expect("a joined notification was started");
    let (session, method) = (self.session.as_ref().map(SessionId::as_str), method.as_str());
    match joined {
      Ok(Notified::Done) => {}
      Ok(Notified::Denied) => tracing::warn!(session, method, "notification not authorized, dropped"),
      Ok(Notified::Failed(HandlerError::Status(status))) => {
        let code = format!("{:?}", status.code());
        tracing::warn!(
          session,
          method,
          code,
          msg = status.msg(),
          "notification handler returned an error"
        );
      }
      Ok(Notified::Failed(HandlerError::Unexpected(error))) => {
        let error: &(dyn Error + 'static) = &*error;
        tracing::error!(session, method, error, "notification failed unexpectedly");
      }
      Err(error) => {
        let error: &(dyn Error + 'static) = &error;
        tracing::error!(session, method, error, "notification failed unexpectedly");
      }
    }
    self.start_next();
  }

  fn start_next(&mut self) {
    if self.running.is_empty()
      && let Some((method, notifying)) = self.waiting.pop_front()
    {
      self.current = Some(method);
      self.running.spawn(notifying);
    }
  }
}
