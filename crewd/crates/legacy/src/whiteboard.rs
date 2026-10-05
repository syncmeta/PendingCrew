use crate::Ctx;
use crate::model::LMessage;
use crate::rows::{conv_att, norm_ts, read_rows};
use crewd_proto::{Mention, Message};
use rusqlite::Connection;
use serde_json::Value;
use std::path::PathBuf;

/// 旧版白板读失败时会重建一行内存警示，不是真实消息。
const READ_FAILURE_ID: &str = "whiteboard-read-failure";

pub fn import_board(tx: &Connection, cx: &mut Ctx, crew_id: &str) -> crewd_store::Result<()> {
    let path = cx.src.join("whiteboards").join(format!("{crew_id}.json"));
    let rows: Vec<LMessage> = read_rows(cx, &path);
    for m in rows {
        if !insert_one(tx, cx, crew_id, m)? {
            // 这行没完整进库（重复 id 或嵌套未知键）：整份文件留给归档原样保存。
            cx.consumed.remove(&path);
        }
    }
    Ok(())
}

/// `whiteboards/outbox/<crewId>.<ts>.<seq>.<uuid>.json`：还没合并进白板的写入。按文件名排序导入。
pub fn import_outbox(tx: &Connection, cx: &mut Ctx) -> crewd_store::Result<()> {
    let dir = cx.src.join("whiteboards").join("outbox");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Ok(());
    };
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    for f in files {
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let crew_id = name.split('.').next().unwrap_or_default().to_string();
        let m: LMessage = match std::fs::read(&f)
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
        {
            Ok(m) => m,
            Err(e) => {
                cx.report.skip(cx.src, &f, None, e);
                continue;
            }
        };
        if !crewd_store::crews::exists(tx, &crew_id)? {
            cx.report
                .skip(cx.src, &f, None, format!("所属机组 {crew_id} 不存在"));
            continue;
        }
        cx.report.bump("outboxMessages");
        if insert_one(tx, cx, &crew_id, m)? {
            cx.consumed.insert(f.clone());
        }
    }
    Ok(())
}

/// 导入一行；返回 false 表示这行没完整进库（id 重复，或嵌套对象里有没处放的键）。
/// 旧版合成的读失败警示行有意丢弃，算处理完。
fn insert_one(
    tx: &Connection,
    cx: &mut Ctx,
    crew_id: &str,
    m: LMessage,
) -> crewd_store::Result<bool> {
    if m.id == READ_FAILURE_ID {
        cx.report.bump("syntheticRowsDropped");
        return Ok(true);
    }
    let nested_extra = m.has_nested_extra();
    let created_at = norm_ts(cx, &m.created_at);
    let mut attachments = Vec::with_capacity(m.attachments.len());
    for a in m.attachments {
        attachments.push(conv_att(cx, a));
    }
    let msg = Message {
        seq: 0,
        id: m.id,
        crew_id: crew_id.to_string(),
        sender_kind: m.sender_kind,
        sender_session_id: m.sender_session_id,
        sender_name: m.sender_name,
        text: m.text,
        category: m.category,
        in_reply_to: m.in_reply_to,
        mentions: m
            .mentions
            .into_iter()
            .map(|x| Mention {
                kind: x.kind,
                target_id: x.target_id,
            })
            .collect(),
        attachments,
        created_at,
    };
    Ok(
        match crewd_store::messages::insert(tx, &msg, Some(&Value::Object(m.extra)))? {
            Some(_) => {
                cx.report.bump("messages");
                if nested_extra {
                    cx.report.bump("rowsWithUnmodeledNestedFields");
                }
                !nested_extra
            }
            None => {
                cx.report.bump("duplicateMessages");
                false
            }
        },
    )
}
