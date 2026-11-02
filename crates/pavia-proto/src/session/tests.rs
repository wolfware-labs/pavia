use bytes::{Bytes, BytesMut};

use std::time::{Duration, Instant};

use super::{AuthFailure, Command, Config, ConfigError, Ending, Input, Output, Session, State, ValidConfig};
use crate::data::Codec;
use crate::frame::Reject;
use crate::frame::{AckMap, Auth, Cancel, Close, Frame, Heartbeat, Hello, Limits, Resume, ServerLimits, Token, Welcome};
use crate::registry::CloseCode;
use crate::session_id::SessionId;
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

trait Feed {
  fn feed(&mut self, input: Input);
}

impl Feed for Session {
  fn feed(&mut self, input: Input) {
    self.handle_input(input, Instant::now());
  }
}

fn server() -> Session {
  Session::server(&ValidConfig::default(), Instant::now())
}

fn hello() -> Hello {
  Hello {
    versions: vec![1],
    codecs: vec!["cbor".into()],
    caps: None,
    limits: None,
    contract: None,
    auth: None,
    resume: None,
    client: None,
  }
}

const ANONYMOUS: [Output; 1] = [Output::Authenticate { auth: None }];

fn greeted() -> Session {
  let mut session = server();
  session.feed(Input::Message(message(&[(0, Frame::Hello(hello()))])));
  assert_eq!(drain(&mut session), ANONYMOUS);
  session
}

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
    let mut session = server();
    session.feed(Input::Command(Command::Close {
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
    let mut session = greeted();
    session.feed(Input::Message(message(&[(0, close(u64::from(u16::from(code)), None))])));
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
    let mut session = greeted();
    session.feed(Input::Message(message(&[(0, close(unknown, Some("later draft")))])));
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
  let mut session = server();
  session.feed(Input::TransportLost);
  assert_eq!(drain(&mut session), [Output::Ended(Ending::TransportLost)]);
  assert!(session.is_ended());
}

#[test]
fn ends_once_when_close_and_transport_loss_arrive_together() {
  let mut session = greeted();
  session.feed(Input::Message(message(&[(0, close(0, None))])));
  session.feed(Input::TransportLost);
  let endings = drain(&mut session)
    .into_iter()
    .filter(|output| matches!(output, Output::Ended(_)))
    .count();
  assert_eq!(endings, 1);
}

#[test]
fn sends_nothing_after_close() {
  let mut session = server();
  session.feed(Input::Command(Command::Close {
    code: CloseCode::Normal,
    msg: None,
  }));
  drain(&mut session);
  session.feed(Input::Command(Command::Close {
    code: CloseCode::Internal,
    msg: None,
  }));
  session.feed(Input::Message(message(&[(0, close(0, None))])));
  session.feed(Input::TransportLost);
  assert_eq!(drain(&mut session), []);
}

#[test]
fn stops_at_close_inside_a_message() {
  let ping = Frame::Ping {
    data: Heartbeat::try_from(Bytes::new()).unwrap(),
  };
  let mut session = greeted();
  session.feed(Input::Message(message(&[
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
    let mut session = server();
    session.feed(Input::Message(Bytes::copy_from_slice(bad)));
    let outputs = drain(&mut session);
    let Some(Output::Transmit(sent)) = outputs.first() else {
      panic!("{bad:02x?}: {outputs:?}");
    };
    let [(0, Frame::Close(Close { code: 1, msg: None }))] = frames(sent).as_slice() else {
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
  let mut session = greeted();
  session.feed(Input::Message(message(&[(2, close(0, None))])));
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
fn ignores_frames_pipelined_after_hello() {
  let ping = Frame::Ping {
    data: Heartbeat::try_from(Bytes::new()).unwrap(),
  };
  let mut session = greeted();
  session.feed(Input::Message(message(&[(0, ping)])));
  assert_eq!(drain(&mut session), []);
  assert!(!session.is_ended());
}

#[test]
fn names_the_peer() {
  assert_eq!(Side::Client.peer(), Side::Server);
  assert_eq!(Side::Server.peer(), Side::Client);
}

#[test]
fn closes_with_protocol_error_on_a_text_message() {
  let mut session = server();
  session.feed(Input::TextMessage);
  let outputs = drain(&mut session);
  let [
    Output::Transmit(sent),
    Output::CloseTransport { code, .. },
    Output::Ended(ending),
  ] = outputs.as_slice()
  else {
    panic!("{outputs:?}");
  };
  let [(0, Frame::Close(Close { code: 1, msg: None }))] = frames(sent).as_slice() else {
    panic!("{outputs:?}");
  };
  assert_eq!(*code, CloseCode::ProtocolError);
  assert!(matches!(
    ending,
    Ending::Closed {
      code: CloseCode::ProtocolError,
      by: Side::Server,
      ..
    }
  ));
}

fn ended_with(session: &mut Session, code: CloseCode) -> bool {
  let outputs = drain(session);
  let sent_close = outputs.iter().any(|output| {
    matches!(output, Output::Transmit(sent) if matches!(frames(sent).as_slice(), [(0, Frame::Close(close))] if close.code == u64::from(u16::from(code))))
  });
  sent_close
    && matches!(
      outputs.last(),
      Some(Output::Ended(Ending::Closed { code: ended, by: Side::Server, .. })) if *ended == code
    )
}

fn raw_hello(header: &[u8]) -> Bytes {
  let mut out = vec![0x00, 0x01, 0x00];
  let mut payload = Vec::new();
  varint::encode(header.len() as u64, &mut payload).unwrap();
  payload.extend_from_slice(header);
  varint::encode(payload.len() as u64, &mut out).unwrap();
  out.extend(payload);
  Bytes::from(out)
}

const CBOR: [u8; 5] = [0x64, b'c', b'b', b'o', b'r'];

fn hello_header(versions: &[u8], codecs: &[u8], limits: &[u8]) -> Vec<u8> {
  let mut entries = Vec::new();
  if !versions.is_empty() {
    entries.extend_from_slice(&[0x0e]);
    entries.extend_from_slice(versions);
  }
  if !codecs.is_empty() {
    entries.extend_from_slice(&[0x10]);
    entries.extend_from_slice(codecs);
  }
  if !limits.is_empty() {
    entries.extend_from_slice(&[0x15]);
    entries.extend_from_slice(limits);
  }
  let count = [versions, codecs, limits].iter().filter(|part| !part.is_empty()).count();
  let mut header = vec![0xa0 + count as u8];
  header.extend(entries);
  header
}

#[test]
fn times_out_with_no_bytes_from_accept() {
  let accepted = Instant::now();
  let config = Config {
    handshake_timeout: Duration::from_millis(200),
    ..Config::default()
  };
  let mut session = Session::server(&config.validate().unwrap(), accepted);
  let deadline = accepted + Duration::from_millis(200);
  assert_eq!(session.poll_timeout(), Some(deadline));
  session.handle_input(Input::Timeout, deadline - Duration::from_millis(1));
  assert_eq!(drain(&mut session), []);
  session.handle_input(Input::Timeout, deadline);
  assert!(ended_with(&mut session, CloseCode::HandshakeTimeout));
  assert_eq!(session.poll_timeout(), None);
}

#[test]
fn keeps_the_timer_running_after_hello() {
  let accepted = Instant::now();
  let mut session = Session::server(&ValidConfig::default(), accepted);
  session.handle_input(
    Input::Message(message(&[(0, Frame::Hello(hello()))])),
    accepted + Duration::from_millis(9_900),
  );
  assert_eq!(drain(&mut session), ANONYMOUS);
  assert_eq!(session.poll_timeout(), Some(accepted + Duration::from_secs(10)));
  session.handle_input(Input::Timeout, accepted + Duration::from_secs(10));
  assert!(ended_with(&mut session, CloseCode::HandshakeTimeout));
}

#[test]
fn times_out_a_hello_that_arrives_after_the_deadline() {
  let accepted = Instant::now();
  let mut session = Session::server(&ValidConfig::default(), accepted);
  session.handle_input(
    Input::Message(message(&[(0, Frame::Hello(hello()))])),
    accepted + Duration::from_millis(10_001),
  );
  assert!(ended_with(&mut session, CloseCode::HandshakeTimeout));
}

#[test]
fn reports_transport_loss_after_the_deadline_as_transport_loss() {
  let accepted = Instant::now();
  let mut session = Session::server(&ValidConfig::default(), accepted);
  session.handle_input(Input::TransportLost, accepted + Duration::from_secs(11));
  assert_eq!(drain(&mut session), [Output::Ended(Ending::TransportLost)]);
}

#[test]
fn refuses_a_first_frame_that_is_not_hello() {
  let ping = Frame::Ping {
    data: Heartbeat::try_from(Bytes::new()).unwrap(),
  };
  for first in [
    message(&[(0, ping)]),
    message(&[(0, close(0, None))]),
    message(&[(2, Frame::Hello(hello()))]),
  ] {
    let mut session = server();
    session.feed(Input::Message(first.clone()));
    assert!(ended_with(&mut session, CloseCode::ProtocolError), "{first:02x?}");
  }
}

#[test]
fn refuses_a_second_hello() {
  let mut session = greeted();
  session.feed(Input::Message(message(&[(0, Frame::Hello(hello()))])));
  assert!(ended_with(&mut session, CloseCode::ProtocolError));
}

#[test]
fn checks_the_required_hello_fields_and_the_limit_floor() {
  let one = [0x81, 0x01];
  let cbor = [&[0x81][..], &CBOR].concat();
  let max_frame_32768 = [0xa1, 0x18, 0x31, 0x19, 0x80, 0x00];
  let window_65535 = [0xa1, 0x18, 0x28, 0x19, 0xff, 0xff];
  let invalid = [
    hello_header(&[], &cbor, &[]),
    hello_header(&one, &[], &[]),
    hello_header(&[0x80], &cbor, &[]),
    hello_header(&one, &[0x80], &[]),
    hello_header(&one, &cbor, &max_frame_32768),
    hello_header(&one, &cbor, &window_65535),
  ];
  for header in invalid {
    let mut session = server();
    session.feed(Input::Message(raw_hello(&header)));
    assert!(ended_with(&mut session, CloseCode::ProtocolError), "{header:02x?}");
  }
  let at_floor = [
    0xa2, 0x18, 0x28, 0x1a, 0x00, 0x01, 0x00, 0x00, 0x18, 0x31, 0x1a, 0x00, 0x01, 0x00, 0x00,
  ];
  let mut session = server();
  session.feed(Input::Message(raw_hello(&hello_header(&one, &cbor, &at_floor))));
  assert_eq!(drain(&mut session), ANONYMOUS);
  let limits = session.hello().and_then(|hello| hello.limits.clone());
  assert_eq!(
    limits,
    Some(Limits {
      max_frame: Some(65_536),
      window: Some(65_536),
      ..Limits::default()
    })
  );
}

fn hello_with_caps(caps: Option<&[&str]>) -> Session {
  let mut session = server();
  session.feed(Input::Message(message(&[(
    0,
    Frame::Hello(Hello {
      caps: caps.map(|names| names.iter().map(|&name| name.to_owned()).collect()),
      ..hello()
    }),
  )])));
  assert_eq!(drain(&mut session), ANONYMOUS);
  session
}

#[test]
fn grants_calls_and_only_implemented_requested_capabilities() {
  let requests: [Option<&[&str]>; 5] = [
    None,
    Some(&[]),
    Some(&["calls", "resume"]),
    Some(&["calls", "teleport"]),
    Some(&["streams", "channels"]),
  ];
  for caps in requests {
    let session = hello_with_caps(caps);
    let granted = &session.negotiated().unwrap().caps;
    assert_eq!(granted.names().collect::<Vec<_>>(), ["calls"], "{caps:?}");
  }
}

#[test]
fn rejects_a_resume_while_resume_is_not_granted() {
  let resume = Resume {
    token: crate::frame::Token::Bytes(Bytes::from_static(b"\x00")),
    session: "s_x".into(),
    ack: [(0, 0)].into(),
    lost: None,
  };
  let mut session = server();
  session.feed(Input::Message(message(&[(
    0,
    Frame::Hello(Hello {
      versions: vec![9],
      caps: Some(vec!["resume".into()]),
      resume: Some(resume),
      ..hello()
    }),
  )])));
  let outputs = drain(&mut session);
  let [Output::Transmit(sent), close_transport, ended] = outputs.as_slice() else {
    panic!("{outputs:?}");
  };
  assert!(
    matches!(frames(sent).as_slice(), [(0, Frame::Reject(Reject { code: 11, .. }))]),
    "{sent:02x?}"
  );
  assert!(matches!(
    close_transport,
    Output::CloseTransport {
      code: CloseCode::ResumeFailed,
      ..
    }
  ));
  assert!(matches!(
    ended,
    Output::Ended(Ending::Rejected {
      code: CloseCode::ResumeFailed,
      ..
    })
  ));
}

#[test]
fn honors_a_close_command_or_text_message_after_the_deadline() {
  let accepted = Instant::now();
  let late = accepted + Duration::from_secs(11);
  let mut session = Session::server(&ValidConfig::default(), accepted);
  session.handle_input(
    Input::Command(Command::Close {
      code: CloseCode::GoingAway,
      msg: Some("draining".into()),
    }),
    late,
  );
  assert_eq!(
    drain(&mut session).last(),
    Some(&Output::Ended(Ending::Closed {
      code: CloseCode::GoingAway,
      msg: Some("draining".into()),
      by: Side::Server
    }))
  );
  let mut session = Session::server(&ValidConfig::default(), accepted);
  session.handle_input(Input::TextMessage, late);
  assert!(ended_with(&mut session, CloseCode::ProtocolError));
}

fn negotiating(versions: Vec<u64>, codecs: &[&str]) -> Session {
  let config = Config {
    versions: vec![1, 2],
    codecs: vec![Codec::Cbor, Codec::Json],
    ..Config::default()
  };
  let mut session = Session::server(&config.validate().unwrap(), Instant::now());
  session.feed(Input::Message(message(&[(
    0,
    Frame::Hello(Hello {
      versions,
      codecs: codecs.iter().map(|&name| name.to_owned()).collect(),
      ..hello()
    }),
  )])));
  session
}

#[test]
fn negotiates_the_highest_version_and_the_client_codec_preference() {
  let mut session = negotiating(vec![1, 2, 3], &["msgpack", "json", "cbor"]);
  assert_eq!(drain(&mut session), ANONYMOUS);
  let negotiated = session.negotiated().unwrap();
  assert_eq!((negotiated.version, negotiated.codec), (2, Codec::Json));
}

#[test]
fn rejects_a_hello_with_no_common_version_or_codec() {
  for (versions, codecs) in [(vec![3], &["cbor"][..]), (vec![1], &["msgpack"][..])] {
    let mut session = negotiating(versions.clone(), codecs);
    let msg = "no common protocol version or codec";
    let outputs = drain(&mut session);
    let [Output::Transmit(sent), close_transport, ended] = outputs.as_slice() else {
      panic!("{versions:?} {codecs:?}: {outputs:?}");
    };
    assert_eq!(
      frames(sent),
      [(
        0,
        Frame::Reject(Reject {
          code: 2,
          msg: msg.into(),
          retry: None
        })
      )]
    );
    assert_eq!(
      *close_transport,
      Output::CloseTransport {
        code: CloseCode::UnsupportedVersion,
        reason: msg.into()
      }
    );
    assert_eq!(
      *ended,
      Output::Ended(Ending::Rejected {
        code: CloseCode::UnsupportedVersion,
        msg: msg.into()
      })
    );
    assert_eq!(session.negotiated(), None);
    assert_eq!(session.poll_timeout(), None);
    session.feed(Input::Message(message(&[(0, Frame::Hello(hello()))])));
    assert_eq!(drain(&mut session), []);
  }
}

#[test]
fn offers_only_cbor_by_default() {
  let mut session = server();
  session.feed(Input::Message(message(&[(
    0,
    Frame::Hello(Hello {
      codecs: vec!["json".into()],
      ..hello()
    }),
  )])));
  assert!(matches!(
    drain(&mut session).last(),
    Some(Output::Ended(Ending::Rejected {
      code: CloseCode::UnsupportedVersion,
      ..
    }))
  ));
  let mut session = greeted();
  let negotiated = session.negotiated().unwrap();
  assert_eq!((negotiated.version, negotiated.codec), (1, Codec::Cbor));
  assert_eq!(negotiated.caps.names().collect::<Vec<_>>(), ["calls"]);
  assert_eq!(drain(&mut session), []);
}

fn bearer(token: &str) -> Auth {
  Auth {
    scheme: "bearer".into(),
    token: Token::Text(token.into()),
  }
}

fn session_id() -> SessionId {
  SessionId::parse("s_test").unwrap()
}

fn authenticated(user: Option<&str>) -> Input {
  Input::Authenticated {
    session: session_id(),
    user: user.map(str::to_owned),
  }
}

fn welcomed(user: Option<&str>) -> (Session, Welcome) {
  let mut session = greeted();
  session.feed(authenticated(user));
  let outputs = drain(&mut session);
  let [Output::Transmit(sent)] = outputs.as_slice() else {
    panic!("{outputs:?}");
  };
  let decoded = frames(sent);
  let [(0, Frame::Welcome(welcome))] = decoded.as_slice() else {
    panic!("{sent:02x?}");
  };
  let welcome = welcome.clone();
  (session, welcome)
}

#[test]
fn hands_the_hello_credential_to_the_driver_and_drops_it() {
  let mut session = server();
  session.feed(Input::Message(message(&[(
    0,
    Frame::Hello(Hello {
      auth: Some(bearer("s3cret")),
      ..hello()
    }),
  )])));
  assert_eq!(
    drain(&mut session),
    [Output::Authenticate {
      auth: Some(bearer("s3cret"))
    }]
  );
  assert_eq!(session.hello().map(|hello| hello.auth.clone()), Some(None));
}

#[test]
fn rejects_with_the_code_the_authenticator_chose() {
  for (failure, code, msg) in [
    (AuthFailure::Unauthenticated, CloseCode::Unauthenticated, "unauthenticated"),
    (AuthFailure::Forbidden, CloseCode::Forbidden, "forbidden"),
  ] {
    let mut session = greeted();
    session.feed(Input::AuthFailed(failure));
    let outputs = drain(&mut session);
    let [Output::Transmit(sent), close_transport, ended] = outputs.as_slice() else {
      panic!("{failure:?}: {outputs:?}");
    };
    assert_eq!(
      frames(sent),
      [(
        0,
        Frame::Reject(Reject {
          code: u64::from(u16::from(code)),
          msg: msg.into(),
          retry: None
        })
      )]
    );
    assert_eq!(
      *close_transport,
      Output::CloseTransport {
        code,
        reason: msg.into()
      }
    );
    assert_eq!(*ended, Output::Ended(Ending::Rejected { code, msg: msg.into() }));
  }
}

#[test]
fn sends_welcome_with_every_required_field_after_authentication() {
  let (session, welcome) = welcomed(None);
  assert_eq!(
    welcome,
    Welcome {
      version: 1,
      codec: "cbor".into(),
      caps: vec!["calls".into()],
      session: "s_test".into(),
      token: None,
      limits: ServerLimits {
        max_calls: 100,
        max_frame: 1_048_576,
        max_subs: 256,
        max_dgram: 1_100,
        window: Some(262_144),
      },
      hb: 15_000,
      idle: 45_000,
      resume_window: None,
      contract: None,
      user: None,
      resumed: None,
      ack: None,
      lost: None,
      transports: None,
    }
  );
  assert!(!session.is_ended());
  assert_eq!(session.poll_timeout(), None);
}

#[test]
fn echoes_the_user_id_only_when_the_principal_has_one() {
  assert_eq!(welcomed(Some("u1")).1.user.as_deref(), Some("u1"));
  assert_eq!(welcomed(None).1.user, None);
}

#[test]
fn announces_the_configured_values() {
  let config = Config {
    heartbeat: Duration::from_secs(5),
    idle_timeout: Duration::from_millis(12_500),
    max_calls: 7,
    max_frame: 65_536,
    window: 65_536,
    ..Config::default()
  };
  let mut session = Session::server(&config.validate().unwrap(), Instant::now());
  session.feed(Input::Message(message(&[(0, Frame::Hello(hello()))])));
  session.feed(authenticated(None));
  let outputs = drain(&mut session);
  let Some(Output::Transmit(sent)) = outputs.last() else {
    panic!("{outputs:?}");
  };
  let decoded = frames(sent);
  let [(0, Frame::Welcome(welcome))] = decoded.as_slice() else {
    panic!("{sent:02x?}");
  };
  assert_eq!((welcome.hb, welcome.idle), (5_000, 12_500));
  assert_eq!(
    (welcome.limits.max_calls, welcome.limits.max_frame, welcome.limits.window),
    (7, 65_536, Some(65_536))
  );
}

#[test]
fn ignores_an_answer_without_a_pending_authentication() {
  let mut session = server();
  session.feed(Input::AuthFailed(AuthFailure::Forbidden));
  session.feed(authenticated(None));
  assert_eq!(drain(&mut session), []);
  assert!(matches!(session.state, State::AwaitHello { .. }));
  let (mut session, _) = welcomed(Some("u-1"));
  session.feed(Input::AuthFailed(AuthFailure::Unauthenticated));
  session.feed(authenticated(Some("u-2")));
  assert_eq!(drain(&mut session), []);
  assert!(matches!(session.state, State::Established { .. }));
}

#[test]
fn times_out_an_authenticator_that_answers_after_the_deadline() {
  for answer in [authenticated(None), Input::AuthFailed(AuthFailure::Forbidden)] {
    let accepted = Instant::now();
    let mut session = Session::server(&ValidConfig::default(), accepted);
    session.handle_input(Input::Message(message(&[(0, Frame::Hello(hello()))])), accepted);
    assert_eq!(drain(&mut session), ANONYMOUS);
    session.handle_input(answer.clone(), accepted + Duration::from_secs(10));
    assert!(ended_with(&mut session, CloseCode::HandshakeTimeout), "{answer:?}");
  }
}

#[test]
fn stops_the_handshake_timer_at_welcome() {
  let (mut session, _) = welcomed(None);
  session.handle_input(Input::Timeout, Instant::now() + Duration::from_secs(60));
  session.handle_input(
    Input::Message(message(&[(2, Frame::Cancel(Cancel { msg: None }))])),
    Instant::now() + Duration::from_secs(60),
  );
  assert_eq!(drain(&mut session), []);
  assert!(!session.is_ended());
}

#[test]
fn gates_frames_after_welcome_on_the_granted_capabilities() {
  let (mut session, _) = welcomed(None);
  session.feed(Input::Message(message(&[(0, Frame::Ack { ack: AckMap::new() })])));
  assert!(ended_with(&mut session, CloseCode::ProtocolError));
}

#[test]
fn refuses_hello_and_closes_on_close_after_welcome() {
  let (mut session, _) = welcomed(None);
  session.feed(Input::Message(message(&[(0, Frame::Hello(hello()))])));
  assert!(ended_with(&mut session, CloseCode::ProtocolError));
  let (mut session, _) = welcomed(None);
  session.feed(Input::Message(message(&[(0, close(0, None))])));
  assert_eq!(
    drain(&mut session).last(),
    Some(&Output::Ended(Ending::Closed {
      code: CloseCode::Normal,
      msg: None,
      by: Side::Client
    }))
  );
}

type Change = fn(&mut Config);

#[test]
fn validates_the_config() {
  let floor = Limits::FLOOR;
  let cases: [(Change, Option<ConfigError>); 12] = [
    (|_| {}, None),
    (|config| config.max_frame = Limits::FLOOR, None),
    (|config| config.window = Limits::FLOOR, None),
    (|config| (config.window, config.max_frame) = (Limits::FLOOR, 1 << 20), None),
    (
      |config| config.max_frame = Limits::FLOOR - 1,
      Some(ConfigError::BelowFloor("max_frame")),
    ),
    (
      |config| config.window = Limits::FLOOR - 1,
      Some(ConfigError::BelowFloor("window")),
    ),
    (
      |config| config.heartbeat = Duration::ZERO,
      Some(ConfigError::Zero("heartbeat")),
    ),
    (
      |config| config.idle_timeout = Duration::from_micros(999),
      Some(ConfigError::Zero("idle_timeout")),
    ),
    (
      |config| config.handshake_timeout = Duration::ZERO,
      Some(ConfigError::Zero("handshake_timeout")),
    ),
    (|config| config.max_calls = 0, Some(ConfigError::Zero("max_calls"))),
    (|config| config.versions.clear(), Some(ConfigError::Empty("versions"))),
    (|config| config.codecs.clear(), Some(ConfigError::Empty("codecs"))),
  ];
  for (index, (change, expected)) in cases.into_iter().enumerate() {
    let mut config = Config::default();
    change(&mut config);
    assert_eq!(config.validate().err(), expected, "case {index}, floor {floor}");
  }
  let mut config = Config::default();
  (config.max_subs, config.max_dgram) = (0, 0);
  assert_eq!(config.validate().err(), Some(ConfigError::Zero("max_subs")));
}

#[test]
fn sends_the_default_welcome_vector() {
  let doc: serde_json::Value = serde_json::from_str(include_str!("../../../../vectors/frames/handshake.json")).unwrap();
  let case = doc["cases"]
    .as_array()
    .unwrap()
    .iter()
    .find(|case| case["id"] == "welcome-defaults-ws")
    .unwrap();
  let expected: Vec<u8> = case["hex"]
    .as_str()
    .unwrap()
    .split_whitespace()
    .map(|byte| u8::from_str_radix(byte, 16).unwrap())
    .collect();
  let mut session = greeted();
  session.feed(authenticated(None));
  assert_eq!(drain(&mut session), [Output::Transmit(Bytes::from(expected))]);
}
