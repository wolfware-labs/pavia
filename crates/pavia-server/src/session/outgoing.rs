//! Server calls to the client (§8.8): each one waits here, by stream, until the client answers,
//! the caller abandons it, or the session ends.

use std::collections::HashMap;
use std::time::Duration;

use pavia_proto::data::{self, Codec, DecodeLimits, Value};
use pavia_proto::registry::StatusCode;
use pavia_proto::session::{Answer, Session};
use tokio::sync::oneshot;
use tokio::time::Instant;

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
      deadline,
      reply,
    } = calling;
    if reply.is_closed() {
      return;
    }
    let left = deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
    if left.is_some_and(|left| left < Duration::from_millis(1)) {
      let _ = reply.send(Err(CallError::deadline_exceeded(true)));
      return;
    }
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
    match session.call(method.as_str().to_owned(), data, left) {
      Ok(stream) => {
        self.waiting.insert(stream, (token, reply));
        self.streams.insert(token, stream);
      }
      Err(refused) => {
        let _ = reply.send(Err(CallError::refused(refused)));
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
      let _ = reply.send(decoded(answer, codec));
    }
  }

  #[cfg(test)]
  pub(super) fn sizes(&self) -> (usize, usize) {
    (self.waiting.len(), self.streams.len())
  }
}

fn decoded(answer: Answer, codec: Codec) -> Result<Value, CallError> {
  let decode = |data: &[u8], section: &str| {
    data::decode(data, codec, &DecodeLimits::default()).map_err(|_| {
      CallError::new(
        StatusCode::Internal,
        format!("the client's {section} does not decode"),
        false,
        None,
      )
    })
  };
  match answer {
    Answer::Result(result) => decode(&result.data, "RESULT"),
    Answer::Error(error) => {
      let detail = if error.data.is_empty() {
        None
      } else {
        Some(decode(&error.data, "ERROR detail")?)
      };
      let code = match StatusCode::from_u64_lossy(error.code) {
        StatusCode::Ok => StatusCode::Unknown,
        code => code,
      };
      Err(CallError::new(code, error.msg, error.nx == Some(true), detail))
    }
  }
}
