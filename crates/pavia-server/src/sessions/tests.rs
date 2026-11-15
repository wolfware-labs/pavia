use std::time::Duration;

use futures_util::FutureExt;
use pavia_proto::registry::{CloseCode, StatusCode};
use pavia_proto::session::Command;
use pavia_proto::session_id::SessionId;
use tokio::sync::mpsc;

use super::{CallOptions, Handle, QUEUE, Registry};
use crate::test_support::{EXPECT_WITHIN, confirm};

fn id(text: &str) -> SessionId {
  SessionId::parse(text).unwrap()
}

fn handle() -> (Handle, mpsc::Receiver<Command>) {
  let (handle, inbox) = Handle::channel();
  (handle, inbox.commands)
}

const CLOSE: Command = Command::Close {
  code: CloseCode::Normal,
  msg: None,
};

#[tokio::test]
async fn reaches_a_registered_session_until_its_registration_drops() {
  let registry = Registry::new();
  let (handle, mut commands) = handle();
  let registration = registry.register(id("s_a"), handle);
  assert_eq!(registry.len(), 1);
  assert!(!registry.is_empty());
  registry.get(&id("s_a")).unwrap().send(CLOSE).await.unwrap();
  let received = tokio::time::timeout(EXPECT_WITHIN, commands.recv()).await;
  assert_eq!(received, Ok(Some(CLOSE)));
  assert!(registry.get(&id("s_b")).is_none());
  drop(registration);
  assert!(registry.is_empty());
  assert!(registry.get(&id("s_a")).is_none());
}

#[test]
fn shares_the_sessions_between_clones() {
  let registry = Registry::new();
  let view = registry.clone();
  let _first = registry.register(id("s_a"), handle().0);
  let _second = registry.register(id("s_b"), handle().0);
  assert_eq!(view.len(), 2);
}

#[test]
fn keeps_a_later_registration_under_the_same_id() {
  let registry = Registry::new();
  let earlier = registry.register(id("s_a"), handle().0);
  let (later_handle, _commands) = handle();
  let _later = registry.register(id("s_a"), later_handle);
  drop(earlier);
  assert_eq!(registry.len(), 1);
}

#[test]
fn keeps_a_later_registration_of_the_same_handle() {
  let registry = Registry::new();
  let (handle, _commands) = handle();
  let earlier = registry.register(id("s_a"), handle.clone());
  let _later = registry.register(id("s_a"), handle);
  drop(earlier);
  assert_eq!(registry.len(), 1);
}

#[tokio::test]
async fn gives_the_command_back_once_the_session_has_ended() {
  let registry = Registry::new();
  let (handle, commands) = handle();
  let _registration = registry.register(id("s_a"), handle);
  let kept = registry.get(&id("s_a")).unwrap();
  drop(commands);
  assert_eq!(kept.send(CLOSE).await.unwrap_err().0, CLOSE);
}

#[test]
fn removes_an_entry_even_after_a_panic_poisoned_the_lock() {
  let registry = Registry::new();
  let registration = registry.register(id("s_a"), handle().0);
  let poisoner = registry.clone();
  let _ = std::thread::spawn(move || {
    let _guard = poisoner.inner.lock().unwrap();
    panic!("poison the registry lock");
  })
  .join();
  drop(registration);
  assert!(registry.is_empty());
}

#[tokio::test]
async fn closes_the_answer_of_a_dropped_call_before_abandoning_it() {
  let (handle, mut inbox) = Handle::channel();
  let method = confirm();
  let dropped = handle.call(&method, Vec::new(), CallOptions::default()).now_or_never();
  assert!(dropped.is_none());
  let calling = inbox.calls.try_recv().unwrap();
  assert!(calling.reply.is_closed());
  assert_eq!(inbox.abandoned.try_recv().unwrap(), calling.token);
}

#[tokio::test]
async fn refuses_queued_calls_as_calls_to_an_ended_session() {
  let (handle, mut inbox) = Handle::channel();
  let method = confirm();
  let call = tokio::spawn(async move { handle.call(&method, Vec::new(), CallOptions::default()).await });
  while inbox.calls.is_empty() {
    tokio::task::yield_now().await;
  }
  inbox.refuse_queued_calls();
  let error = call.await.unwrap().unwrap_err();
  assert_eq!(
    (error.code(), error.nx(), error.msg()),
    (StatusCode::NotFound, true, "the session has ended")
  );
}

#[tokio::test]
async fn counts_the_wait_for_queue_space_against_the_deadline() {
  let (handle, _inbox) = Handle::channel();
  let method = confirm();
  for _ in 0..QUEUE {
    let _ = handle.call(&method, Vec::new(), CallOptions::default()).now_or_never();
  }
  let options = CallOptions::default().with_deadline(Duration::from_millis(50));
  let error = tokio::time::timeout(EXPECT_WITHIN, handle.call(&method, Vec::new(), options))
    .await
    .expect("the deadline to end the wait for queue space")
    .unwrap_err();
  assert_eq!((error.code(), error.nx()), (StatusCode::DeadlineExceeded, true));
}
