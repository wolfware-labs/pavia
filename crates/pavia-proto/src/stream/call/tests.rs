use proptest::prelude::*;

use super::{CallState, Effect, PeerEvent, Shape, Step};
use crate::error::ProtocolError;
use crate::frame::FrameType;
use crate::stream::Role;

const SHAPES: [Shape; 4] = [Shape::Unary, Shape::ServerStream, Shape::ClientStream, Shape::Bidirectional];

const CALLER: Role = Role::Caller;
const CALLEE: Role = Role::Callee;

const AFTER_CALL: [(Role, FrameType, &str); 14] = [
  (CALLER, FrameType::Call, "nnnn"),
  (CALLER, FrameType::Item, "ttaa"),
  (CALLER, FrameType::End, "ttaa"),
  (CALLER, FrameType::Result, "nnnn"),
  (CALLER, FrameType::Error, "nnnn"),
  (CALLER, FrameType::Cancel, "aaaa"),
  (CALLER, FrameType::Credit, "aaaa"),
  (CALLEE, FrameType::Call, "nnnn"),
  (CALLEE, FrameType::Item, "sasa"),
  (CALLEE, FrameType::End, "sasa"),
  (CALLEE, FrameType::Result, "asas"),
  (CALLEE, FrameType::Error, "aaaa"),
  (CALLEE, FrameType::Cancel, "nnnn"),
  (CALLEE, FrameType::Credit, "aaaa"),
];

fn code(outcome: Result<Step, ProtocolError>) -> char {
  match outcome {
    Ok(Step::Accepted) => 'a',
    Ok(Step::Ignored) => 'i',
    Err(ProtocolError::FrameAfterTerminalFrame) => 't',
    Err(ProtocolError::FrameNotAllowedInShape) => 's',
    Err(ProtocolError::FrameTypeNotAllowed) => 'n',
    Err(ProtocolError::FrameAfterCallCompleted) => 'c',
    Err(other) => panic!("unexpected {other:?}"),
  }
}

fn run(shape: Shape, local: Role, frames: &[(Role, FrameType)]) -> (CallState, String) {
  let mut call = CallState::new(shape, local);
  let codes = frames
    .iter()
    .map(|(sender, frame_type)| code(call.on_frame(*sender, *frame_type)))
    .collect();
  (call, codes)
}

#[test]
fn matches_the_lifecycle_table_right_after_call() {
  for local in [CALLER, CALLEE] {
    for (sender, frame_type, row) in AFTER_CALL {
      for (shape, expected) in SHAPES.into_iter().zip(row.chars()) {
        let (_, codes) = run(shape, local, &[(sender, frame_type)]);
        assert_eq!(
          codes,
          expected.to_string(),
          "{frame_type:?} from {sender:?} on {shape:?} at the {local:?}"
        );
      }
    }
  }
}

#[test]
fn rejects_every_frame_type_that_never_travels_on_a_call_stream() {
  let call_frames = [
    FrameType::Call,
    FrameType::Item,
    FrameType::End,
    FrameType::Result,
    FrameType::Error,
    FrameType::Cancel,
    FrameType::Credit,
  ];
  for frame_type in FrameType::ALL
    .into_iter()
    .filter(|frame_type| !call_frames.contains(frame_type))
  {
    for sender in [CALLER, CALLEE] {
      let (_, codes) = run(Shape::Bidirectional, CALLEE, &[(sender, frame_type)]);
      assert_eq!(codes, "n", "{frame_type:?} from {sender:?}");
    }
  }
}

#[test]
fn completes_a_unary_call_on_result_or_error() {
  for terminal in [FrameType::Result, FrameType::Error] {
    for local in [CALLER, CALLEE] {
      let (call, codes) = run(Shape::Unary, local, &[(CALLEE, terminal)]);
      assert_eq!(codes, "a");
      assert!(call.is_complete());
    }
  }
}

#[test]
fn completes_a_server_stream_on_the_callee_end() {
  let (call, codes) = run(
    Shape::ServerStream,
    CALLER,
    &[(CALLEE, FrameType::Item), (CALLEE, FrameType::Item), (CALLEE, FrameType::End)],
  );
  assert_eq!(codes, "aaa");
  assert!(call.is_complete());
}

#[test]
fn rejects_frames_after_the_terminal_frame_of_their_half() {
  let (_, codes) = run(
    Shape::ClientStream,
    CALLEE,
    &[(CALLER, FrameType::Item), (CALLER, FrameType::End), (CALLER, FrameType::Item)],
  );
  assert_eq!(codes, "aat");
  let (_, codes) = run(
    Shape::Bidirectional,
    CALLER,
    &[
      (CALLEE, FrameType::End),
      (CALLEE, FrameType::Item),
      (CALLEE, FrameType::Error),
    ],
  );
  assert_eq!(codes, "att");
}

#[test]
fn rejects_frames_the_shape_does_not_use() {
  let (_, codes) = run(Shape::ServerStream, CALLER, &[(CALLEE, FrameType::Result)]);
  assert_eq!(codes, "s");
  let (_, codes) = run(Shape::Unary, CALLER, &[(CALLEE, FrameType::End)]);
  assert_eq!(codes, "s");
}

#[test]
fn completes_a_client_stream_on_an_early_terminal_frame() {
  for local in [CALLER, CALLEE] {
    let (call, codes) = run(
      Shape::ClientStream,
      local,
      &[
        (CALLER, FrameType::Item),
        (CALLEE, FrameType::Error),
        (CALLER, FrameType::Item),
        (CALLER, FrameType::End),
      ],
    );
    assert_eq!(codes, "aacc", "at the {local:?}");
    assert!(call.is_complete());
  }
  let (call, codes) = run(Shape::ClientStream, CALLEE, &[(CALLEE, FrameType::Result)]);
  assert_eq!(codes, "a");
  assert!(call.is_complete());
}

#[test]
fn accepts_cancel_after_the_caller_end() {
  let (call, codes) = run(
    Shape::Bidirectional,
    CALLEE,
    &[
      (CALLER, FrameType::End),
      (CALLER, FrameType::Cancel),
      (CALLEE, FrameType::Error),
    ],
  );
  assert_eq!(codes, "aaa");
  assert!(call.is_complete());
}

#[test]
fn ignores_a_second_cancel() {
  let (call, codes) = run(
    Shape::Unary,
    CALLEE,
    &[(CALLER, FrameType::Cancel), (CALLER, FrameType::Cancel)],
  );
  assert_eq!(codes, "ai");
  assert!(call.response_open());
}

#[test]
fn drops_a_cancel_that_crossed_the_terminal_frame() {
  let (_, codes) = run(
    Shape::Unary,
    CALLEE,
    &[(CALLEE, FrameType::Result), (CALLER, FrameType::Cancel)],
  );
  assert_eq!(codes, "ac");
}

#[test]
fn ignores_everything_but_the_terminal_frame_after_cancelling_as_caller() {
  let (call, codes) = run(
    Shape::ServerStream,
    CALLER,
    &[
      (CALLER, FrameType::Cancel),
      (CALLEE, FrameType::Item),
      (CALLEE, FrameType::Credit),
      (CALLEE, FrameType::Result),
      (CALLEE, FrameType::Error),
    ],
  );
  assert_eq!(codes, "aiiia");
  assert!(call.is_complete());
  let (_, codes) = run(
    Shape::Unary,
    CALLER,
    &[(CALLER, FrameType::Cancel), (CALLEE, FrameType::Item)],
  );
  assert_eq!(codes, "ai");
}

#[test]
fn keeps_a_bidirectional_call_open_after_the_callee_end() {
  let (call, codes) = run(
    Shape::Bidirectional,
    CALLEE,
    &[(CALLEE, FrameType::End), (CALLER, FrameType::Item), (CALLER, FrameType::Item)],
  );
  assert_eq!(codes, "aaa");
  assert!(!call.is_complete());
  for closing in [FrameType::End, FrameType::Cancel] {
    let mut open = call;
    assert_eq!(open.on_frame(CALLER, closing).ok(), Some(Step::Accepted));
    assert!(open.is_complete(), "{closing:?}");
  }
}

#[test]
fn closes_the_request_half_normally_on_a_fin_after_call() {
  for shape in [Shape::Unary, Shape::ServerStream] {
    let mut call = CallState::new(shape, CALLEE);
    assert_eq!(call.on_peer_event(PeerEvent::Fin), Effect::None);
    assert!(!call.is_complete());
    assert_eq!(call.on_frame(CALLEE, FrameType::Error).ok(), Some(Step::Accepted));
    assert!(call.is_complete());
  }
}

#[test]
fn treats_a_caller_fin_before_end_as_cancel() {
  let mut call = CallState::new(Shape::ClientStream, CALLEE);
  assert_eq!(call.on_frame(CALLER, FrameType::Item).ok(), Some(Step::Accepted));
  assert_eq!(call.on_peer_event(PeerEvent::Fin), Effect::Cancelled);
  assert!(call.response_open());
  assert_eq!(call.on_frame(CALLEE, FrameType::Error).ok(), Some(Step::Accepted));
  assert!(call.is_complete());
}

#[test]
fn treats_a_callee_fin_before_its_terminal_frame_as_unavailable() {
  let mut call = CallState::new(Shape::ServerStream, CALLER);
  assert_eq!(call.on_frame(CALLEE, FrameType::Item).ok(), Some(Step::Accepted));
  assert_eq!(call.on_peer_event(PeerEvent::Fin), Effect::Unavailable);
  assert!(call.is_complete());
}

#[test]
fn treats_resets_by_role() {
  let mut callee = CallState::new(Shape::Bidirectional, CALLEE);
  assert_eq!(callee.on_peer_event(PeerEvent::Reset), Effect::Cancelled);
  assert_eq!(callee.on_peer_event(PeerEvent::Reset), Effect::None);
  let mut caller = CallState::new(Shape::Bidirectional, CALLER);
  assert_eq!(caller.on_peer_event(PeerEvent::Reset), Effect::Unavailable);
  assert!(caller.is_complete());
}

#[test]
fn ignores_a_callee_reset_after_its_terminal_frame() {
  let mut call = CallState::new(Shape::Bidirectional, CALLER);
  assert_eq!(call.on_frame(CALLEE, FrameType::End).ok(), Some(Step::Accepted));
  assert_eq!(call.on_peer_event(PeerEvent::Reset), Effect::None);
  assert!(!call.is_complete());
}

#[test]
fn completes_with_cancelled_when_the_callee_resets_after_a_cancel() {
  let mut call = CallState::new(Shape::ClientStream, CALLER);
  assert_eq!(call.on_frame(CALLER, FrameType::End).ok(), Some(Step::Accepted));
  assert_eq!(call.on_frame(CALLER, FrameType::Cancel).ok(), Some(Step::Accepted));
  assert_eq!(call.on_peer_event(PeerEvent::Reset), Effect::Cancelled);
  assert!(call.is_complete());
}

#[test]
fn treats_stop_sending_from_the_caller_as_cancel() {
  let mut call = CallState::new(Shape::ClientStream, CALLEE);
  assert_eq!(call.on_frame(CALLER, FrameType::End).ok(), Some(Step::Accepted));
  assert_eq!(call.on_peer_event(PeerEvent::StopSending), Effect::Cancelled);
  assert!(call.is_complete());
}

#[test]
fn ignores_peer_events_after_the_call_is_complete() {
  for local in [CALLER, CALLEE] {
    for event in [PeerEvent::Reset, PeerEvent::Fin, PeerEvent::StopSending] {
      let mut call = CallState::new(Shape::Unary, local);
      assert_eq!(call.on_frame(CALLEE, FrameType::Result).ok(), Some(Step::Accepted));
      assert_eq!(call.on_peer_event(event), Effect::None);
      assert!(call.is_complete());
    }
  }
}

#[test]
fn reads_shapes_from_the_wire() {
  for (value, shape) in (0..).zip(SHAPES) {
    assert_eq!(Shape::try_from(value), Ok(shape));
  }
  assert_eq!(Shape::try_from(4), Err(4));
}

#[derive(Debug, Clone, Copy)]
enum Ending {
  Normal,
  EarlyError(usize),
  Cancel(usize),
}

fn legal_sequence(
  shape: Shape,
  caller_items: usize,
  callee_items: usize,
  picks: &[bool],
  ending: Ending,
) -> Vec<(Role, FrameType)> {
  let mut request: Vec<FrameType> = Vec::new();
  if shape.caller_streams() {
    request.extend(std::iter::repeat_n(FrameType::Item, caller_items));
    request.push(FrameType::End);
  }
  let mut response: Vec<FrameType> = Vec::new();
  if shape.callee_streams() {
    response.extend(std::iter::repeat_n(FrameType::Item, callee_items));
    response.push(FrameType::End);
  } else {
    response.push(FrameType::Result);
  }
  request.reverse();
  response.reverse();

  let mut frames = Vec::new();
  let mut response_open = true;
  for step in 0.. {
    if request.is_empty() && !response_open {
      return frames;
    }
    match ending {
      Ending::EarlyError(at) if at == step && response_open => {
        frames.push((CALLEE, FrameType::Error));
        return frames;
      }
      Ending::Cancel(at) if at == step => {
        frames.push((CALLER, FrameType::Cancel));
        if response_open {
          frames.push((CALLEE, FrameType::Error));
        }
        return frames;
      }
      _ => {}
    }
    let from_caller = picks.get(step % picks.len().max(1)).copied().unwrap_or(true);
    let pick_request = !request.is_empty() && (from_caller || response.is_empty());
    if pick_request {
      frames.push((CALLER, request.pop().unwrap()));
    } else if let Some(frame_type) = response.pop() {
      frames.push((CALLEE, frame_type));
      if response.is_empty() {
        response_open = false;
        if frame_type == FrameType::Result {
          return frames;
        }
      }
    } else {
      return frames;
    }
  }
  frames
}

fn any_ending() -> impl Strategy<Value = Ending> {
  prop_oneof![
    Just(Ending::Normal),
    (0_usize..10).prop_map(Ending::EarlyError),
    (0_usize..10).prop_map(Ending::Cancel),
  ]
}

proptest! {
  #[test]
  fn completes_every_legal_sequence_without_an_error(
    shape in prop::sample::select(SHAPES.to_vec()),
    caller_items in 0_usize..4,
    callee_items in 0_usize..4,
    picks in prop::collection::vec(any::<bool>(), 0..12),
    ending in any_ending(),
  ) {
    let frames = legal_sequence(shape, caller_items, callee_items, &picks, ending);
    for local in [CALLER, CALLEE] {
      let mut call = CallState::new(shape, local);
      for (index, (sender, frame_type)) in frames.iter().enumerate() {
        prop_assert!(!call.is_complete(), "complete before frame {index} of {frames:?}");
        prop_assert_eq!(call.on_frame(*sender, *frame_type).ok(), Some(Step::Accepted), "frame {} of {:?}", index, frames);
      }
      prop_assert!(call.is_complete(), "not complete after {frames:?} at the {local:?}");
    }
  }
}
