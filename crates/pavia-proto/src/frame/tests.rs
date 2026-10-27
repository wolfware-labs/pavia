use proptest::prelude::*;

use super::*;
use crate::error::ProtocolError;
use crate::wire::frame::Frame as RawFrame;
use crate::wire::frame::FrameDecoder;
use crate::wire::header::{self, DecodeLimits, Value};

fn raw(frame_type: u8, entries: &[(Value<'_>, Value<'_>)], data: &[u8]) -> RawFrame {
  let mut header = Vec::new();
  if !entries.is_empty() {
    header::encode(entries, &mut header).unwrap();
  }
  RawFrame {
    frame_type,
    flags: 0,
    header: Bytes::from(header),
    data: Bytes::copy_from_slice(data),
  }
}

fn typed(raw: &RawFrame) -> Result<Frame, FrameError> {
  Frame::from_raw(raw, &DecodeLimits::default())
}

fn field_error(raw: &RawFrame) -> FieldError {
  match typed(raw) {
    Err(FrameError::Field(error)) => error,
    other => panic!("expected a field error, got {other:?}"),
  }
}

fn hex_bytes(hex: &str) -> Vec<u8> {
  let digits: Vec<u8> = hex.bytes().filter(|byte| !byte.is_ascii_whitespace()).collect();
  digits
    .chunks(2)
    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
    .collect()
}

fn decode_wire(bytes: &[u8]) -> Frame {
  let mut decoder = FrameDecoder::new(65_536);
  decoder.feed(bytes).unwrap();
  typed(&decoder.next_frame().unwrap().unwrap()).unwrap()
}

fn u(n: u64) -> Value<'static> {
  Value::Unsigned(n)
}

#[test]
fn decodes_the_minimal_hello_vector() {
  let bytes = hex_bytes("01 00 14 13 a3 0e 81 01 10 81 64 63 62 6f 72 12 81 65 63 61 6c 6c 73");
  let Frame::Hello(hello) = decode_wire(&bytes) else {
    panic!("not a HELLO");
  };
  assert_eq!(hello.versions, [1]);
  assert_eq!(hello.codecs, ["cbor"]);
  assert_eq!(hello.caps.as_deref(), Some(&["calls".to_owned()][..]));
  assert_eq!(hello.limits, None);
}

#[test]
fn writes_the_minimal_hello_vector_back() {
  let bytes = hex_bytes("01 00 14 13 a3 0e 81 01 10 81 64 63 62 6f 72 12 81 65 63 61 6c 6c 73");
  let mut out = Vec::new();
  decode_wire(&bytes).encode(&mut out).unwrap();
  assert_eq!(out, bytes);
}

#[test]
fn decodes_the_unary_call_vector() {
  let bytes =
    hex_bytes("10 00 22 12 a3 02 69 63 68 61 74 2f 73 65 6e 64 04 19 13 88 05 00 82 67 67 65 6e 65 72 61 6c 65 68 65 6c 6c 6f");
  let Frame::Call(call) = decode_wire(&bytes) else {
    panic!("not a CALL");
  };
  assert_eq!(call.method, "chat/send");
  assert_eq!(call.shape, 0);
  assert_eq!(call.deadline, Some(5000));
  assert_eq!(call.data.len(), 15);
}

#[test]
fn names_frame_and_key_for_a_hello_without_versions() {
  let error = field_error(&raw(0x01, &[(u(16), Value::Array(vec![Value::Text("cbor")]))], &[]));
  assert_eq!(error.frame_type, FrameType::Hello);
  assert_eq!(error.key, key::VERSIONS);
  assert_eq!(error.problem, FieldProblem::Missing);
  assert!(matches!(
    ProtocolError::from(FrameError::Field(error)),
    ProtocolError::InvalidHeaderField
  ));
}

#[test]
fn names_frame_and_key_for_a_call_without_method() {
  let error = field_error(&raw(0x10, &[(u(5), u(0))], &[]));
  assert_eq!((error.frame_type, error.key), (FrameType::Call, key::METHOD));
}

#[test]
fn reports_versions_given_as_text_as_the_wrong_type() {
  let entries = [(u(14), Value::Text("1")), (u(16), Value::Array(vec![Value::Text("cbor")]))];
  let error = field_error(&raw(0x01, &entries, &[]));
  assert_eq!((error.key, error.problem), (key::VERSIONS, FieldProblem::WrongType));
}

#[test]
fn keeps_the_rid_of_a_subscribe_without_channel() {
  let error = field_error(&raw(0x30, &[(u(1), u(7))], &[]));
  assert_eq!((error.key, error.rid), (key::CHANNEL, Some(7)));
  assert!(matches!(
    ProtocolError::from(FrameError::Field(error)),
    ProtocolError::InvalidControlRequestField { rid: 7 }
  ));
}

#[test]
fn keeps_the_rid_of_an_auth_refresh_with_auth_not_a_map() {
  let error = field_error(&raw(0x08, &[(u(1), u(3)), (u(33), Value::Text("x"))], &[]));
  assert_eq!(
    (error.key, error.problem, error.rid),
    (key::AUTH, FieldProblem::WrongType, Some(3))
  );
  assert!(matches!(
    ProtocolError::from(FrameError::Field(error)),
    ProtocolError::InvalidControlRequestField { rid: 3 }
  ));
}

#[test]
fn ends_the_session_for_a_bad_reply_even_with_a_rid() {
  let error = field_error(&raw(0x31, &[(u(1), u(7))], &[]));
  assert!(matches!(
    ProtocolError::from(FrameError::Field(error)),
    ProtocolError::InvalidHeaderField
  ));
}

#[test]
fn decodes_the_resume_map_with_the_session_in_key_19() {
  let resume = Value::Map(vec![
    (u(13), Value::Bytes(&[0x01])),
    (u(19), Value::Text("s1")),
    (u(44), Value::Map(vec![(u(0), u(4))])),
  ]);
  let entries = [
    (u(14), Value::Array(vec![u(1)])),
    (u(16), Value::Array(vec![Value::Text("cbor")])),
    (u(20), resume),
  ];
  let Ok(Frame::Hello(hello)) = typed(&raw(0x01, &entries, &[])) else {
    panic!("not a HELLO");
  };
  let resume = hello.resume.unwrap();
  assert_eq!(resume.session, "s1");
  assert_eq!(resume.token, Token::Bytes(Bytes::from_static(&[0x01])));
  assert_eq!(resume.ack, AckMap::from([(0, 4)]));
  assert_eq!(resume.lost, None);
}

#[test]
fn reports_a_bad_nested_entry_as_its_outer_key() {
  let resume = Value::Map(vec![(u(13), Value::Bytes(&[0x01])), (u(44), Value::Map(vec![]))]);
  let entries = [
    (u(14), Value::Array(vec![u(1)])),
    (u(16), Value::Array(vec![Value::Text("cbor")])),
    (u(20), resume),
  ];
  let error = field_error(&raw(0x01, &entries, &[]));
  assert_eq!((error.key, error.problem), (key::RESUME, FieldProblem::WrongType));
}

#[test]
fn rejects_unassigned_must_understand_types() {
  for frame_type in [0x00, 0x0D, 0x7E, 0x7F] {
    assert_eq!(typed(&raw(frame_type, &[], &[])), Err(FrameError::UnknownType(frame_type)));
  }
  assert!(matches!(
    ProtocolError::from(FrameError::UnknownType(0x7E)),
    ProtocolError::FrameTypeNotAllowed
  ));
}

#[test]
fn keeps_extension_frames_intact_so_they_can_be_dropped() {
  for frame_type in [0x80, 0x8E, 0xFF] {
    let raw = RawFrame {
      frame_type,
      flags: 0,
      header: Bytes::from_static(&[0xff, 0x00]),
      data: Bytes::from_static(b"abc"),
    };
    let Ok(Frame::Extension(extension)) = typed(&raw) else {
      panic!("{frame_type:#04x} is not an extension");
    };
    assert_eq!(extension.frame_type, frame_type);
    assert_eq!(extension.header, raw.header);
    assert_eq!(extension.data, raw.data);
  }
}

#[test]
fn ignores_unknown_header_keys() {
  let entries = [
    (u(2), Value::Text("m")),
    (u(5), u(0)),
    (u(1024), u(1)),
    (u(999), Value::Text("x")),
  ];
  assert!(matches!(typed(&raw(0x10, &entries, &[])), Ok(Frame::Call(_))));
}

#[test]
fn reports_a_malformed_header() {
  let raw = RawFrame {
    frame_type: 0x10,
    flags: 0,
    header: Bytes::from_static(&[0x01]),
    data: Bytes::new(),
  };
  assert_eq!(typed(&raw), Err(FrameError::MalformedHeader));
}

fn welcome_with_limits(limits: Vec<(Value<'static>, Value<'static>)>) -> RawFrame {
  let entries = [
    (u(key::VERSION), u(1)),
    (u(key::CODEC), Value::Text("cbor")),
    (u(key::CAPS), Value::Array(vec![])),
    (u(key::SESSION), Value::Text("s")),
    (u(key::LIMITS), Value::Map(limits)),
    (u(key::HB), u(15_000)),
    (u(key::IDLE), u(45_000)),
  ];
  raw(0x02, &entries, &[])
}

#[test]
fn accepts_welcome_limits_without_the_websocket_window() {
  let limits = vec![
    (u(key::MAX_CALLS), u(100)),
    (u(key::MAX_FRAME), u(65_536)),
    (u(key::MAX_SUBS), u(10)),
    (u(key::MAX_DGRAM), u(1_200)),
  ];
  let Ok(Frame::Welcome(welcome)) = typed(&welcome_with_limits(limits)) else {
    panic!("expected WELCOME");
  };
  assert_eq!(welcome.limits.window, None);
}

#[test]
fn rejects_welcome_limits_missing_a_required_entry() {
  let required = [key::MAX_CALLS, key::MAX_FRAME, key::MAX_SUBS, key::MAX_DGRAM];
  for missing in required {
    let limits = required
      .iter()
      .filter(|&&entry| entry != missing)
      .map(|&entry| (u(entry), u(65_536)))
      .collect();
    let error = field_error(&welcome_with_limits(limits));
    assert_eq!(
      (error.key, error.problem),
      (key::LIMITS, FieldProblem::WrongType),
      "{missing}"
    );
  }
}

#[test]
fn rejects_a_text_key_in_the_header_map() {
  let entries = [(Value::Text("bad"), u(1)), (u(2), Value::Text("m")), (u(5), u(0))];
  assert_eq!(typed(&raw(0x10, &entries, &[])), Err(FrameError::MalformedHeader));
}

#[test]
fn reports_a_text_key_in_a_nested_integer_map_as_its_outer_key() {
  let limits = vec![
    (u(key::MAX_CALLS), u(100)),
    (u(key::MAX_FRAME), u(65_536)),
    (u(key::MAX_SUBS), u(10)),
    (u(key::MAX_DGRAM), u(1_200)),
    (Value::Text("window"), u(65_536)),
  ];
  let error = field_error(&welcome_with_limits(limits));
  assert_eq!((error.key, error.problem), (key::LIMITS, FieldProblem::WrongType));
}

#[test]
fn ends_the_session_for_data_on_a_frame_type_without_any() {
  for frame_type in FrameType::ALL.into_iter().filter(|frame_type| !frame_type.carries_data()) {
    let error = typed(&raw(frame_type.into(), &[], b"x")).unwrap_err();
    assert_eq!(error, FrameError::UnexpectedData, "{frame_type:?}");
    assert!(matches!(ProtocolError::from(error), ProtocolError::UnexpectedDataSection));
  }
}

#[test]
fn accepts_heartbeats_up_to_eight_bytes() {
  for frame_type in [0x04, 0x05] {
    let frame = typed(&raw(frame_type, &[], b"12345678")).unwrap();
    assert_eq!(frame.frame_type().map(u8::from), Some(frame_type));
  }
}

#[test]
fn ends_the_session_for_a_heartbeat_over_eight_bytes() {
  for frame_type in [0x04, 0x05] {
    let error = typed(&raw(frame_type, &[], b"123456789")).unwrap_err();
    assert_eq!(error, FrameError::HeartbeatTooLarge(HeartbeatTooLarge));
    assert!(matches!(ProtocolError::from(error), ProtocolError::HeartbeatTooLarge));
  }
}

fn call_with_meta_name(name: &str) -> RawFrame {
  let entries = [
    (u(2), Value::Text("m")),
    (u(3), Value::Map(vec![(Value::Text(name), Value::Text("v"))])),
    (u(5), u(0)),
  ];
  raw(0x10, &entries, &[])
}

#[test]
fn accepts_metadata_names_in_the_grammar() {
  for name in ["traceparent", "pavia-idempotency-key", "x-2"] {
    assert!(matches!(typed(&call_with_meta_name(name)), Ok(Frame::Call(_))), "{name}");
  }
}

#[test]
fn rejects_metadata_names_outside_the_grammar() {
  for name in ["", "Trace", "a_b", "a b", "é"] {
    let error = field_error(&call_with_meta_name(name));
    assert_eq!((error.key, error.problem), (key::META, FieldProblem::WrongType), "{name:?}");
    assert!(matches!(
      ProtocolError::from(FrameError::Field(error)),
      ProtocolError::InvalidHeaderField
    ));
  }
}

#[test]
fn accepts_both_forms_of_unsubscribed() {
  let reply = [(u(9), Value::Text("c")), (u(1), u(4))];
  let Ok(Frame::Unsubscribed(unsubscribed)) = typed(&raw(0x33, &reply, &[])) else {
    panic!("not an UNSUBSCRIBED");
  };
  assert_eq!(unsubscribed.form, UnsubscribedForm::Reply { rid: 4 });
  let notice = [(u(9), Value::Text("c")), (u(6), u(2)), (u(7), Value::Text("resync"))];
  let Ok(Frame::Unsubscribed(unsubscribed)) = typed(&raw(0x33, &notice, &[])) else {
    panic!("not an UNSUBSCRIBED");
  };
  assert_eq!(
    unsubscribed.form,
    UnsubscribedForm::Notice {
      code: 2,
      msg: "resync".to_owned()
    }
  );
}

#[test]
fn rejects_unsubscribed_that_mixes_or_lacks_its_forms() {
  let c = || (u(9), Value::Text("c"));
  let cases = [
    (vec![c()], key::CODE, FieldProblem::Missing),
    (vec![c(), (u(6), u(2))], key::MSG, FieldProblem::Missing),
    (
      vec![c(), (u(1), u(4)), (u(6), u(2)), (u(7), Value::Text("m"))],
      key::CODE,
      FieldProblem::Unexpected,
    ),
    (
      vec![c(), (u(1), u(4)), (u(7), Value::Text("m"))],
      key::MSG,
      FieldProblem::Unexpected,
    ),
  ];
  for (entries, key, problem) in cases {
    let error = field_error(&raw(0x33, &entries, &[]));
    assert_eq!((error.key, error.problem), (key, problem), "{entries:?}");
  }
}

#[test]
fn processes_a_frame_with_flags_set_as_if_they_were_zero() {
  for flags in [0x01, 0xFF] {
    let mut flagged = raw(0x04, &[], &[1, 2]);
    flagged.flags = flags;
    assert_eq!(typed(&flagged), typed(&raw(0x04, &[], &[1, 2])), "flags {flags:#04x}");
  }
}

#[test]
fn round_trips_every_registered_type_through_the_byte_value() {
  for frame_type in FrameType::ALL {
    assert_eq!(FrameType::try_from(u8::from(frame_type)), Ok(frame_type));
  }
}

fn small_text() -> impl Strategy<Value = String> {
  "[a-z/]{0,8}"
}

fn small_uint() -> impl Strategy<Value = u64> {
  prop_oneof![0u64..30, any::<u64>()]
}

fn any_bytes() -> impl Strategy<Value = Bytes> {
  prop::collection::vec(any::<u8>(), 0..12).prop_map(Bytes::from)
}

fn any_token() -> impl Strategy<Value = Token> {
  prop_oneof![any_bytes().prop_map(Token::Bytes), small_text().prop_map(Token::Text)]
}

fn any_heartbeat() -> impl Strategy<Value = Heartbeat> {
  prop::collection::vec(any::<u8>(), 0..=Heartbeat::MAX_LEN).prop_map(|data| Heartbeat(Bytes::from(data)))
}

fn any_meta() -> impl Strategy<Value = Option<Meta>> {
  prop::option::of(prop::collection::btree_map(
    "[a-z0-9-]{1,8}",
    prop_oneof![small_text().prop_map(MetaValue::Text), any_bytes().prop_map(MetaValue::Bytes)],
    0..3,
  ))
}

fn any_ack() -> impl Strategy<Value = AckMap> {
  prop::collection::btree_map(0u64..17, small_uint(), 0..3)
}

fn any_sequencing() -> impl Strategy<Value = Sequencing> {
  (prop::option::of(small_uint()), prop::option::of(any_ack())).prop_map(|(seq, ack)| Sequencing { seq, ack })
}

fn any_limits() -> impl Strategy<Value = Limits> {
  prop::array::uniform5(prop::option::of(small_uint())).prop_map(|[max_calls, max_frame, max_subs, max_dgram, window]| Limits {
    max_calls,
    max_frame,
    max_subs,
    max_dgram,
    window,
  })
}

fn any_server_limits() -> impl Strategy<Value = ServerLimits> {
  (prop::array::uniform4(small_uint()), prop::option::of(small_uint())).prop_map(
    |([max_calls, max_frame, max_subs, max_dgram], window)| ServerLimits {
      max_calls,
      max_frame,
      max_subs,
      max_dgram,
      window,
    },
  )
}

fn any_auth() -> impl Strategy<Value = Auth> {
  (small_text(), any_token()).prop_map(|(scheme, token)| Auth { scheme, token })
}

fn texts() -> impl Strategy<Value = Vec<String>> {
  prop::collection::vec(small_text(), 0..3)
}

fn uints() -> impl Strategy<Value = Vec<u64>> {
  prop::collection::vec(small_uint(), 0..3)
}

fn any_hello() -> BoxedStrategy<Frame> {
  (
    uints(),
    texts(),
    prop::option::of(texts()),
    prop::option::of(any_limits()),
    prop::option::of(any::<[u8; 32]>()),
    prop::option::of(any_auth()),
    prop::option::of((any_token(), small_text(), any_ack(), prop::option::of(uints()))),
    prop::option::of((prop::option::of(small_text()), prop::option::of(small_text()))),
  )
    .prop_map(|(versions, codecs, caps, limits, contract, auth, resume, client)| {
      Frame::Hello(Hello {
        versions,
        codecs,
        caps,
        limits,
        contract,
        auth,
        resume: resume.map(|(token, session, ack, lost)| Resume {
          token,
          session,
          ack,
          lost,
        }),
        client: client.map(|(name, version)| ClientInfo { name, version }),
      })
    })
    .boxed()
}

fn any_welcome() -> BoxedStrategy<Frame> {
  (
    (
      small_uint(),
      small_text(),
      texts(),
      small_text(),
      prop::option::of(any_token()),
      any_server_limits(),
    ),
    (
      small_uint(),
      small_uint(),
      prop::option::of(small_uint()),
      prop::option::of(any::<[u8; 32]>()),
    ),
    (
      prop::option::of(small_text()),
      prop::option::of(any::<bool>()),
      prop::option::of(any_ack()),
      prop::option::of(uints()),
      prop::option::of(texts()),
    ),
  )
    .prop_map(
      |(
        (version, codec, caps, session, token, limits),
        (hb, idle, resume_window, contract),
        (user, resumed, ack, lost, transports),
      )| {
        Frame::Welcome(Welcome {
          version,
          codec,
          caps,
          session,
          token,
          limits,
          hb,
          idle,
          resume_window,
          contract,
          user,
          resumed,
          ack,
          lost,
          transports,
        })
      },
    )
    .boxed()
}

fn any_control_frame() -> BoxedStrategy<Frame> {
  prop_oneof![
    (small_uint(), small_text(), prop::option::of(small_uint())).prop_map(|(code, msg, retry)| Frame::Reject(Reject {
      code,
      msg,
      retry
    })),
    any_heartbeat().prop_map(|data| Frame::Ping { data }),
    any_heartbeat().prop_map(|data| Frame::Pong { data }),
    (
      small_uint(),
      small_text(),
      prop::option::of(small_text()),
      prop::option::of(small_uint())
    )
      .prop_map(|(code, msg, url, retry)| Frame::Goaway(Goaway { code, msg, url, retry })),
    (small_uint(), prop::option::of(small_text())).prop_map(|(code, msg)| Frame::Close(Close { code, msg })),
    (small_uint(), any_auth(), any_sequencing()).prop_map(|(rid, auth, sequencing)| Frame::AuthRefresh(AuthRefresh {
      rid,
      auth,
      sequencing
    })),
    (small_uint(), small_uint(), prop::option::of(small_uint()), any_sequencing()).prop_map(
      |(rid, code, expires, sequencing)| Frame::AuthResult(AuthResult {
        rid,
        code,
        expires,
        sequencing
      })
    ),
    any_ack().prop_map(|ack| Frame::Ack { ack }),
    (small_uint(), any_sequencing())
      .prop_map(|(expires, sequencing)| Frame::AuthExpiring(AuthExpiring { expires, sequencing })),
    (small_uint(), small_uint(), small_text(), any_sequencing()).prop_map(|(rid, code, msg, sequencing)| {
      Frame::RequestError(RequestError {
        rid,
        code,
        msg,
        sequencing,
      })
    }),
  ]
  .boxed()
}

fn any_call_frame() -> BoxedStrategy<Frame> {
  prop_oneof![
    (
      small_text(),
      small_uint(),
      prop::option::of(small_uint()),
      any_meta(),
      any_bytes()
    )
      .prop_map(|(method, shape, deadline, meta, data)| Frame::Call(Call {
        method,
        shape,
        deadline,
        meta,
        data
      })),
    (any_meta(), any_bytes()).prop_map(|(meta, data)| Frame::Item(Item { meta, data })),
    any_meta().prop_map(|meta| Frame::End(End { meta })),
    (any_meta(), any_bytes()).prop_map(|(meta, data)| Frame::Result(CallResult { meta, data })),
    (
      small_uint(),
      small_text(),
      prop::option::of(any::<bool>()),
      any_meta(),
      any_bytes()
    )
      .prop_map(|(code, msg, nx, meta, data)| Frame::Error(CallError {
        code,
        msg,
        nx,
        meta,
        data
      })),
    prop::option::of(small_text()).prop_map(|msg| Frame::Cancel(Cancel { msg })),
    small_uint().prop_map(|credit| Frame::Credit(Credit { credit })),
  ]
  .boxed()
}

fn any_channel_frame() -> BoxedStrategy<Frame> {
  prop_oneof![
    (small_text(), any_meta(), any_sequencing(), any_bytes()).prop_map(|(method, meta, sequencing, data)| {
      Frame::Notify(Notify {
        method,
        meta,
        sequencing,
        data,
      })
    }),
    small_uint().prop_map(|lane| Frame::LaneOpen(LaneOpen { lane })),
    (
      small_uint(),
      small_text(),
      prop::option::of((small_text(), small_uint())),
      prop::option::of(any::<bool>()),
      any_meta(),
      any_sequencing(),
      any_bytes()
    )
      .prop_map(|(rid, channel, since, presence, meta, sequencing, data)| {
        Frame::Subscribe(Subscribe {
          rid,
          channel,
          since: since.map(|(epoch, offset)| Since { epoch, offset }),
          presence,
          meta,
          sequencing,
          data,
        })
      }),
    (
      small_uint(),
      small_text(),
      small_text(),
      small_uint(),
      prop::option::of(any::<bool>()),
      any_sequencing(),
      any_bytes()
    )
      .prop_map(|(rid, channel, epoch, offset, recovered, sequencing, data)| {
        Frame::Subscribed(Subscribed {
          rid,
          channel,
          epoch,
          offset,
          recovered,
          sequencing,
          data,
        })
      }),
    (small_uint(), small_text(), any_sequencing()).prop_map(|(rid, channel, sequencing)| Frame::Unsubscribe(Unsubscribe {
      rid,
      channel,
      sequencing
    })),
    (
      small_text(),
      prop_oneof![
        small_uint().prop_map(|rid| UnsubscribedForm::Reply { rid }),
        (small_uint(), small_text()).prop_map(|(code, msg)| UnsubscribedForm::Notice { code, msg })
      ],
      any_sequencing()
    )
      .prop_map(|(channel, form, sequencing)| Frame::Unsubscribed(Unsubscribed {
        channel,
        form,
        sequencing
      })),
    (small_text(), small_uint(), any_sequencing(), any_bytes()).prop_map(|(channel, offset, sequencing, data)| {
      Frame::Publication(Publication {
        channel,
        offset,
        sequencing,
        data,
      })
    }),
    (small_text(), small_uint(), any_sequencing(), any_bytes()).prop_map(|(channel, kind, sequencing, data)| {
      Frame::Presence(Presence {
        channel,
        kind,
        sequencing,
        data,
      })
    }),
    (small_uint(), small_uint(), any_bytes()).prop_map(|(key, seq, data)| Frame::Datagram(Datagram { key, seq, data })),
    (small_uint(), small_text(), any_sequencing()).prop_map(|(key, name, sequencing)| Frame::DgramBind(DgramBind {
      key,
      name,
      sequencing
    })),
  ]
  .boxed()
}

fn any_extension() -> BoxedStrategy<Frame> {
  (0x80u8..=0xFF, any_bytes(), any_bytes())
    .prop_map(|(frame_type, header, data)| {
      Frame::Extension(Extension {
        frame_type,
        header,
        data,
      })
    })
    .boxed()
}

fn any_frame() -> BoxedStrategy<Frame> {
  prop_oneof![
    any_extension(),
    any_hello(),
    any_welcome(),
    any_control_frame(),
    any_call_frame(),
    any_channel_frame()
  ]
  .boxed()
}

fn encoded(frame: &Frame) -> Vec<u8> {
  let mut bytes = Vec::new();
  frame.encode(&mut bytes).unwrap();
  bytes
}

fn raw_frame(bytes: &[u8]) -> RawFrame {
  let mut decoder = FrameDecoder::new(65_536);
  decoder.feed(bytes).unwrap();
  decoder.next_frame().unwrap().unwrap()
}

#[test]
fn sorts_metadata_names_by_length_then_bytes() {
  let meta: Meta = ["ab", "b", "a"]
    .into_iter()
    .map(|name| (name.to_owned(), MetaValue::Text(String::new())))
    .collect();
  let header = raw_frame(&encoded(&Frame::End(End { meta: Some(meta) }))).header;
  assert_eq!(
    header.as_ref(),
    hex_bytes("a1 03 a3 61 61 60 61 62 60 62 61 62 60").as_slice()
  );
}

proptest! {
  #[test]
  fn decodes_what_it_encodes(frame in any_frame()) {
    let bytes = encoded(&frame);
    prop_assert_eq!(bytes[1], 0);
    prop_assert_eq!(decode_wire(&bytes), frame);
  }

  #[test]
  fn writes_the_same_header_bytes_as_the_deterministic_encoder(frame in any_frame()) {
    prop_assume!(!matches!(frame, Frame::Extension(_)));
    let header = raw_frame(&encoded(&frame)).header;
    if let Some(entries) = header::decode(&header, &DecodeLimits::default()).unwrap() {
      let mut reference = Vec::new();
      header::encode(&entries, &mut reference).unwrap();
      prop_assert_eq!(header.as_ref(), reference.as_slice());
    } else {
      prop_assert!(header.is_empty());
    }
  }
}
