use pavia_proto::registry::CloseCode;
use pavia_proto::session::Command;
use pavia_proto::session_id::SessionId;
use tokio::sync::mpsc;

use super::{Handle, Registry};
use crate::test_support::EXPECT_WITHIN;

fn id(text: &str) -> SessionId {
  SessionId::parse(text).unwrap()
}

fn handle() -> (Handle, mpsc::Receiver<Command>) {
  let (sender, receiver) = mpsc::channel(1);
  (Handle::new(sender), receiver)
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
    let _guard = poisoner.sessions.lock().unwrap();
    panic!("poison the registry lock");
  })
  .join();
  drop(registration);
  assert!(registry.is_empty());
}
