use super::{Binding, Origin, Role, Side, allowed, check};
use crate::error::ProtocolError;
use crate::frame::FrameType;

const ORIGINS: [Origin; 6] = [
  Origin::Control(Side::Client),
  Origin::Control(Side::Server),
  Origin::Call(Role::Caller),
  Origin::Call(Role::Callee),
  Origin::Lane,
  Origin::Datagram,
];

const REGISTRY: [(FrameType, &str, &str); 29] = [
  (FrameType::Hello, "x.....", "x....."),
  (FrameType::Welcome, ".x....", ".x...."),
  (FrameType::Reject, ".x....", ".x...."),
  (FrameType::Ping, "xx....", "xx...."),
  (FrameType::Pong, "xx....", "xx...."),
  (FrameType::Goaway, ".x....", ".x...."),
  (FrameType::Close, "xx....", "xx...."),
  (FrameType::AuthRefresh, "x.....", "x....."),
  (FrameType::AuthResult, ".x....", ".x...."),
  (FrameType::Ack, "xx....", "xx...."),
  (FrameType::AuthExpiring, ".x....", ".x...."),
  (FrameType::RequestError, ".x....", ".x...."),
  (FrameType::Call, "..xx..", "..xx.."),
  (FrameType::Item, "..xx..", "..xx.."),
  (FrameType::End, "..xx..", "..xx.."),
  (FrameType::Result, "...x..", "...x.."),
  (FrameType::Error, "...x..", "...x.."),
  (FrameType::Cancel, "..x...", "..x..."),
  (FrameType::Credit, "......", "..xx.."),
  (FrameType::Notify, "xx..x.", "xx..x."),
  (FrameType::LaneOpen, "....x.", "....x."),
  (FrameType::Subscribe, "x.....", "x....."),
  (FrameType::Subscribed, ".x....", ".x...."),
  (FrameType::Unsubscribe, "x.....", "x....."),
  (FrameType::Unsubscribed, ".x....", ".x...."),
  (FrameType::Publication, ".x....", ".x...."),
  (FrameType::Presence, ".x....", ".x...."),
  (FrameType::Datagram, ".....x", "xx...."),
  (FrameType::DgramBind, "xx....", "xx...."),
];

fn rejected(frame_type: FrameType, origin: Origin, binding: Binding) -> bool {
  matches!(check(frame_type, origin, binding), Err(ProtocolError::FrameTypeNotAllowed))
}

#[test]
fn the_expected_table_lists_every_registered_type_once() {
  let listed: Vec<FrameType> = REGISTRY.iter().map(|(frame_type, ..)| *frame_type).collect();
  assert_eq!(listed, FrameType::ALL);
}

#[test]
fn matches_the_registry_for_every_type_origin_and_binding() {
  for (frame_type, webtransport, websocket) in REGISTRY {
    for (binding, row) in [(Binding::WebTransport, webtransport), (Binding::WebSocket, websocket)] {
      for (origin, mark) in ORIGINS.into_iter().zip(row.chars()) {
        assert_eq!(
          allowed(frame_type, origin, binding),
          mark == 'x',
          "{frame_type:?} from {origin:?} on {binding:?}"
        );
      }
    }
  }
}

#[test]
fn rejects_call_on_the_control_stream() {
  for side in [Side::Client, Side::Server] {
    assert!(rejected(FrameType::Call, Origin::Control(side), Binding::WebSocket));
  }
}

#[test]
fn rejects_credit_on_a_webtransport_call_stream() {
  assert!(rejected(FrameType::Credit, Origin::Call(Role::Caller), Binding::WebTransport));
  assert!(check(FrameType::Credit, Origin::Call(Role::Caller), Binding::WebSocket).is_ok());
}

#[test]
fn rejects_result_and_error_from_the_caller() {
  for frame_type in [FrameType::Result, FrameType::Error] {
    assert!(rejected(frame_type, Origin::Call(Role::Caller), Binding::WebTransport));
    assert!(check(frame_type, Origin::Call(Role::Callee), Binding::WebTransport).is_ok());
  }
}

#[test]
fn rejects_cancel_from_the_callee() {
  assert!(rejected(FrameType::Cancel, Origin::Call(Role::Callee), Binding::WebSocket));
}

#[test]
fn allows_datagram_frames_on_the_control_stream_only_on_websocket() {
  assert!(rejected(
    FrameType::Datagram,
    Origin::Control(Side::Server),
    Binding::WebTransport
  ));
  assert!(check(FrameType::Datagram, Origin::Control(Side::Server), Binding::WebSocket).is_ok());
  assert!(check(FrameType::Datagram, Origin::Datagram, Binding::WebTransport).is_ok());
}

#[test]
fn allows_notify_on_a_lane_but_not_on_a_call_stream() {
  assert!(rejected(FrameType::Notify, Origin::Call(Role::Caller), Binding::WebTransport));
  assert!(check(FrameType::Notify, Origin::Lane, Binding::WebTransport).is_ok());
}

#[test]
fn rejects_hello_from_the_server_and_welcome_from_the_client() {
  assert!(rejected(
    FrameType::Hello,
    Origin::Control(Side::Server),
    Binding::WebTransport
  ));
  assert!(rejected(
    FrameType::Welcome,
    Origin::Control(Side::Client),
    Binding::WebTransport
  ));
}
