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
    let s = Store::open_in_memory().unwrap();
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
