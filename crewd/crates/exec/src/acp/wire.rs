//! ACP JSON control and native terminal framing never share a decoder or a byte channel.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
pub const MAX_CONTROL: usize = 1_048_576;
pub const MAX_TERMINAL_PAYLOAD: usize = 65_536;
pub const HEADER_LEN: usize = 25;
pub const NATIVE_NAMESPACE: &str = "pendingcrew.native";
pub const EXTENSION_VERSION: u32 = 1;
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    Oversize,
    Malformed,
    WrongVersion,
    CapabilityMissing(&'static str),
    StreamMismatch,
    SequenceGap,
    UnknownResponse,
    DuplicateRequest,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeCapabilities {
    pub version: u32,
    pub controlling_pty: bool,
    pub separate_terminal_stream: bool,
    pub confirmed_delivery: bool,
    pub native_decisions: bool,
    pub raw_input: bool,
    pub resize: bool,
    pub native_identity: bool,
    pub group_stop: bool,
}
impl NativeCapabilities {
    /// Explicit synthetic peer fixture only; not a production capability claim.
    #[cfg(any(test, feature = "testkit"))]
    pub fn synthetic_verified() -> Self {
        Self {
            version: EXTENSION_VERSION,
            controlling_pty: true,
            separate_terminal_stream: true,
            confirmed_delivery: true,
            native_decisions: true,
            raw_input: true,
            resize: true,
            native_identity: true,
            group_stop: true,
        }
    }
    pub fn require(&self) -> Result<(), WireError> {
        if self.version != EXTENSION_VERSION {
            return Err(WireError::WrongVersion);
        }
        for (ok, name) in [
            (self.controlling_pty, "controllingPty"),
            (self.separate_terminal_stream, "separateTerminalStream"),
            (self.confirmed_delivery, "confirmedDelivery"),
            (self.native_decisions, "nativeDecisions"),
            (self.raw_input, "rawInput"),
            (self.resize, "resize"),
            (self.native_identity, "nativeIdentity"),
            (self.group_stop, "groupStop"),
        ] {
            if !ok {
                return Err(WireError::CapabilityMissing(name));
            }
        }
        Ok(())
    }
}
pub fn negotiate(result: &Value) -> Result<NativeCapabilities, WireError> {
    if result["protocolVersion"] != 1 {
        return Err(WireError::WrongVersion);
    }
    let caps = result["agentCapabilities"]["_meta"][NATIVE_NAMESPACE].clone();
    let c: NativeCapabilities = serde_json::from_value(caps)
        .map_err(|_| WireError::CapabilityMissing("pendingcrew.native"))?;
    c.require()?;
    Ok(c)
}
#[derive(Clone, PartialEq)]
pub enum Control {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Response {
        id: Value,
        result: Result<Value, Value>,
    },
}
impl std::fmt::Debug for Control {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request { id, method, .. } => f
                .debug_struct("Request")
                .field("id", id)
                .field("method", method)
                .field("params", &"<opaque>")
                .finish(),
            Self::Notification { method, .. } => f
                .debug_struct("Notification")
                .field("method", method)
                .field("params", &"<opaque>")
                .finish(),
            Self::Response { id, result } => f
                .debug_struct("Response")
                .field("id", id)
                .field("success", &result.is_ok())
                .finish(),
        }
    }
}
pub fn control(bytes: &[u8]) -> Result<Control, WireError> {
    if bytes.len() > MAX_CONTROL {
        return Err(WireError::Oversize);
    }
    let v: Value = serde_json::from_slice(bytes).map_err(|_| WireError::Malformed)?;
    if !v.is_object() || v["jsonrpc"] != "2.0" {
        return Err(WireError::Malformed);
    }
    let valid_id = |id: &Value| {
        id.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 128) || id.is_i64() || id.is_u64()
    };
    if let Some(method) = v.get("method") {
        let method = method.as_str().ok_or(WireError::Malformed)?.to_string();
        if method.is_empty()
            || method.len() > 256
            || v.get("result").is_some()
            || v.get("error").is_some()
        {
            return Err(WireError::Malformed);
        }
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        if !params.is_null() && !params.is_object() {
            return Err(WireError::Malformed);
        }
        return match v.get("id") {
            Some(id) if valid_id(id) => Ok(Control::Request {
                id: id.clone(),
                method,
                params,
            }),
            Some(_) => Err(WireError::Malformed),
            None => Ok(Control::Notification { method, params }),
        };
    }
    let id = v
        .get("id")
        .filter(|v| valid_id(v))
        .ok_or(WireError::Malformed)?
        .clone();
    match (v.get("result"), v.get("error")) {
        (Some(r), None) => Ok(Control::Response {
            id,
            result: Ok(r.clone()),
        }),
        (None, Some(e)) if e.is_object() && e["code"].is_i64() && e["message"].is_string() => {
            Ok(Control::Response {
                id,
                result: Err(e.clone()),
            })
        }
        _ => Err(WireError::Malformed),
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TerminalKind {
    Data = 1,
    SnapshotStart = 2,
    SnapshotChunk = 3,
    SnapshotEnd = 4,
    Resync = 5,
    Closed = 6,
}
#[derive(Clone, PartialEq, Eq)]
pub struct TerminalFrame {
    pub epoch: u64,
    pub sequence: u64,
    pub kind: TerminalKind,
    pub bytes: Vec<u8>,
}
impl std::fmt::Debug for TerminalFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalFrame")
            .field("epoch", &self.epoch)
            .field("sequence", &self.sequence)
            .field("kind", &self.kind)
            .field("byte_count", &self.bytes.len())
            .finish()
    }
}
impl TerminalFrame {
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        if self.bytes.len() > MAX_TERMINAL_PAYLOAD {
            return Err(WireError::Oversize);
        }
        if self.epoch == 0 || self.sequence == 0 {
            return Err(WireError::Malformed);
        }
        let mut b = Vec::with_capacity(HEADER_LEN + self.bytes.len());
        b.extend_from_slice(b"PCPT");
        b.extend_from_slice(&(self.bytes.len() as u32).to_be_bytes());
        b.extend_from_slice(&self.epoch.to_be_bytes());
        b.extend_from_slice(&self.sequence.to_be_bytes());
        b.push(self.kind as u8);
        b.extend_from_slice(&self.bytes);
        Ok(b)
    }
}
#[derive(Default)]
pub struct TerminalDecoder {
    buffer: Vec<u8>,
}
impl TerminalDecoder {
    pub fn push(&mut self, mut bytes: &[u8]) -> Result<Vec<TerminalFrame>, WireError> {
        let mut frames = Vec::new();
        while !bytes.is_empty() {
            let needed = if self.buffer.len() < HEADER_LEN {
                HEADER_LEN
            } else {
                let n = u32::from_be_bytes(self.buffer[4..8].try_into().expect("header")) as usize;
                if n > MAX_TERMINAL_PAYLOAD {
                    self.buffer.clear();
                    return Err(WireError::Oversize);
                }
                HEADER_LEN + n
            };
            let take = (needed - self.buffer.len()).min(bytes.len());
            self.buffer.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.buffer.len() < needed {
                continue;
            }
            if &self.buffer[..4] != b"PCPT" {
                self.buffer.clear();
                return Err(WireError::StreamMismatch);
            }
            let n = u32::from_be_bytes(self.buffer[4..8].try_into().expect("header")) as usize;
            if n > MAX_TERMINAL_PAYLOAD {
                self.buffer.clear();
                return Err(WireError::Oversize);
            }
            if self.buffer.len() < HEADER_LEN + n {
                continue;
            }
            let epoch = u64::from_be_bytes(self.buffer[8..16].try_into().expect("epoch"));
            let sequence = u64::from_be_bytes(self.buffer[16..24].try_into().expect("sequence"));
            let kind = match self.buffer[24] {
                1 => TerminalKind::Data,
                2 => TerminalKind::SnapshotStart,
                3 => TerminalKind::SnapshotChunk,
                4 => TerminalKind::SnapshotEnd,
                5 => TerminalKind::Resync,
                6 => TerminalKind::Closed,
                _ => {
                    self.buffer.clear();
                    return Err(WireError::Malformed);
                }
            };
            if epoch == 0 || sequence == 0 {
                self.buffer.clear();
                return Err(WireError::Malformed);
            }
            frames.push(TerminalFrame {
                epoch,
                sequence,
                kind,
                bytes: self.buffer[HEADER_LEN..].to_vec(),
            });
            self.buffer.clear();
            if frames.len() > 1024 {
                return Err(WireError::Oversize);
            }
        }
        Ok(frames)
    }
}
/// Epoch and sequence validation never silently splices a new incarnation into an old screen.
pub struct StreamOrder {
    pub epoch: u64,
    last: u64,
}
impl StreamOrder {
    pub fn new(epoch: u64) -> Self {
        Self { epoch, last: 0 }
    }
    /// Only call after a requested coherent screen snapshot was fully assembled.
    /// A raw Resync frame is not itself authority to skip missing terminal bytes.
    pub fn snapshot_baseline(&mut self, epoch: u64, sequence: u64) -> Result<(), WireError> {
        if epoch != self.epoch || sequence < self.last {
            return Err(WireError::StreamMismatch);
        }
        self.last = sequence;
        Ok(())
    }
    pub fn accept(&mut self, f: &TerminalFrame) -> Result<bool, WireError> {
        if f.epoch != self.epoch {
            return Err(WireError::StreamMismatch);
        }
        if f.sequence <= self.last {
            return Ok(false);
        }
        if self.last.checked_add(1) != Some(f.sequence) {
            return Err(WireError::SequenceGap);
        }
        self.last = f.sequence;
        Ok(true)
    }
}
/// Request ownership is separate from terminal/history replay. Unknown or old responses cannot finish a turn.
#[derive(Default)]
pub struct Requests {
    pending: BTreeMap<String, (u64, String)>,
    settled: BTreeMap<String, u64>,
}
impl Requests {
    pub fn begin(&mut self, id: String, epoch: u64, method: String) -> Result<(), WireError> {
        if epoch == 0
            || id.is_empty()
            || id.len() > 128
            || method.is_empty()
            || method.len() > 256
            || self.pending.len() >= 128
        {
            return Err(WireError::Oversize);
        }
        if self.pending.contains_key(&id) || self.settled.contains_key(&id) {
            return Err(WireError::DuplicateRequest);
        }
        self.pending.insert(id, (epoch, method));
        Ok(())
    }
    pub fn settle(&mut self, id: &str, epoch: u64) -> Result<Option<String>, WireError> {
        if self.settled.get(id) == Some(&epoch) {
            return Ok(None);
        }
        let (owner, method) = self.pending.get(id).ok_or(WireError::UnknownResponse)?;
        if *owner != epoch {
            return Err(WireError::StreamMismatch);
        }
        let method = method.clone();
        self.pending.remove(id);
        self.settled.insert(id.into(), epoch);
        if self.settled.len() > 4096 {
            let key = self
                .settled
                .first_key_value()
                .expect("bounded map")
                .0
                .clone();
            self.settled.remove(&key);
        }
        Ok(Some(method))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn payloads_never_appear_in_debug_and_snapshot_requires_current_epoch() {
        let message = control(br#"{"jsonrpc":"2.0","id":1,"method":"session/prompt","params":{"text":"SECRET-PROMPT"}}"#).unwrap();
        assert!(!format!("{message:?}").contains("SECRET-PROMPT"));
        let mut stream = StreamOrder::new(4);
        assert_eq!(
            stream.snapshot_baseline(3, 99),
            Err(WireError::StreamMismatch)
        );
        stream.snapshot_baseline(4, 99).unwrap();
        assert!(
            stream
                .accept(&TerminalFrame {
                    epoch: 4,
                    sequence: 100,
                    kind: TerminalKind::Data,
                    bytes: vec![]
                })
                .unwrap()
        );
        assert!(
            control(
                serde_json::to_string(
                    &json!({"jsonrpc":"2.0","id":"x".repeat(129),"method":"initialize"})
                )
                .unwrap()
                .as_bytes()
            )
            .is_err()
        );
    }
    #[test]
    fn capability_requires_real_native_extensions_not_just_acp_handshake() {
        let mut v = json!({"protocolVersion":1,"agentCapabilities":{"loadSession":true}});
        assert!(matches!(
            negotiate(&v),
            Err(WireError::CapabilityMissing(_))
        ));
        v["agentCapabilities"]["_meta"] =
            json!({NATIVE_NAMESPACE:NativeCapabilities::synthetic_verified()});
        assert!(negotiate(&v).is_ok());
        v["agentCapabilities"]["_meta"][NATIVE_NAMESPACE]["groupStop"] = json!(false);
        assert_eq!(
            negotiate(&v),
            Err(WireError::CapabilityMissing("groupStop"))
        );
    }
    #[test]
    fn terminal_ansi_utf8_fragments_never_enter_json_control() {
        let frame = TerminalFrame {
            epoch: 2,
            sequence: 1,
            kind: TerminalKind::Data,
            bytes: "\x1b[32m原生🙂\x1b[0m".as_bytes().to_vec(),
        };
        let encoded = frame.encode().unwrap();
        let mut d = TerminalDecoder::default();
        let mut seen = Vec::new();
        for b in encoded.chunks(1) {
            seen.extend(d.push(b).unwrap())
        }
        assert_eq!(seen, vec![frame]);
        assert_eq!(control(&encoded), Err(WireError::Malformed));
        assert!(
            TerminalDecoder::default()
                .push(br#"{"jsonrpc":"2.0","method":"session/update"}"#)
                .is_err()
        );
    }
    #[test]
    fn sequence_and_generation_do_not_allow_late_stream_splicing() {
        let mut s = StreamOrder::new(3);
        let mut f = TerminalFrame {
            epoch: 3,
            sequence: 1,
            kind: TerminalKind::Data,
            bytes: vec![0],
        };
        assert_eq!(s.accept(&f), Ok(true));
        assert_eq!(s.accept(&f), Ok(false));
        f.sequence = 3;
        assert_eq!(s.accept(&f), Err(WireError::SequenceGap));
        f.epoch = 2;
        assert_eq!(s.accept(&f), Err(WireError::StreamMismatch));
    }
    #[test]
    fn duplicate_and_old_response_are_not_a_second_turn_completion() {
        let mut r = Requests::default();
        r.begin("p1".into(), 4, "session/prompt".into()).unwrap();
        assert_eq!(r.settle("p1", 3), Err(WireError::StreamMismatch));
        assert_eq!(r.settle("other", 4), Err(WireError::UnknownResponse));
        assert_eq!(r.settle("p1", 4), Ok(Some("session/prompt".into())));
        assert_eq!(r.settle("p1", 4), Ok(None));
        assert_eq!(
            r.begin("p1".into(), 4, "session/prompt".into()),
            Err(WireError::DuplicateRequest)
        );
    }
    #[test]
    fn control_rejects_ambiguous_and_non_protocol_frames() {
        for v in [
            json!({"jsonrpc":"2.0","id":1,"result":{},"error":{"code":1,"message":"x"}}),
            json!({"jsonrpc":"1.0","id":1,"result":{}}),
            json!({"jsonrpc":"2.0","id":null,"method":"session/prompt"}),
        ] {
            assert_eq!(
                control(&serde_json::to_vec(&v).unwrap()),
                Err(WireError::Malformed)
            )
        }
        assert!(matches!(
            control(br#"{"jsonrpc":"2.0","id":"p1","method":"session/prompt","params":{}}"#),
            Ok(Control::Request { .. })
        ));
    }
}
