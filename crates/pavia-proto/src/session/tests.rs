use bytes::{Bytes, BytesMut};

use super::{Command, Ending, Input, Output, Session};
use crate::frame::{Close, Frame, Heartbeat};
use crate::registry::CloseCode;
use crate::stream::Side;
use crate::wire::envelope::{self, MAX_FRAME_FLOOR};
use crate::wire::header::DecodeLimits;
use crate::wire::varint;

const CODES: [CloseCode; 14] = [
  CloseCode::Normal,
  CloseCode::ProtocolError,
  CloseCode::UnsupportedVersion,
  CloseCode::Unauthenticated,
  CloseCode::Forbidden,
  CloseCode::IdleTimeout,
  CloseCode::SlowConsumer,
  CloseCode::GoingAway,
  CloseCode::Internal,
  CloseCode::FrameTooLarge,
  CloseCode::LimitExceeded,
  CloseCode::ResumeFailed,
  CloseCode::HandshakeTimeout,
  CloseCode::ContractMismatch,
];

fn message(frames: &[(u64, Frame)]) -> Bytes {
  let mut out = BytesMut::new();
  for (stream, frame) in frames {
    varint::encode(*stream, &mut out).unwrap();
    frame.encode(&mut out).unwrap();
  }
  out.freeze()
}

fn close(code: u64, msg: Option<&str>) -> Frame {
  Frame::Close(Close {
    code,
    msg: msg.map(str::to_owned),
  })
}

fn drain(session: &mut Session) -> Vec<Output> {
  std::iter::from_fn(|| session.poll_output()).collect()
}

fn frames(message: &Bytes) -> Vec<(u64, Frame)> {
  envelope::decode_message(message.clone(), MAX_FRAME_FLOOR)
    .unwrap()
    .into_iter()
    .map(|ws_frame| {
      (
        ws_frame.stream_id,
        Frame::from_raw(&ws_frame.frame, &DecodeLimits::default()).unwrap(),
      )
    })
    .collect()
}

#[test]
fn closes_with_every_code() {
  for code in CODES {
    let mut session = Session::new(Side::Server);
    session.handle_input(Input::Command(Command::Close {
      code,
      msg: Some("bye".into()),
    }));
    let outputs = drain(&mut session);
    let [Output::Transmit(sent), close_transport, ended] = outputs.as_slice() else {
      panic!("{code:?}: {outputs:?}");
    };
    assert_eq!(
      frames(sent),
      [(0, close(u64::from(u16::from(code)), Some("bye")))],
      "{code:?}"
    );
    assert_eq!(
      *close_transport,
      Output::CloseTransport {
        code,
        reason: "bye".into()
      }
    );
    assert_eq!(
      *ended,
      Output::Ended(Ending::Closed {
        code,
        msg: Some("bye".into()),
        by: Side::Server
      })
    );
  }
}

#[test]
fn mirrors_every_code_the_peer_closes_with() {
  for code in CODES {
    let mut session = Session::new(Side::Server);
    session.handle_input(Input::Message(message(&[(0, close(u64::from(u16::from(code)), None))])));
    assert_eq!(
      drain(&mut session),
      [
        Output::CloseTransport {
          code,
          reason: String::new()
        },
        Output::Ended(Ending::Closed {
          code,
          msg: None,
          by: Side::Client
        }),
      ]
    );
  }
}

#[test]
fn records_an_unknown_received_code_as_internal() {
  for unknown in [14, 99, u64::from(u16::MAX) + 1] {
    let mut session = Session::new(Side::Server);
    session.handle_input(Input::Message(message(&[(0, close(unknown, Some("later draft")))])));
    assert_eq!(
      drain(&mut session).last(),
      Some(&Output::Ended(Ending::Closed {
        code: CloseCode::Internal,
        msg: Some("later draft".into()),
        by: Side::Client
      })),
      "{unknown}"
    );
  }
}

#[test]
fn ends_on_transport_loss() {
  let mut session = Session::new(Side::Server);
  session.handle_input(Input::TransportLost);
  assert_eq!(drain(&mut session), [Output::Ended(Ending::TransportLost)]);
  assert!(session.is_ended());
}

#[test]
fn ends_once_when_close_and_transport_loss_arrive_together() {
  let mut session = Session::new(Side::Server);
  session.handle_input(Input::Message(message(&[(0, close(0, None))])));
  session.handle_input(Input::TransportLost);
  let endings = drain(&mut session)
    .into_iter()
    .filter(|output| matches!(output, Output::Ended(_)))
    .count();
  assert_eq!(endings, 1);
}

#[test]
fn sends_nothing_after_close() {
  let mut session = Session::new(Side::Server);
  session.handle_input(Input::Command(Command::Close {
    code: CloseCode::Normal,
    msg: None,
  }));
  drain(&mut session);
  session.handle_input(Input::Command(Command::Close {
    code: CloseCode::Internal,
    msg: None,
  }));
  session.handle_input(Input::Message(message(&[(0, close(0, None))])));
  session.handle_input(Input::TransportLost);
  assert_eq!(drain(&mut session), []);
}

#[test]
fn stops_at_close_inside_a_message() {
  let ping = Frame::Ping {
    data: Heartbeat::try_from(Bytes::new()).unwrap(),
  };
  let mut session = Session::new(Side::Server);
  session.handle_input(Input::Message(message(&[
    (0, close(0, None)),
    (0, close(1, None)),
    (0, ping),
  ])));
  let outputs = drain(&mut session);
  assert_eq!(outputs.len(), 2);
  assert!(matches!(
    outputs[1],
    Output::Ended(Ending::Closed {
      code: CloseCode::Normal,
      ..
    })
  ));
}

#[test]
fn closes_with_protocol_error_on_a_malformed_message() {
  for bad in [&b""[..], b"\x00\x07", b"\x00\x7f\x00\x00", b"\x02\x07\x00\x03\x02\xa1\x06"] {
    let mut session = Session::new(Side::Server);
    session.handle_input(Input::Message(Bytes::copy_from_slice(bad)));
    let outputs = drain(&mut session);
    let Some(Output::Transmit(sent)) = outputs.first() else {
      panic!("{bad:02x?}: {outputs:?}");
    };
    let [(0, Frame::Close(Close { code: 1, .. }))] = frames(sent).as_slice() else {
      panic!("{bad:02x?}: {outputs:?}");
    };
    assert!(matches!(
      outputs.last(),
      Some(Output::Ended(Ending::Closed {
        code: CloseCode::ProtocolError,
        by: Side::Server,
        ..
      }))
    ));
  }
}

#[test]
fn refuses_close_outside_the_control_stream() {
  let mut session = Session::new(Side::Server);
  session.handle_input(Input::Message(message(&[(2, close(0, None))])));
  assert!(matches!(
    drain(&mut session).last(),
    Some(Output::Ended(Ending::Closed {
      code: CloseCode::ProtocolError,
      by: Side::Server,
      ..
    }))
  ));
}

#[test]
fn ignores_other_frames_until_the_handshake_exists() {
  let ping = Frame::Ping {
    data: Heartbeat::try_from(Bytes::new()).unwrap(),
  };
  let mut session = Session::new(Side::Server);
  session.handle_input(Input::Message(message(&[(0, ping)])));
  assert_eq!(drain(&mut session), []);
  assert!(!session.is_ended());
}

#[test]
fn names_the_peer() {
  assert_eq!(Side::Client.peer(), Side::Server);
  assert_eq!(Side::Server.peer(), Side::Client);
}
