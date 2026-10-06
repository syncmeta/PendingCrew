use crate::Ctx;
use crate::model::{LCrew, LMember};
use crate::rows::{norm_ts, read_file};
use crewd_proto::Crew;
use rusqlite::Connection;
use serde_json::Value;

pub struct ImportedCrew {
    pub id: String,
    pub members: Vec<LMember>,
}

/// `local-crews.json`：旧版要求整份可解码；这里逐行宽容，坏行跳过。
pub fn import(tx: &Connection, cx: &mut Ctx) -> crewd_store::Result<Vec<ImportedCrew>> {
    let path = cx.src.join("local-crews.json");
    let Some(bytes) = read_file(&path)? else {
        cx.report
            .warn("源目录里没有 local-crews.json，没有机组可导入");
        return Ok(vec![]);
    };
    let file: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            cx.report
                .skip(cx.src, &path, None, format!("JSON 无效: {e}"));
            return Ok(vec![]);
        }
    };
    // 这个文件从不标 consumed：归档总会原样保存它。顶层的 nextCrewNumber、成员的 createdAt、
    // 坏掉的机组行等没有建模的东西都还在原件里。
    let Some(rows) = file.get("crews").and_then(Value::as_array).cloned() else {
        cx.report.skip(cx.src, &path, None, "缺少 crews 数组");
        return Ok(vec![]);
    };

    let mut crews = Vec::new();
    let mut parents: Vec<(String, Vec<String>)> = Vec::new();
    for (i, v) in rows.into_iter().enumerate() {
        let c: LCrew = match serde_json::from_value(v) {
            Ok(c) => c,
            Err(e) => {
                cx.report.skip(cx.src, &path, Some(i), e.to_string());
                continue;
            }
        };
        let hidden_at = c.manually_hidden_at.as_deref().map(|t| norm_ts(cx, t));
        let crew = Crew {
            id: c.id.clone(),
            title: c.title.clone(),
            number: c.crew_number,
            builtin: c.is_builtin,
            parent_ids: vec![],
            working_directory: c.working_directory.clone(),
            captain_executor: c
                .captain_agent_kind
                .as_deref()
                .map(crate::sessions::executor_of),
            hidden_at,
            created_at: norm_ts(cx, &c.created_at),
            updated_at: norm_ts(cx, &c.updated_at),
        };
        crewd_store::crews::upsert(tx, &crew, Some(&Value::Object(c.extra)))?;
        crewd_store::channels::ensure_lobby(tx, &crew.id, &crew.created_at)?;
        cx.report.bump("crews");
        parents.push((c.id.clone(), c.parent_crew_ids));
        crews.push(ImportedCrew {
            id: c.id,
            members: c.session_members,
        });
    }

    for (id, mut ps) in parents {
        let mut dedup = std::collections::BTreeSet::new();
        ps.retain(|p| dedup.insert(p.clone()));
        let mut known: Vec<String> = Vec::new();
        for p in ps {
            if crews.iter().any(|c| c.id == p) {
                known.push(p);
            } else {
                cx.report.bump("danglingParentEdges");
            }
        }
        crewd_store::crews::set_parents(tx, &id, &known)?;
    }
    Ok(crews)
}
