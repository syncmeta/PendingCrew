use crate::Ctx;
use crate::crews::ImportedCrew;
use crate::model::LAgentSession;
use crate::rows::{norm_ts, read_rows};
use crewd_proto::Session;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn role_of(session_id: &str) -> String {
    if session_id.starts_with("captain-") {
        "captain"
    } else {
        "worker"
    }
    .to_string()
}

/// session = `agent-sessions.json` 的记录（同 id 取 updatedAt 最新）+ 机组成员表里的显示名；
/// 只在成员表里出现、没有记录的，以 agent_kind="unknown" 入库。
pub fn import(tx: &Connection, cx: &mut Ctx, crews: &[ImportedCrew]) -> crewd_store::Result<()> {
    let path = cx.src.join("whiteboards").join("agent-sessions.json");
    let recs: Vec<LAgentSession> = read_rows(cx, &path);

    let mut by_id: BTreeMap<String, LAgentSession> = BTreeMap::new();
    for r in recs {
        let keep_old = by_id
            .get(&r.session_id)
            .is_some_and(|old| old.updated_at >= r.updated_at);
        if keep_old {
            cx.report.bump("sessionRecordsSuperseded");
        } else {
            if by_id.contains_key(&r.session_id) {
                cx.report.bump("sessionRecordsSuperseded");
            }
            by_id.insert(r.session_id.clone(), r);
        }
    }

    let names: BTreeMap<String, String> = crews
        .iter()
        .flat_map(|c| {
            c.members
                .iter()
                .map(|m| (m.session_id.clone(), m.display_name.clone()))
        })
        .collect();
    let mut seen = BTreeSet::new();

    for r in by_id.into_values() {
        if !crews.iter().any(|c| c.id == r.crew_id) {
            cx.report.bump("sessionsForUnknownCrew");
            continue;
        }
        let s = Session {
            id: r.session_id.clone(),
            crew_id: r.crew_id.clone(),
            role: role_of(&r.session_id),
            agent_kind: r.kind.clone(),
            agent_session_id: Some(r.agent_session_id.clone()),
            display_name: names.get(&r.session_id).cloned(),
            working_directory: r.working_directory.clone(),
            model: r.model.clone(),
            effort: r.effort.clone(),
            updated_at: norm_ts(cx, &r.updated_at),
        };
        crewd_store::sessions::upsert(tx, &s, Some(&Value::Object(r.extra)))?;
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
                agent_kind: "unknown".into(),
                agent_session_id: None,
                display_name: Some(m.display_name.clone()),
                working_directory: None,
                model: None,
                effort: None,
                updated_at: norm_ts(cx, &m.created_at),
            };
            crewd_store::sessions::upsert(tx, &s, None)?;
            cx.report.bump("sessions");
            cx.report.bump("sessionsWithoutRecord");
        }
    }
    Ok(())
}
