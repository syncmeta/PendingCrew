use crate::Ctx;
use crate::crews::ImportedCrew;
use crate::model::{LAgentSession, LMember};
use crate::rows::{norm_ts, read_rows};
use chrono::DateTime;
use crewd_proto::Session;
use rusqlite::Connection;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

fn role_of(session_id: &str) -> String {
    if session_id.starts_with("captain-") {
        "captain"
    } else {
        "worker"
    }
    .to_string()
}

/// `a` 严格晚于 `b`。两边都能解析就比时刻（时区偏移、毫秒都算数）；任一边解析不了，退回原串比较。
fn is_newer(a: &str, b: &str) -> bool {
    match (
        DateTime::parse_from_rfc3339(a.trim()),
        DateTime::parse_from_rfc3339(b.trim()),
    ) {
        (Ok(a), Ok(b)) => a > b,
        _ => a > b,
    }
}

/// 成员表里没建模的字段（如 `extensionNumber`）放进 session extra 的 `member` 键。
fn with_member_extra(mut extra: Map<String, Value>, m: Option<&LMember>) -> Map<String, Value> {
    if let Some(m) = m
        && !m.extra.is_empty()
    {
        extra.insert("member".into(), Value::Object(m.extra.clone()));
    }
    extra
}

/// 旧版 runner 种类 → 执行器类型（spec §16）。只有 `claude_code` 改名；`codex`、`terminal`
/// 以及认不出的原样保留。
pub fn executor_of(kind: &str) -> String {
    match kind {
        "claude_code" => "claude".to_string(),
        other => other.to_string(),
    }
}

/// session = `agent-sessions.json` 的记录（同 id 取 updatedAt 最新）+ 机组成员表里的显示名；
/// 只在成员表里出现、没有记录的，以 executor="unknown" 入库。
pub fn import(tx: &Connection, cx: &mut Ctx, crews: &[ImportedCrew]) -> crewd_store::Result<()> {
    let path = cx.src.join("whiteboards").join("agent-sessions.json");
    let recs: Vec<LAgentSession> = read_rows(cx, &path)?;

    let mut by_id: BTreeMap<String, LAgentSession> = BTreeMap::new();
    for r in recs {
        if let Some(old) = by_id.get(&r.session_id) {
            // 每条落选的记录（新来的或已存的）只计一次。落选的记录不进库（例如旧的
            // agentSessionId），所以整份文件留给归档原样保存。
            cx.report.bump("sessionRecordsSuperseded");
            cx.consumed.remove(&path);
            if !is_newer(&r.updated_at, &old.updated_at) {
                continue;
            }
        }
        by_id.insert(r.session_id.clone(), r);
    }

    let members: BTreeMap<&str, &LMember> = crews
        .iter()
        .flat_map(|c| c.members.iter().map(|m| (m.session_id.as_str(), m)))
        .collect();
    let mut seen = BTreeSet::new();

    for r in by_id.into_values() {
        if !crews.iter().any(|c| c.id == r.crew_id) {
            cx.report.bump("sessionsForUnknownCrew");
            cx.report.skip(
                cx.src,
                &path,
                None,
                format!("session {} 的机组 {} 不存在", r.session_id, r.crew_id),
            );
            cx.consumed.remove(&path);
            continue;
        }
        let s = Session {
            id: r.session_id.clone(),
            crew_id: r.crew_id.clone(),
            role: role_of(&r.session_id),
            executor: executor_of(&r.kind),
            agent_session_id: Some(r.agent_session_id.clone()),
            display_name: members
                .get(r.session_id.as_str())
                .map(|m| m.display_name.clone()),
            working_directory: r.working_directory.clone(),
            model: r.model.clone(),
            effort: r.effort.clone(),
            updated_at: norm_ts(cx, &r.updated_at),
        };
        let extra = with_member_extra(r.extra, members.get(r.session_id.as_str()).copied());
        crewd_store::sessions::upsert(tx, &s, Some(&Value::Object(extra)))?;
        cx.report.bump("sessions");
        seen.insert(r.session_id);
    }

    for c in crews {
        for m in &c.members {
            if !seen.insert(m.session_id.clone()) {
                continue;
            }
            let s = Session {
                id: m.session_id.clone(),
                crew_id: c.id.clone(),
                role: role_of(&m.session_id),
                executor: "unknown".into(),
                agent_session_id: None,
                display_name: Some(m.display_name.clone()),
                working_directory: None,
                model: None,
                effort: None,
                updated_at: norm_ts(cx, &m.created_at),
            };
            let extra = with_member_extra(Map::new(), Some(m));
            let extra = (!extra.is_empty()).then_some(Value::Object(extra));
            crewd_store::sessions::upsert(tx, &s, extra.as_ref())?;
            cx.report.bump("sessions");
            cx.report.bump("sessionsWithoutRecord");
        }
    }
    Ok(())
}
