//! Agent 那本 Todo（人派给机组的活）的回应：追加一条，不改状态（§9：状态 / 证据参数已删）。
//! 不发客户端事件：`todo.list` 在 M3 不变，客户端重拉即可（M4b 重做 Todo 时补事件）。
use crate::refusal::{Code, Refusal};
use crate::{CollabError, Result, Tx, sessions};
use crewd_proto::TodoResponse;
use rusqlite::{OptionalExtension, params};

/// `session` 所在机组的 Agent Todo `#number` 追加回应。删掉的、不存在的 → NotFound；空回应 → Invalid。
pub fn respond(tx: &mut Tx<'_>, session: &str, number: i64, text: &str) -> Result<TodoResponse> {
    let text = text.trim();
    if text.is_empty() {
        return Err(CollabError::Invalid(Refusal::new(Code::TodoResponseEmpty)));
    }
    let s = sessions::get(tx.conn(), session)?;
    let todo: Option<String> = tx
        .conn()
        .query_row(
            "SELECT id FROM todos WHERE crew_id=?1 AND ledger='agent' AND number=?2 AND deleted_at IS NULL",
            params![s.crew_id, number],
            |r| r.get(0),
        )
        .optional()?;
    let Some(todo) = todo else {
        return Err(CollabError::NotFound(
            Refusal::new(Code::TodoNotFound).arg("number", number.to_string()),
        ));
    };
    let ord: i64 = tx.conn().query_row(
        "SELECT COALESCE(MAX(ord),-1)+1 FROM todo_responses WHERE todo_id=?1",
        [&todo],
        |r| r.get(0),
    )?;
    let r = TodoResponse {
        id: format!("tr-{}", uuid::Uuid::new_v4().simple()),
        session_id: s.id.clone(),
        sender_name: Some(sessions::display_name(tx.conn(), &s.id)?),
        text: text.to_string(),
        status: None,
        attachments: vec![],
        created_at: tx.now_ts(),
    };
    tx.conn().execute(
        "INSERT INTO todo_responses(id,todo_id,ord,session_id,sender_name,text,status,attachments,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,NULL,'[]',?7)",
        params![r.id, todo, ord, r.session_id, r.sender_name, r.text, r.created_at],
    )?;
    tx.conn().execute(
        "UPDATE todos SET updated_at=?2 WHERE id=?1",
        params![todo, r.created_at],
    )?;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{count, err, ok, store};

    #[test]
    fn respond_appends_in_order_without_touching_status() {
        let mut s = store();
        s.conn()
            .execute(
                "INSERT INTO todos(id,crew_id,ledger,number,text,status,created_at) VALUES('t1','crew-a','agent',1,'示例：补文档','pending','t')",
                [],
            )
            .unwrap();
        ok(&mut s, |tx| respond(tx, "w-a1", 1, "认领，今天做"));
        ok(&mut s, |tx| respond(tx, "cap-a", 1, "好"));
        assert_eq!(
            count(&s, "SELECT COUNT(*) FROM todo_responses WHERE todo_id='t1'"),
            2
        );
        assert_eq!(
            count(&s, "SELECT ord FROM todo_responses WHERE text='好'"),
            1
        );
        let status: String = s
            .conn()
            .query_row("SELECT status FROM todos WHERE id='t1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "pending", "回应不改状态（§9）");
        assert!(matches!(
            err(&mut s, |tx| respond(tx, "w-a1", 99, "x")),
            CollabError::NotFound(_)
        ));
        assert!(
            matches!(
                err(&mut s, |tx| respond(tx, "w-b1", 1, "x")),
                CollabError::NotFound(_)
            ),
            "别的机组的 Todo 不归它"
        );
        assert!(matches!(
            err(&mut s, |tx| respond(tx, "w-a1", 1, "  ")),
            CollabError::Invalid(_)
        ));
    }
}
