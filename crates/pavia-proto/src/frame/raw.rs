//! Conversion between typed frames and raw frames from the wire codec.

use bytes::Bytes;

use super::fields::{self, Bad, Fields, Problem, Writer};
use super::*;
use crate::wire::frame::Frame as RawFrame;
use crate::wire::header::{self, DecodeLimits};

impl Frame {
  /// Builds a typed frame from a raw one. Unknown header keys are ignored (§5.4.1) and so are the
  /// flags (§5.2).
  ///
  /// # Errors
  ///
  /// - [`FrameError::UnknownType`] for an unregistered type from `0x00` to `0x7F`.
  /// - [`FrameError::MalformedHeader`] if the header is not a valid CBOR map.
  /// - [`FrameError::HeartbeatTooLarge`] for a PING or PONG data section over 8 bytes.
  /// - [`FrameError::Field`] for a missing or ill-typed field.
  ///
  /// Every error converts into the [`ProtocolError`] the session acts on.
  pub(crate) fn from_raw(raw: &RawFrame, limits: &DecodeLimits) -> Result<Self, FrameError> {
    let Ok(frame_type) = FrameType::try_from(raw.frame_type) else {
      return if raw.frame_type >= 0x80 {
        Ok(Self::Extension(Extension {
          frame_type: raw.frame_type,
          header: raw.header.clone(),
          data: raw.data.clone(),
        }))
      } else {
        Err(FrameError::UnknownType(raw.frame_type))
      };
    };
    if matches!(frame_type, FrameType::Ping | FrameType::Pong) {
      Heartbeat::try_from(raw.data.clone())?;
    }
    let entries = header::decode(&raw.header, limits)
      .map_err(|_| FrameError::MalformedHeader)?
      .unwrap_or_default();
    let fields = Fields::new(&entries);
    decode_typed(frame_type, &fields, raw.data.clone()).map_err(|bad| {
      FrameError::Field(FieldError {
        frame_type,
        key: bad.key,
        problem: bad.problem,
        rid: fields.optional(key::RID, fields::uint).ok().flatten(),
      })
    })
  }

  /// Turns the frame into a raw one with flags 0, ready for the wire codec.
  pub(crate) fn to_raw(&self) -> RawFrame {
    let mut writer = Writer::default();
    let mut data = Bytes::new();
    let frame_type = match self {
      Self::Extension(extension) => {
        return RawFrame {
          frame_type: extension.frame_type,
          flags: 0,
          header: extension.header.clone(),
          data: extension.data.clone(),
        };
      }
      _ => self.frame_type().expect("every other variant is registered"),
    };
    encode_typed(self, &mut writer, &mut data);
    RawFrame {
      frame_type: frame_type.into(),
      flags: 0,
      header: writer.finish(),
      data,
    }
  }
}

fn sequencing(fields: &Fields<'_, '_>) -> Result<Sequencing, Bad> {
  Ok(Sequencing {
    seq: fields.optional(key::SEQ, fields::uint)?,
    ack: fields.optional(key::ACK, fields::ack_map)?,
  })
}

fn decode_typed(frame_type: FrameType, f: &Fields<'_, '_>, data: Bytes) -> Result<Frame, Bad> {
  use fields::{
    ack_map, auth, boolean, client, fingerprint, limits, meta, resume, server_limits, since, text, text_array, token, uint,
    uint_array,
  };
  Ok(match frame_type {
    FrameType::Hello => Frame::Hello(Hello {
      versions: f.required(key::VERSIONS, uint_array)?,
      codecs: f.required(key::CODECS, text_array)?,
      caps: f.optional(key::CAPS, text_array)?,
      limits: f.optional(key::LIMITS, limits)?,
      contract: f.optional(key::CONTRACT, fingerprint)?,
      auth: f.optional(key::AUTH, auth)?,
      resume: f.optional(key::RESUME, resume)?,
      client: f.optional(key::CLIENT, client)?,
    }),
    FrameType::Welcome => Frame::Welcome(Welcome {
      version: f.required(key::VERSION, uint)?,
      codec: f.required(key::CODEC, text)?,
      caps: f.required(key::CAPS, text_array)?,
      session: f.required(key::SESSION, text)?,
      token: f.optional(key::TOKEN, token)?,
      limits: f.required(key::LIMITS, server_limits)?,
      hb: f.required(key::HB, uint)?,
      idle: f.required(key::IDLE, uint)?,
      resume_window: f.optional(key::RESUME_WINDOW, uint)?,
      contract: f.optional(key::CONTRACT, fingerprint)?,
      user: f.optional(key::USER, text)?,
      resumed: f.optional(key::RESUMED, boolean)?,
      ack: f.optional(key::ACK, ack_map)?,
      lost: f.optional(key::LOST, uint_array)?,
      transports: f.optional(key::TRANSPORTS, text_array)?,
    }),
    FrameType::Reject => Frame::Reject(Reject {
      code: f.required(key::CODE, uint)?,
      msg: f.required(key::MSG, text)?,
      retry: f.optional(key::RETRY, uint)?,
    }),
    FrameType::Ping => Frame::Ping { data: Heartbeat(data) },
    FrameType::Pong => Frame::Pong { data: Heartbeat(data) },
    FrameType::Goaway => Frame::Goaway(Goaway {
      code: f.required(key::CODE, uint)?,
      msg: f.required(key::MSG, text)?,
      url: f.optional(key::URL, text)?,
      retry: f.optional(key::RETRY, uint)?,
    }),
    FrameType::Close => Frame::Close(Close {
      code: f.required(key::CODE, uint)?,
      msg: f.optional(key::MSG, text)?,
    }),
    FrameType::AuthRefresh => Frame::AuthRefresh(AuthRefresh {
      rid: f.required(key::RID, uint)?,
      auth: f.required(key::AUTH, auth)?,
      sequencing: sequencing(f)?,
    }),
    FrameType::AuthResult => Frame::AuthResult(AuthResult {
      rid: f.required(key::RID, uint)?,
      code: f.required(key::CODE, uint)?,
      expires: f.optional(key::EXPIRES, uint)?,
      sequencing: sequencing(f)?,
    }),
    FrameType::Ack => Frame::Ack {
      ack: f.required(key::ACK, ack_map)?,
    },
    FrameType::AuthExpiring => Frame::AuthExpiring(AuthExpiring {
      expires: f.required(key::EXPIRES, uint)?,
      sequencing: sequencing(f)?,
    }),
    FrameType::RequestError => Frame::RequestError(RequestError {
      rid: f.required(key::RID, uint)?,
      code: f.required(key::CODE, uint)?,
      msg: f.required(key::MSG, text)?,
      sequencing: sequencing(f)?,
    }),
    FrameType::Call => Frame::Call(Call {
      method: f.required(key::METHOD, text)?,
      shape: f.required(key::SHAPE, uint)?,
      deadline: f.optional(key::DEADLINE, uint)?,
      meta: f.optional(key::META, meta)?,
      data,
    }),
    FrameType::Item => Frame::Item(Item {
      meta: f.optional(key::META, meta)?,
      data,
    }),
    FrameType::End => Frame::End(End {
      meta: f.optional(key::META, meta)?,
    }),
    FrameType::Result => Frame::Result(CallResult {
      meta: f.optional(key::META, meta)?,
      data,
    }),
    FrameType::Error => Frame::Error(CallError {
      code: f.required(key::CODE, uint)?,
      msg: f.required(key::MSG, text)?,
      nx: f.optional(key::NX, boolean)?,
      meta: f.optional(key::META, meta)?,
      data,
    }),
    FrameType::Cancel => Frame::Cancel(Cancel {
      msg: f.optional(key::MSG, text)?,
    }),
    FrameType::Credit => Frame::Credit(Credit {
      credit: f.required(key::CREDIT, uint)?,
    }),
    FrameType::Notify => Frame::Notify(Notify {
      method: f.required(key::METHOD, text)?,
      meta: f.optional(key::META, meta)?,
      sequencing: sequencing(f)?,
      data,
    }),
    FrameType::LaneOpen => Frame::LaneOpen(LaneOpen {
      lane: f.required(key::LANE, uint)?,
    }),
    FrameType::Subscribe => Frame::Subscribe(Subscribe {
      rid: f.required(key::RID, uint)?,
      channel: f.required(key::CHANNEL, text)?,
      since: f.optional(key::SINCE, since)?,
      presence: f.optional(key::PRESENCE, boolean)?,
      meta: f.optional(key::META, meta)?,
      sequencing: sequencing(f)?,
      data,
    }),
    FrameType::Subscribed => Frame::Subscribed(Subscribed {
      rid: f.required(key::RID, uint)?,
      channel: f.required(key::CHANNEL, text)?,
      epoch: f.required(key::EPOCH, text)?,
      offset: f.required(key::OFFSET, uint)?,
      recovered: f.optional(key::RECOVERED, boolean)?,
      sequencing: sequencing(f)?,
      data,
    }),
    FrameType::Unsubscribe => Frame::Unsubscribe(Unsubscribe {
      rid: f.required(key::RID, uint)?,
      channel: f.required(key::CHANNEL, text)?,
      sequencing: sequencing(f)?,
    }),
    FrameType::Unsubscribed => {
      let bad = |key, problem| Bad { key, problem };
      let form = match (
        f.optional(key::RID, uint)?,
        f.optional(key::CODE, uint)?,
        f.optional(key::MSG, text)?,
      ) {
        (Some(rid), None, None) => UnsubscribedForm::Reply { rid },
        (None, Some(code), Some(msg)) => UnsubscribedForm::Notice { code, msg },
        (None, Some(_), None) => return Err(bad(key::MSG, Problem::Missing)),
        (None, None, _) => return Err(bad(key::CODE, Problem::Missing)),
        (Some(_), Some(_), _) => return Err(bad(key::CODE, Problem::Unexpected)),
        (Some(_), None, Some(_)) => return Err(bad(key::MSG, Problem::Unexpected)),
      };
      Frame::Unsubscribed(Unsubscribed {
        channel: f.required(key::CHANNEL, text)?,
        form,
        sequencing: sequencing(f)?,
      })
    }
    FrameType::Publication => Frame::Publication(Publication {
      channel: f.required(key::CHANNEL, text)?,
      offset: f.required(key::OFFSET, uint)?,
      sequencing: sequencing(f)?,
      data,
    }),
    FrameType::Presence => Frame::Presence(Presence {
      channel: f.required(key::CHANNEL, text)?,
      kind: f.required(key::KIND, uint)?,
      sequencing: sequencing(f)?,
      data,
    }),
    FrameType::Datagram => Frame::Datagram(Datagram {
      key: f.required(key::KEY, uint)?,
      seq: f.required(key::SEQ, uint)?,
      data,
    }),
    FrameType::DgramBind => Frame::DgramBind(DgramBind {
      key: f.required(key::KEY, uint)?,
      name: f.required(key::NAME, text)?,
      sequencing: sequencing(f)?,
    }),
  })
}

fn put_sequencing<'a>(w: &mut Writer<'a>, sequencing: &'a Sequencing) {
  w.put_some(key::SEQ, sequencing.seq.as_ref(), fields::uint_value);
  w.put_some(key::ACK, sequencing.ack.as_ref(), fields::ack_map_value);
}

fn encode_typed<'a>(frame: &'a Frame, w: &mut Writer<'a>, data: &mut Bytes) {
  use fields::{
    ack_map_value, auth_value, bool_value, client_value, fingerprint_value, limits_value, meta_value, resume_value,
    server_limits_value, since_value, text_array_value, text_value, token_value, uint_array_value, uint_value,
  };
  match frame {
    Frame::Hello(hello) => {
      w.put(key::VERSIONS, uint_array_value(&hello.versions));
      w.put(key::CODECS, text_array_value(&hello.codecs));
      w.put_some(key::CAPS, hello.caps.as_deref(), text_array_value);
      w.put_some(key::LIMITS, hello.limits.as_ref(), limits_value);
      w.put_some(key::CONTRACT, hello.contract.as_ref(), fingerprint_value);
      w.put_some(key::AUTH, hello.auth.as_ref(), auth_value);
      w.put_some(key::RESUME, hello.resume.as_ref(), resume_value);
      w.put_some(key::CLIENT, hello.client.as_ref(), client_value);
    }
    Frame::Welcome(welcome) => {
      w.put(key::VERSION, uint_value(&welcome.version));
      w.put(key::CODEC, text_value(&welcome.codec));
      w.put(key::CAPS, text_array_value(&welcome.caps));
      w.put(key::SESSION, text_value(&welcome.session));
      w.put_some(key::TOKEN, welcome.token.as_ref(), token_value);
      w.put(key::LIMITS, server_limits_value(&welcome.limits));
      w.put(key::HB, uint_value(&welcome.hb));
      w.put(key::IDLE, uint_value(&welcome.idle));
      w.put_some(key::RESUME_WINDOW, welcome.resume_window.as_ref(), uint_value);
      w.put_some(key::CONTRACT, welcome.contract.as_ref(), fingerprint_value);
      w.put_some(key::USER, welcome.user.as_deref(), text_value);
      w.put_some(key::RESUMED, welcome.resumed.as_ref(), bool_value);
      w.put_some(key::ACK, welcome.ack.as_ref(), ack_map_value);
      w.put_some(key::LOST, welcome.lost.as_deref(), uint_array_value);
      w.put_some(key::TRANSPORTS, welcome.transports.as_deref(), text_array_value);
    }
    Frame::Reject(reject) => {
      w.put(key::CODE, uint_value(&reject.code));
      w.put(key::MSG, text_value(&reject.msg));
      w.put_some(key::RETRY, reject.retry.as_ref(), uint_value);
    }
    Frame::Ping { data: payload } | Frame::Pong { data: payload } => *data = payload.as_bytes().clone(),
    Frame::Goaway(goaway) => {
      w.put(key::CODE, uint_value(&goaway.code));
      w.put(key::MSG, text_value(&goaway.msg));
      w.put_some(key::URL, goaway.url.as_deref(), text_value);
      w.put_some(key::RETRY, goaway.retry.as_ref(), uint_value);
    }
    Frame::Close(close) => {
      w.put(key::CODE, uint_value(&close.code));
      w.put_some(key::MSG, close.msg.as_deref(), text_value);
    }
    Frame::AuthRefresh(refresh) => {
      w.put(key::RID, uint_value(&refresh.rid));
      w.put(key::AUTH, auth_value(&refresh.auth));
      put_sequencing(w, &refresh.sequencing);
    }
    Frame::AuthResult(result) => {
      w.put(key::RID, uint_value(&result.rid));
      w.put(key::CODE, uint_value(&result.code));
      w.put_some(key::EXPIRES, result.expires.as_ref(), uint_value);
      put_sequencing(w, &result.sequencing);
    }
    Frame::Ack { ack } => w.put(key::ACK, ack_map_value(ack)),
    Frame::AuthExpiring(expiring) => {
      w.put(key::EXPIRES, uint_value(&expiring.expires));
      put_sequencing(w, &expiring.sequencing);
    }
    Frame::RequestError(error) => {
      w.put(key::RID, uint_value(&error.rid));
      w.put(key::CODE, uint_value(&error.code));
      w.put(key::MSG, text_value(&error.msg));
      put_sequencing(w, &error.sequencing);
    }
    Frame::Call(call) => {
      w.put(key::METHOD, text_value(&call.method));
      w.put(key::SHAPE, uint_value(&call.shape));
      w.put_some(key::DEADLINE, call.deadline.as_ref(), uint_value);
      w.put_some(key::META, call.meta.as_ref(), meta_value);
      *data = call.data.clone();
    }
    Frame::Item(item) => {
      w.put_some(key::META, item.meta.as_ref(), meta_value);
      *data = item.data.clone();
    }
    Frame::End(end) => w.put_some(key::META, end.meta.as_ref(), meta_value),
    Frame::Result(result) => {
      w.put_some(key::META, result.meta.as_ref(), meta_value);
      *data = result.data.clone();
    }
    Frame::Error(error) => {
      w.put(key::CODE, uint_value(&error.code));
      w.put(key::MSG, text_value(&error.msg));
      w.put_some(key::NX, error.nx.as_ref(), bool_value);
      w.put_some(key::META, error.meta.as_ref(), meta_value);
      *data = error.data.clone();
    }
    Frame::Cancel(cancel) => w.put_some(key::MSG, cancel.msg.as_deref(), text_value),
    Frame::Credit(credit) => w.put(key::CREDIT, uint_value(&credit.credit)),
    Frame::Notify(notify) => {
      w.put(key::METHOD, text_value(&notify.method));
      w.put_some(key::META, notify.meta.as_ref(), meta_value);
      put_sequencing(w, &notify.sequencing);
      *data = notify.data.clone();
    }
    Frame::LaneOpen(open) => w.put(key::LANE, uint_value(&open.lane)),
    Frame::Subscribe(subscribe) => {
      w.put(key::RID, uint_value(&subscribe.rid));
      w.put(key::CHANNEL, text_value(&subscribe.channel));
      w.put_some(key::SINCE, subscribe.since.as_ref(), since_value);
      w.put_some(key::PRESENCE, subscribe.presence.as_ref(), bool_value);
      w.put_some(key::META, subscribe.meta.as_ref(), meta_value);
      put_sequencing(w, &subscribe.sequencing);
      *data = subscribe.data.clone();
    }
    Frame::Subscribed(subscribed) => {
      w.put(key::RID, uint_value(&subscribed.rid));
      w.put(key::CHANNEL, text_value(&subscribed.channel));
      w.put(key::EPOCH, text_value(&subscribed.epoch));
      w.put(key::OFFSET, uint_value(&subscribed.offset));
      w.put_some(key::RECOVERED, subscribed.recovered.as_ref(), bool_value);
      put_sequencing(w, &subscribed.sequencing);
      *data = subscribed.data.clone();
    }
    Frame::Unsubscribe(unsubscribe) => {
      w.put(key::RID, uint_value(&unsubscribe.rid));
      w.put(key::CHANNEL, text_value(&unsubscribe.channel));
      put_sequencing(w, &unsubscribe.sequencing);
    }
    Frame::Unsubscribed(unsubscribed) => {
      w.put(key::CHANNEL, text_value(&unsubscribed.channel));
      match &unsubscribed.form {
        UnsubscribedForm::Reply { rid } => w.put(key::RID, uint_value(rid)),
        UnsubscribedForm::Notice { code, msg } => {
          w.put(key::CODE, uint_value(code));
          w.put(key::MSG, text_value(msg));
        }
      }
      put_sequencing(w, &unsubscribed.sequencing);
    }
    Frame::Publication(publication) => {
      w.put(key::CHANNEL, text_value(&publication.channel));
      w.put(key::OFFSET, uint_value(&publication.offset));
      put_sequencing(w, &publication.sequencing);
      *data = publication.data.clone();
    }
    Frame::Presence(presence) => {
      w.put(key::CHANNEL, text_value(&presence.channel));
      w.put(key::KIND, uint_value(&presence.kind));
      put_sequencing(w, &presence.sequencing);
      *data = presence.data.clone();
    }
    Frame::Datagram(datagram) => {
      w.put(key::KEY, uint_value(&datagram.key));
      w.put(key::SEQ, uint_value(&datagram.seq));
      *data = datagram.data.clone();
    }
    Frame::DgramBind(bind) => {
      w.put(key::KEY, uint_value(&bind.key));
      w.put(key::NAME, text_value(&bind.name));
      put_sequencing(w, &bind.sequencing);
    }
    Frame::Extension(_) => {}
  }
}
