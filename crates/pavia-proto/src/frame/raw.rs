//! Decoding typed frames from raw frames of the wire codec, and encoding them straight to bytes.

use bytes::{BufMut, Bytes};

use super::fields::{self, Bad, Entries, Fields, MapBody, Problem, encode_map, entry_count};
use super::*;
use crate::wire::frame::{EncodeError, Frame as RawFrame, put_prefix};
use crate::wire::header::{self, DecodeLimits, Measure, Out, Sink};

impl Frame {
  /// Builds a typed frame from a raw one. Unknown header keys are ignored (§5.4.1) and so are the
  /// flags (§5.2).
  ///
  /// # Errors
  ///
  /// - [`FrameError::UnknownType`] for an unregistered type from `0x00` to `0x7F`.
  /// - [`FrameError::MalformedHeader`] if the header is not a valid CBOR map with unsigned
  ///   integer keys.
  /// - [`FrameError::UnexpectedData`] for a data section on a type without one.
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
    if !frame_type.carries_data() && !raw.data.is_empty() {
      return Err(FrameError::UnexpectedData);
    }
    if matches!(frame_type, FrameType::Ping | FrameType::Pong) {
      Heartbeat::try_from(raw.data.clone())?;
    }
    let entries = header::decode(&raw.header, limits)
      .map_err(|_| FrameError::MalformedHeader)?
      .unwrap_or_default();
    let fields = Fields::new(&entries).ok_or(FrameError::MalformedHeader)?;
    decode_typed(frame_type, &fields, raw.data.clone()).map_err(|bad| {
      FrameError::Field(FieldError {
        frame_type,
        key: bad.key,
        problem: bad.problem,
        rid: fields.optional(key::RID, fields::uint).ok().flatten(),
      })
    })
  }

  /// Writes the frame to `out` with flags 0: the prefix, then the header straight from the typed
  /// fields in deterministic encoding (§5.4.1), then the data section. Nothing is allocated.
  ///
  /// # Errors
  ///
  /// Returns [`EncodeError::PayloadTooLarge`] if the payload is above 2^62 − 1 bytes. Nothing is
  /// written to `out` in that case.
  pub(crate) fn encode(&self, out: &mut impl BufMut) -> Result<(), EncodeError> {
    if let Self::Extension(extension) = self {
      put_prefix(extension.frame_type, 0, extension.header.len(), extension.data.len(), out)?;
      out.put_slice(&extension.header);
      out.put_slice(&extension.data);
      return Ok(());
    }
    let frame_type = self.frame_type().expect("every other variant is registered");
    let header_length = if entry_count(self) == 0 {
      0
    } else {
      let mut measure = Measure::default();
      encode_map(self, &mut measure);
      measure.0
    };
    let data = self.data();
    put_prefix(frame_type.into(), 0, header_length, data.len(), out)?;
    if header_length > 0 {
      encode_map(self, &mut Out(out));
    }
    out.put_slice(data);
    Ok(())
  }

  fn data(&self) -> &[u8] {
    match self {
      Self::Ping { data } | Self::Pong { data } => data.as_bytes(),
      Self::Call(Call { data, .. })
      | Self::Item(Item { data, .. })
      | Self::Result(CallResult { data, .. })
      | Self::Error(CallError { data, .. })
      | Self::Notify(Notify { data, .. })
      | Self::Subscribe(Subscribe { data, .. })
      | Self::Subscribed(Subscribed { data, .. })
      | Self::Publication(Publication { data, .. })
      | Self::Presence(Presence { data, .. })
      | Self::Datagram(Datagram { data, .. })
      | Self::Extension(Extension { data, .. }) => data,
      _ => &[],
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

impl MapBody for Frame {
  fn entries<S: Sink>(&self, e: &mut Entries<'_, S>) {
    match self {
      Frame::Hello(hello) => {
        e.put(key::VERSIONS, &hello.versions);
        e.put(key::CODECS, &hello.codecs);
        e.put_some(key::CAPS, hello.caps.as_ref());
        e.put_some(key::RESUME, hello.resume.as_ref());
        e.put_some(key::LIMITS, hello.limits.as_ref());
        e.put_some(key::CONTRACT, hello.contract.as_ref());
        e.put_some(key::AUTH, hello.auth.as_ref());
        e.put_some(key::CLIENT, hello.client.as_ref());
      }
      Frame::Welcome(welcome) => {
        e.put_some(key::TOKEN, welcome.token.as_ref());
        e.put(key::VERSION, &welcome.version);
        e.put(key::CODEC, &welcome.codec);
        e.put(key::CAPS, &welcome.caps);
        e.put(key::SESSION, &welcome.session);
        e.put(key::LIMITS, &welcome.limits);
        e.put(key::HB, &welcome.hb);
        e.put(key::IDLE, &welcome.idle);
        e.put_some(key::CONTRACT, welcome.contract.as_ref());
        e.put_some(key::USER, welcome.user.as_ref());
        e.put_some(key::RESUME_WINDOW, welcome.resume_window.as_ref());
        e.put_some(key::RESUMED, welcome.resumed.as_ref());
        e.put_some(key::ACK, welcome.ack.as_ref());
        e.put_some(key::TRANSPORTS, welcome.transports.as_ref());
        e.put_some(key::LOST, welcome.lost.as_ref());
      }
      Frame::Reject(reject) => {
        e.put(key::CODE, &reject.code);
        e.put(key::MSG, &reject.msg);
        e.put_some(key::RETRY, reject.retry.as_ref());
      }
      Frame::Ping { .. } | Frame::Pong { .. } | Frame::Extension(_) => {}
      Frame::Goaway(goaway) => {
        e.put(key::CODE, &goaway.code);
        e.put(key::MSG, &goaway.msg);
        e.put_some(key::RETRY, goaway.retry.as_ref());
        e.put_some(key::URL, goaway.url.as_ref());
      }
      Frame::Close(close) => {
        e.put(key::CODE, &close.code);
        e.put_some(key::MSG, close.msg.as_ref());
      }
      Frame::AuthRefresh(refresh) => {
        e.put(key::RID, &refresh.rid);
        e.put_some(key::SEQ, refresh.sequencing.seq.as_ref());
        e.put(key::AUTH, &refresh.auth);
        e.put_some(key::ACK, refresh.sequencing.ack.as_ref());
      }
      Frame::AuthResult(result) => {
        e.put(key::RID, &result.rid);
        e.put(key::CODE, &result.code);
        e.put_some(key::SEQ, result.sequencing.seq.as_ref());
        e.put_some(key::EXPIRES, result.expires.as_ref());
        e.put_some(key::ACK, result.sequencing.ack.as_ref());
      }
      Frame::Ack { ack } => e.put(key::ACK, ack),
      Frame::AuthExpiring(expiring) => {
        e.put_some(key::SEQ, expiring.sequencing.seq.as_ref());
        e.put(key::EXPIRES, &expiring.expires);
        e.put_some(key::ACK, expiring.sequencing.ack.as_ref());
      }
      Frame::RequestError(error) => {
        e.put(key::RID, &error.rid);
        e.put(key::CODE, &error.code);
        e.put(key::MSG, &error.msg);
        e.put_some(key::SEQ, error.sequencing.seq.as_ref());
        e.put_some(key::ACK, error.sequencing.ack.as_ref());
      }
      Frame::Call(call) => {
        e.put(key::METHOD, &call.method);
        e.put_some(key::META, call.meta.as_ref());
        e.put_some(key::DEADLINE, call.deadline.as_ref());
        e.put(key::SHAPE, &call.shape);
      }
      Frame::Item(Item { meta, .. }) | Frame::End(End { meta }) | Frame::Result(CallResult { meta, .. }) => {
        e.put_some(key::META, meta.as_ref());
      }
      Frame::Error(error) => {
        e.put_some(key::META, error.meta.as_ref());
        e.put(key::CODE, &error.code);
        e.put(key::MSG, &error.msg);
        e.put_some(key::NX, error.nx.as_ref());
      }
      Frame::Cancel(cancel) => e.put_some(key::MSG, cancel.msg.as_ref()),
      Frame::Credit(credit) => e.put(key::CREDIT, &credit.credit),
      Frame::Notify(notify) => {
        e.put(key::METHOD, &notify.method);
        e.put_some(key::META, notify.meta.as_ref());
        e.put_some(key::SEQ, notify.sequencing.seq.as_ref());
        e.put_some(key::ACK, notify.sequencing.ack.as_ref());
      }
      Frame::LaneOpen(open) => e.put(key::LANE, &open.lane),
      Frame::Subscribe(subscribe) => {
        e.put(key::RID, &subscribe.rid);
        e.put_some(key::META, subscribe.meta.as_ref());
        e.put(key::CHANNEL, &subscribe.channel);
        e.put_some(key::SEQ, subscribe.sequencing.seq.as_ref());
        e.put_some(key::PRESENCE, subscribe.presence.as_ref());
        e.put_some(key::SINCE, subscribe.since.as_ref());
        e.put_some(key::ACK, subscribe.sequencing.ack.as_ref());
      }
      Frame::Subscribed(subscribed) => {
        e.put(key::RID, &subscribed.rid);
        e.put(key::CHANNEL, &subscribed.channel);
        e.put(key::EPOCH, &subscribed.epoch);
        e.put(key::OFFSET, &subscribed.offset);
        e.put_some(key::SEQ, subscribed.sequencing.seq.as_ref());
        e.put_some(key::RECOVERED, subscribed.recovered.as_ref());
        e.put_some(key::ACK, subscribed.sequencing.ack.as_ref());
      }
      Frame::Unsubscribe(unsubscribe) => {
        e.put(key::RID, &unsubscribe.rid);
        e.put(key::CHANNEL, &unsubscribe.channel);
        e.put_some(key::SEQ, unsubscribe.sequencing.seq.as_ref());
        e.put_some(key::ACK, unsubscribe.sequencing.ack.as_ref());
      }
      Frame::Unsubscribed(unsubscribed) => {
        match &unsubscribed.form {
          UnsubscribedForm::Reply { rid } => e.put(key::RID, rid),
          UnsubscribedForm::Notice { code, msg } => {
            e.put(key::CODE, code);
            e.put(key::MSG, msg);
          }
        }
        e.put(key::CHANNEL, &unsubscribed.channel);
        e.put_some(key::SEQ, unsubscribed.sequencing.seq.as_ref());
        e.put_some(key::ACK, unsubscribed.sequencing.ack.as_ref());
      }
      Frame::Publication(publication) => {
        e.put(key::CHANNEL, &publication.channel);
        e.put(key::OFFSET, &publication.offset);
        e.put_some(key::SEQ, publication.sequencing.seq.as_ref());
        e.put_some(key::ACK, publication.sequencing.ack.as_ref());
      }
      Frame::Presence(presence) => {
        e.put(key::CHANNEL, &presence.channel);
        e.put_some(key::SEQ, presence.sequencing.seq.as_ref());
        e.put(key::KIND, &presence.kind);
        e.put_some(key::ACK, presence.sequencing.ack.as_ref());
      }
      Frame::Datagram(datagram) => {
        e.put(key::SEQ, &datagram.seq);
        e.put(key::KEY, &datagram.key);
      }
      Frame::DgramBind(bind) => {
        e.put_some(key::SEQ, bind.sequencing.seq.as_ref());
        e.put(key::KEY, &bind.key);
        e.put(key::NAME, &bind.name);
        e.put_some(key::ACK, bind.sequencing.ack.as_ref());
      }
    }
  }
}
