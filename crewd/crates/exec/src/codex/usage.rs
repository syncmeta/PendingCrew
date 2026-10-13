//! Lossless members of the already-parsed tokenUsage object, independent of compaction.
use crate::session::{RUNNER_USAGE_MAX_ID_BYTES, RUNNER_USAGE_MAX_JSON_BYTES, RunnerUsageSnapshot};
use serde_json::Value;

pub fn notification(
    params: &Value,
    expected_thread: &str,
) -> Result<Option<RunnerUsageSnapshot>, &'static str> {
    let thread = params
        .get("threadId")
        .and_then(Value::as_str)
        .ok_or("usage notification has no explicit thread identity")?;
    if thread != expected_thread {
        return Ok(None);
    }
    if thread.is_empty() || thread.len() > RUNNER_USAGE_MAX_ID_BYTES {
        return Err("usage thread identity exceeds the supported bound");
    }
    let turn = match params.get("turnId") {
        None | Some(Value::Null) => None,
        Some(Value::String(v)) if v.len() <= RUNNER_USAGE_MAX_ID_BYTES => Some(v.clone()),
        _ => return Err("usage turn identity is invalid or exceeds the supported bound"),
    };
    let raw = params
        .get("tokenUsage")
        .filter(|v| v.is_object())
        .ok_or("usage notification has no object snapshot")?;
    // Stop serialization at the limit, rather than allocate a giant unknown member first.
    struct Bounded(Vec<u8>);
    impl std::io::Write for Bounded {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            if data.len() > RUNNER_USAGE_MAX_JSON_BYTES - self.0.len() {
                return Err(std::io::Error::other("usage bound"));
            }
            self.0.extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Bounded(Vec::new());
    serde_json::to_writer(&mut output, raw)
        .map_err(|_| "usage snapshot exceeds the supported 64 KiB bound")?;
    let json = String::from_utf8(output.0).expect("JSON serializer emits UTF-8");
    Ok(Some(RunnerUsageSnapshot {
        provider: "codex".into(),
        thread_id: thread.into(),
        turn_id: turn,
        token_usage_json: json,
    }))
}
