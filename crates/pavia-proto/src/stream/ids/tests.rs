use super::{Admitted, MAX_STREAM_ID, StreamIds};
use crate::error::ProtocolError;
use crate::frame::FrameType;
use crate::stream::{Side, opens_stream};

const OPENING: bool = true;
const FOLLOWING: bool = false;

fn server() -> StreamIds {
  StreamIds::new(Side::Server)
}

fn assert_rejects(ids: &mut StreamIds, stream_id: u64, opening: bool, expected: &ProtocolError) {
  let result = ids.receive(stream_id, opening);
  assert!(
    matches!(&result, Err(error) if std::mem::discriminant(error) == std::mem::discriminant(expected)),
    "stream {stream_id}, opening {opening}: {result:?}"
  );
}

#[test]
fn opens_a_client_stream_with_its_first_frame() {
  let mut ids = server();
  assert_eq!(ids.receive(2, OPENING).ok(), Some(Admitted::Opened));
  assert_eq!(ids.receive(2, FOLLOWING).ok(), Some(Admitted::Open));
  assert!(ids.is_open(2));
}

#[test]
fn admits_any_frame_on_the_control_stream() {
  let mut ids = server();
  assert_eq!(ids.receive(0, FOLLOWING).ok(), Some(Admitted::Control));
  assert!(ids.is_open(0));
}

#[test]
fn rejects_a_client_opening_an_odd_stream() {
  assert_rejects(&mut server(), 3, OPENING, &ProtocolError::StreamIdWrongParity);
}

#[test]
fn rejects_a_server_opening_an_even_stream() {
  assert_rejects(
    &mut StreamIds::new(Side::Client),
    2,
    OPENING,
    &ProtocolError::StreamIdWrongParity,
  );
}

#[test]
fn rejects_decreasing_ids() {
  let mut ids = server();
  ids.receive(4, OPENING).unwrap();
  assert_rejects(&mut ids, 2, OPENING, &ProtocolError::StreamIdOutOfOrder);
}

#[test]
fn rejects_reopening_a_closed_stream() {
  let mut ids = server();
  ids.receive(2, OPENING).unwrap();
  ids.close(2);
  assert_rejects(&mut ids, 2, OPENING, &ProtocolError::StreamIdOutOfOrder);
}

#[test]
fn rejects_a_second_opening_frame_on_an_open_stream() {
  let mut ids = server();
  ids.receive(2, OPENING).unwrap();
  assert_rejects(&mut ids, 2, OPENING, &ProtocolError::StreamIdOutOfOrder);
}

#[test]
fn ignores_a_frame_on_a_closed_stream() {
  let mut ids = server();
  ids.receive(2, OPENING).unwrap();
  ids.close(2);
  assert_rejects(&mut ids, 2, FOLLOWING, &ProtocolError::FrameOnClosedStream);
  assert!(!ids.is_open(2));
}

#[test]
fn rejects_a_frame_above_the_highest_opened_id() {
  let mut ids = server();
  ids.receive(2, OPENING).unwrap();
  assert_rejects(&mut ids, 6, FOLLOWING, &ProtocolError::StreamIdOutOfOrder);
}

#[test]
fn ignores_frames_on_a_skipped_id_but_refuses_to_open_it() {
  let mut ids = server();
  ids.receive(2, OPENING).unwrap();
  ids.receive(6, OPENING).unwrap();
  assert_rejects(&mut ids, 4, FOLLOWING, &ProtocolError::FrameOnClosedStream);
  assert_rejects(&mut ids, 4, OPENING, &ProtocolError::StreamIdOutOfOrder);
}

#[test]
fn checks_frames_on_own_streams_against_own_highest_id() {
  let mut ids = server();
  assert_eq!(ids.open_next(), Some(1));
  assert_eq!(ids.open_next(), Some(3));
  ids.close(1);
  assert_rejects(&mut ids, 1, FOLLOWING, &ProtocolError::FrameOnClosedStream);
  assert_eq!(ids.receive(3, FOLLOWING).ok(), Some(Admitted::Open));
  assert_rejects(&mut ids, 5, FOLLOWING, &ProtocolError::StreamIdOutOfOrder);
  assert_rejects(&mut ids, 5, OPENING, &ProtocolError::StreamIdWrongParity);
}

#[test]
fn keeps_the_two_initiators_apart() {
  let mut ids = server();
  ids.receive(8, OPENING).unwrap();
  assert_eq!(ids.open_next(), Some(1));
  assert_eq!(ids.receive(2, OPENING).ok(), None);
  assert_eq!(ids.receive(10, OPENING).ok(), Some(Admitted::Opened));
}

#[test]
fn hands_out_even_ids_to_the_client_and_odd_ids_to_the_server() {
  let mut client = StreamIds::new(Side::Client);
  let mut server = server();
  let client_ids: Vec<_> = (0..3).filter_map(|_| client.open_next()).collect();
  let server_ids: Vec<_> = (0..3).filter_map(|_| server.open_next()).collect();
  assert_eq!(client_ids, [2, 4, 6]);
  assert_eq!(server_ids, [1, 3, 5]);
}

#[test]
fn restarts_numbering_on_a_new_transport() {
  let mut ids = StreamIds::new(Side::Client);
  ids.open_next();
  ids.open_next();
  let mut ids = StreamIds::new(Side::Client);
  assert_eq!(ids.open_next(), Some(2));
}

#[test]
fn stops_allocating_at_the_top_of_the_varint_range() {
  let mut ids = server();
  ids.highest_by_server = MAX_STREAM_ID;
  assert_eq!(ids.open_next(), None);
  let mut ids = StreamIds::new(Side::Client);
  ids.highest_by_client = MAX_STREAM_ID - 1;
  assert_eq!(ids.open_next(), None);
  ids.highest_by_client = MAX_STREAM_ID - 3;
  assert_eq!(ids.open_next(), Some(MAX_STREAM_ID - 1));
}

#[test]
fn opens_a_stream_with_call_or_lane_open_only() {
  let mut ids = StreamIds::new(Side::Client);
  assert_eq!(ids.receive(1, opens_stream(FrameType::LaneOpen)).ok(), Some(Admitted::Opened));
  assert_eq!(ids.receive(3, opens_stream(FrameType::Call)).ok(), Some(Admitted::Opened));
  assert_rejects(
    &mut ids,
    5,
    opens_stream(FrameType::Notify),
    &ProtocolError::StreamIdOutOfOrder,
  );
  let openers: Vec<_> = FrameType::ALL
    .into_iter()
    .filter(|frame_type| opens_stream(*frame_type))
    .collect();
  assert_eq!(openers, [FrameType::Call, FrameType::LaneOpen]);
}

#[test]
fn does_not_grow_with_closed_streams() {
  let mut ids = server();
  for n in 1..=1_000_000_u64 {
    let stream_id = 2 * n;
    ids.receive(stream_id, OPENING).unwrap();
    ids.close(stream_id);
  }
  assert!(ids.open.is_empty());
  assert!(ids.open.capacity() < 16);
  assert_rejects(&mut ids, 2, FOLLOWING, &ProtocolError::FrameOnClosedStream);
  assert_rejects(&mut ids, 1_000_000, FOLLOWING, &ProtocolError::FrameOnClosedStream);
}
