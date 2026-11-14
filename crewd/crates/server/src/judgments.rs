//! Decision planning is separate from effects. All public/model names are opaque
//! request-local aliases; only a current authority snapshot can resolve them.
mod admin_ask_groups;
mod admin_cases;
mod configuration;
mod coordination;
mod reassignment;
mod requirement_similarity;
mod retirement;
pub(crate) use configuration::handle;
use crewd_collab::judgments::InputSnapshot;
use crewd_jev::provider::{
    Batch, DecisionConfig, Judgment, JudgmentAnswer, JudgmentResult, ProbabilitySource,
};
use serde_json::json;
use std::collections::BTreeMap;

// Reserve at most 16 KiB for additional member context across the 30 wake
// candidates. Internal snapshots retain full revisions; only provider text is cut.
fn member_input(m: &crewd_collab::judgments::Recipient, count: usize) -> serde_json::Value {
    let budget = 16_384 / count.max(1);
    let mut limit = 120;
    let context = loop {
        let sample = m.work_context["sample"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        let text_cut = m
            .title
            .iter()
            .chain(m.brief.iter())
            .any(|v| v.chars().count() > limit);
        let work_cut = m.work_context["truncated"].as_bool().unwrap_or(false)
            || sample.len() > 1
            || sample
                .iter()
                .take(1)
                .any(|w| w["title"].as_str().unwrap_or_default().chars().count() > limit);
        let v = json!({
            "title":m.title.as_ref().map(|v|v.chars().take(limit).collect::<String>()),
            "brief":m.brief.as_ref().map(|v|v.chars().take(limit).collect::<String>()),
            "context_truncated":text_cut || work_cut,
            "current_work":{"scope":m.work_context["scope"],"sample_only":true,
                "truncated":work_cut,"sample":sample.iter().take(1).map(|w|json!({
                    "title":w["title"].as_str().unwrap_or_default().chars().take(limit).collect::<String>(),
                    "status":w["status"],"blocked":w["blocked"]})).collect::<Vec<_>>()}
        });
        if serde_json::to_vec(&v).is_ok_and(|bytes| bytes.len() <= budget) || limit == 0 {
            break v;
        }
        limit /= 2;
    };
    let mut member = context;
    member["name"] = json!(m.name);
    member["captain"] = json!(m.captain);
    member
}

pub(crate) fn batch(s: &InputSnapshot, wake: bool) -> Batch {
    let mut questions = BTreeMap::new();
    if s.kind == "route" && !s.recipients.is_empty() {
        questions.insert("route".into(),Judgment::Choice {instructions:json!("Choose the member to receive this human requirement. This does not assign a Work owner or authorize tools. current_work is only a bounded sample in the stated source scope, not a workload inventory; an empty sample does not mean idle."),
            candidates:s.recipients.iter().take(crewd_jev::decisions::MAX_MEMBERS).enumerate().map(|(i,m)|(format!("s{i}"),member_input(m,s.recipients.len()))).collect()});
    }
    if s.kind == "message" {
        if wake {
            for (i, m) in s.recipients.iter().enumerate() {
                questions.insert(format!("wake_s{i}"),Judgment::Predicate{instructions:json!({"question":"Should this member wake now for the human message? current_work is only a bounded sample in the stated source scope, not a workload inventory; an empty sample does not mean idle.","member":member_input(m,s.recipients.len())}),yes:json!("needs to act now"),no:json!("does not need to act now")});
            }
        }
        if !s.links.is_empty() {
            let mut candidates: Vec<_> = s
                .links
                .iter()
                .enumerate()
                .map(|(i, c)| (format!("o{i}"), json!({"kind":c.kind,"summary":c.summary})))
                .collect();
            candidates.push(("none".into(), json!("no relation")));
            questions.insert("attribute".into(),Judgment::Choice{instructions:json!("Choose the existing object this message concerns, or none. This only records a reference; it never answers a question, changes visibility or assigns work."),candidates});
        }
    }
    Batch {
        state_version: s.version.clone(),
        state: json!({"text":s.text.chars().take(crewd_jev::decisions::MAX_TEXT_CHARS).collect::<String>()}),
        questions,
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Selection {
    pub recipients: Vec<String>,
    pub link: Option<usize>,
    pub adopted: bool,
}
/// Generated estimates can only be suggestions; they never enter the native
/// threshold adoption path, even if a future adapter supplies identical JSON.
pub(crate) fn selection(
    s: &InputSnapshot,
    cfg: &DecisionConfig,
    result: &JudgmentResult,
) -> Selection {
    let fallback = || Selection {
        recipients: s
            .captain
            .as_ref()
            .filter(|c| s.recipients.iter().any(|m| &m.id == *c))
            .cloned()
            .into_iter()
            .collect(),
        link: None,
        adopted: false,
    };
    let JudgmentResult::Answered {
        answers,
        probability_source: ProbabilitySource::Native,
        suggestion_only: false,
        ..
    } = result
    else {
        return fallback();
    };
    if s.kind == "route" {
        let member = match answers.get("route") {
            Some(JudgmentAnswer::Choice {
                key, confidence, ..
            }) if *confidence >= cfg.choice_min => alias_index(
                key,
                's',
                s.recipients.len().min(crewd_jev::decisions::MAX_MEMBERS),
            )
            .and_then(|i| s.recipients.get(i)),
            _ => None,
        };
        return member
            .map(|m| Selection {
                recipients: vec![m.id.clone()],
                link: None,
                adopted: true,
            })
            .unwrap_or_else(fallback);
    }

    let mut out = Selection {
        recipients: vec![],
        link: None,
        adopted: true,
    };
    for (i, m) in s.recipients.iter().enumerate() {
        match answers.get(&format!("wake_s{i}")) {
            Some(JudgmentAnswer::Predicate { probability }) if *probability >= cfg.wake_yes => {
                out.recipients.push(m.id.clone())
            }
            _ => {}
        }
    }
    if out.recipients.is_empty() {
        out.adopted = false;
    }
    out.link = match answers.get("attribute") {
        Some(JudgmentAnswer::Choice {
            key, confidence, ..
        }) if *confidence >= cfg.choice_min => alias_index(key, 'o', s.links.len()),
        _ => None,
    };
    out
}

use crate::AppState;
use crewd_collab::{Change, judgments as facts};
use crewd_jev::provider::DecisionProvider;
use crewd_proto::judgments::*;
use crewd_wake::{Ms, plan};
fn alias_index(key: &str, prefix: char, len: usize) -> Option<usize> {
    key.strip_prefix(prefix)
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|i| *i < len && key == format!("{prefix}{i}"))
}

use std::path::PathBuf;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Mutex, Semaphore, broadcast};
use tokio::task::{JoinHandle, JoinSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum FactoryFailure {
    MissingRevision,
    MissingConfiguration,
    Disabled,
    InvalidConfiguration,
    StateUnavailable,
    CredentialMissing,
    Credential(configuration::KeyLoadFailure),
    ClientInitialization,
}

#[derive(serde::Serialize)]
struct SafeDecisionDiagnostic {
    event: &'static str,
    case_id: String,
    decision_id: String,
    snapshot: String,
    factory: Option<FactoryFailure>,
    failure: Option<crewd_jev::error::FailureDiagnostic>,
    attempts: Option<u32>,
    latency_ms: u64,
}
impl SafeDecisionDiagnostic {
    fn new(
        snapshot: &crewd_collab::admin_cases::DecisionSnapshot,
        factory: Option<FactoryFailure>,
        failure: Option<crewd_jev::error::FailureDiagnostic>,
        attempts: Option<u32>,
        latency_ms: u64,
    ) -> Option<Self> {
        if uuid::Uuid::parse_str(&snapshot.decision_id).is_err()
            || snapshot.id.len() != 22
            || !snapshot.id.starts_with("AC")
            || !snapshot.id[2..].bytes().all(|b| b.is_ascii_digit())
            || snapshot.fingerprint.len() != 64
            || !snapshot.fingerprint.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return None;
        }
        Some(Self {
            event: "jev_decision_diagnostic_v1",
            case_id: snapshot.id.clone(),
            decision_id: snapshot.decision_id.clone(),
            snapshot: snapshot.fingerprint.clone(),
            factory,
            failure,
            attempts,
            latency_ms,
        })
    }
}

const DIAGNOSTIC_CURRENT: &str = "jev-decision-diagnostics-v1.jsonl";
const DIAGNOSTIC_PREVIOUS: &str = "jev-decision-diagnostics-v1.jsonl.1";
const DIAGNOSTIC_FILE_LIMIT: usize = 128 * 1024;
const DIAGNOSTIC_HEADER: &[u8] = b"{\"event\":\"jev_diagnostic_file_v1\"}\n";
fn diagnostic_invalid() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "safe diagnostic file unavailable",
    )
}

// Validate the original bytes before Value can collapse duplicate object keys.
// The caller bounds bytes, and serde_json retains its normal nesting limit.
struct UniqueDiagnosticJson;
impl<'de> serde::Deserialize<'de> for UniqueDiagnosticJson {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueDiagnosticJson;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON without duplicate object keys")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut keys = std::collections::BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !keys.insert(key) {
                        return Err(serde::de::Error::custom("duplicate diagnostic key"));
                    }
                    map.next_value::<UniqueDiagnosticJson>()?;
                }
                Ok(UniqueDiagnosticJson)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                while sequence.next_element::<UniqueDiagnosticJson>()?.is_some() {}
                Ok(UniqueDiagnosticJson)
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(UniqueDiagnosticJson)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(UniqueDiagnosticJson)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(UniqueDiagnosticJson)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(UniqueDiagnosticJson)
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(UniqueDiagnosticJson)
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueDiagnosticJson)
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

fn safe_diagnostic_line(line: &[u8]) -> bool {
    if line.len() > 2048 || !line.ends_with(b"\n") {
        return false;
    }
    if serde_json::from_slice::<UniqueDiagnosticJson>(line).is_err() {
        return false;
    }
    let Ok(serde_json::Value::Object(object)) = serde_json::from_slice(line) else {
        return false;
    };
    let names: std::collections::BTreeSet<_> = object.keys().map(String::as_str).collect();
    match object.get("event").and_then(serde_json::Value::as_str) {
        Some("jev_diagnostic_file_v1") => names == std::collections::BTreeSet::from(["event"]),
        Some("jev_diagnostic_open_v1") => {
            names == std::collections::BTreeSet::from(["event", "runtime_binding_id"])
                && object["runtime_binding_id"]
                    .as_str()
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
        }
        Some("jev_diagnostic_shutdown_v1") => {
            names == std::collections::BTreeSet::from(["event", "dropped", "runtime_binding_id"])
                && object.get("dropped").is_some_and(serde_json::Value::is_u64)
                && object["runtime_binding_id"]
                    .as_str()
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
        }
        Some("jev_decision_diagnostic_v1") => {
            names
                == std::collections::BTreeSet::from([
                    "event",
                    "case_id",
                    "decision_id",
                    "snapshot",
                    "factory",
                    "failure",
                    "attempts",
                    "latency_ms",
                    "runtime_binding_id",
                ])
                && object["case_id"].as_str().is_some_and(|id| {
                    id.len() == 22
                        && id.starts_with("AC")
                        && id[2..].bytes().all(|b| b.is_ascii_digit())
                })
                && object["decision_id"]
                    .as_str()
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
                && object["snapshot"]
                    .as_str()
                    .is_some_and(|id| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()))
                && serde_json::from_value::<Option<FactoryFailure>>(object["factory"].clone())
                    .is_ok()
                && serde_json::from_value::<Option<crewd_jev::error::FailureDiagnostic>>(
                    object["failure"].clone(),
                )
                .is_ok_and(|diagnostic| {
                    diagnostic
                        .is_none_or(|d| d.status.is_none_or(|status| (100..=599).contains(&status)))
                })
                && serde_json::from_value::<Option<u32>>(object["attempts"].clone()).is_ok()
                && object["latency_ms"].is_u64()
                && object["runtime_binding_id"]
                    .as_str()
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
        }
        _ => false,
    }
}
struct PrivateDiagnosticFiles {
    directory: std::fs::File,
    current: std::fs::File,
}
impl PrivateDiagnosticFiles {
    fn trusted(file: &std::fs::File, directory: bool) -> std::io::Result<()> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let meta = file.metadata()?;
        if meta.uid() != unsafe { libc::geteuid() }
            || (directory && (!meta.is_dir() || meta.permissions().mode() & 0o077 != 0))
            || (!directory
                && (!meta.is_file()
                    || meta.nlink() != 1
                    || meta.permissions().mode() & 0o777 != 0o600))
        {
            return Err(diagnostic_invalid());
        }
        Ok(())
    }
    fn contents(file: &mut std::fs::File) -> std::io::Result<Vec<u8>> {
        use std::io::{Read, Seek};
        Self::trusted(file, false)?;
        if file.metadata()?.len() > DIAGNOSTIC_FILE_LIMIT as u64 {
            return Err(diagnostic_invalid());
        }
        file.rewind()?;
        let mut bytes = Vec::new();
        file.take((DIAGNOSTIC_FILE_LIMIT + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > DIAGNOSTIC_FILE_LIMIT
            || !bytes.starts_with(DIAGNOSTIC_HEADER)
            || !bytes.ends_with(b"\n")
            || !bytes
                .split_inclusive(|b| *b == b'\n')
                .all(safe_diagnostic_line)
        {
            return Err(diagnostic_invalid());
        }
        Ok(bytes)
    }
    fn open_file(directory: &std::fs::File, name: &str) -> std::io::Result<std::fs::File> {
        use std::io::Write;
        use std::os::fd::{AsRawFd, FromRawFd};
        let name = std::ffi::CString::new(name).map_err(|_| diagnostic_invalid())?;
        let flags =
            libc::O_RDWR | libc::O_APPEND | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
        let mut fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        let mut created = false;
        if fd < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
            fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    flags | libc::O_CREAT | libc::O_EXCL,
                    0o600,
                )
            };
            created = fd >= 0;
        }
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        Self::trusted(&file, false)?;
        if created {
            file.write_all(DIAGNOSTIC_HEADER)?;
        }
        Self::contents(&mut file)?;
        Ok(file)
    }
    fn open(data: &std::path::Path) -> std::io::Result<Self> {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        if !data.is_absolute() || data.canonicalize()? != data {
            return Err(diagnostic_invalid());
        }
        let before = std::fs::symlink_metadata(data)?;
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(data)?;
        Self::trusted(&directory, true)?;
        let opened = directory.metadata()?;
        let after = std::fs::symlink_metadata(data)?;
        if !before.is_dir()
            || before.file_type().is_symlink()
            || (before.dev(), before.ino()) != (opened.dev(), opened.ino())
            || (after.dev(), after.ino()) != (opened.dev(), opened.ino())
            || data.canonicalize()? != data
        {
            return Err(diagnostic_invalid());
        }
        // Refuse an unsafe/unknown pre-existing second file before touching either log.
        use std::os::fd::AsRawFd;
        let previous =
            std::ffi::CString::new(DIAGNOSTIC_PREVIOUS).map_err(|_| diagnostic_invalid())?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        let exists = unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                previous.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if exists == 0 {
            let _ = Self::open_file(&directory, DIAGNOSTIC_PREVIOUS)?;
        } else if std::io::Error::last_os_error().kind() != std::io::ErrorKind::NotFound {
            return Err(std::io::Error::last_os_error());
        }
        let current = Self::open_file(&directory, DIAGNOSTIC_CURRENT)?;
        Ok(Self { directory, current })
    }
}
impl std::io::Write for PrivateDiagnosticFiles {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if !safe_diagnostic_line(bytes) {
            return Err(diagnostic_invalid());
        }
        let current = Self::contents(&mut self.current)?;
        if current.len() + bytes.len() > DIAGNOSTIC_FILE_LIMIT {
            let mut previous = Self::open_file(&self.directory, DIAGNOSTIC_PREVIOUS)?;
            // Only validated, owned open inodes are reset; never unlink/rename a path.
            previous.set_len(0)?;
            previous.write_all(&current)?;
            self.current.set_len(0)?;
            self.current.write_all(DIAGNOSTIC_HEADER)?;
        }
        self.current.write_all(bytes)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut self.current)
    }
}

/// Bounded, best-effort observation. No I/O or blocking send on the decision path.
struct DiagnosticSink {
    sender: std::sync::Mutex<Option<std::sync::mpsc::SyncSender<Vec<u8>>>>,
    worker: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
    dropped: Arc<std::sync::atomic::AtomicU64>,
    failed: Arc<AtomicBool>,
    runtime_binding_id: String,
}
impl DiagnosticSink {
    fn new(data: PathBuf, runtime_binding_id: String) -> Self {
        Self::with_factory(runtime_binding_id, move || {
            Ok(Box::new(PrivateDiagnosticFiles::open(&data)?) as Box<dyn std::io::Write + Send>)
        })
    }
    #[cfg(test)]
    fn with_writer(writer: Box<dyn std::io::Write + Send>) -> Self {
        Self::with_factory("00000000-0000-0000-0000-000000000001".into(), move || {
            Ok(writer)
        })
    }
    fn with_factory(
        runtime_binding_id: String,
        factory: impl FnOnce() -> std::io::Result<Box<dyn std::io::Write + Send>> + Send + 'static,
    ) -> Self {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<Vec<u8>>(64);
        let dropped = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let failed = Arc::new(AtomicBool::new(false));
        let worker_failed = failed.clone();
        let worker_dropped = dropped.clone();
        let identity = runtime_binding_id.clone();
        let worker = std::thread::Builder::new().name("jev-safe-diagnostics".into()).spawn(move || {
            let write=|| -> std::io::Result<()> {
                if uuid::Uuid::parse_str(&identity).is_err() { return Err(diagnostic_invalid()); }
                let mut writer=factory()?;
                let mut opening=serde_json::to_vec(&serde_json::json!({"event":"jev_diagnostic_open_v1","runtime_binding_id":identity})).map_err(std::io::Error::other)?;
                opening.push(b'\n'); writer.write_all(&opening)?;
                while let Ok(line) = receiver.recv() { writer.write_all(&line)?; }
                writer.flush()?;
                let mut footer=serde_json::to_vec(&serde_json::json!({"event":"jev_diagnostic_shutdown_v1","runtime_binding_id":identity,"dropped":worker_dropped.load(Ordering::Relaxed)})).map_err(std::io::Error::other)?;
                footer.push(b'\n');writer.write_all(&footer)?;writer.flush()
            };
            if write().is_err() { worker_failed.store(true,Ordering::Release); }
        }).ok();
        if worker.is_none() {
            failed.store(true, Ordering::Release);
        }
        Self {
            sender: std::sync::Mutex::new(worker.as_ref().map(|_| sender)),
            worker: std::sync::Mutex::new(worker),
            dropped,
            failed,
            runtime_binding_id,
        }
    }
    fn try_line(&self, line: Vec<u8>) {
        let sent = line.len() <= 2048
            && self.sender.try_lock().ok().is_some_and(|guard| {
                guard
                    .as_ref()
                    .is_some_and(|sender| sender.try_send(line).is_ok())
            });
        if !sent {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn record(&self, record: Option<SafeDecisionDiagnostic>) {
        #[derive(serde::Serialize)]
        struct Envelope<'a> {
            runtime_binding_id: &'a str,
            #[serde(flatten)]
            diagnostic: SafeDecisionDiagnostic,
        }
        if let Some(record) = record
            && let Ok(mut line) = serde_json::to_vec(&Envelope {
                runtime_binding_id: &self.runtime_binding_id,
                diagnostic: record,
            })
        {
            line.push(b'\n');
            self.try_line(line);
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    async fn shutdown(&self, budget: std::time::Duration) -> bool {
        if let Ok(mut sender) = self.sender.lock() {
            sender.take();
        }
        let deadline = std::time::Instant::now() + budget;
        loop {
            let finished = match self.worker.lock() {
                Ok(worker) => worker
                    .as_ref()
                    .is_none_or(std::thread::JoinHandle::is_finished),
                Err(_) => return false,
            };
            if finished {
                let joined = match self.worker.lock() {
                    Ok(mut worker) => worker.take().is_none_or(|worker| worker.join().is_ok()),
                    Err(_) => false,
                };
                return joined && !self.failed.load(Ordering::Acquire);
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(
                deadline
                    .saturating_duration_since(std::time::Instant::now())
                    .min(std::time::Duration::from_millis(5)),
            )
            .await;
        }
    }
}

type ProviderCache = Option<(String, Arc<DecisionProvider>, Option<FactoryFailure>)>;
pub(crate) struct Runtime {
    state: Weak<AppState>,
    data: PathBuf,
    closed: Arc<AtomicBool>,
    slots: Arc<Semaphore>,
    provider: Mutex<ProviderCache>,
    diagnostics: DiagnosticSink,
    tasks: Mutex<JoinSet<()>>,
    pump: Mutex<Option<JoinHandle<()>>>,
    automatic_admin: bool,
    admin_launch_slots: Arc<Semaphore>,
    admin_launch_cursor: Mutex<Option<String>>,
    admin_launch_pending: Mutex<std::collections::VecDeque<crewd_collab::admin_cases::CaseRef>>,
    admin_launch_inflight: Mutex<std::collections::BTreeSet<String>>,
    similarity_slot: Arc<Semaphore>,
    ask_groups_slot: Arc<Semaphore>,
    ask_groups_ready: AtomicBool,
}
fn provider_config(c: &JudgmentConfig) -> DecisionConfig {
    DecisionConfig {
        enabled: c.enabled,
        provider: c.provider.clone(),
        base_url: c.base_url.clone(),
        model: c.model.clone(),
        choice_min: c.choice_min,
        wake_yes: c.wake_yes,
    }
}
impl Runtime {
    pub(crate) fn ask_groups_healthy(&self) -> bool {
        !self.closed.load(Ordering::Acquire)
            && self.ask_groups_ready.load(Ordering::Acquire)
            && self
                .pump
                .try_lock()
                .is_ok_and(|p| p.as_ref().is_some_and(|h| !h.is_finished()))
    }
    pub(crate) async fn start(
        state: &Arc<AppState>,
        data: PathBuf,
        active: bool,
    ) -> anyhow::Result<Option<Arc<Self>>> {
        Self::start_with_admin_dispatch(state, data, active, true).await
    }
    async fn start_with_admin_dispatch(
        state: &Arc<AppState>,
        data: PathBuf,
        active: bool,
        automatic_admin: bool,
    ) -> anyhow::Result<Option<Arc<Self>>> {
        let active = active
            && state
                .db
                .call_result(|s| crewd_store::judgment_state::format_available(s.conn()))
                .await??;
        let runtime_id = state.runtime_binding_id.clone();
        state
            .collab(move |tx| {
                tx.conn()
                    .execute("DELETE FROM meta WHERE key=?1", [facts::ACTIVE_KEY])?;
                if active {
                    crewd_store::meta_set(tx.conn(), facts::ACTIVE_KEY, &runtime_id)?;
                }
                Ok(())
            })
            .await?;
        if !active {
            return Ok(None);
        }
        let diagnostics = DiagnosticSink::new(data.clone(), state.runtime_binding_id.clone());
        let this = Arc::new(Self {
            state: Arc::downgrade(state),
            data,
            closed: Arc::new(AtomicBool::new(false)),
            slots: Arc::new(Semaphore::new(32)),
            provider: Mutex::new(None),
            diagnostics,
            tasks: Mutex::new(JoinSet::new()),
            pump: Mutex::new(None),
            automatic_admin,
            admin_launch_slots: Arc::new(Semaphore::new(8)),
            admin_launch_cursor: Mutex::new(None),
            admin_launch_pending: Mutex::new(Default::default()),
            admin_launch_inflight: Mutex::new(Default::default()),
            similarity_slot: Arc::new(Semaphore::new(1)),
            ask_groups_slot: Arc::new(Semaphore::new(1)),
            ask_groups_ready: AtomicBool::new(false),
        });
        // Settle old pending attempts without invoking a provider. A restored DB
        // has a different dbId and is historical instead of a new dispatch.
        let _ = state.judgment_runtime.set(this.clone());
        let mut rx = state.hub.subscribe_collab();
        state
            .collab(crewd_collab::work_coordination::observe_reports)
            .await?;
        this.recover_coordination().await?;
        this.recover_reassignment().await?;
        this.recover_retirement().await?;
        this.recover_admin_cases().await?;
        if let Err(error) = this.recover_similarity().await {
            eprintln!("crewd: requirement similarity recovery failed closed: {error}");
        }
        match this.recover_ask_groups().await {
            Ok(()) => this.ask_groups_ready.store(true, Ordering::Release),
            Err(error) => {
                eprintln!("crewd: pending-question grouping recovery failed closed: {error}")
            }
        }
        // Import failures are visible discovery failures, not a reason to stop
        // the daemon or restart unknown administrative dispatches.
        if let Err(error) = this.scan_legacy_admin_cases().await {
            eprintln!("crewd: historical administrative discovery failed closed: {error}");
        }
        this.scan_pending(true).await?;
        let weak = Arc::downgrade(&this);
        *this.pump.lock().await = Some(tokio::spawn(async move {
            let mut pending_tick = tokio::time::interval(std::time::Duration::from_secs(5));
            pending_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                let event = tokio::select! {
                    event = rx.recv() => event,
                    _ = pending_tick.tick() => {
                        let Some(runtime) = weak.upgrade() else { break };
                        runtime.schedule_similarity().await;
                        runtime.schedule_ask_groups().await;
                        if let Err(e) = runtime.resume_pending_coordination().await {
                            eprintln!("crewd: coordination pending scan failed: {e}");
                        }
                        if let Err(e) = runtime.resume_pending_reassignment().await {
                            eprintln!("crewd: reassignment pending scan failed: {e}");
                        }
                        if let Err(e) = runtime.scan_retirement().await {
                            eprintln!("crewd: retirement scan failed: {e}");
                        }
                        if let Err(e) = runtime.scan_legacy_admin_cases().await {
                            eprintln!("crewd: historical administrative discovery scan failed closed: {e}");
                        }
                        if let Err(e) = runtime.resume_pending_admin_cases().await {
                            eprintln!("crewd: administrative pending scan failed: {e}");
                        }
                        if let Err(e) = runtime.schedule_admin_launches().await {
                            eprintln!("crewd: administrative launch scan failed closed: {e}");
                        }
                        if let Err(e) = runtime.verify_admin_outcomes().await {
                            eprintln!("crewd: administrative outcome scan failed: {e}");
                        }
                        continue;
                    }
                };
                let Some(runtime) = weak.upgrade() else { break };
                if runtime.closed.load(Ordering::Acquire) {
                    break;
                }
                match event {
                    Ok(Change::MessageAppended { message_id, .. }) => {
                        runtime
                            .schedule(JudgmentSourceKind::Message, message_id)
                            .await
                    }
                    Ok(Change::WorkChanged { id, .. }) if id.starts_with('#') => {
                        runtime.schedule(JudgmentSourceKind::Requirement, id).await
                    }
                    Ok(Change::WorkChanged { id, .. }) if id.starts_with('W') => {
                        runtime.schedule_coordination(id.clone()).await;
                        runtime.schedule_reassignment(id).await
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if let Err(e) = runtime.resume_pending_admin_cases().await {
                            eprintln!("crewd: administrative catch-up failed: {e}");
                        }
                        if let Err(e) = runtime.resume_pending_coordination().await {
                            eprintln!("crewd: coordination catch-up failed: {e}");
                        }
                        if let Err(e) = runtime.resume_pending_reassignment().await {
                            eprintln!("crewd: reassignment catch-up failed: {e}");
                        }
                        // A backlog scan is recovery, not a new Human action.
                        if let Err(e) = runtime.scan_pending(true).await {
                            eprintln!("crewd: judgment pending scan failed: {e}");
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    _ => {}
                }
            }
        }));
        Ok(Some(this))
    }
    async fn scan_pending(self: &Arc<Self>, recovery: bool) -> anyhow::Result<()> {
        let Some(state) = self.state.upgrade() else {
            return Ok(());
        };
        // Keyset pages bound both the DB actor turn and retained IDs. Historical
        // and settled rows advance the cursor too; they cannot trap a scan.
        for kind in [JudgmentSourceKind::Message, JudgmentSourceKind::Requirement] {
            let mut after = String::new();
            loop {
                if self.closed.load(Ordering::Acquire) {
                    return Ok(());
                }
                let cursor = after.clone();
                let page_kind = kind.clone();
                let (rows, pending) = state.db.call_result(move |s| {
                    let sql = match page_kind {
                        JudgmentSourceKind::Message => "SELECT id FROM messages WHERE id>?1 AND extra LIKE '%judgmentV1%' ORDER BY id LIMIT 64",
                        JudgmentSourceKind::Requirement => "SELECT '#'||r.number AS id FROM object_refs r JOIN todos t ON t.id=r.raw_todo_id WHERE r.kind='requirement' AND ('#'||r.number)>?1 AND t.extra LIKE '%judgmentV1%' ORDER BY id LIMIT 64",
                    };
                    let mut q = s.conn().prepare(sql)?;
                    let rows = q.query_map([cursor], |r| r.get::<_,String>(0))?
                        .collect::<std::result::Result<Vec<_>,_>>()?;
                    let mut pending = Vec::new();
                    for id in &rows {
                        if facts::record(s.conn(), &page_kind, id)?.is_some_and(|r|
                            matches!(r.phase, JudgmentPhase::Pending | JudgmentPhase::Dispatched)) {
                            pending.push(id.clone());
                        }
                    }
                    Ok::<_,crewd_collab::CollabError>((rows, pending))
                }).await??;
                let Some(last) = rows.last() else { break };
                after = last.clone();
                for id in pending {
                    if recovery {
                        self.run(kind.clone(), id, true).await;
                    } else {
                        self.schedule(kind.clone(), id).await;
                    }
                }
                if rows.len() < 64 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        }
        Ok(())
    }
    async fn schedule(self: &Arc<Self>, kind: JudgmentSourceKind, id: String) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let Ok(slot) = self.slots.clone().try_acquire_owned() else {
            self.run(kind, id, true).await;
            return;
        };
        let runtime = self.clone();
        let mut tasks = self.tasks.lock().await;
        while tasks.try_join_next().is_some() {}
        tasks.spawn(async move {
            let _slot = slot;
            runtime.run(kind, id, false).await;
        });
    }
    async fn current_provider(&self, settings: &JudgmentSettings) -> Option<Arc<DecisionProvider>> {
        self.current_provider_observed(settings).await.0
    }
    async fn current_provider_observed(
        &self,
        settings: &JudgmentSettings,
    ) -> (Option<Arc<DecisionProvider>>, Option<FactoryFailure>) {
        let Some(revision) = settings.revision.as_ref() else {
            return (None, Some(FactoryFailure::MissingRevision));
        };
        let Some(config) = settings.config.as_ref() else {
            return (None, Some(FactoryFailure::MissingConfiguration));
        };
        if !config.enabled {
            return (None, Some(FactoryFailure::Disabled));
        }
        let Some(state) = self.state.upgrade() else {
            return (None, Some(FactoryFailure::StateUnavailable));
        };
        if configuration::validate_config(&state, config).is_err() {
            return (None, Some(FactoryFailure::InvalidConfiguration));
        }
        let epoch = match state
            .db
            .call_result(|s| crewd_store::judgment_state::epoch(s.conn()))
            .await
        {
            Ok(Ok(epoch)) => epoch,
            _ => return (None, Some(FactoryFailure::StateUnavailable)),
        };
        let cache_key = format!("{revision}:{epoch}");
        let mut slot = self.provider.lock().await;
        if let Some((old, p, diagnostic)) = &*slot
            && old == &cache_key
        {
            return (Some(p.clone()), *diagnostic);
        }
        if let Some((_, old, _)) = slot.take() {
            old.revoke();
        }
        let (key, diagnostic) =
            match configuration::load_key_observed(&state, self.data.clone(), settings).await {
                Ok(Some(key)) => (Some(key), None),
                Ok(None) => (None, Some(FactoryFailure::CredentialMissing)),
                Err(cause) => (None, Some(FactoryFailure::Credential(cause))),
            };
        let provider = match DecisionProvider::new(provider_config(config), key) {
            Ok(provider) => Arc::new(provider),
            Err(_) => return (None, Some(FactoryFailure::ClientInitialization)),
        };
        *slot = Some((cache_key, provider.clone(), diagnostic));
        (Some(provider), diagnostic)
    }
    async fn run(&self, kind: JudgmentSourceKind, id: String, recovery: bool) {
        if let Err(e) = self.run_inner(kind, id, recovery).await {
            eprintln!("crewd: judgment failed closed: {e}");
        }
    }
    async fn run_inner(
        &self,
        kind: JudgmentSourceKind,
        id: String,
        recovery: bool,
    ) -> anyhow::Result<()> {
        let Some(state) = self.state.upgrade() else {
            return Ok(());
        };
        let key = kind.clone();
        let source = id.clone();
        let Some((record, snapshot, settings)) = state
            .collab(move |tx| {
                // Restart never re-dispatches a previously admitted external call.
                if recovery
                    && let Some(mut old) = facts::record(tx.conn(), &key, &source)?
                    && old.phase == JudgmentPhase::Dispatched
                {
                    old.phase = JudgmentPhase::Pending;
                    facts::restore_pending_for_settlement(tx, &key, &source, &old)?;
                }
                facts::dispatch(tx, &key, &source)
            })
            .await?
        else {
            return Ok(());
        };
        let started = std::time::Instant::now();
        let started_at = state.admission.started_at;
        let (can_wake, fresh_human_message) = if kind == JudgmentSourceKind::Message {
            let mid = id.clone();
            state
                .db
                .call_result(move |s| {
                    let m = crewd_collab::messages::get(s.conn(), &mid)?;
                    let now = Ms(crewd_store::now_ms());
                    let f = crate::wake_adapter::message_facts(s.conn(), &m, now)?;
                    Ok::<_, crewd_collab::CollabError>((
                        crate::wake_adapter::channel_context(s.conn(), &m.channel_id, now)?
                            .is_some_and(|ctx| plan::needs_arbiter(&f, &ctx, now)),
                        !recovery
                            && f.sender == plan::Sender::Human
                            && f.provenance == plan::MessageProvenance::Ordinary
                            && f.created_at.0 >= started_at,
                    ))
                })
                .await??
        } else {
            (false, false)
        };
        let request = batch(&snapshot, can_wake);
        let config = settings
            .config
            .as_ref()
            .map(provider_config)
            .unwrap_or_default();
        let elapsed = crewd_store::now_ms().saturating_sub(record.prepared_at_ms);
        let remaining =
            std::time::Duration::from_millis(2000u64.saturating_sub(elapsed.max(0) as u64));
        let result = if recovery
            || remaining.is_zero()
            || self.closed.load(Ordering::Acquire)
            || !state.admission.can_admit()
            || (!state.admission.any() && !fresh_human_message)
        {
            JudgmentResult::Deferred {
                reason: if recovery {
                    "restart_unknown"
                } else if !state.admission.can_admit()
                    || (!state.admission.any() && !fresh_human_message)
                {
                    "awaiting_manual_resume"
                } else {
                    "deadline"
                }
                .into(),
                attempts: if recovery { None } else { Some(0) },
                latency_ms: 0,
            }
        } else {
            let deadline = std::time::Instant::now() + remaining;
            let eval = async {
                match self.current_provider(&settings).await {
                    Some(p) => p.evaluate_until(&request, deadline).await,
                    None => JudgmentResult::Deferred {
                        reason: "unconfigured".into(),
                        attempts: Some(0),
                        latency_ms: 0,
                    },
                }
            };
            tokio::time::timeout(remaining, eval)
                .await
                .unwrap_or(JudgmentResult::Deferred {
                    reason: "deadline".into(),
                    attempts: None,
                    latency_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                })
        };
        let chosen = selection(&snapshot, &config, &result);
        let key = kind.clone();
        let source = id.clone();
        let closed = self.closed.clone();
        let admitted = state
            .collab(move |tx| {
                if closed.load(Ordering::Acquire) {
                    return Ok(Vec::new());
                }
                let mut empty_message = None;
                let targets = if key == JudgmentSourceKind::Requirement {
                    chosen
                        .recipients
                        .iter()
                        .cloned()
                        .map(|id| (id, crewd_collab::wakes::WakePriority::Human, None))
                        .collect::<Vec<_>>()
                } else {
                    let m = crewd_collab::messages::get(tx.conn(), &source)?;
                    let now = Ms(tx.now_ms());
                    let facts = crate::wake_adapter::message_facts(tx.conn(), &m, now)?;
                    let Some(ctx) =
                        crate::wake_adapter::channel_context(tx.conn(), &m.channel_id, now)?
                    else {
                        return Ok(Vec::new());
                    };
                    let targets = plan::explained_plan(
                        &facts,
                        &ctx,
                        chosen.adopted.then_some(chosen.recipients.as_slice()),
                        now,
                    );
                    if targets.is_empty() {
                        empty_message = Some((m.id, m.channel_id, m.seq));
                    }
                    targets
                        .into_iter()
                        .map(|plan::ExplainedTarget { target: t, reasons }| {
                            let trace = crate::wake_adapter::routing_capture(
                                &facts,
                                crewd_proto::message_delivery::MessageWakePath::Jev,
                                reasons,
                            );
                            (
                                t.session,
                                match t.priority {
                                    crewd_wake::Priority::Human => {
                                        crewd_collab::wakes::WakePriority::Human
                                    }
                                    crewd_wake::Priority::System => {
                                        crewd_collab::wakes::WakePriority::System
                                    }
                                    crewd_wake::Priority::Auto => {
                                        crewd_collab::wakes::WakePriority::Auto
                                    }
                                },
                                Some(trace),
                            )
                        })
                        .collect()
                };
                let link = chosen
                    .link
                    .and_then(|n| snapshot.links.get(n))
                    .and_then(|l| {
                        Some(JudgmentLink {
                            kind: JudgmentObjectKind::from_key(&l.kind)?,
                            id: l.id.clone(),
                            revision: l.revision.clone(),
                        })
                    });
                let diagnostic =
                    serde_json::to_value(&result).map_err(crewd_store::StoreError::from)?;
                if !facts::settle(
                    tx,
                    &key,
                    &source,
                    &record.decision_id,
                    targets.iter().map(|(id, _, _)| id.clone()).collect(),
                    link,
                    diagnostic,
                )? {
                    return Ok(Vec::new());
                }
                if let Some((id, channel, seq)) = empty_message {
                    crewd_collab::message_delivery::no_targets(tx, &id, &channel, seq)?;
                }
                let mut admitted = Vec::new();
                for (target, priority, trace) in targets {
                    let request = crewd_collab::wakes::WakeRequest {
                        session: target.clone(),
                        kind: if key == JudgmentSourceKind::Requirement {
                            crewd_collab::wakes::WakeKind::System
                        } else {
                            crewd_collab::wakes::WakeKind::Message
                        },
                        source: if key == JudgmentSourceKind::Requirement {
                            format!("judgment-route:{source}")
                        } else {
                            crewd_collab::wakes::source::message(&source)
                        },
                        priority,
                        not_before_ms: None,
                        note: None,
                        origin: None,
                    };
                    let enqueued = match trace {
                        Some(trace) => crewd_collab::message_delivery::enqueue(tx, request, trace)?,
                        None => crewd_collab::wakes::enqueue(tx, request)?,
                    };
                    if fresh_human_message
                        && priority == crewd_collab::wakes::WakePriority::Human
                        && let crewd_collab::wakes::Enqueued::New(wake) = enqueued
                    {
                        admitted.push((target, wake));
                    }
                }
                Ok(admitted)
            })
            .await?;
        // Admission follows the successful settlement/queue commit, serialized
        // with Human stop writes: a stop between commit and admission has already
        // dropped this exact row and must not be undone by a late evaluation.
        let admission = state.admission.clone();
        let admitted = state
            .db
            .call_result(move |store| {
                let mut ready = Vec::new();
                for (target, wake) in admitted {
                    let pending = crewd_collab::wakes::by_ids(store.conn(), &[wake])?
                        .into_iter()
                        .any(|row| {
                            row.session == target
                                && row.state == crewd_collab::wakes::WakeState::Pending
                        });
                    if pending
                        && wake > crewd_collab::wakes::human_stop_floor(store.conn(), &target)?
                    {
                        admission.allow(&target);
                        if admission.contains(&target) {
                            ready.push(target);
                        }
                    }
                }
                Ok::<_, crewd_collab::CollabError>(ready)
            })
            .await??;
        // Hub may already have delivered WakeEnqueued before the read above;
        // notify only these exact targets using the existing idempotent event.
        for target in admitted {
            if let Some(orchestra) = state.orchestra.get() {
                orchestra
                    .collab(crate::orchestra::core::CollabFact::WakeEnqueued { session: target });
            }
        }
        Ok(())
    }
    async fn invalidate(&self) {
        if let Some((_, p, _)) = self.provider.lock().await.take() {
            p.revoke();
        }
    }
    pub(crate) async fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        if let Some((_, p, _)) = self.provider.lock().await.take() {
            p.revoke();
        }
        if let Some(pump) = self.pump.lock().await.take() {
            pump.abort();
            let _ = pump.await;
        }
        let mut tasks = self.tasks.lock().await;
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        let _ = self
            .diagnostics
            .shutdown(std::time::Duration::from_millis(200))
            .await;
    }
}

/// Public inspection is a closed projection: raw usage/error bodies and captured
/// authority floors remain private, never serialized into this view.
fn public_view(r: &JudgmentRecord) -> JudgmentView {
    let d = &r.diagnostic;
    let status = d
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let reason = match status {
        "answered" => JudgmentReason::Answered,
        "prepared" => JudgmentReason::Prepared,
        "stale" => JudgmentReason::Stale,
        "deferred" => match d.get("reason").and_then(serde_json::Value::as_str) {
            Some("unconfigured") => JudgmentReason::Unconfigured,
            Some("deadline") => JudgmentReason::Deadline,
            Some("provider_error") => JudgmentReason::ProviderError,
            Some("invalid_response" | "invalid_input") => JudgmentReason::InvalidResponse,
            Some("restart_unknown") => JudgmentReason::RestartUnknown,
            Some("busy") => JudgmentReason::Busy,
            Some("configuration_changed") => JudgmentReason::ConfigurationChanged,
            _ => JudgmentReason::Unknown,
        },
        _ => JudgmentReason::Unknown,
    };
    JudgmentView {
        decision_id: r.decision_id.clone(),
        phase: r.phase.clone(),
        reason,
        backend: JudgmentBackend::Typesafe,
        model: d
            .get("model")
            .and_then(serde_json::Value::as_str)
            .filter(|s| s.len() <= 256 && !s.chars().any(char::is_control))
            .map(str::to_owned),
        probability_source: match d
            .get("probability_source")
            .and_then(serde_json::Value::as_str)
        {
            Some("native") => Some(JudgmentProbabilitySource::Native),
            Some("generated_estimate") => Some(JudgmentProbabilitySource::GeneratedEstimate),
            _ => None,
        },
        calibrated: false,
        attempts: d
            .get("attempts")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| n.try_into().ok()),
        latency_ms: d.get("latency_ms").and_then(serde_json::Value::as_u64),
        usage: d.get("raw_usage").map(|u| JudgmentUsageView {
            input_tokens: u.get("input_tokens").and_then(serde_json::Value::as_u64),
            output_tokens: u.get("output_tokens").and_then(serde_json::Value::as_u64),
        }),
        recipients: r.recipients.clone(),
        attribution: r.attribution.clone(),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod safe_diagnostic_tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    fn record() -> SafeDecisionDiagnostic {
        SafeDecisionDiagnostic {
            event: "jev_decision_diagnostic_v1",
            case_id: "AC00000000000000000002".into(),
            decision_id: "00000000-0000-0000-0000-000000000002".into(),
            snapshot: "a".repeat(64),
            factory: Some(FactoryFailure::Credential(
                configuration::KeyLoadFailure::ReadFailed,
            )),
            failure: Some(crewd_jev::error::FailureDiagnostic {
                kind: crewd_jev::error::FailureKind::Unauthorized,
                status: Some(401),
            }),
            attempts: Some(1),
            latency_ms: 17,
        }
    }
    fn line() -> Vec<u8> {
        let mut value = serde_json::to_value(record()).unwrap();
        value.as_object_mut().unwrap().insert(
            "runtime_binding_id".into(),
            serde_json::json!("00000000-0000-0000-0000-000000000001"),
        );
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        bytes
    }
    #[test]
    fn only_closed_typed_records_are_accepted() {
        let bytes = line();
        assert!(safe_diagnostic_line(&bytes));
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["raw_error"] =
            serde_json::json!("Authorization private-body https://private.invalid");
        let mut unsafe_line = serde_json::to_vec(&value).unwrap();
        unsafe_line.push(b'\n');
        assert!(!safe_diagnostic_line(&unsafe_line));
        for field in ["case_id", "decision_id", "snapshot", "factory", "failure"] {
            let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            value[field] = serde_json::json!("private-token");
            let mut bad = serde_json::to_vec(&value).unwrap();
            bad.push(b'\n');
            assert!(!safe_diagnostic_line(&bad));
        }
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains("Authorization"));
        assert!(!text.contains("https://"));
    }
    #[test]
    fn duplicate_keys_at_any_depth_refuse_both_logs_without_modifying_them() {
        use std::os::unix::fs::MetadataExt;
        let decision = String::from_utf8(line()).unwrap();
        let inputs = [
            "{\"event\":\"jev_diagnostic_open_v1\",\"runtime_binding_id\":\"private raw body\",\"runtime_binding_id\":\"00000000-0000-0000-0000-000000000001\"}\n".to_owned(),
            decision.replace("\"status\":401", "\"status\":\"private raw body\",\"status\":401"),
            decision.replace("\"credential\":\"read_failed\"", "\"credential\":\"private raw body\",\"credential\":\"read_failed\""),
            "{\"event\":\"jev_diagnostic_open_v1\",\"runtime_binding_id\":\"private raw body\",\"runtime_\\u0062inding_id\":\"00000000-0000-0000-0000-000000000001\"}\n".to_owned(),
        ];
        // Exercise duplicate detection inside arrays as well as objects.
        assert!(serde_json::from_slice::<UniqueDiagnosticJson>(br#"[{"a":1,"a":2}]"#).is_err());
        for raw in inputs {
            assert!(!safe_diagnostic_line(raw.as_bytes()));
            for damaged in [DIAGNOSTIC_CURRENT, DIAGNOSTIC_PREVIOUS] {
                let dir = tempfile::tempdir().unwrap();
                let root = dir.path().canonicalize().unwrap();
                std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
                let mut before = Vec::new();
                for name in [DIAGNOSTIC_CURRENT, DIAGNOSTIC_PREVIOUS] {
                    let path = root.join(name);
                    let mut bytes = DIAGNOSTIC_HEADER.to_vec();
                    if name == damaged {
                        bytes.extend_from_slice(raw.as_bytes());
                    }
                    std::fs::write(&path, &bytes).unwrap();
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                        .unwrap();
                    let meta = std::fs::metadata(&path).unwrap();
                    before.push((
                        path,
                        bytes,
                        meta.dev(),
                        meta.ino(),
                        meta.permissions().mode(),
                    ));
                }
                assert!(PrivateDiagnosticFiles::open(&root).is_err());
                for (path, bytes, dev, ino, mode) in before {
                    assert_eq!(std::fs::read(&path).unwrap(), bytes);
                    let after = std::fs::metadata(path).unwrap();
                    assert_eq!(
                        (after.dev(), after.ino(), after.permissions().mode()),
                        (dev, ino, mode)
                    );
                }
            }
        }
    }
    #[test]
    fn unknown_symlink_hardlink_and_insecure_logs_are_not_modified() {
        for shape in ["unknown", "symlink", "hardlink", "insecure"] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
            let target = root.join("private-user-file");
            std::fs::write(&target, b"user bytes remain\n").unwrap();
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
            let log = root.join(DIAGNOSTIC_CURRENT);
            match shape {
                "symlink" => std::os::unix::fs::symlink(&target, &log).unwrap(),
                "hardlink" => std::fs::hard_link(&target, &log).unwrap(),
                "insecure" => {
                    std::fs::write(&log, DIAGNOSTIC_HEADER).unwrap();
                    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o644)).unwrap();
                }
                _ => {
                    std::fs::write(&log, b"unknown contents\n").unwrap();
                    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o600)).unwrap();
                }
            }
            let original = std::fs::read(&target).unwrap();
            let metadata = std::fs::metadata(&target).unwrap();
            let log_before = std::fs::read(&log).unwrap();
            assert!(PrivateDiagnosticFiles::open(&root).is_err());
            assert_eq!(std::fs::read(&target).unwrap(), original);
            assert_eq!(std::fs::read(&log).unwrap(), log_before);
            assert_eq!(
                std::fs::metadata(&target).unwrap().permissions().mode(),
                metadata.permissions().mode()
            );
        }
    }
    #[test]
    fn rotation_is_two_bounded_owned_files_and_never_accepts_free_text() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut files = PrivateDiagnosticFiles::open(&root).unwrap();
        assert!(files.write_all(b"private arbitrary string\n").is_err());
        for _ in 0..900 {
            files.write_all(&line()).unwrap();
        }
        drop(files);
        let mut names = std::fs::read_dir(&root)
            .unwrap()
            .map(|q| q.unwrap().file_name())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            vec![
                std::ffi::OsString::from(DIAGNOSTIC_CURRENT),
                std::ffi::OsString::from(DIAGNOSTIC_PREVIOUS)
            ]
        );
        for name in [DIAGNOSTIC_CURRENT, DIAGNOSTIC_PREVIOUS] {
            let path = root.join(name);
            assert!(std::fs::metadata(&path).unwrap().len() <= 128 * 1024);
            let bytes = std::fs::read(path).unwrap();
            assert!(
                bytes
                    .split_inclusive(|b| *b == b'\n')
                    .all(safe_diagnostic_line)
            );
        }
    }
    struct ShortWriter(Arc<std::sync::Mutex<Vec<u8>>>);
    impl Write for ShortWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let n = bytes.len().min(3);
            self.0.lock().unwrap().extend_from_slice(&bytes[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[tokio::test]
    async fn short_writes_are_completed_and_footer_is_not_guessed() {
        let bytes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = DiagnosticSink::with_writer(Box::new(ShortWriter(bytes.clone())));
        sink.record(Some(record()));
        assert!(sink.shutdown(std::time::Duration::from_secs(1)).await);
        let data = bytes.lock().unwrap().clone();
        let lines = data.split_inclusive(|b| *b == b'\n').collect::<Vec<_>>();
        assert_eq!(lines.len(), 3);
        assert!(lines.into_iter().all(safe_diagnostic_line));
        assert!(
            String::from_utf8(data)
                .unwrap()
                .contains("jev_diagnostic_shutdown_v1")
        );
    }
    struct FailedWriter;
    impl Write for FailedWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("controlled writer failure"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("controlled flush failure"))
        }
    }
    #[tokio::test]
    async fn worker_error_is_incomplete_and_cannot_block_authority() {
        let sink = DiagnosticSink::with_writer(Box::new(FailedWriter));
        sink.record(Some(record()));
        assert!(!sink.shutdown(std::time::Duration::from_secs(1)).await);
        assert_eq!(
            crewd_jev::provider::BUDGET,
            std::time::Duration::from_secs(2)
        );
    }
    struct BlockedWriter(Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>);
    impl Write for BlockedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let (lock, changed) = &*self.0;
            let mut ready = lock.lock().unwrap();
            while !*ready {
                ready = changed.wait(ready).unwrap();
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    struct Release(Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>);
    impl Drop for Release {
        fn drop(&mut self) {
            let (lock, changed) = &*self.0;
            *lock.lock().unwrap() = true;
            changed.notify_all();
        }
    }
    #[test]
    fn blocked_reader_full_queue_does_not_hold_runtime_drop_or_shutdown() {
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let release = Release(gate.clone());
        let (sent, received) = std::sync::mpsc::sync_channel(1);
        let owner = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let sink = DiagnosticSink::with_writer(Box::new(BlockedWriter(gate)));
            for _ in 0..1000 {
                sink.record(Some(record()));
            }
            let complete = runtime.block_on(sink.shutdown(std::time::Duration::from_millis(200)));
            let dropped = sink.dropped.load(Ordering::Relaxed);
            drop(sink);
            drop(runtime);
            sent.send((complete, dropped)).unwrap();
        });
        let (complete, dropped) = received
            .recv_timeout(std::time::Duration::from_secs(3))
            .expect("diagnostic blocked Core-like runtime drop");
        assert!(!complete);
        assert!(dropped >= 935);
        drop(release);
        owner.join().unwrap();
    }
}
