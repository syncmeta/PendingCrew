//! Bounded observations of PCW-owned managed tasks, never execution authority.
//! All writers require the original live seal and original Core transaction.
pub mod reads;
use super::{ManagedRuntime, Phase, get, invalid, managed_current, original_input};
use crate::Result;
use ring::digest::{SHA256, digest};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

pub const PREFIX: &str = "claude.managed.observations.v1/";
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024;
pub const MAX_OBSERVATIONS: usize = 4;
pub const MAX_SESSION_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_CORE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_META_BYTES: usize = 13 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Observation {
    pub observation_no: u64,
    pub prompt_id: String,
    pub observed_at: String,
    pub text: Option<String>,
    pub source_bytes: Option<u64>,
    pub captured_bytes: u64,
    pub source_sha256: Option<String>,
    pub availability: String,
}
impl std::fmt::Debug for Observation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedStopObservation")
            .field("observation_no", &self.observation_no)
            .field("text", &"<private>")
            .finish()
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct State {
    pub version: u8,
    pub runtime_id: String,
    pub request_id: String,
    pub authorized_task: Option<String>,
    pub task_unavailable: Option<String>,
    pub observations: Vec<Observation>,
    pub latest: Option<Observation>,
    pub latest_observation_no: u64,
    pub omitted_observations: u64,
    pub waiting_observation_no: Option<u64>,
    pub retained_bytes: u64,
}
impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedOwnedObservations")
            .field("runtime_id", &self.runtime_id)
            .field("request_id", &self.request_id)
            .field("bodies", &"<private>")
            .finish()
    }
}
pub fn envelope(request: &str, text: &str) -> Result<String> {
    if text.trim().is_empty() {
        return Err(invalid("empty managed task"));
    }
    let result = format!(
        "<pendingcrew-managed-request id=\"{request}\">\n{text}\n</pendingcrew-managed-request id=\"{request}\">"
    );
    if result.len() > 1_048_576 {
        return Err(invalid("managed task exceeds bound"));
    }
    Ok(result)
}
fn hash(body: &[u8]) -> String {
    digest(&SHA256, body)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn fresh(runtime: &str, request: &str) -> State {
    State {
        version: 1,
        runtime_id: runtime.into(),
        request_id: request.into(),
        authorized_task: None,
        task_unavailable: Some("notRecorded".into()),
        observations: vec![],
        latest: None,
        latest_observation_no: 0,
        omitted_observations: 0,
        waiting_observation_no: None,
        retained_bytes: 0,
    }
}
fn key(request: &str) -> String {
    format!("{PREFIX}{request}")
}
fn body_bytes(state: &State) -> u64 {
    state.authorized_task.as_ref().map_or(0, |s| s.len() as u64)
        + state
            .observations
            .iter()
            .map(|o| o.text.as_ref().map_or(0, |s| s.len() as u64))
            .sum::<u64>()
}
pub fn load(c: &Connection, runtime: &str, request: &str) -> Result<Option<State>> {
    let size: Option<i64> = c
        .query_row(
            "SELECT length(CAST(value AS BLOB)) FROM meta WHERE key=?1",
            [key(request)],
            |r| r.get(0),
        )
        .optional()?;
    let Some(size) = size else { return Ok(None) };
    if size < 0 || size as usize > MAX_META_BYTES {
        return Err(invalid("managed observation metadata exceeds bound"));
    }
    let raw = crate::meta_get(c, &key(request))?
        .ok_or_else(|| invalid("managed observation disappeared"))?;
    let state: State =
        serde_json::from_str(&raw).map_err(|_| invalid("managed observation metadata invalid"))?;
    if state.version != 1
        || state.runtime_id != runtime
        || state.request_id != request
        || state.observations.len() > MAX_OBSERVATIONS
        || state
            .authorized_task
            .as_ref()
            .is_some_and(|t| t.len() > 1_048_576)
        || state.retained_bytes != body_bytes(&state)
        || state.observations.iter().any(|o| {
            o.text
                .as_ref()
                .is_some_and(|t| t.len() > MAX_RESPONSE_BYTES)
                || o.captured_bytes != o.text.as_ref().map_or(0, |s| s.len() as u64)
                || o.observation_no > state.latest_observation_no
        })
        || state
            .latest
            .as_ref()
            .is_some_and(|o| o.text.is_some() || o.observation_no != state.latest_observation_no)
    {
        return Err(invalid("managed observation identity/content invalid"));
    }
    Ok(Some(state))
}
fn available(c: &Connection, session: &str, prior: &State) -> Result<u64> {
    let core: i64 = c.query_row(
        "SELECT COALESCE(SUM(json_extract(value,'$.retainedBytes')),0) FROM meta WHERE key GLOB ?1",
        [format!("{PREFIX}*")],
        |r| r.get(0),
    )?;
    let session_bytes:i64=c.query_row("SELECT COALESCE(SUM(json_extract(m.value,'$.retainedBytes')),0) FROM meta m JOIN native_execution_receipts r ON m.key=?1||r.request_id JOIN native_execution_runtimes n ON n.runtime_id=r.runtime_id WHERE n.session_id=?2",params![PREFIX,session],|r|r.get(0))?;
    if core < 0 || session_bytes < 0 {
        return Err(invalid("managed retention accounting invalid"));
    }
    // Quotas count retained plaintext bytes, not bounded header facts/JSON escaping.
    let own = prior.retained_bytes;
    Ok(MAX_CORE_BYTES
        .saturating_sub((core as u64).saturating_sub(own))
        .min(MAX_SESSION_BYTES.saturating_sub((session_bytes as u64).saturating_sub(own)))
        .saturating_sub(own))
}
fn save(tx: &Transaction<'_>, state: &mut State) -> Result<()> {
    state.retained_bytes = body_bytes(state);
    let raw = serde_json::to_string(state)?;
    if raw.len() > MAX_META_BYTES {
        return Err(invalid("managed observation encoded bound exceeded"));
    }
    crate::meta_set(tx, &key(&state.request_id), &raw)
}
pub fn record_task(
    tx: &Transaction<'_>,
    seal: &ManagedRuntime,
    request: &str,
    task: Option<&str>,
) -> Result<()> {
    managed_current(tx, seal)?;
    let receipt = get(tx, request)?.ok_or_else(|| invalid("managed task receipt absent"))?;
    if receipt.runtime_id != seal.runtime.id || receipt.phase != Phase::Prepared {
        return Err(invalid("managed original task unbound"));
    }
    let original = original_input(tx, &seal.runtime.id, request)?
        .ok_or_else(|| invalid("managed task input absent"))?;
    if let Some(task) = task
        && envelope(request, task)?.as_bytes() != original.bytes()
    {
        return Err(invalid(
            "managed authorized task differs from immutable envelope",
        ));
    }
    let prior = load(tx, &seal.runtime.id, request)?;
    if let Some(prior) = prior {
        if prior
            .authorized_task
            .as_deref()
            .is_some_and(|original| Some(original) != task)
        {
            return Err(invalid("managed original task is immutable"));
        }
        return Ok(());
    }
    let mut state = fresh(&seal.runtime.id, request);
    if let Some(task) = task {
        if task.len() as u64 <= available(tx, &seal.runtime.session, &state)? {
            state.authorized_task = Some(task.into());
            state.task_unavailable = None
        } else {
            state.task_unavailable = Some("retentionLimit".into())
        }
    }
    save(tx, &mut state)
}
pub fn observe_stop(
    tx: &Transaction<'_>,
    seal: &ManagedRuntime,
    request: &str,
    prompt: &str,
    text: Option<&str>,
    at: &str,
) -> Result<()> {
    managed_current(tx, seal)?;
    let receipt = get(tx, request)?.ok_or_else(|| invalid("managed observation receipt absent"))?;
    if receipt.runtime_id != seal.runtime.id
        || receipt.prompt_id.as_deref() != Some(prompt)
        || receipt.turn_id.is_some()
        || !super::uuid(prompt)
        || !matches!(
            receipt.phase,
            Phase::ContentAccepted | Phase::Completed | Phase::Unknown
        )
        || !super::valid_text(at, 128)
    {
        return Err(invalid("managed observation prompt/receipt unavailable"));
    }
    let mut state =
        load(tx, &seal.runtime.id, request)?.unwrap_or_else(|| fresh(&seal.runtime.id, request));
    state.latest_observation_no = state
        .latest_observation_no
        .checked_add(1)
        .ok_or_else(|| invalid("managed observation counter exhausted"))?;
    state.waiting_observation_no = None;
    let mut observation = Observation {
        observation_no: state.latest_observation_no,
        prompt_id: prompt.into(),
        observed_at: at.into(),
        text: None,
        source_bytes: text.map(|s| s.len() as u64),
        captured_bytes: 0,
        source_sha256: text.map(|s| hash(s.as_bytes())),
        availability: if text.is_some() {
            "observed"
        } else {
            "missing"
        }
        .into(),
    };
    if state.observations.len() < MAX_OBSERVATIONS {
        if let Some(text) = text {
            let limit = available(tx, &seal.runtime.session, &state)?.min(MAX_RESPONSE_BYTES as u64)
                as usize;
            let mut end = text.len().min(limit);
            while !text.is_char_boundary(end) {
                end -= 1
            }
            observation.text = if end > 0 || text.is_empty() {
                Some(text[..end].to_owned())
            } else {
                None
            };
            observation.captured_bytes = end as u64;
            if end < text.len() {
                observation.availability = if limit < MAX_RESPONSE_BYTES {
                    "retentionLimit"
                } else {
                    "truncated"
                }
                .into()
            }
        }
        state.observations.push(observation.clone());
    } else {
        state.omitted_observations = state
            .omitted_observations
            .checked_add(1)
            .ok_or_else(|| invalid("managed omitted observation counter exhausted"))?;
        observation.availability = "observationLimit".into();
    }
    // The head references the newest observation without storing a second body.
    observation.text = None;
    state.latest = Some(observation);
    save(tx, &mut state)
}
pub fn mark_waiting(
    tx: &Transaction<'_>,
    seal: &ManagedRuntime,
    request: &str,
    prompt: &str,
) -> Result<()> {
    managed_current(tx, seal)?;
    let receipt = get(tx, request)?.ok_or_else(|| invalid("managed waiting receipt absent"))?;
    if receipt.runtime_id != seal.runtime.id
        || receipt.prompt_id.as_deref() != Some(prompt)
        || receipt.turn_id.is_some()
        || receipt.phase != Phase::Completed
    {
        return Err(invalid("managed observed waiting unbound"));
    }
    if let Some(mut state) = load(tx, &seal.runtime.id, request)?
        && state.latest.as_ref().is_some_and(|o| o.prompt_id == prompt)
    {
        state.waiting_observation_no = Some(state.latest_observation_no);
        save(tx, &mut state)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{
        Runtime, invalidate_unknown, managed_accept_content, managed_response_resumed,
        managed_response_waiting, prepare, register_managed,
    };
    use super::*;
    fn id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
    fn fixture() -> (crate::Store, ManagedRuntime) {
        let s = crate::Store::open_in_memory().unwrap();
        s.conn().execute_batch("INSERT INTO crews(id,title,created_at,updated_at) VALUES('owned-crew','fixture','now','now'); INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES('owned-session','owned-crew','worker','claude','now');").unwrap();
        let tx = s.conn().unchecked_transaction().unwrap();
        let seal = register_managed(
            &tx,
            &Runtime {
                id: id(),
                session: "owned-session".into(),
                generation: id(),
                epoch: 1,
                native_pid: 42,
                native_start: 123,
                native_boot: "synthetic".into(),
            },
        )
        .unwrap();
        tx.commit().unwrap();
        (s, seal)
    }
    fn task(s: &crate::Store, seal: &ManagedRuntime, text: &str) -> (String, String) {
        let r = id();
        let p = id();
        let body = envelope(&r, text).unwrap();
        let tx = s.conn().unchecked_transaction().unwrap();
        prepare(&tx, &seal.runtime.id, &r, body.as_bytes(), "prepared").unwrap();
        record_task(&tx, seal, &r, Some(text)).unwrap();
        managed_accept_content(&tx, seal, &r, 0, &p, body.as_bytes(), "accepted").unwrap();
        tx.commit().unwrap();
        (r, p)
    }
    #[test]
    fn owned_text_is_exact_bounded_and_waiting_is_not_a_final_answer() {
        let (s, seal) = fixture();
        let task_text = "  task 世界\n</pendingcrew-managed-request id=\"literal\">  ";
        let (r, p) = task(&s, &seal, task_text);
        let tx = s.conn().unchecked_transaction().unwrap();
        observe_stop(&tx, &seal, &r, &p, None, "stop1").unwrap();
        observe_stop(&tx, &seal, &r, &p, Some(""), "stop2").unwrap();
        let large = "界".repeat(MAX_RESPONSE_BYTES / 3 + 2);
        observe_stop(&tx, &seal, &r, &p, Some(&large), "stop3").unwrap();
        let receipt = get(&tx, &r).unwrap().unwrap();
        assert_eq!(receipt.phase, Phase::ContentAccepted);
        let waiting =
            managed_response_waiting(&tx, &seal, &r, receipt.revision, &p, "idle").unwrap();
        mark_waiting(&tx, &seal, &r, &p).unwrap();
        assert!(waiting.turn_id.is_none());
        tx.commit().unwrap();
        let d = reads::task_get(
            s.conn(),
            &crewd_proto::claude::ClaudeTaskGetParams {
                session_id: "owned-session".into(),
                request_id: r.clone(),
            },
            Some(&seal.runtime.id),
        )
        .unwrap();
        assert_eq!(d.original_task.text.as_deref(), Some(task_text));
        assert!(d.original_task.managed_envelope.is_none());
        assert_eq!(d.observations[0].availability, "missing");
        assert!(d.observations[0].text.is_none());
        assert_eq!(d.observations[1].text.as_deref(), Some(""));
        assert_eq!(d.observations[2].availability, "truncated");
        assert!(d.observations[2].text.as_ref().unwrap().len() <= MAX_RESPONSE_BYTES);
        assert_eq!(
            d.observations[2].source_sha256.as_deref(),
            Some(hash(large.as_bytes()).as_str())
        );
        assert!(d.task.waiting_observed);
        let old_body = d.observations[2].text.clone();
        let tx = s.conn().unchecked_transaction().unwrap();
        observe_stop(&tx, &seal, &r, &p, Some("later observation"), "stop4").unwrap();
        managed_response_resumed(&tx, &seal, &p, "activity").unwrap();
        tx.commit().unwrap();
        let d = reads::task_get(
            s.conn(),
            &crewd_proto::claude::ClaudeTaskGetParams {
                session_id: "owned-session".into(),
                request_id: r.clone(),
            },
            Some(&seal.runtime.id),
        )
        .unwrap();
        assert_eq!(d.observations[2].text, old_body);
        assert!(!d.task.waiting_observed);
        assert_eq!(d.task.phase, "unknown");
        assert!(d.waiting_observation_no.is_none());
        let tx = s.conn().unchecked_transaction().unwrap();
        observe_stop(&tx, &seal, &r, &p, Some("not retained"), "stop5").unwrap();
        tx.commit().unwrap();
        let state = load(s.conn(), &seal.runtime.id, &r).unwrap().unwrap();
        assert_eq!(state.observations.len(), 4);
        assert_eq!(state.omitted_observations, 1);
        assert_eq!(state.latest.unwrap().availability, "observationLimit");
        assert!(
            !format!(
                "{:?}",
                reads::task_get(
                    s.conn(),
                    &crewd_proto::claude::ClaudeTaskGetParams {
                        session_id: "owned-session".into(),
                        request_id: r
                    },
                    None
                )
                .unwrap()
            )
            .contains(task_text)
        );
    }
    #[test]
    fn history_cursor_freezes_members_not_mutable_facts_and_restore_cannot_regrant() {
        let (s, seal) = fixture();
        let (r1, p1) = task(&s, &seal, "first exact original");
        let tx = s.conn().unchecked_transaction().unwrap();
        observe_stop(&tx, &seal, &r1, &p1, Some("first observed"), "s1").unwrap();
        let receipt = get(&tx, &r1).unwrap().unwrap();
        managed_response_waiting(&tx, &seal, &r1, receipt.revision, &p1, "idle").unwrap();
        mark_waiting(&tx, &seal, &r1, &p1).unwrap();
        tx.commit().unwrap();
        let (r2, p2) = task(&s, &seal, "second");
        let tx = s.conn().unchecked_transaction().unwrap();
        observe_stop(&tx, &seal, &r2, &p2, Some("second observed"), "s2").unwrap();
        let receipt = get(&tx, &r2).unwrap().unwrap();
        managed_response_waiting(&tx, &seal, &r2, receipt.revision, &p2, "idle").unwrap();
        mark_waiting(&tx, &seal, &r2, &p2).unwrap();
        tx.commit().unwrap();
        let p = crewd_proto::claude::ClaudeHistoryParams {
            session_id: "owned-session".into(),
            cursor: None,
            limit: 1,
        };
        let first = reads::history(s.conn(), &p, Some(&seal.runtime.id)).unwrap();
        assert_eq!(first.tasks[0].request_id, r2);
        let (r3, _) = task(&s, &seal, "third, must not enter old pages");
        let mut next = p.clone();
        next.cursor = first.next_cursor.clone();
        let second = reads::history(s.conn(), &next, None).unwrap();
        assert_eq!(second.tasks[0].request_id, r1);
        assert!(second.next_cursor.is_none());
        assert_ne!(second.tasks[0].request_id, r3);
        assert!(!second.tasks[0].waiting_observed);
        let wrong = crewd_proto::claude::ClaudeTaskGetParams {
            session_id: "other-session".into(),
            request_id: r1.clone(),
        };
        assert!(reads::task_get(s.conn(), &wrong, None).is_err());
        let tx = s.conn().unchecked_transaction().unwrap();
        invalidate_unknown(&tx).unwrap();
        crate::judgment_state::restored(&tx).unwrap();
        tx.commit().unwrap();
        assert!(reads::history(s.conn(), &next, None).is_err());
        assert!(
            observe_stop(
                &s.conn().unchecked_transaction().unwrap(),
                &seal,
                &r1,
                &p1,
                Some("cannot restore live seal"),
                "late"
            )
            .is_err()
        );
        let d = reads::task_get(
            s.conn(),
            &crewd_proto::claude::ClaudeTaskGetParams {
                session_id: "owned-session".into(),
                request_id: r1,
            },
            None,
        )
        .unwrap();
        assert_eq!(d.observations[0].text.as_deref(), Some("first observed"));
        assert!(!d.task.waiting_observed);
    }
    #[test]
    fn raw_custody_keeps_historical_stamp_but_cannot_report_current_waiting() {
        let (s, seal) = fixture();
        let (r, p) = task(&s, &seal, "original");
        let tx = s.conn().unchecked_transaction().unwrap();
        observe_stop(&tx, &seal, &r, &p, Some("actual observation"), "s").unwrap();
        let receipt = get(&tx, &r).unwrap().unwrap();
        managed_response_waiting(&tx, &seal, &r, receipt.revision, &p, "idle").unwrap();
        mark_waiting(&tx, &seal, &r, &p).unwrap();
        super::super::revoke_before_raw(&tx, &seal.runtime.id, 0, "raw").unwrap();
        tx.commit().unwrap();
        let detail = reads::task_get(
            s.conn(),
            &crewd_proto::claude::ClaudeTaskGetParams {
                session_id: "owned-session".into(),
                request_id: r,
            },
            Some(&seal.runtime.id),
        )
        .unwrap();
        assert_eq!(detail.task.phase, "completed");
        assert!(detail.waiting_observation_no.is_some());
        assert!(!detail.task.waiting_observed);
        assert!(!detail.task.current_runtime);
        assert_eq!(
            detail.observations[0].text.as_deref(),
            Some("actual observation")
        );
    }
    #[test]
    fn immutable_task_observation_bindings_reject_wrong_prompt_and_rollback() {
        let (s, seal) = fixture();
        let (r, p) = task(&s, &seal, "original");
        let before = crate::meta_get(s.conn(), &key(&r)).unwrap();
        let tx = s.conn().unchecked_transaction().unwrap();
        assert!(observe_stop(&tx, &seal, &r, &id(), Some("foreign"), "t").is_err());
        assert!(record_task(&tx, &seal, &r, Some("rewritten")).is_err());
        tx.rollback().unwrap();
        assert_eq!(crate::meta_get(s.conn(), &key(&r)).unwrap(), before);
        let tx = s.conn().unchecked_transaction().unwrap();
        observe_stop(&tx, &seal, &r, &p, Some("uncommitted"), "t").unwrap();
        tx.rollback().unwrap();
        assert_eq!(crate::meta_get(s.conn(), &key(&r)).unwrap(), before);
    }
}

#[cfg(test)]
mod old_writer_tests {
    use super::super::{
        Runtime, managed_accept_content, managed_response_waiting, prepare, register_managed,
    };
    use super::*;
    #[test]
    #[ignore = "requires approved exact original Store20 production helper path and hash"]
    fn owned_history_exact_old20_restore_preserves_observations_but_never_a_live_seal() {
        let binary = std::path::PathBuf::from(std::env::var("PCW_CLAUDE_OLD20_CORE").unwrap());
        let expected = std::env::var("PCW_CLAUDE_OLD20_SHA256").unwrap();
        let binary_before = std::fs::read(&binary).unwrap();
        assert_eq!(hash(&binary_before), expected);
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        std::fs::create_dir(&data).unwrap();
        let s = crate::Store::open_data_dir(&data).unwrap();
        s.conn().execute_batch("INSERT INTO crews(id,title,created_at,updated_at)VALUES('owned-crew','fixture','now','now');INSERT INTO sessions(id,crew_id,role,executor,agent_session_id,updated_at)VALUES('owned-session','owned-crew','worker','claude','71bd51cf-3df2-4638-9a3f-d0d58d630174','now');").unwrap();
        let runtime = Runtime {
            id: uuid::Uuid::new_v4().to_string(),
            session: "owned-session".into(),
            generation: uuid::Uuid::new_v4().to_string(),
            epoch: 1,
            native_pid: 42,
            native_start: 123,
            native_boot: "synthetic".into(),
        };
        let request = uuid::Uuid::new_v4().to_string();
        let prompt = uuid::Uuid::new_v4().to_string();
        let task = "original task with exact\n世界";
        let body = envelope(&request, task).unwrap();
        let tx = s.conn().unchecked_transaction().unwrap();
        let seal = register_managed(&tx, &runtime).unwrap();
        prepare(&tx, &runtime.id, &request, body.as_bytes(), "prepared").unwrap();
        record_task(&tx, &seal, &request, Some(task)).unwrap();
        managed_accept_content(
            &tx,
            &seal,
            &request,
            0,
            &prompt,
            body.as_bytes(),
            "accepted",
        )
        .unwrap();
        observe_stop(
            &tx,
            &seal,
            &request,
            &prompt,
            Some("durable observed reply"),
            "observed",
        )
        .unwrap();
        let r = get(&tx, &request).unwrap().unwrap();
        managed_response_waiting(&tx, &seal, &request, r.revision, &prompt, "idle").unwrap();
        mark_waiting(&tx, &seal, &request, &prompt).unwrap();
        tx.commit().unwrap();
        let raw = crate::meta_get(s.conn(), &key(&request)).unwrap().unwrap();
        let db_id = crate::meta_get(s.conn(), "dbId").unwrap();
        let backup = root
            .path()
            .join("original20-with-new-optional-observation.db");
        crate::backup::backup_db(&data.join(crate::DB_FILE), &backup).unwrap();
        let backup_before = std::fs::read(&backup).unwrap();
        drop(s);
        let result = std::process::Command::new(&binary)
            .arg("--data-dir")
            .arg(&data)
            .arg("restore")
            .arg("--from")
            .arg(&backup)
            .output()
            .unwrap();
        eprintln!(
            "exact old20 restore status={} stderr={}",
            result.status,
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.status.success());
        assert_eq!(std::fs::read(&binary).unwrap(), binary_before);
        assert_eq!(std::fs::read(&backup).unwrap(), backup_before);
        let restored = crate::Store::open_data_dir(&data).unwrap();
        assert_eq!(
            restored
                .conn()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            20
        );
        assert_eq!(crate::meta_get(restored.conn(), "dbId").unwrap(), db_id);
        assert_eq!(
            crate::meta_get(restored.conn(), &key(&request))
                .unwrap()
                .as_deref(),
            Some(raw.as_str())
        );
        assert!(
            super::super::current_runtime(restored.conn(), "owned-session")
                .unwrap()
                .is_none()
        );
        let detail = reads::task_get(
            restored.conn(),
            &crewd_proto::claude::ClaudeTaskGetParams {
                session_id: "owned-session".into(),
                request_id: request.clone(),
            },
            None,
        )
        .unwrap();
        assert_eq!(detail.original_task.text.as_deref(), Some(task));
        assert_eq!(
            detail.observations[0].text.as_deref(),
            Some("durable observed reply")
        );
        assert!(!detail.task.waiting_observed);
        let tx = restored.conn().unchecked_transaction().unwrap();
        assert!(super::super::verify_managed_authority(&tx, &seal).is_err());
        assert!(
            prepare(
                &tx,
                &runtime.id,
                &uuid::Uuid::new_v4().to_string(),
                body.as_bytes(),
                "no-replay"
            )
            .is_err()
        );
        tx.rollback().unwrap();
        let native: String = restored
            .conn()
            .query_row(
                "SELECT agent_session_id FROM sessions WHERE id='owned-session'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(native, "71bd51cf-3df2-4638-9a3f-d0d58d630174");
    }
}
