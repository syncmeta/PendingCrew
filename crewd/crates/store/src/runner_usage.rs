//! Append-only snapshots. Dedup is exact canonical JSON plus explicit provider identity.
use crate::{Result, StoreError};
use crewd_proto::{RunnerUsagePage, RunnerUsageRecord};
use rusqlite::{Connection, params};
pub const MAX_JSON_BYTES: usize = 64 * 1024;
pub const MAX_ID_BYTES: usize = 4096;
fn invalid(message: &str) -> StoreError {
    StoreError::Invalid(message.into())
}

pub struct Snapshot<'a> {
    pub provider: &'a str,
    pub thread_id: &'a str,
    pub turn_id: Option<&'a str>,
    pub token_usage_json: &'a str,
}
pub fn append(
    conn: &Connection,
    session: &str,
    snapshot: Snapshot<'_>,
    observed_ms: i64,
) -> Result<bool> {
    if conn.is_autocommit() {
        return Err(invalid("usage append requires a single-writer transaction"));
    }
    if session.is_empty()
        || session.len() > MAX_ID_BYTES
        || snapshot.provider.is_empty()
        || snapshot.provider.len() > 128
        || snapshot.thread_id.is_empty()
        || snapshot.thread_id.len() > MAX_ID_BYTES
        || snapshot.turn_id.is_some_and(|t| t.len() > MAX_ID_BYTES)
        || snapshot.token_usage_json.len() > MAX_JSON_BYTES
    {
        return Err(invalid("usage snapshot exceeds identity or payload bounds"));
    }
    let raw: serde_json::Value = serde_json::from_str(snapshot.token_usage_json)?;
    if !raw.is_object() {
        return Err(invalid("usage snapshot must be an object"));
    }
    let raw = serde_json::to_string(&raw)?;
    if raw.len() > MAX_JSON_BYTES {
        return Err(invalid("canonical usage snapshot exceeds payload bound"));
    }
    let turn = serde_json::to_string(&snapshot.turn_id)?;
    let inserted = conn.execute("INSERT INTO runner_usage(session_id,provider,thread_id,turn_id_json,token_usage_json,observed_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(session_id,provider,thread_id,turn_id_json,token_usage_json) DO NOTHING", params![session,snapshot.provider,snapshot.thread_id,turn,raw,crate::ts_from_ms(observed_ms)])?;
    Ok(inserted == 1)
}

pub fn list(conn: &Connection, session: &str, after: i64, limit: u16) -> Result<RunnerUsagePage> {
    if after < 0 || !(1..=200).contains(&limit) {
        return Err(invalid("invalid usage page bounds"));
    }
    let mut query = conn.prepare("SELECT id,provider,thread_id,turn_id_json,token_usage_json,observed_at FROM runner_usage WHERE session_id=?1 AND id>?2 ORDER BY id LIMIT ?3")?;
    let mut items = query
        .query_map(params![session, after, i64::from(limit) + 1], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?
        .map(|row| {
            let (id, provider, thread_id, turn, token_usage_json, observed_at) = row?;
            Ok(RunnerUsageRecord {
                id: id.to_string(),
                session_id: session.into(),
                provider,
                thread_id,
                turn_id: serde_json::from_str(&turn)?,
                token_usage_json,
                observed_at,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let more = items.len() > usize::from(limit);
    items.truncate(usize::from(limit));
    let next_after = more.then(|| items.last().expect("positive limit").id.clone());
    Ok(RunnerUsagePage { items, next_after })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;
    use crewd_proto::{Crew, Session};
    fn seed(s: &Store) {
        crate::crews::upsert(
            s.conn(),
            &Crew {
                id: "a".into(),
                title: "synthetic".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        crate::sessions::upsert(
            s.conn(),
            &Session {
                id: "s".into(),
                crew_id: "a".into(),
                executor: "codex".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    }
    fn put(s: &Store, turn: Option<&str>, raw: &str, at: i64) -> bool {
        let tx = s.conn().unchecked_transaction().unwrap();
        let inserted = append(
            &tx,
            "s",
            Snapshot {
                provider: "codex",
                thread_id: "thread",
                turn_id: turn,
                token_usage_json: raw,
            },
            at,
        )
        .unwrap();
        tx.commit().unwrap();
        inserted
    }
    #[test]
    fn usage_snapshots_survive_reopen_and_dedup_without_inferred_sums_or_turns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let missing =
            r#"{"last":{"inputTokens":10},"total":{"totalTokens":20},"future":{"kept":true}}"#;
        let zero = r#"{"last":{"cacheWriteInputTokens":0,"inputTokens":10},"total":{"totalTokens":20},"future":{"kept":true}}"#;
        let first_id;
        {
            let s = Store::open(&path).unwrap();
            seed(&s);
            assert!(put(&s, None, missing, 100));
            first_id = list(s.conn(), "s", 0, 1).unwrap().items[0].id.clone();
            assert!(!put(&s, None, missing, 200));
            assert!(!put(
                &s,
                None,
                r#"{"future":{"kept":true},"total":{"totalTokens":20},"last":{"inputTokens":10}}"#,
                300
            ));
            assert!(put(&s, None, zero, 400));
            assert!(put(&s, Some(""), missing, 500));
            assert!(put(
                &s,
                Some("explicit-turn"),
                r#"{"total":{"totalTokens":21}}"#,
                600
            ));
            let p = list(s.conn(), "s", 0, 1).unwrap();
            assert_eq!(p.items[0].observed_at, crate::ts_from_ms(100));
            assert_eq!(p.next_after, Some(first_id.clone()));
            assert_eq!(p.items[0].turn_id, None);
        }
        let s = Store::open(&path).unwrap();
        let p = list(s.conn(), "s", first_id.parse().unwrap(), 200).unwrap();
        assert_eq!(p.items.len(), 3);
        assert_eq!(
            p.items[0].token_usage_json,
            serde_json::to_string(&serde_json::from_str::<serde_json::Value>(zero).unwrap())
                .unwrap()
        );
        assert_eq!(p.items[1].turn_id.as_deref(), Some(""));
        assert_eq!(p.items[2].turn_id.as_deref(), Some("explicit-turn"));
        assert!(p.next_after.is_none());
        let first = list(s.conn(), "s", 0, 1).unwrap().items.remove(0);
        assert_eq!(first.id, first_id);
        assert!(!first.token_usage_json.contains("cacheWriteInputTokens"));
        assert!(
            !put(&s, None, missing, 700),
            "dedup survives process restart"
        );
    }
    #[test]
    fn usage_ids_are_exact_above_js_precision_and_writes_are_transactional() {
        let s = Store::open_in_memory().unwrap();
        seed(&s);
        s.conn()
            .execute(
                "INSERT INTO sqlite_sequence(name,seq) VALUES('runner_usage',9007199254740992)",
                [],
            )
            .unwrap();
        let raw = r#"{"total":{"totalTokens":9007199254740993}}"#;
        assert!(
            append(
                s.conn(),
                "s",
                Snapshot {
                    provider: "codex",
                    thread_id: "thread",
                    turn_id: None,
                    token_usage_json: raw
                },
                0
            )
            .is_err()
        );
        {
            let tx = s.conn().unchecked_transaction().unwrap();
            append(
                &tx,
                "s",
                Snapshot {
                    provider: "codex",
                    thread_id: "thread",
                    turn_id: None,
                    token_usage_json: raw,
                },
                0,
            )
            .unwrap();
            // A dropped transaction creates no evidence.
        }
        assert!(list(s.conn(), "s", 0, 50).unwrap().items.is_empty());
        assert!(put(&s, None, raw, 0));
        let record = list(s.conn(), "s", 0, 50).unwrap().items.remove(0);
        assert_eq!(record.id, "9007199254740993");
        assert!(record.token_usage_json.contains("9007199254740993"));
        s.conn()
            .execute("DELETE FROM sessions WHERE id='s'", [])
            .unwrap();
        assert!(list(s.conn(), "s", 0, 50).unwrap().items.is_empty());
    }
    #[test]
    fn v7_upgrade_has_formal_usage_schema_and_keeps_existing_state() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        conn.execute_batch("DROP TABLE runner_usage; PRAGMA user_version=7;")
            .unwrap();
        conn.execute(
            "INSERT INTO meta(key,value) VALUES('usage-migration-marker','kept')",
            [],
        )
        .unwrap();
        crate::schema::migrate(&conn).unwrap();
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            8
        );
        assert_eq!(
            conn.query_row(
                "SELECT value FROM meta WHERE key='usage-migration-marker'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "kept"
        );
        assert!(list(&conn, "unused", 0, 50).unwrap().items.is_empty());
    }
}
