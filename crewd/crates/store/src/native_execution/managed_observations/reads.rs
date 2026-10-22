//! Bounded original-row membership cursors; mutable observations remain current facts.
use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use crewd_proto::claude::*;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    db: String,
    session: String,
    restore: String,
    cut: i64,
    last: i64,
    request: String,
}
fn bound(c: &Connection, session: &str) -> Result<(String, String)> {
    let db = crate::meta_get(c, "dbId")?.ok_or_else(|| invalid("database identity unavailable"))?;
    if session.is_empty() || session.len() > 256 {
        return Err(invalid("invalid managed history session"));
    }
    Ok((db, crate::judgment_state::epoch(c)?))
}
// A prior managed task without the new optional body metadata is recognizable
// only by its exact original request-bound envelope, never by parsing task text.
const OWNED: &str = "r.turn_id IS NULL AND (m.key IS NOT NULL OR (substr(CAST(i.body AS TEXT),1,length('<pendingcrew-managed-request id=\"'||r.request_id||'\">'||char(10)))='<pendingcrew-managed-request id=\"'||r.request_id||'\">'||char(10) AND substr(CAST(i.body AS TEXT),-length(char(10)||'</pendingcrew-managed-request id=\"'||r.request_id||'\">'))=char(10)||'</pendingcrew-managed-request id=\"'||r.request_id||'\">'))";
const JOIN: &str = " FROM native_execution_receipts r JOIN native_execution_runtimes n ON n.runtime_id=r.runtime_id JOIN native_execution_inputs i ON i.request_id=r.request_id LEFT JOIN meta m ON m.key='claude.managed.observations.v1/'||r.request_id WHERE n.session_id=?1 AND ";
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Header {
    version: u8,
    runtime_id: String,
    request_id: String,
    latest_observation_no: u64,
    omitted_observations: u64,
    waiting_observation_no: Option<u64>,
    latest_observed_at: Option<String>,
}
fn header(c: &Connection, runtime: &str, request: &str) -> Result<Option<Header>> {
    let size: Option<i64> = c
        .query_row(
            "SELECT length(CAST(value AS BLOB)) FROM meta WHERE key=?1",
            [key(request)],
            |r| r.get(0),
        )
        .optional()?;
    if size.is_none() {
        return Ok(None);
    }
    if size.is_some_and(|s| s < 0 || s as usize > MAX_META_BYTES) {
        return Err(invalid("managed observation exceeds bound"));
    }
    let raw:String=c.query_row("SELECT json_object('version',json_extract(value,'$.version'),'runtimeId',json_extract(value,'$.runtimeId'),'requestId',json_extract(value,'$.requestId'),'latestObservationNo',json_extract(value,'$.latestObservationNo'),'omittedObservations',json_extract(value,'$.omittedObservations'),'waitingObservationNo',json_extract(value,'$.waitingObservationNo'),'latestObservedAt',json_extract(value,'$.latest.observedAt')) FROM meta WHERE key=?1",[key(request)],|r|r.get(0))?;
    let h: Header =
        serde_json::from_str(&raw).map_err(|_| invalid("managed observation header invalid"))?;
    if h.version != 1
        || h.runtime_id != runtime
        || h.request_id != request
        || h.omitted_observations > h.latest_observation_no
        || h.waiting_observation_no
            .is_some_and(|n| n != h.latest_observation_no)
    {
        return Err(invalid("managed observation header unbound"));
    }
    Ok(Some(h))
}
pub fn summary(
    c: &Connection,
    session: &str,
    request: &str,
    live_runtime: Option<&str>,
) -> Result<ClaudeTaskSummary> {
    let sql = format!(
        "SELECT r.runtime_id,r.created_at,r.updated_at,r.phase,r.revision,r.prompt_id,r.turn_id,r.rowid{JOIN}{OWNED} AND r.request_id=?2"
    );
    let (runtime, created, updated, phase, revision, prompt, _turn, _row): (
        String,
        String,
        String,
        String,
        i64,
        Option<String>,
        Option<String>,
        i64,
    ) = c.query_row(&sql, params![session, request], |r| {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
            r.get(7)?,
        ))
    })?;
    let h = header(c, &runtime, request)?;
    let automatic:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM native_execution_runtimes WHERE runtime_id=?1 AND revoked=0 AND manual=0)",[&runtime],|r|r.get(0))?;
    let current = automatic && live_runtime == Some(runtime.as_str());
    let waiting = current
        && phase == "completed"
        && prompt.is_some()
        && h.as_ref().is_some_and(|h| {
            h.waiting_observation_no.is_some()
                && h.waiting_observation_no == Some(h.latest_observation_no)
        });
    Ok(ClaudeTaskSummary {
        request_id: request.into(),
        runtime_id: runtime,
        created_at: created,
        updated_at: updated,
        phase,
        receipt_revision: revision.to_string(),
        prompt_id: prompt,
        observation_count: h
            .as_ref()
            .map_or(0, |h| h.latest_observation_no)
            .to_string(),
        omitted_observations: h.as_ref().map_or(0, |h| h.omitted_observations).to_string(),
        latest_observed_at: h.and_then(|h| h.latest_observed_at),
        waiting_observed: waiting,
        current_runtime: current,
    })
}
pub fn history(
    c: &Connection,
    p: &ClaudeHistoryParams,
    live_runtime: Option<&str>,
) -> Result<ClaudeHistoryPage> {
    if !(1..=64).contains(&p.limit) {
        return Err(invalid("managed history limit must be 1..64"));
    }
    let (db, restore) = bound(c, &p.session_id)?;
    let max: i64 = c.query_row(
        &format!("SELECT COALESCE(MAX(r.rowid),0){JOIN}{OWNED}"),
        [&p.session_id],
        |r| r.get(0),
    )?;
    let cursor = if let Some(raw) = &p.cursor {
        if raw.len() > 4096 {
            return Err(invalid("managed history cursor exceeds bound"));
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(raw)
            .map_err(|_| invalid("invalid managed history cursor"))?;
        if URL_SAFE_NO_PAD.encode(&bytes) != *raw {
            return Err(invalid("noncanonical managed cursor"));
        }
        let v: Cursor = serde_json::from_slice(&bytes)
            .map_err(|_| invalid("invalid managed history cursor"))?;
        if v.version != 1
            || v.db != db
            || v.restore != restore
            || v.session != p.session_id
            || v.cut < 1
            || v.cut > max
            || v.last < 1
            || v.last > v.cut
        {
            return Err(invalid("managed history cursor source changed"));
        }
        let found: bool = c.query_row(
            &format!("SELECT EXISTS(SELECT 1{JOIN}{OWNED} AND r.rowid=?2 AND r.request_id=?3)"),
            params![p.session_id, v.last, v.request],
            |r| r.get(0),
        )?;
        if !found {
            return Err(invalid("managed history cursor row changed"));
        }
        Some(v)
    } else {
        None
    };
    let cut = cursor.as_ref().map_or(max, |v| v.cut);
    let before = cursor.as_ref().map(|v| v.last);
    let sql = format!(
        "SELECT r.rowid,r.request_id{JOIN}{OWNED} AND r.rowid<=?2 AND (?3 IS NULL OR r.rowid<?3) ORDER BY r.rowid DESC LIMIT ?4"
    );
    let mut q = c.prepare(&sql)?;
    let rows = q
        .query_map(params![p.session_id, cut, before, p.limit + 1], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let has_more = rows.len() > p.limit as usize;
    let rows = &rows[..rows.len().min(p.limit as usize)];
    let mut tasks = Vec::with_capacity(rows.len());
    for (_, request) in rows {
        tasks.push(summary(c, &p.session_id, request, live_runtime)?)
    }
    let next_cursor = if has_more {
        let (last, request) = rows.last().ok_or_else(|| invalid("managed cursor empty"))?;
        Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Cursor {
            version: 1,
            db,
            restore,
            session: p.session_id.clone(),
            cut,
            last: *last,
            request: request.clone(),
        })?))
    } else {
        None
    };
    Ok(ClaudeHistoryPage {
        session_id: p.session_id.clone(),
        tasks,
        next_cursor,
        membership_cut: cut.to_string(),
        observed_at: crate::now_ts(),
    })
}
fn observation(o: Observation) -> ClaudeResponseObservation {
    ClaudeResponseObservation {
        observation_no: o.observation_no.to_string(),
        prompt_id: o.prompt_id,
        observed_at: o.observed_at,
        text: o.text,
        source_bytes: o.source_bytes.map(|v| v.to_string()),
        captured_bytes: o.captured_bytes.to_string(),
        source_sha256: o.source_sha256,
        availability: o.availability,
    }
}
pub fn task_get(
    c: &Connection,
    p: &ClaudeTaskGetParams,
    live_runtime: Option<&str>,
) -> Result<ClaudeTaskDetail> {
    if !super::super::uuid(&p.request_id) {
        return Err(invalid("managed request UUID invalid"));
    }
    let task = summary(c, &p.session_id, &p.request_id, live_runtime)?;
    let original = super::super::original_input(c, &task.runtime_id, &p.request_id)?
        .ok_or_else(|| invalid("managed original task unavailable"))?;
    let state = load(c, &task.runtime_id, &p.request_id)?;
    let original_task = if let Some(text) = state.as_ref().and_then(|s| s.authorized_task.clone()) {
        if envelope(&p.request_id, &text)?.as_bytes() != original.bytes() {
            return Err(invalid("managed original task does not match witness"));
        }
        ClaudeTaskText {
            text: Some(text),
            managed_envelope: None,
            fidelity: "authorizedTask".into(),
        }
    } else {
        ClaudeTaskText {
            text: None,
            managed_envelope: Some(
                std::str::from_utf8(original.bytes())
                    .map_err(|_| invalid("managed envelope is not UTF8"))?
                    .into(),
            ),
            fidelity: "exactManagedEnvelope".into(),
        }
    };
    Ok(ClaudeTaskDetail {
        session_id: p.session_id.clone(),
        task,
        original_task,
        observations: state
            .as_ref()
            .map(|s| {
                s.observations
                    .clone()
                    .into_iter()
                    .map(observation)
                    .collect()
            })
            .unwrap_or_default(),
        latest_observation: state
            .as_ref()
            .and_then(|s| s.latest.clone())
            .map(observation),
        waiting_observation_no: state
            .and_then(|s| s.waiting_observation_no)
            .map(|n| n.to_string()),
        observed_at: crate::now_ts(),
    })
}
