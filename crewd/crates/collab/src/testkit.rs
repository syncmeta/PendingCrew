//! 单测夹具（cfg(test)）。全是编的名字，不碰真实数据（仓库公开）。
use crate::{Change, CollabError, Result, Tx};
use crewd_proto::{Crew, Role, Session};
use crewd_store::Store;

/// 2026-09-21T13:33:20Z 附近。
pub(crate) const NOW: i64 = 1_790_000_000_000;
pub(crate) const MIN: i64 = 60_000;

/// 合成的机组：
/// ```text
/// chief（总机组 = 管理机组，号 0，无 session，不在树里）
/// crew-a（1 号，顶层）  机长 cap-a；w-a1（分机 2）、w-a2（分机 3）
///   └ crew-b（2 号）    机长 cap-b；w-b1（分机 2）
/// crew-x（3 号，顶层）  机长 cap-x
/// ```
/// 大厅、机长指针、next_ext、游标由 `backfill_v3` 建好，总机组的标记由 `backfill_v4` 落定（与迁移同一条路）。
pub(crate) fn store() -> Store {
    populate(Store::open_in_memory().unwrap())
}
/// A genuine pre-V3 fixture; callers seed legacy requests before migrating it.
pub(crate) fn v31_store() -> Store {
    let s = populate(crewd_store::testing::open_legacy_store(31).unwrap());
    assert_eq!(
        s.conn()
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        31
    );
    s
}
pub(crate) fn v8_store() -> Store {
    populate(crewd_store::testing::open_v8_store().unwrap())
}
fn populate(s: Store) -> Store {
    let c = s.conn();
    for (id, title, number, management, at) in [
        (
            "crew-a",
            "机组甲",
            Some(1),
            false,
            "2026-09-01T00:00:00.000Z",
        ),
        (
            "crew-b",
            "机组乙",
            Some(2),
            false,
            "2026-09-02T00:00:00.000Z",
        ),
        (
            "crew-x",
            "机组丙",
            Some(3),
            false,
            "2026-09-03T00:00:00.000Z",
        ),
        ("chief", "总机组", None, true, "2026-08-01T00:00:00.000Z"),
    ] {
        crewd_store::crews::upsert(
            c,
            &Crew {
                id: id.into(),
                title: title.into(),
                number,
                management,
                working_directory: Some("/tmp/example".into()),
                created_at: at.into(),
                updated_at: at.into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    }
    crewd_store::crews::set_parents(c, "crew-b", &["crew-a".to_string()]).unwrap();
    for (id, crew, role, ext) in [
        ("cap-a", "crew-a", Role::Captain, None),
        ("w-a1", "crew-a", Role::Worker, Some(2)),
        ("w-a2", "crew-a", Role::Worker, Some(3)),
        ("cap-b", "crew-b", Role::Captain, None),
        ("w-b1", "crew-b", Role::Worker, Some(2)),
        ("cap-x", "crew-x", Role::Captain, None),
    ] {
        crewd_store::sessions::upsert(
            c,
            &Session {
                id: id.into(),
                crew_id: crew.into(),
                role,
                executor: "claude".into(),
                display_name: Some(format!("名字-{id}")),
                ext,
                updated_at: "2026-09-01T00:00:00.000Z".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    }
    crewd_store::schema::backfill_v3(c).unwrap();
    crewd_store::schema::backfill_v4(c).unwrap();
    s
}

/// 在总机组（管理层，读全组织）里登记一个 worker，返回它的 id。夹具本身不放总机组的 session（别的测试数着行数），
/// 要「读得到、但不在群里」的收听者时用它（终审 I4：收听只能落在读得到的群上）。
pub(crate) fn org_worker(s: &mut Store) -> String {
    ok(s, |tx| {
        crate::sessions::register(
            tx,
            crate::sessions::NewSession {
                crew_id: "chief".into(),
                role: Role::Worker,
                executor: "claude".into(),
                title: Some("管理层示例".into()),
                ..Default::default()
            },
        )
    })
    .id
}

/// 在总机组里登记机长，返回它的 id。权限上它当作每个机组的机长（PM 2026-10-07）；总机组的其它 session 没有这份权限。
pub(crate) fn org_captain(s: &mut Store) -> String {
    ok(s, |tx| {
        crate::sessions::register(
            tx,
            crate::sessions::NewSession {
                crew_id: "chief".into(),
                role: Role::Captain,
                executor: "claude".into(),
                title: Some("管理层机长".into()),
                ..Default::default()
            },
        )
    })
    .id
}

/// 在 NOW 跑一个操作，失败就 panic（测试的正路径）。
pub(crate) fn ok<T>(s: &mut Store, f: impl FnOnce(&mut Tx<'_>) -> Result<T>) -> T {
    crate::run(s, NOW, f).unwrap().0
}

/// 在指定时间跑，连同变更一起返回。
pub(crate) fn ok_at<T>(
    s: &mut Store,
    now: i64,
    f: impl FnOnce(&mut Tx<'_>) -> Result<T>,
) -> (T, Vec<Change>) {
    crate::run(s, now, f).unwrap()
}

/// 在 NOW 跑一个应当失败的操作，返回错误。
pub(crate) fn err<T: std::fmt::Debug>(
    s: &mut Store,
    f: impl FnOnce(&mut Tx<'_>) -> Result<T>,
) -> CollabError {
    crate::run(s, NOW, f).unwrap_err()
}

pub(crate) fn count(s: &Store, sql: &str) -> i64 {
    s.conn().query_row(sql, [], |r| r.get(0)).unwrap()
}

/// 直接往群里插一条消息（绕过 collab 的规则，只为造测试前提），返回 seq。
pub(crate) fn msg(
    s: &Store,
    channel: &str,
    id: &str,
    sender_kind: crewd_proto::SenderKind,
    sender_session: Option<&str>,
    mentions: Vec<crewd_proto::Mention>,
) -> i64 {
    crewd_store::messages::insert(
        s.conn(),
        &crewd_proto::Message {
            id: id.into(),
            channel_id: channel.into(),
            sender_kind,
            sender_session_id: sender_session.map(Into::into),
            text: format!("合成消息 {id}"),
            mentions,
            created_at: crewd_store::ts_from_ms(NOW),
            ..Default::default()
        },
        None,
    )
    .unwrap()
    .unwrap()
}

/// Read genuine organization events; legacy archive is intentionally not consulted.
pub fn organization_events(
    conn: &rusqlite::Connection,
    crew: &str,
    after: u64,
    limit: usize,
) -> crewd_store::Result<Vec<crewd_work::Event>> {
    let mut q = conn.prepare("SELECT body FROM organization_work_events WHERE crew_id=?1 AND seq>?2 ORDER BY seq LIMIT ?3")?;
    let rows = q.query_map(rusqlite::params![crew, after as i64, limit as i64], |r| {
        r.get::<_, String>(0)
    })?;
    rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
}
pub fn public_ask(s: &crewd_store::Store, core: &str) -> String {
    let r = crewd_store::work_organization::reference_for_core(
        s.conn(),
        crewd_store::object_refs::ObjectKind::HumanTask,
        core,
    )
    .unwrap()
    .unwrap();
    format!("H{}", r.number)
}

/// Full typed row snapshot, including accessible hidden row identity on ordinary tables.
/// WITHOUT ROWID tables have their primary key in `*`; neither kind relies on scan order.
pub(crate) fn database_snapshot(s: &Store) -> Vec<(String, Vec<Vec<rusqlite::types::Value>>)> {
    let c = s.conn();
    let mut q = c
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap();
    let names = q
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    names
        .into_iter()
        .map(|name| {
            let without_rowid: bool = c
                .query_row(
                    "SELECT wr FROM pragma_table_list WHERE schema='main' AND name=?1",
                    [&name],
                    |r| r.get(0),
                )
                .unwrap();
            let mut columns = c
                .prepare("SELECT name FROM pragma_table_xinfo(?1)")
                .unwrap();
            let columns = columns
                .query_map([&name], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            let rowid = (!without_rowid)
                .then(|| {
                    ["rowid", "_rowid_", "oid"]
                        .into_iter()
                        .find(|alias| !columns.iter().any(|name| name.eq_ignore_ascii_case(alias)))
                })
                .flatten();
            let prefix = rowid.map(|alias| format!("{alias}, ")).unwrap_or_default();
            let quoted = name.replace('"', "\"\"");
            let mut q = c
                .prepare(&format!("SELECT {prefix}* FROM \"{quoted}\""))
                .unwrap();
            let n = q.column_count();
            let mut rows = q
                .query_map([], |r| {
                    (0..n)
                        .map(|i| r.get::<_, rusqlite::types::Value>(i))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            rows.sort_by_cached_key(|r| format!("{r:?}"));
            (name, rows)
        })
        .collect()
}

#[test]
fn full_snapshot_preserves_row_identity_types_duplicates_and_without_rowid_tables() {
    use rusqlite::types::Value;
    let s = store();
    s.conn().execute_batch(
        "CREATE TABLE snapshot_rows(value);
         INSERT INTO snapshot_rows(rowid,value) VALUES(8,NULL),(9,7),(10,7.0),(11,'7'),(12,X'37'),(13,'duplicate'),(14,'duplicate');
         CREATE TABLE snapshot_keys(a TEXT,b INTEGER,value,PRIMARY KEY(a,b)) WITHOUT ROWID;
         INSERT INTO snapshot_keys VALUES('b',2,NULL),('a',1,X'00FF');"
    ).unwrap();
    let before = database_snapshot(&s);
    let ordinary = &before
        .iter()
        .find(|(name, _)| name == "snapshot_rows")
        .unwrap()
        .1;
    assert_eq!(ordinary.len(), 7);
    for value in [
        Value::Null,
        Value::Integer(7),
        Value::Real(7.0),
        Value::Text("7".into()),
        Value::Blob(vec![0x37]),
    ] {
        assert!(
            ordinary.iter().any(|row| row[1] == value),
            "missing {value:?}"
        );
    }
    assert_eq!(
        ordinary
            .iter()
            .filter(|row| row[1] == Value::Text("duplicate".into()))
            .count(),
        2
    );
    let keyed = &before
        .iter()
        .find(|(name, _)| name == "snapshot_keys")
        .unwrap()
        .1;
    assert_eq!(keyed.len(), 2);
    assert_eq!(
        keyed[0],
        vec![
            Value::Text("a".into()),
            Value::Integer(1),
            Value::Blob(vec![0, 255])
        ]
    );
    // Same complete rows in a different insertion order remain equal.
    s.conn().execute_batch("DELETE FROM snapshot_keys; INSERT INTO snapshot_keys VALUES('a',1,X'00FF'),('b',2,NULL);").unwrap();
    assert_eq!(database_snapshot(&s), before);
    // A lost duplicate, changed SQLite type, changed hidden identity or keyed value is visible.
    for sql in [
        "DELETE FROM snapshot_rows WHERE rowid=14",
        "UPDATE snapshot_rows SET value='7' WHERE rowid=9",
        "UPDATE snapshot_rows SET rowid=15 WHERE rowid=8",
        "UPDATE snapshot_keys SET value=X'01FF' WHERE a='a'",
        "UPDATE snapshot_keys SET a='new-key' WHERE a='a'",
    ] {
        s.conn()
            .execute_batch("SAVEPOINT snapshot_mutation")
            .unwrap();
        s.conn().execute_batch(sql).unwrap();
        assert_ne!(database_snapshot(&s), before, "missed mutation: {sql}");
        s.conn()
            .execute_batch("ROLLBACK TO snapshot_mutation; RELEASE snapshot_mutation")
            .unwrap();
        assert_eq!(database_snapshot(&s), before);
    }
}
