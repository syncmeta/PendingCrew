//! 持久化唤醒队列（spec §22.4：取代只在内存里的 `CrewDeferredWakeQueue`）与熔断台账（§22.7 Q5）。
//! 这里只有读写；叫谁、何时投、准入与熔断判据都在 M3c（`crewd-wake` + `server::orchestra`）。
use crate::refusal::{Code, Refusal};
use crate::{Change, CollabError, Result, Tx};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeKind {
    Message,
    Timer,
    AskAnswer,
    Restart,
    System,
    /// M4b 的检查点（M3 里没有写者）。
    Checkpoint,
    /// M4b 的停下即检查（M3 里没有写者）。
    StopCheck,
}

/// 优先级：`Human > System > Auto`（`Ord` 按这个顺序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WakePriority {
    Auto,
    System,
    Human,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WakeState {
    Pending,
    Inflight,
    Delivered,
    Dropped,
}

/// 入队请求。存来源身份，不存渲染好的正文（出队时现渲染，`render::build_batch`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeRequest {
    pub session: String,
    pub kind: WakeKind,
    /// 去重键的一部分：`UNIQUE(session, kind, source)`。用 `source::*` 拼。
    pub source: String,
    pub priority: WakePriority,
    /// 不早于这个时间投（Unix 毫秒）；None = 现在。
    pub not_before_ms: Option<i64>,
    pub note: Option<String>,
    /// 触发这次唤醒的 session（「来源 → 目标」熔断用，§22.7 Q5）；人 / 系统触发的是 None。
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enqueued {
    New(i64),
    /// 同一 (session, kind, source) 已经在表里（不论状态）：不重复入队。
    Duplicate(i64),
    /// 目标 session 的机组已归档（终审 I3）：不入队、不写行、不发变更。调用方当作无事发生——
    /// 归档机组起不来 session；解档也不会把归档期间的叫醒冲出来（同 `crews::archive` 作废排着的唤醒）。
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeRow {
    pub id: i64,
    pub session: String,
    pub kind: WakeKind,
    pub source: String,
    pub origin: Option<String>,
    pub priority: WakePriority,
    pub not_before_ms: i64,
    pub note: Option<String>,
    pub state: WakeState,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub created_at: String,
    pub settled_at: Option<String>,
}

/// source 的拼法，一处定死（裁定 13）。
///
/// 去重键 `(session, kind, source)` 在**所有状态**里都生效（delivered / dropped 也算），直到 `prune` 清掉那一行：
/// 所以会重复发生的唤醒（`timer`、`system`、`quota` 等）的 source 必须每次出现都唯一（带时刻 / 序号 / token）；
/// 同一 source 再入队只会得到 `Enqueued::Duplicate`，不会升级优先级、也不会改 `not_before`。
pub mod source {
    pub fn message(message_id: &str) -> String {
        format!("msg:{message_id}")
    }
    /// H 号按机组发，所以带上机组。
    pub fn ask(crew_id: &str, ask: &str) -> String {
        format!("ask:{crew_id}/{ask}")
    }
    pub fn timer(token: &str) -> String {
        format!("timer:{token}")
    }
    pub fn quota(reset_ms: i64) -> String {
        format!("quota:{reset_ms}")
    }
    pub fn restart(prompt_id: i64) -> String {
        format!("restart:{prompt_id}")
    }
    pub fn system(tag: &str) -> String {
        format!("system:{tag}")
    }
}

impl WakeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WakeKind::Message => "message",
            WakeKind::Timer => "timer",
            WakeKind::AskAnswer => "ask_answer",
            WakeKind::Restart => "restart",
            WakeKind::System => "system",
            WakeKind::Checkpoint => "checkpoint",
            WakeKind::StopCheck => "stop_check",
        }
    }
    pub fn parse(s: &str) -> Option<WakeKind> {
        [
            WakeKind::Message,
            WakeKind::Timer,
            WakeKind::AskAnswer,
            WakeKind::Restart,
            WakeKind::System,
            WakeKind::Checkpoint,
            WakeKind::StopCheck,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

impl WakePriority {
    pub fn as_str(self) -> &'static str {
        match self {
            WakePriority::Auto => "auto",
            WakePriority::System => "system",
            WakePriority::Human => "human",
        }
    }
    pub fn parse(s: &str) -> Option<WakePriority> {
        match s {
            "auto" => Some(WakePriority::Auto),
            "system" => Some(WakePriority::System),
            "human" => Some(WakePriority::Human),
            _ => None,
        }
    }
}

impl WakeState {
    pub fn as_str(self) -> &'static str {
        match self {
            WakeState::Pending => "pending",
            WakeState::Inflight => "inflight",
            WakeState::Delivered => "delivered",
            WakeState::Dropped => "dropped",
        }
    }
    pub fn parse(s: &str) -> Option<WakeState> {
        match s {
            "pending" => Some(WakeState::Pending),
            "inflight" => Some(WakeState::Inflight),
            "delivered" => Some(WakeState::Delivered),
            "dropped" => Some(WakeState::Dropped),
            _ => None,
        }
    }
}

const COLS: &str = "id,session_id,kind,source,origin,priority,not_before,note,state,attempts,last_error,created_at,settled_at";
/// 优先级高的先出：human 2、system 1、auto 0。
const RANK: &str = "CASE priority WHEN 'human' THEN 2 WHEN 'system' THEN 1 ELSE 0 END";

fn bad(idx: usize, what: &str, v: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        idx,
        rusqlite::types::Type::Text,
        format!("wakes.{what} = {v:?}").into(),
    )
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<WakeRow> {
    let kind: String = r.get(2)?;
    let priority: String = r.get(5)?;
    let state: String = r.get(8)?;
    Ok(WakeRow {
        id: r.get(0)?,
        session: r.get(1)?,
        kind: WakeKind::parse(&kind).ok_or_else(|| bad(2, "kind", kind.clone()))?,
        source: r.get(3)?,
        origin: r.get(4)?,
        priority: WakePriority::parse(&priority)
            .ok_or_else(|| bad(5, "priority", priority.clone()))?,
        not_before_ms: r.get(6)?,
        note: r.get(7)?,
        state: WakeState::parse(&state).ok_or_else(|| bad(8, "state", state.clone()))?,
        attempts: r.get(9)?,
        last_error: r.get(10)?,
        created_at: r.get(11)?,
        settled_at: r.get(12)?,
    })
}

fn rows(conn: &Connection, sql: &str, p: impl rusqlite::Params) -> Result<Vec<WakeRow>> {
    let mut st = conn.prepare(sql)?;
    Ok(st
        .query_map(p, row)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 入队。去重键 `(session, kind, source)` 跨所有状态（见 `source`）直到 `prune`；
/// 命中已有行返回 `Duplicate`，**不**改它的优先级、`not_before`、`note`，也不发变更。
/// 目标 session 的机组已归档 → `Skipped`（不写行、不发变更；不变式在 collab 这里，入队方不必各自判）。
pub fn enqueue(tx: &mut Tx<'_>, r: WakeRequest) -> Result<Enqueued> {
    if r.source.trim().is_empty() {
        return Err(CollabError::Invalid(Refusal::new(Code::WakeSourceEmpty)));
    }
    let Some(target) = crewd_store::sessions::get(tx.conn(), &r.session)? else {
        return Err(CollabError::NotFound(
            Refusal::new(Code::SessionNotFound).arg("session", r.session.clone()),
        ));
    };
    if crate::crews::is_archived(tx.conn(), &target.crew_id)? {
        return Ok(Enqueued::Skipped);
    }
    let n = tx.conn().execute(
        "INSERT INTO wakes(session_id,kind,source,origin,priority,not_before,note,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
         ON CONFLICT(session_id,kind,source) DO NOTHING",
        params![
            r.session,
            r.kind.as_str(),
            r.source,
            r.origin,
            r.priority.as_str(),
            r.not_before_ms.unwrap_or(tx.now_ms()),
            r.note,
            tx.now_ts(),
        ],
    )?;
    if n == 0 {
        let id: i64 = tx.conn().query_row(
            "SELECT id FROM wakes WHERE session_id=?1 AND kind=?2 AND source=?3",
            params![r.session, r.kind.as_str(), r.source],
            |x| x.get(0),
        )?;
        return Ok(Enqueued::Duplicate(id));
    }
    let id = tx.conn().last_insert_rowid();
    tx.note(Change::WakeEnqueued {
        wake_id: id,
        session_id: r.session,
    });
    Ok(Enqueued::New(id))
}

/// 该 session 已到期的 pending，优先级高的在前、同档按入队先后。
pub fn due_for(conn: &Connection, session: &str, now_ms: i64) -> Result<Vec<WakeRow>> {
    rows(
        conn,
        &format!(
            "SELECT {COLS} FROM wakes WHERE session_id=?1 AND state='pending' AND not_before<=?2
             ORDER BY {RANK} DESC, id"
        ),
        params![session, now_ms],
    )
}

/// 有已到期 pending 的 session。顺序：该 session 最高的 pending 优先级在前（human > system > auto），
/// 同档按它最老的那条唤醒（`MIN(id)`）；不按 session id，这样有并发上限时人的唤醒不会被饿死。
pub fn sessions_with_due(conn: &Connection, now_ms: i64) -> Result<Vec<String>> {
    let mut st = conn.prepare(&format!(
        "SELECT session_id FROM wakes WHERE state='pending' AND not_before<=?1
         GROUP BY session_id ORDER BY MAX({RANK}) DESC, MIN(id)"
    ))?;
    Ok(st
        .query_map([now_ms], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?)
}

/// 最早的 pending 的 not_before（含已过期的）。
pub fn next_due_at(conn: &Connection) -> Result<Option<i64>> {
    Ok(conn.query_row(
        "SELECT MIN(not_before) FROM wakes WHERE state='pending'",
        [],
        |r| r.get(0),
    )?)
}

/// 晚于 `now_ms` 的最早 pending 的 not_before（M3c 的单个定时器按它醒；已过期的由 idle 边沿 / 定时拍处理，不让定时器空转）。
pub fn next_due_after(conn: &Connection, now_ms: i64) -> Result<Option<i64>> {
    Ok(conn.query_row(
        "SELECT MIN(not_before) FROM wakes WHERE state='pending' AND not_before>?1",
        [now_ms],
        |r| r.get(0),
    )?)
}

/// 按 id 取行（M3c 渲染一批时用），按 id 升序；不存在的 id 略过。
pub fn by_ids(conn: &Connection, ids: &[i64]) -> Result<Vec<WakeRow>> {
    let mut out = Vec::new();
    for id in ids {
        out.extend(rows(
            conn,
            &format!("SELECT {COLS} FROM wakes WHERE id=?1"),
            [id],
        )?);
    }
    out.sort_by_key(|r| r.id);
    Ok(out)
}

fn set_state(
    tx: &mut Tx<'_>,
    ids: &[i64],
    from: &[&str],
    to: WakeState,
    settle: bool,
) -> Result<()> {
    for id in ids {
        let state: Option<String> = tx
            .conn()
            .query_row("SELECT state FROM wakes WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        let Some(state) = state else {
            return Err(CollabError::NotFound(
                Refusal::new(Code::WakeNotFound).arg("wake", id.to_string()),
            ));
        };
        if !from.contains(&state.as_str()) {
            return Err(CollabError::Conflict(
                Refusal::new(Code::WakeBadTransition)
                    .arg("wake", id.to_string())
                    .arg("from", state)
                    .arg("to", to.as_str()),
            ));
        }
        tx.conn().execute(
            "UPDATE wakes SET state=?2, settled_at=CASE WHEN ?3 THEN ?4 ELSE settled_at END WHERE id=?1",
            params![id, to.as_str(), settle, tx.now_ts()],
        )?;
    }
    Ok(())
}

pub fn mark_inflight(tx: &mut Tx<'_>, ids: &[i64]) -> Result<()> {
    set_state(tx, ids, &["pending"], WakeState::Inflight, false)
}

/// 投递确认（`Delivery::Delivered`）或「游标已越过」时记。
pub fn mark_delivered(tx: &mut Tx<'_>, ids: &[i64]) -> Result<()> {
    set_state(
        tx,
        ids,
        &["pending", "inflight"],
        WakeState::Delivered,
        true,
    )
}

/// 投递失败：回到 pending、次数 +1、记下原因；`retry_at_ms` 有值 = 退避（改 `not_before`，持久化，重启不会立刻重试）。
/// 返回累计失败次数（M3c 连续 3 次就放弃并告警）。
pub fn mark_failed(tx: &mut Tx<'_>, id: i64, error: &str, retry_at_ms: Option<i64>) -> Result<u32> {
    set_state(tx, &[id], &["inflight"], WakeState::Pending, false)?;
    tx.conn().execute(
        "UPDATE wakes SET attempts=attempts+1, last_error=?2, not_before=COALESCE(?3, not_before) WHERE id=?1",
        params![id, error, retry_at_ms],
    )?;
    Ok(tx
        .conn()
        .query_row("SELECT attempts FROM wakes WHERE id=?1", [id], |r| r.get(0))?)
}

pub fn mark_dropped(tx: &mut Tx<'_>, id: i64, why: &str) -> Result<()> {
    set_state(
        tx,
        &[id],
        &["pending", "inflight"],
        WakeState::Dropped,
        true,
    )?;
    tx.conn().execute(
        "UPDATE wakes SET last_error=?2 WHERE id=?1",
        params![id, why],
    )?;
    Ok(())
}

/// 机组的 session 名下正在投递（inflight）的唤醒条数。`crews::archive` 见到非零就拒：投递方随后还要
/// `mark_delivered` / `requeue`，作废了它们会撞上 dropped。
pub fn count_inflight_for_crew(conn: &Connection, crew: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM wakes w JOIN sessions s ON s.id=w.session_id
          WHERE s.crew_id=?1 AND w.state='inflight'",
        [crew],
        |r| r.get(0),
    )?)
}

/// 归档机组时把它的 session 还排着队的唤醒（**pending**，含 requeue 回来的）作废：归档机组不让 session 起来，
/// 留着只会占队、解档后还会把陈旧的叫醒冲出来。同一个事务里调（`crews::archive`，它已先确认没有 inflight）；
/// 已投递 / 已作废的不动。返回作废了几条。原因写进 `last_error`（`wake.list` 排障看得到）。
pub fn drop_for_crew(tx: &mut Tx<'_>, crew: &str, why: &str) -> Result<usize> {
    let ids: Vec<i64> = {
        let mut st = tx.conn().prepare(
            "SELECT w.id FROM wakes w JOIN sessions s ON s.id=w.session_id
              WHERE s.crew_id=?1 AND w.state='pending' ORDER BY w.id",
        )?;
        st.query_map([crew], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for id in &ids {
        mark_dropped(tx, *id, why)?;
    }
    Ok(ids.len())
}

/// inflight 原样退回 pending（不算失败、attempts 不变）：会话退出 / 投递被打断时 M3c 用。
pub fn requeue(tx: &mut Tx<'_>, ids: &[i64]) -> Result<()> {
    set_state(tx, ids, &["inflight"], WakeState::Pending, false)
}

/// 清理：删掉 `settled_before_ms` 之前就已 delivered / dropped 的行（M3c 的定时拍按保留期调，缺省 7 天）。
/// 删了之后同一 source 可以再入队——消息类有 6h 陈旧闸，带时刻 / 序号的 source 不会重复，所以安全。
pub fn prune(tx: &mut Tx<'_>, settled_before_ms: i64) -> Result<usize> {
    Ok(tx.conn().execute(
        "DELETE FROM wakes WHERE state IN ('delivered','dropped') AND settled_at IS NOT NULL AND settled_at<?1",
        [crewd_store::ts_from_ms(settled_before_ms)],
    )?)
}

/// 启动时：上次没确认的 inflight 全部退回 pending（spec §22.4「重启时 inflight 退回 pending」）。
/// 只在启动时调（不是运行期的重试路径，那是 `requeue` / `mark_failed`）。语义是至少一次：上次可能已经
/// 投出去只是没来得及记，所以会再投一遍；这不算一次失败尝试，`attempts` 不变。delivered / dropped 不动。
pub fn reset_inflight(tx: &mut Tx<'_>) -> Result<usize> {
    Ok(tx.conn().execute(
        "UPDATE wakes SET state='pending' WHERE state='inflight'",
        [],
    )?)
}

/// 某个目标 `since_ms` 之后入队的全部唤醒（任何状态），按入队先后。熔断判定读它（§22.7 Q5）。
pub fn recent_for(conn: &Connection, session: &str, since_ms: i64) -> Result<Vec<WakeRow>> {
    rows(
        conn,
        &format!("SELECT {COLS} FROM wakes WHERE session_id=?1 AND created_at>=?2 ORDER BY id"),
        params![session, crewd_store::ts_from_ms(since_ms)],
    )
}

/// 排障用（`wake.list`）：最近的 `limit` 条，新的在前。
pub fn list(conn: &Connection, session: Option<&str>, limit: u32) -> Result<Vec<WakeRow>> {
    match session {
        Some(s) => rows(
            conn,
            &format!("SELECT {COLS} FROM wakes WHERE session_id=?1 ORDER BY id DESC LIMIT ?2"),
            params![s, limit],
        ),
        None => rows(
            conn,
            &format!("SELECT {COLS} FROM wakes ORDER BY id DESC LIMIT ?1"),
            params![limit],
        ),
    }
}

/// 熔断台账（§22.7 Q5：「来源 → 目标」与「目标 session」两级；绝不按全机总量）。
/// 只有人能从客户端解（M3c 的 `admission.recover`）。
pub mod latches {
    use crate::refusal::{Code, Refusal};
    use crate::{CollabError, Result, Tx};
    use rusqlite::{Connection, OptionalExtension, params};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Scope {
        Edge { from: String, to: String },
        Session(String),
    }

    impl Scope {
        /// 台账键。空 id、或 id 里带分隔符 `→`（会让 edge 的键有歧义）一律拒。
        fn key(&self) -> Result<(&'static str, String)> {
            let ok = |id: &str| !id.is_empty() && !id.contains('→');
            match self {
                Scope::Edge { from, to } if ok(from) && ok(to) => {
                    Ok(("edge", format!("{from}→{to}")))
                }
                Scope::Session(s) if ok(s) => Ok(("session", s.clone())),
                _ => Err(CollabError::Invalid(
                    Refusal::new(Code::WakeLatchBadScope).arg("scope", format!("{self:?}")),
                )),
            }
        }
        fn from_key(scope: &str, id: &str) -> Option<Scope> {
            let parsed = match scope {
                "edge" => id.split_once('→').map(|(f, t)| Scope::Edge {
                    from: f.to_string(),
                    to: t.to_string(),
                }),
                "session" => Some(Scope::Session(id.to_string())),
                _ => None,
            }?;
            // 往返一致才算认得出（空段、多个箭头都不是我们写的）。
            let (s, k) = parsed.key().ok()?;
            (s == scope && k == id).then_some(parsed)
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Latch {
        pub scope: Scope,
        pub reason: String,
        pub at: String,
    }

    pub fn set(tx: &mut Tx<'_>, scope: &Scope, reason: &str) -> Result<()> {
        let (s, id) = scope.key()?;
        tx.conn().execute(
            "INSERT INTO wake_latches(scope,id,reason,at) VALUES(?1,?2,?3,?4)
             ON CONFLICT(scope,id) DO UPDATE SET reason=excluded.reason, at=excluded.at",
            params![s, id, reason, tx.now_ts()],
        )?;
        Ok(())
    }

    /// 返回是否真的清掉了一条。
    pub fn clear(tx: &mut Tx<'_>, scope: &Scope) -> Result<bool> {
        let (s, id) = scope.key()?;
        Ok(tx.conn().execute(
            "DELETE FROM wake_latches WHERE scope=?1 AND id=?2",
            params![s, id],
        )? > 0)
    }

    pub fn get(conn: &Connection, scope: &Scope) -> Result<Option<Latch>> {
        let (s, id) = scope.key()?;
        Ok(conn
            .query_row(
                "SELECT reason, at FROM wake_latches WHERE scope=?1 AND id=?2",
                params![s, id],
                |r| {
                    Ok(Latch {
                        scope: scope.clone(),
                        reason: r.get(0)?,
                        at: r.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn list(conn: &Connection) -> Result<Vec<Latch>> {
        let mut st =
            conn.prepare("SELECT scope, id, reason, at FROM wake_latches ORDER BY at, scope, id")?;
        let raw = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        raw.into_iter()
            .map(|(s, id, reason, at)| {
                Scope::from_key(&s, &id)
                    .map(|scope| Latch { scope, reason, at })
                    .ok_or_else(|| {
                        CollabError::Invalid(
                            Refusal::new(Code::WakeLatchRowUnreadable)
                                .arg("scope", s.clone())
                                .arg("id", id.clone()),
                        )
                    })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{MIN, NOW, count, err, ok, ok_at, store};

    fn req(session: &str, source: &str, p: WakePriority) -> WakeRequest {
        WakeRequest {
            session: session.into(),
            kind: WakeKind::Message,
            source: source.into(),
            priority: p,
            not_before_ms: None,
            note: None,
            origin: Some("w-a2".into()),
        }
    }

    #[test]
    fn enqueue_dedupes_on_session_kind_source() {
        let mut s = store();
        let (a, ch) = ok_at(&mut s, NOW, |tx| {
            enqueue(tx, req("w-a1", "msg:m1", WakePriority::Auto))
        });
        let Enqueued::New(id) = a else {
            panic!("{a:?}")
        };
        assert_eq!(
            ch,
            [Change::WakeEnqueued {
                wake_id: id,
                session_id: "w-a1".into()
            }]
        );
        let (b, ch) = ok_at(&mut s, NOW, |tx| {
            enqueue(tx, req("w-a1", "msg:m1", WakePriority::Human))
        });
        assert_eq!(b, Enqueued::Duplicate(id));
        assert!(ch.is_empty(), "重复入队不发变更");
        assert_eq!(count(&s, "SELECT COUNT(*) FROM wakes"), 1);
        let row = &list(s.conn(), Some("w-a1"), 10).unwrap()[0];
        assert_eq!(row.origin.as_deref(), Some("w-a2"));
        assert_eq!(row.not_before_ms, NOW, "None = 现在");
    }

    #[test]
    fn enqueue_for_unknown_session_is_not_found() {
        let mut s = store();
        assert!(matches!(
            err(&mut s, |tx| enqueue(
                tx,
                req("ghost", "msg:m1", WakePriority::Auto)
            )),
            CollabError::NotFound(_)
        ));
    }

    /// 终审 I3：归档机组的 session 不入队（归档机组起不来 session；排着的唤醒解档后会把陈旧的叫醒冲出来）。
    /// 它在别的机组的 topic 里被 @ 了：入队 = Skipped、不写行、不发变更；解档之后队里也没有陈旧的唤醒。
    #[test]
    fn enqueue_skips_sessions_of_archived_crews_and_unarchive_flushes_nothing() {
        use crate::Sender;
        use crewd_proto::{ChannelKind, MemberRef, Mention, MentionKind};
        let mut s = store();
        let t = ok(&mut s, |tx| {
            crate::channels::create(
                tx,
                &Sender::Session("w-a1".into()),
                crate::channels::NewChannel {
                    crew_id: "crew-a".into(),
                    kind: ChannelKind::Topic,
                    name: "示例".into(),
                    purpose: None,
                    members: vec![MemberRef::session("cap-x")],
                },
            )
        });
        ok(&mut s, |tx| crate::crews::archive(tx, "crew-x"));
        let m = ok(&mut s, |tx| {
            crate::messages::post(
                tx,
                Sender::Session("w-a1".into()),
                crate::messages::NewPost {
                    channel_id: t.id.clone(),
                    text: "叫一下归档机组的机长".into(),
                    mentions: vec![Mention {
                        kind: MentionKind::Session,
                        target_id: Some("cap-x".into()),
                    }],
                    ..Default::default()
                },
            )
        })
        .message;
        let (got, ch) = ok_at(&mut s, NOW, |tx| {
            enqueue(
                tx,
                req("cap-x", &source::message(&m.id), WakePriority::Auto),
            )
        });
        assert_eq!(got, Enqueued::Skipped);
        assert!(ch.is_empty(), "跳过不发变更");
        assert_eq!(
            count(&s, "SELECT COUNT(*) FROM wakes WHERE session_id='cap-x'"),
            0
        );
        // 活机组的 session 照常入队（跳过只看 session 自己的机组）。
        assert!(matches!(
            ok(&mut s, |tx| enqueue(
                tx,
                req("w-a2", &source::message(&m.id), WakePriority::Auto)
            )),
            Enqueued::New(_)
        ));
        ok(&mut s, |tx| crate::crews::unarchive(tx, "crew-x"));
        assert!(due_for(s.conn(), "cap-x", NOW + MIN).unwrap().is_empty());
        assert!(sessions_with_due(s.conn(), NOW + MIN).unwrap() == ["w-a2"]);
    }

    #[test]
    fn due_for_orders_by_priority_then_age_and_respects_not_before() {
        let mut s = store();
        ok(&mut s, |tx| {
            enqueue(tx, req("w-a1", "msg:1", WakePriority::Auto))?;
            enqueue(tx, req("w-a1", "msg:2", WakePriority::Human))?;
            enqueue(tx, req("w-a1", "msg:3", WakePriority::System))?;
            enqueue(
                tx,
                WakeRequest {
                    not_before_ms: Some(NOW + 10 * MIN),
                    ..req("w-a1", "timer:t", WakePriority::Human)
                },
            )
        });
        let due: Vec<String> = due_for(s.conn(), "w-a1", NOW)
            .unwrap()
            .into_iter()
            .map(|r| r.source)
            .collect();
        assert_eq!(due, ["msg:2", "msg:3", "msg:1"]);
        assert_eq!(next_due_at(s.conn()).unwrap(), Some(NOW));
        assert_eq!(
            next_due_after(s.conn(), NOW).unwrap(),
            Some(NOW + 10 * MIN),
            "已到期的不算，定时器不空转"
        );
        assert_eq!(next_due_after(s.conn(), NOW + 10 * MIN).unwrap(), None);
        assert_eq!(sessions_with_due(s.conn(), NOW).unwrap(), ["w-a1"]);
        let later: Vec<String> = due_for(s.conn(), "w-a1", NOW + 10 * MIN)
            .unwrap()
            .into_iter()
            .map(|r| r.source)
            .collect();
        assert_eq!(later.len(), 4);
    }

    #[test]
    fn lifecycle_pending_inflight_delivered_and_failures() {
        let mut s = store();
        let Enqueued::New(id) = ok(&mut s, |tx| {
            enqueue(tx, req("w-a1", "msg:1", WakePriority::Auto))
        }) else {
            panic!()
        };
        ok(&mut s, |tx| mark_inflight(tx, &[id]));
        assert!(
            due_for(s.conn(), "w-a1", NOW).unwrap().is_empty(),
            "inflight 不再算到期"
        );
        assert!(matches!(
            err(&mut s, |tx| mark_inflight(tx, &[id])),
            CollabError::Conflict(_)
        ));
        assert_eq!(
            ok(&mut s, |tx| mark_failed(tx, id, "输入框没就绪", None)),
            1
        );
        assert_eq!(
            due_for(s.conn(), "w-a1", NOW).unwrap()[0].attempts,
            1,
            "失败回到 pending、记次数"
        );
        ok(&mut s, |tx| mark_inflight(tx, &[id]));
        assert_eq!(
            ok(&mut s, |tx| mark_failed(
                tx,
                id,
                "又没就绪",
                Some(NOW + 2 * MIN)
            )),
            2
        );
        assert!(
            due_for(s.conn(), "w-a1", NOW).unwrap().is_empty(),
            "退避：not_before 推后，持久化（重启后不会立刻重试）"
        );
        assert_eq!(
            by_ids(s.conn(), &[id]).unwrap()[0].not_before_ms,
            NOW + 2 * MIN
        );
        ok(&mut s, |tx| mark_inflight(tx, &[id]));
        ok(&mut s, |tx| requeue(tx, &[id]));
        let back = &by_ids(s.conn(), &[id]).unwrap()[0];
        assert_eq!(
            (back.state, back.attempts),
            (WakeState::Pending, 2),
            "原样退回不记失败"
        );
        ok(&mut s, |tx| mark_inflight(tx, &[id]));
        ok(&mut s, |tx| mark_delivered(tx, &[id]));
        let row = &list(s.conn(), None, 10).unwrap()[0];
        assert_eq!(row.state, WakeState::Delivered);
        assert!(row.settled_at.is_some());
        let Enqueued::New(id2) = ok(&mut s, |tx| {
            enqueue(tx, req("w-a1", "msg:2", WakePriority::Auto))
        }) else {
            panic!()
        };
        ok(&mut s, |tx| {
            mark_dropped(tx, id2, "游标已越过：hook 注入过了")
        });
        assert_eq!(
            list(s.conn(), Some("w-a1"), 10)
                .unwrap()
                .iter()
                .filter(|r| r.state == WakeState::Dropped)
                .count(),
            1
        );
    }

    #[test]
    fn reset_inflight_returns_everything_in_flight_to_pending() {
        let mut s = store();
        let ids: Vec<i64> = ok(&mut s, |tx| {
            let mut v = Vec::new();
            for src in ["msg:1", "msg:2"] {
                if let Enqueued::New(id) = enqueue(tx, req("w-a1", src, WakePriority::Auto))? {
                    v.push(id);
                }
            }
            Ok(v)
        });
        ok(&mut s, |tx| mark_inflight(tx, &ids));
        assert_eq!(ok(&mut s, reset_inflight), 2);
        assert_eq!(due_for(s.conn(), "w-a1", NOW).unwrap().len(), 2);
    }

    #[test]
    fn prune_deletes_only_settled_rows_older_than_the_cutoff() {
        let mut s = store();
        let mut ids = Vec::new();
        for src in ["msg:a", "msg:b", "msg:c"] {
            if let (Enqueued::New(id), _) = ok_at(&mut s, NOW - 10 * MIN, |tx| {
                enqueue(tx, req("w-a1", src, WakePriority::Auto))
            }) {
                ids.push(id);
            }
        }
        ok_at(&mut s, NOW - 9 * MIN, |tx| mark_delivered(tx, &ids[..1]));
        ok_at(&mut s, NOW - 9 * MIN, |tx| {
            mark_dropped(tx, ids[1], "游标已越过")
        });
        assert_eq!(ok(&mut s, |tx| prune(tx, NOW - 5 * MIN)), 2);
        let left: Vec<i64> = list(s.conn(), None, 10)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(left, [ids[2]], "pending 的不删");
    }

    #[test]
    fn recent_for_lists_wakes_created_since() {
        let mut s = store();
        ok_at(&mut s, NOW - 5 * MIN, |tx| {
            enqueue(tx, req("w-a1", "msg:old", WakePriority::Auto))
        });
        ok_at(&mut s, NOW, |tx| {
            enqueue(tx, req("w-a1", "msg:new", WakePriority::Auto))
        });
        let got: Vec<String> = recent_for(s.conn(), "w-a1", NOW - MIN)
            .unwrap()
            .into_iter()
            .map(|r| r.source)
            .collect();
        assert_eq!(got, ["msg:new"]);
    }

    #[test]
    fn latches_by_edge_and_by_session() {
        let mut s = store();
        let edge = latches::Scope::Edge {
            from: "w-a1".into(),
            to: "w-a2".into(),
        };
        ok(&mut s, |tx| {
            latches::set(tx, &edge, "两人互相叫醒、没有新产出")
        });
        ok(&mut s, |tx| {
            latches::set(tx, &latches::Scope::Session("w-b1".into()), "反复被叫醒")
        });
        let got = latches::get(s.conn(), &edge).unwrap().unwrap();
        assert_eq!(got.scope, edge);
        assert_eq!(latches::list(s.conn()).unwrap().len(), 2);
        assert!(ok(&mut s, |tx| latches::clear(tx, &edge)));
        assert!(
            !ok(&mut s, |tx| latches::clear(tx, &edge)),
            "再清一次 = 本来就没有"
        );
        assert!(latches::get(s.conn(), &edge).unwrap().is_none());
    }

    #[test]
    fn reset_inflight_leaves_settled_rows_and_does_not_count_an_attempt() {
        let mut s = store();
        let mut ids = Vec::new();
        for src in ["msg:1", "msg:2", "msg:3"] {
            if let Enqueued::New(id) = ok(&mut s, |tx| {
                enqueue(tx, req("w-a1", src, WakePriority::Auto))
            }) {
                ids.push(id);
            }
        }
        ok(&mut s, |tx| mark_inflight(tx, &ids[..1]));
        ok(&mut s, |tx| mark_failed(tx, ids[0], "没就绪", None));
        ok(&mut s, |tx| mark_inflight(tx, &ids[..1]));
        ok(&mut s, |tx| mark_delivered(tx, &ids[1..2]));
        ok(&mut s, |tx| mark_dropped(tx, ids[2], "游标已越过"));
        let before = by_ids(s.conn(), &ids).unwrap();
        assert_eq!(ok(&mut s, reset_inflight), 1, "只动 inflight 那一条");
        let after = by_ids(s.conn(), &ids).unwrap();
        assert_eq!(after[0].state, WakeState::Pending);
        assert_eq!(after[0].attempts, 1, "重启退回不算一次尝试");
        assert_eq!(after[1], before[1], "delivered 原样");
        assert_eq!(after[2], before[2], "dropped 原样");
    }

    #[test]
    fn prune_keeps_rows_settled_after_the_cutoff() {
        let mut s = store();
        let mut ids = Vec::new();
        for src in ["msg:a", "msg:b"] {
            if let (Enqueued::New(id), _) = ok_at(&mut s, NOW - 10 * MIN, |tx| {
                enqueue(tx, req("w-a1", src, WakePriority::Auto))
            }) {
                ids.push(id);
            }
        }
        ok_at(&mut s, NOW - 9 * MIN, |tx| mark_delivered(tx, &ids[..1]));
        ok_at(&mut s, NOW - MIN, |tx| mark_delivered(tx, &ids[1..]));
        assert_eq!(ok(&mut s, |tx| prune(tx, NOW - 5 * MIN)), 1);
        let left: Vec<i64> = list(s.conn(), None, 10)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(left, [ids[1]], "截止之后才结算的不删");
    }

    #[test]
    fn due_for_and_sessions_with_due_are_scoped_per_session_and_time() {
        let mut s = store();
        ok(&mut s, |tx| {
            enqueue(tx, req("w-a1", "msg:1", WakePriority::Auto))?;
            enqueue(tx, req("w-a2", "msg:2", WakePriority::Auto))?;
            enqueue(
                tx,
                WakeRequest {
                    not_before_ms: Some(NOW + 10 * MIN),
                    ..req("w-b1", "timer:t", WakePriority::Human)
                },
            )
        });
        let a1: Vec<String> = due_for(s.conn(), "w-a1", NOW)
            .unwrap()
            .into_iter()
            .map(|r| r.source)
            .collect();
        assert_eq!(a1, ["msg:1"], "别的 session 的唤醒不混进来");
        assert_eq!(
            sessions_with_due(s.conn(), NOW).unwrap(),
            ["w-a1", "w-a2"],
            "全在未来的 session 不算到期"
        );
    }

    #[test]
    fn sessions_with_due_orders_by_highest_priority_then_oldest_wake() {
        let mut s = store();
        ok(&mut s, |tx| {
            enqueue(tx, req("w-a1", "msg:1", WakePriority::Auto))?;
            enqueue(tx, req("w-a2", "msg:2", WakePriority::Auto))?;
            enqueue(tx, req("w-b1", "msg:3", WakePriority::Human))?;
            enqueue(tx, req("cap-a", "msg:4", WakePriority::System))
        });
        assert_eq!(
            sessions_with_due(s.conn(), NOW).unwrap(),
            ["w-b1", "cap-a", "w-a1", "w-a2"],
            "人的先、同档按最老的唤醒；不按 session id（有并发上限时不饿死人的唤醒）"
        );
    }

    #[test]
    fn recent_for_boundary_is_inclusive() {
        let mut s = store();
        ok_at(&mut s, NOW - MIN, |tx| {
            enqueue(tx, req("w-a1", "msg:edge", WakePriority::Auto))
        });
        let got = recent_for(s.conn(), "w-a1", NOW - MIN).unwrap();
        assert_eq!(got.len(), 1, "恰好等于 since 的算在内");
        assert!(
            recent_for(s.conn(), "w-a1", NOW - MIN + 1)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn latch_rows_use_the_documented_key_and_list_reads_them_back() {
        let mut s = store();
        let edge = latches::Scope::Edge {
            from: "w-a1".into(),
            to: "w-a2".into(),
        };
        ok(&mut s, |tx| latches::set(tx, &edge, "原因"));
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) FROM wake_latches WHERE scope='edge' AND id='w-a1→w-a2'"
            ),
            1
        );
        let all = latches::list(s.conn()).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].scope, edge);
        assert_eq!(all[0].reason, "原因");
    }

    #[test]
    fn latches_reject_ambiguous_ids_and_list_reports_corrupt_rows() {
        let mut s = store();
        for bad in [
            latches::Scope::Session(String::new()),
            latches::Scope::Session("w-a1→w-a2".into()),
            latches::Scope::Edge {
                from: String::new(),
                to: "w-a2".into(),
            },
            latches::Scope::Edge {
                from: "w-a1".into(),
                to: String::new(),
            },
            latches::Scope::Edge {
                from: "w-a1→x".into(),
                to: "w-a2".into(),
            },
        ] {
            assert!(
                matches!(
                    err(&mut s, |tx| latches::set(tx, &bad, "r")),
                    CollabError::Invalid(_)
                ),
                "{bad:?}"
            );
        }
        assert_eq!(count(&s, "SELECT COUNT(*) FROM wake_latches"), 0);
        s.conn()
            .execute(
                "INSERT INTO wake_latches(scope,id,reason,at) VALUES('edge','没有箭头','r','2026-09-01T00:00:00.000Z')",
                [],
            )
            .unwrap();
        assert!(
            latches::list(s.conn()).is_err(),
            "存了认不出的行要报，不能静默漏掉一条熔断"
        );
    }
}
