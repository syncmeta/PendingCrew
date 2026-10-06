use crate::Ctx;
use crate::model::LMessage;
use crate::rows::{conv_att, io_fatal, norm_ts, read_file, read_rows_indexed};
use crewd_proto::{Mention, MentionKind, Message, SenderKind};
use rusqlite::Connection;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 旧版白板读失败时会重建一行内存警示，不是真实消息。
const READ_FAILURE_ID: &str = "whiteboard-read-failure";

pub fn import_board(tx: &Connection, cx: &mut Ctx, crew_id: &str) -> crewd_store::Result<()> {
    let path = cx.src.join("whiteboards").join(format!("{crew_id}.json"));
    let rows: Vec<(usize, LMessage)> = read_rows_indexed(cx, &path)?;
    for (i, m) in rows {
        if !insert_one(tx, cx, crew_id, m, &path, Some(i))? {
            // 这行没完整进库（重复 id 或嵌套未知键）：整份文件留给归档原样保存。
            cx.consumed.remove(&path);
        }
    }
    Ok(())
}

/// `whiteboards/outbox/<crewId>.<ts>.<seq>.<uuid>.json`：还没合并进白板的写入。按文件名排序导入。
pub fn import_outbox(tx: &Connection, cx: &mut Ctx) -> crewd_store::Result<()> {
    let dir = cx.src.join("whiteboards").join("outbox");
    let rd = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io_fatal(&dir, e)),
    };
    let mut files: Vec<PathBuf> = Vec::new();
    for e in rd {
        let p = e.map_err(|e| io_fatal(&dir, e))?.path();
        if p.extension().is_some_and(|x| x == "json") {
            files.push(p);
        }
    }
    files.sort();
    for f in files {
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let crew_id = name.split('.').next().unwrap_or_default().to_string();
        // 列目录之后被旧版合并删掉了：它已在白板里，不算丢。
        let Some(bytes) = read_file(&f)? else {
            continue;
        };
        let m: LMessage = match serde_json::from_slice(&bytes) {
            Ok(m) => m,
            Err(e) => {
                cx.report.skip(cx.src, &f, None, e.to_string());
                continue;
            }
        };
        if !crewd_store::crews::exists(tx, &crew_id)? {
            cx.report
                .skip(cx.src, &f, None, format!("所属机组 {crew_id} 不存在"));
            continue;
        }
        cx.report.bump("outboxMessages");
        if insert_one(tx, cx, &crew_id, m, &f, None)? {
            cx.consumed.insert(f.clone());
        }
    }
    Ok(())
}

/// 导入一行；返回 false 表示这行没完整进库（id 重复，嵌套对象里有没处放的键，或 sender / mention
/// 词认不出——后者记进报告的 skipped 并加一条警告，裁定 6 / 控制者裁定 R14）。
/// 旧版合成的读失败警示行有意丢弃，算处理完。`file` / `index` 只用于报告定位。
fn insert_one(
    tx: &Connection,
    cx: &mut Ctx,
    crew_id: &str,
    m: LMessage,
    file: &Path,
    index: Option<usize>,
) -> crewd_store::Result<bool> {
    if m.id == READ_FAILURE_ID {
        cx.report.bump("syntheticRowsDropped");
        return Ok(true);
    }
    let Some(sender_kind) = crewd_store::messages::normalize_sender_kind(&m.sender_kind) else {
        cx.report.bump("messagesWithUnknownSender");
        let why = format!(
            "机组 {crew_id} 的消息 {} senderKind={:?} 认不出，未导入（原件留在归档里）",
            m.id, m.sender_kind
        );
        cx.report.skip(cx.src, file, index, why.clone());
        cx.report.warn(why);
        return Ok(false);
    };
    if let Some(bad) = m
        .mentions
        .iter()
        .find(|x| !crewd_store::messages::MENTION_KINDS.contains(&x.kind.as_str()))
    {
        cx.report.bump("messagesWithUnknownMention");
        let why = format!(
            "机组 {crew_id} 的消息 {} 的 mention kind={:?} 认不出，未导入（原件留在归档里）",
            m.id, bad.kind
        );
        cx.report.skip(cx.src, file, index, why.clone());
        cx.report.warn(why);
        return Ok(false);
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
        channel_id: crewd_store::channels::lobby_id(crew_id),
        sender_kind: SenderKind::parse(sender_kind)
            .expect("normalize_sender_kind 只返回词表内的值"),
        sender_session_id: m.sender_session_id,
        sender_name: m.sender_name,
        text: m.text,
        category: m.category,
        in_reply_to: m.in_reply_to,
        mentions: m
            .mentions
            .into_iter()
            .map(|x| Mention {
                kind: MentionKind::parse(&x.kind).expect("上面已检查"),
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
