//! Lossless bounded native observation transport. These chunks carry no input
//! permission and never establish delivery/completion on their own. The Core
//! receipt adapter must still authenticate the physical peer, bind the genuine
//! prompt identity, and commit the reconstructed effective content before ACK.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    io,
    time::{Duration, Instant},
};
const CHUNK: usize = 64 * 1024;
const BODY: usize = super::ownership::MAX_BODY;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentChunk {
    pub request: String,
    pub revision: i64,
    pub prompt: String,
    pub total_bytes: u32,
    pub index: u32,
    pub final_chunk: bool,
    pub data_base64: String,
}
impl std::fmt::Debug for ContentChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContentChunk")
            .field("index", &self.index)
            .field("total_bytes", &self.total_bytes)
            .field("payload", &"<private>")
            .finish()
    }
}
fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "native effective content assembly unavailable",
    )
}
fn id(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok_and(|v| !v.is_nil() && v.to_string() == s)
}
pub fn encode(
    request: &str,
    revision: i64,
    prompt: &str,
    body: &str,
) -> io::Result<Vec<ContentChunk>> {
    if !id(request) || !id(prompt) || revision < 0 || body.is_empty() || body.len() > BODY {
        return Err(denied());
    }
    let count = body.len().div_ceil(CHUNK);
    body.as_bytes()
        .chunks(CHUNK)
        .enumerate()
        .map(|(index, bytes)| {
            let chunk = ContentChunk {
                request: request.into(),
                revision,
                prompt: prompt.into(),
                total_bytes: body.len() as u32,
                index: index as u32,
                final_chunk: index + 1 == count,
                data_base64: STANDARD.encode(bytes),
            };
            if serde_json::to_vec(&chunk).map_err(|_| denied())?.len()
                > super::wire::MAX_CONTROL - 4096
            {
                return Err(denied());
            }
            Ok(chunk)
        })
        .collect()
}
/// One actual authenticated bridge call, never a global/cross-connection cache.
/// Any interrupted/foreign/gapped/duplicate chunk poisons it; reconnect cannot
/// replay a remainder. Caller uses its existing whole-frame deadline as well.
pub struct Assembly {
    request: String,
    revision: i64,
    prompt: String,
    total: usize,
    index: usize,
    bytes: Vec<u8>,
    deadline: Instant,
    closed: bool,
}
impl std::fmt::Debug for Assembly {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeContentAssembly")
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}
impl Assembly {
    pub fn new(
        request: String,
        revision: i64,
        prompt: String,
        total: usize,
        budget: Duration,
    ) -> io::Result<Self> {
        if !id(&request)
            || !id(&prompt)
            || revision < 0
            || total == 0
            || total > BODY
            || budget.is_zero()
            || budget > Duration::from_secs(2)
        {
            return Err(denied());
        }
        let deadline = Instant::now().checked_add(budget).ok_or_else(denied)?;
        Ok(Self {
            request,
            revision,
            prompt,
            total,
            index: 0,
            bytes: Vec::with_capacity(total),
            deadline,
            closed: false,
        })
    }
    /// Only returns bytes after the whole bound call has completed. It is not an
    /// ACK: downstream durable receipt validation remains mandatory.
    pub fn push(&mut self, frame: ContentChunk) -> io::Result<Option<Vec<u8>>> {
        let result = self.push_inner(frame);
        if result.is_err() {
            self.closed = true;
            self.bytes.clear();
        }
        result
    }
    fn push_inner(&mut self, frame: ContentChunk) -> io::Result<Option<Vec<u8>>> {
        if self.closed
            || Instant::now() >= self.deadline
            || frame.request != self.request
            || frame.revision != self.revision
            || frame.prompt != self.prompt
            || frame.total_bytes as usize != self.total
            || frame.index as usize != self.index
            || frame.data_base64.len() > CHUNK.div_ceil(3) * 4
        {
            return Err(denied());
        }
        let remaining = self.total - self.bytes.len();
        let expected = remaining.min(CHUNK);
        if frame.final_chunk != (remaining <= CHUNK) {
            return Err(denied());
        }
        let decoded = STANDARD.decode(&frame.data_base64).map_err(|_| denied())?;
        if decoded.len() != expected || STANDARD.encode(&decoded) != frame.data_base64 {
            return Err(denied());
        }
        self.bytes.extend_from_slice(&decoded);
        self.index += 1;
        if frame.final_chunk {
            if std::str::from_utf8(&self.bytes).is_err() {
                return Err(denied());
            }
            self.closed = true;
            return Ok(Some(std::mem::take(&mut self.bytes)));
        }
        Ok(None)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn ids() -> (String, String) {
        (
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
        )
    }
    #[test]
    fn legal_mib_utf8_body_never_exceeds_encoded_control_and_has_no_partial_release() {
        let (r, p) = ids();
        let body = "🛫\0".repeat(209715);
        let frames = encode(&r, 4, &p, &body).unwrap();
        assert!(frames.len() > 1);
        let mut assembly = Assembly::new(r, 4, p, body.len(), Duration::from_secs(2)).unwrap();
        let mut reconstructed = None;
        let last = frames.len() - 1;
        for (i, frame) in frames.into_iter().enumerate() {
            assert!(serde_json::to_vec(&frame).unwrap().len() < super::super::wire::MAX_CONTROL);
            let part = assembly.push(frame).unwrap();
            assert_eq!(part.is_some(), i == last);
            if part.is_some() {
                reconstructed = part;
            }
        }
        assert_eq!(reconstructed.unwrap(), body.as_bytes());
    }
    #[test]
    fn foreign_gap_or_expired_call_poison_without_replay_or_partial_effect() {
        for fault in [
            "request",
            "revision",
            "prompt",
            "gap",
            "final",
            "expired",
            "duplicate",
        ] {
            let (r, p) = ids();
            let body = "a".repeat(CHUNK + 1);
            let frames = encode(&r, 4, &p, &body).unwrap();
            let mut assembly = Assembly::new(r, 4, p, body.len(), Duration::from_secs(2)).unwrap();
            let mut frame = frames[0].clone();
            match fault {
                "request" => frame.request = uuid::Uuid::new_v4().to_string(),
                "revision" => frame.revision += 1,
                "prompt" => frame.prompt = uuid::Uuid::new_v4().to_string(),
                "gap" => frame.index = 1,
                "final" => frame.final_chunk = true,
                "expired" => assembly.deadline = Instant::now(),
                "duplicate" => {
                    assert!(assembly.push(frame.clone()).unwrap().is_none());
                }
                _ => unreachable!(),
            }
            assert!(assembly.push(frame).is_err());
            assert!(assembly.push(frames[1].clone()).is_err());
            assert!(assembly.bytes.is_empty());
        }
    }
}
