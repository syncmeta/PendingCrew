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

    let mut parsed: Vec<(LCrew, String)> = Vec::new();
    for (i, v) in rows.into_iter().enumerate() {
        match serde_json::from_value::<LCrew>(v) {
            Ok(c) => {
                let created_at = norm_ts(cx, &c.created_at);
                parsed.push((c, created_at));
            }
            Err(e) => cx.report.skip(cx.src, &path, Some(i), e.to_string()),
        }
    }
    // 组织根至多一个（v4 的部分唯一索引在 upsert 之前就在）：多个 isBuiltin 时留 createdAt 最早的（再按 id），
    // 其余降为普通机组并警告——与 `backfill_v4` 对已有库的口径相同。
    let root_id = parsed
        .iter()
        .filter(|(c, _)| c.is_builtin)
        .min_by(|(a, at), (b, bt)| (at, &a.id).cmp(&(bt, &b.id)))
        .map(|(c, _)| c.id.clone());
    for (c, _) in &parsed {
        if c.is_builtin && Some(&c.id) != root_id.as_ref() {
            cx.report.warn(format!(
                "机组 {} 也标着 isBuiltin；组织根只留最早的 {}，它按普通机组导入",
                c.id,
                root_id.as_deref().unwrap_or_default()
            ));
        }
    }

    let mut crews = Vec::new();
    let mut parents: Vec<(String, Vec<String>)> = Vec::new();
    for (c, created_at) in parsed {
        let hidden_at = c.manually_hidden_at.as_deref().map(|t| norm_ts(cx, t));
        let crew = Crew {
            id: c.id.clone(),
            title: c.title.clone(),
            number: c.crew_number,
            management: c.is_builtin && Some(&c.id) == root_id.as_ref(),
            parent_ids: vec![],
            working_directory: c.working_directory.clone(),
            captain_executor: c
                .captain_agent_kind
                .as_deref()
                .map(crate::sessions::executor_of),
            hidden_at,
            created_at,
            updated_at: norm_ts(cx, &c.updated_at),
            ..Default::default()
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
