use crate::Ctx;
use crate::model::LTodo;
use crate::rows::{conv_att, norm_ts, read_rows_indexed};
use crewd_proto::{Ledger, Todo, TodoResponse};
use crewd_store::StoreError;
use rusqlite::Connection;
use serde_json::Value;

pub fn import(
    tx: &Connection,
    cx: &mut Ctx,
    crew_id: &str,
    ledger: Ledger,
) -> crewd_store::Result<()> {
    let (suffix, count_key) = match ledger {
        Ledger::Agent => ("todos", "agentTodos"),
        Ledger::Human => ("human-todos", "humanTodos"),
    };
    let path = cx
        .src
        .join("whiteboards")
        .join(format!("{crew_id}.{suffix}.json"));
    let rows: Vec<(usize, LTodo)> = read_rows_indexed(cx, &path)?;
    for (index, t) in rows {
        let nested_extra = t.has_nested_extra();
        let mut responses = Vec::with_capacity(t.responses.len());
        for r in t.responses {
            let mut atts = Vec::with_capacity(r.attachments.len());
            for a in r.attachments {
                atts.push(conv_att(cx, a));
            }
            let created_at = norm_ts(cx, &r.created_at);
            responses.push(TodoResponse {
                id: r.id,
                session_id: r.session_id,
                sender_name: r.sender_name,
                text: r.text,
                status: r.status,
                attachments: atts,
                created_at,
            });
        }
        let mut attachments = Vec::with_capacity(t.attachments.len());
        for a in t.attachments {
            attachments.push(conv_att(cx, a));
        }
        let created_at = norm_ts(cx, &t.created_at);
        let updated_at = t.updated_at.as_deref().map(|x| norm_ts(cx, x));
        let deleted_at = t.deleted_at.as_deref().map(|x| norm_ts(cx, x));
        let n_resp = responses.len();
        let todo = Todo {
            id: t.id,
            crew_id: crew_id.to_string(),
            ledger,
            number: t.number,
            text: t.text,
            status: t.status,
            created_at,
            updated_at,
            deleted_at,
            attachments,
            responses,
        };
        match crewd_store::todos::insert(tx, &todo, Some(&Value::Object(t.extra))) {
            Ok(()) => {
                cx.report.bump(count_key);
                cx.report.add("todoResponses", n_resp);
                if nested_extra {
                    // 嵌套对象的未知键没有列可放：整份文件留给归档原样保存。
                    cx.report.bump("rowsWithUnmodeledNestedFields");
                    cx.consumed.remove(&path);
                }
            }
            Err(StoreError::Sql(e)) if crewd_store::is_unique_violation(&e) => {
                cx.report.skip(
                    cx.src,
                    &path,
                    Some(index),
                    format!("编号 #{} / id {} 重复: {e}", todo.number, todo.id),
                );
                // 这行没进库：别把整份文件算作已消费，让归档原样保存它。
                cx.consumed.remove(&path);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
