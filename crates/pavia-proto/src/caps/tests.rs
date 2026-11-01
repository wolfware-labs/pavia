use bytes::Bytes;
use proptest::prelude::*;

use super::{Capability, Caps, IMPLEMENTED, check};
use crate::error::ProtocolError;
use crate::frame::{
  AckMap, Call, Datagram, DgramBind, Frame, Heartbeat, LaneOpen, Meta, MetaValue, Notify, Presence, Publication, Sequencing,
  Since, Subscribe, Subscribed, Unsubscribe, Unsubscribed, UnsubscribedForm,
};
use crate::stream::Side;

fn granted(capabilities: &[Capability]) -> Caps {
  Caps(capabilities.iter().copied().chain([Capability::Calls]).collect())
}

fn call(shape: u64, meta: Option<Meta>) -> Frame {
  Frame::Call(Call {
    method: "chat.send".into(),
    shape,
    deadline: None,
    meta,
    data: Bytes::new(),
  })
}

fn subscribe(since: Option<Since>, presence: Option<bool>, sequencing: Sequencing) -> Frame {
  Frame::Subscribe(Subscribe {
    rid: 1,
    channel: "room".into(),
    since,
    presence,
    meta: None,
    sequencing,
    data: Bytes::new(),
  })
}

fn sequenced() -> Sequencing {
  Sequencing { seq: Some(1), ack: None }
}

#[test]
fn round_trips_every_name() {
  for capability in Capability::ALL {
    assert_eq!(Capability::from_name(capability.name()), Some(capability));
  }
  assert_eq!(Capability::from_name("teleport"), None);
  assert_eq!(Capability::from_name("Calls"), None);
}

#[test]
fn grants_calls_and_ignores_what_is_not_implemented() {
  assert_eq!(IMPLEMENTED, [Capability::Calls]);
  let requests: [Option<&[&str]>; 5] = [
    None,
    Some(&[]),
    Some(&["calls", "resume"]),
    Some(&["calls", "teleport"]),
    Some(&["streams"]),
  ];
  for requested in requests {
    let names: Vec<_> = Caps::grant(requested).names().collect();
    assert_eq!(names, ["calls"], "{requested:?}");
  }
}

#[test]
fn lists_names_in_spec_order() {
  let implemented = Capability::ALL;
  let granted = Caps::grant_from(Some(&["lanes", "teleport", "resume", "streams"]), &implemented);
  assert_eq!(granted.names().collect::<Vec<_>>(), ["calls", "streams", "resume", "lanes"]);
}

proptest! {
  #[test]
  fn grants_only_requested_and_implemented_plus_calls(
    requested in prop::collection::vec(prop::sample::select(Capability::ALL.to_vec()), 0..10),
    implemented in prop::collection::vec(prop::sample::select(Capability::ALL.to_vec()), 0..10),
  ) {
    let names: Vec<_> = requested.iter().map(|capability| capability.name()).collect();
    let granted = Caps::grant_from(Some(&names), &implemented);
    prop_assert!(granted.contains(Capability::Calls));
    for capability in Capability::ALL {
      if capability != Capability::Calls && granted.contains(capability) {
        prop_assert!(requested.contains(&capability) && implemented.contains(&capability));
      }
      if requested.contains(&capability) && implemented.contains(&capability) {
        prop_assert!(granted.contains(capability));
      }
    }
  }
}

#[test]
fn refuses_frames_whose_capability_was_not_granted() {
  let calls_only = granted(&[]);
  let ack = Frame::Ack { ack: AckMap::new() };
  let notify = Frame::Notify(Notify {
    method: "typing".into(),
    meta: None,
    sequencing: sequenced(),
    data: Bytes::new(),
  });
  let refused = [
    (subscribe(None, None, Sequencing::default()), Side::Client),
    (call(1, None), Side::Client),
    (call(3, None), Side::Client),
    (call(0, None), Side::Server),
    (ack, Side::Client),
    (notify, Side::Client),
  ];
  for (frame, sender) in refused {
    assert!(
      matches!(check(&calls_only, &frame, sender), Err(ProtocolError::CapabilityNotGranted)),
      "{frame:?} from {sender:?}"
    );
  }
}

#[test]
fn checks_every_capability_a_subscribe_needs() {
  let since = || {
    Some(Since {
      epoch: "e1".into(),
      offset: 4,
    })
  };
  let frame = subscribe(since(), Some(true), sequenced());
  let all = [
    Capability::Channels,
    Capability::History,
    Capability::Presence,
    Capability::Resume,
  ];
  assert!(check(&granted(&all), &frame, Side::Client).is_ok());
  for missing in all {
    let without: Vec<_> = all.into_iter().filter(|&capability| capability != missing).collect();
    assert!(
      matches!(
        check(&granted(&without), &frame, Side::Client),
        Err(ProtocolError::CapabilityNotGranted)
      ),
      "without {missing:?}"
    );
  }
}

#[test]
fn passes_frames_the_grant_covers() {
  let calls_only = granted(&[]);
  let mut meta = Meta::new();
  meta.insert("pavia-idempotency-key".try_into().unwrap(), MetaValue::Text("k1".into()));
  let passed = [
    call(0, None),
    call(0, Some(meta)),
    Frame::Ping {
      data: Heartbeat::default(),
    },
  ];
  for frame in passed {
    assert!(check(&calls_only, &frame, Side::Client).is_ok(), "{frame:?}");
  }
  let streams_and_server_calls = granted(&[Capability::Streams, Capability::ServerCalls]);
  assert!(check(&streams_and_server_calls, &call(2, None), Side::Server).is_ok());
}

#[test]
fn maps_every_gated_frame_to_its_capability() {
  let channel = || "room".to_owned();
  let none = Sequencing::default;
  let rows = [
    (
      Frame::Subscribed(Subscribed {
        rid: 1,
        channel: channel(),
        epoch: "e1".into(),
        offset: 0,
        recovered: None,
        sequencing: none(),
        data: Bytes::new(),
      }),
      Capability::Channels,
    ),
    (
      Frame::Unsubscribe(Unsubscribe {
        rid: 2,
        channel: channel(),
        sequencing: none(),
      }),
      Capability::Channels,
    ),
    (
      Frame::Unsubscribed(Unsubscribed {
        channel: channel(),
        form: UnsubscribedForm::Reply { rid: 2 },
        sequencing: none(),
      }),
      Capability::Channels,
    ),
    (
      Frame::Publication(Publication {
        channel: channel(),
        offset: 1,
        sequencing: none(),
        data: Bytes::new(),
      }),
      Capability::Channels,
    ),
    (
      Frame::Presence(Presence {
        channel: channel(),
        kind: 0,
        sequencing: none(),
        data: Bytes::new(),
      }),
      Capability::Presence,
    ),
    (
      Frame::Datagram(Datagram {
        key: 1,
        seq: 0,
        data: Bytes::new(),
      }),
      Capability::Datagrams,
    ),
    (
      Frame::DgramBind(DgramBind {
        key: 1,
        name: "cursor".into(),
        sequencing: none(),
      }),
      Capability::Datagrams,
    ),
    (Frame::LaneOpen(LaneOpen { lane: 1 }), Capability::Lanes),
    (subscribe(None, Some(false), none()), Capability::Presence),
  ];
  for (frame, capability) in rows {
    assert!(
      matches!(
        check(&granted(&[]), &frame, Side::Server),
        Err(ProtocolError::CapabilityNotGranted)
      ),
      "{frame:?}"
    );
    let with = if capability == Capability::Presence {
      granted(&[Capability::Channels, capability])
    } else {
      granted(&[capability])
    };
    assert!(check(&with, &frame, Side::Server).is_ok(), "{frame:?} with {capability:?}");
  }
}
