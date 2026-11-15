//! Server calls to the client (§8.8): each one waits here, by stream, until the client answers,
//! the caller abandons it, or the session ends.

use std::collections::HashMap;

use pavia_proto::data::{self, Codec, DecodeLimits, Value};
use pavia_proto::registry::StatusCode;
use pavia_proto::session::{Answer, Session};
use tokio::sync::oneshot;

use crate::methods::encode;
use crate::sessions::{CallError, Calling};

type Reply = oneshot::Sender<Result<Value, CallError>>;

#[derive(Default)]
pub(super) struct Outgoing {
  waiting: HashMap<u64, (u64, Reply)>,
  streams: HashMap<u64, u64>,
}

impl Outgoing {
  pub(super) fn open(&mut self, session: &mut Session, calling: Calling, codec: Codec) {
    let Calling {
      token,
      method,
      arguments,
      options,
      reply,
    } = calling;
    let Ok(data) = encode(&Value::Array(arguments), codec) else {
      let refusal = CallError::new(
        StatusCode::InvalidArgument,
        "arguments the session codec cannot encode",
        true,
        None,
      );
      let _ = reply.send(Err(refusal));
      return;
    };
    let deadline = options
      .deadline
      .map(|deadline| u64::try_from(deadline.as_millis()).unwrap_or(u64::MAX));
    match session.call(method.as_str().to_owned(), data, deadline) {
      Ok(stream) => {
        self.waiting.insert(stream, (token, reply));
        self.streams.insert(token, stream);
      }
      Err(refused) => {
        let _ = reply.send(Err(CallError::new(refused.code(), refused.msg(), true, None)));
      }
    }
  }

  pub(super) fn abandon(&mut self, token: u64) -> Option<u64> {
    let stream = self.streams.remove(&token)?;
    self.waiting.remove(&stream);
    Some(stream)
  }

  pub(super) fn answered(&mut self, stream: u64, answer: Answer, codec: Codec) {
    if let Some((token, reply)) = self.waiting.remove(&stream) {
      self.streams.remove(&token);
      let _ = reply.send(outcome(answer, codec));
    }
  }

  pub(super) fn clear(&mut self) {
    self.waiting.clear();
    self.streams.clear();
  }
}

fn outcome(answer: Answer, codec: Codec) -> Result<Value, CallError> {
  let decode = |data: &[u8]| data::decode(data, codec, &DecodeLimits::default());
  match answer {
    Answer::Result(result) => {
      decode(&result.data).map_err(|_| CallError::new(StatusCode::Internal, "the client's RESULT does not decode", false, None))
    }
    Answer::Error(error) => {
      let code = u16::try_from(error.code)
        .ok()
        .and_then(|code| StatusCode::try_from(code).ok())
        .unwrap_or(StatusCode::Unknown);
      let detail = if error.data.is_empty() {
        None
      } else {
        decode(&error.data).ok()
      };
      Err(CallError::new(code, error.msg, error.nx == Some(true), detail))
    }
  }
}
