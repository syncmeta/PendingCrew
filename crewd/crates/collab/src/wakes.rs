//! 持久化唤醒队列（spec §22.4：取代只在内存里的 `CrewDeferredWakeQueue`）与熔断台账（§22.7 Q5）。
//! 这里只有读写；叫谁、何时投、准入与熔断判据都在 M3c（`crewd-wake` + `server::orchestra`）。
use crate::{Change, CollabError, Result, Tx};
use crewd_proto::wake_lifecycle as lifecycle;
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

/// 唤醒行的状态与改状态的事件（与 M3c 的测试实现共用同一张转移表，见 [`crewd_proto::wake_lifecycle`]，C24）。
pub use crewd_proto::wake_lifecycle::{WakeEvent, WakeState};

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
    /// 审批有了结果（控制者裁定 C2）：kind 仍是 `ask_answer`，按 source 前缀区分。
    pub fn approval(approval_id: &str) -> String {
        format!("approval:{approval_id}")
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
pub fn enqueue(tx: &mut Tx<'_>, r: WakeRequest) -> Result<Enqueued> {
    if r.source.trim().is_empty() {
        return Err(CollabError::Invalid("唤醒的 source 不能为空".into()));
    }
    if crewd_store::sessions::get(tx.conn(), &r.session)?.is_none() {
        return Err(CollabError::NotFound(format!("没有 session {}", r.session)));
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

/// 一批行按同一个生命周期事件改状态（[`crewd_proto::wake_lifecycle`] 那张表是**唯一**的规则，C24）。
/// 先把每一行都核一遍（没有 = `NotFound`，表不认 = `Conflict`），全过了才写——一行不对整批不动
/// （调用方的 `Tx` 再兜一层回滚）。次数按 [`lifecycle::attempts_after`] 改；`Fail { retry_at: Some(t) }` 改 `not_before`；
/// 进终局的记 `settled_at`。返回每一行事件之后的次数（与 `ids` 同序）。
pub fn apply(tx: &mut Tx<'_>, ids: &[i64], event: WakeEvent) -> Result<Vec<u32>> {
    let mut planned = Vec::with_capacity(ids.len());
    for id in ids {
        let row: Option<(String, u32)> = tx
            .conn()
            .query_row("SELECT state,attempts FROM wakes WHERE id=?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let Some((state, attempts)) = row else {
            return Err(CollabError::NotFound(format!("没有唤醒 {id}")));
        };
        let from = WakeState::parse(&state)
            .ok_or_else(|| CollabError::Invalid(format!("唤醒 {id} 的状态坏了：{state:?}")))?;
        let to = lifecycle::next(from, event)
            .map_err(|e| CollabError::Conflict(format!("唤醒 {id}：{e}")))?;
        planned.push((*id, to, lifecycle::attempts_after(event, attempts)));
    }
    let retry_at = match event {
        WakeEvent::Fail { retry_at } => retry_at,
        _ => None,
    };
    for (id, to, attempts) in &planned {
        let settled = matches!(to, WakeState::Delivered | WakeState::Dropped);
        tx.conn().execute(
            "UPDATE wakes SET state=?2, attempts=?3, not_before=COALESCE(?4, not_before),
                    settled_at=CASE WHEN ?5 THEN ?6 ELSE settled_at END
              WHERE id=?1",
            params![id, to.as_str(), attempts, retry_at, settled, tx.now_ts()],
        )?;
    }
    Ok(planned.into_iter().map(|(_, _, a)| a).collect())
}

fn note_error(tx: &mut Tx<'_>, ids: &[i64], error: &str) -> Result<()> {
    for id in ids {
        tx.conn().execute(
            "UPDATE wakes SET last_error=?2 WHERE id=?1",
            params![id, error],
        )?;
    }
    Ok(())
}

/// 一批拿走这些行（含随批的窗口外行，C24）：pending → inflight，次数在这里 +1（这是第几次尝试；Oban / River 同）。
/// 返回每一行拿走后的次数（与 `ids` 同序）。
pub fn claim(tx: &mut Tx<'_>, ids: &[i64]) -> Result<Vec<u32>> {
    apply(tx, ids, WakeEvent::Claim)
}

/// 随批窗口外行（M3a R6 `deferred`）在这批送到时怎么办。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Riders<'a> {
    /// 放回 pending（退回 `claim` 记的那一次），下一批带上。
    Release,
    /// 一起丢掉（人正在停这个 session：跟别的 pending 一样不再投）。
    Drop(&'a str),
}

/// 一批送到了：`wakes` → delivered、`riders` 放回或丢掉、各群游标推到 `cursor_targets`——**全在调用方的这一个
/// `Tx` 里**（River 的 `JobCompleteTx`：游标是消息唤醒的幂等键，必须跟确认一起提交，否则崩在两者之间就会重投）。
/// 任何一步出错整批回滚，一行都不改、游标也不动。`wakes` / `riders` 都必须是 `session` 的行。
pub fn ack_delivered(
    tx: &mut Tx<'_>,
    session: &str,
    wakes: &[i64],
    riders: Riders<'_>,
    rider_ids: &[i64],
    cursor_targets: &[(String, i64)],
) -> Result<()> {
    let all: Vec<i64> = wakes.iter().chain(rider_ids).copied().collect();
    for r in by_ids(tx.conn(), &all)? {
        if r.session != session {
            return Err(CollabError::Invalid(format!(
                "唤醒 {} 属于 {}，不是 {session}",
                r.id, r.session
            )));
        }
    }
    apply(tx, wakes, WakeEvent::Deliver)?;
    match riders {
        Riders::Release => {
            apply(tx, rider_ids, WakeEvent::Requeue)?;
        }
        Riders::Drop(why) => {
            apply(tx, rider_ids, WakeEvent::Drop)?;
            note_error(tx, rider_ids, why)?;
        }
    }
    for (channel, seq) in cursor_targets {
        crate::cursors::advance(tx, session, channel, *seq)?;
    }
    Ok(())
}

/// [`ack_failed`] 里一行的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failed {
    pub id: i64,
    /// 库里的次数（`claim` 记的）。
    pub attempts: u32,
    /// 再试的时刻（新的 `not_before`）；None = 到上限放弃了（dropped）。
    pub retry_at_ms: Option<i64>,
}

/// 一批没送到（终局失败）：每一行按**库里的**次数（`claim` 时记的，不是调用方手里那份）问 `decide`——
/// `Some(t)` = 回 pending、`not_before = t`；`None` = 放弃（dropped）。全部记下 `error`。一个 `Tx`、全有或全无：
/// 任何一行不是 inflight 整批不动。
pub fn ack_failed(
    tx: &mut Tx<'_>,
    ids: &[i64],
    error: &str,
    decide: impl Fn(u32) -> Option<i64>,
) -> Result<Vec<Failed>> {
    let rows = by_ids(tx.conn(), ids)?;
    let mut counts = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(r) = rows.iter().find(|r| r.id == *id) else {
            return Err(CollabError::NotFound(format!("没有唤醒 {id}")));
        };
        // 先按表把每一行都核一遍（Fail 与 GiveUp 认的起始状态相同）：有一行不对整批不动，也不问 `decide`。
        lifecycle::next(r.state, WakeEvent::GiveUp)
            .map_err(|e| CollabError::Conflict(format!("唤醒 {id}：{e}")))?;
        counts.push((*id, r.attempts));
    }
    let out: Vec<Failed> = counts
        .into_iter()
        .map(|(id, attempts)| Failed {
            id,
            attempts,
            retry_at_ms: decide(attempts),
        })
        .collect();
    for f in &out {
        let event = match f.retry_at_ms {
            Some(t) => WakeEvent::Fail { retry_at: Some(t) },
            None => WakeEvent::GiveUp,
        };
        apply(tx, &[f.id], event)?;
    }
    note_error(tx, ids, error)?;
    Ok(out)
}

/// 投递确认（`Delivery::Delivered`）以外的「算送到」：游标已越过、无附言的恢复在跑就算到（pending / inflight → delivered）。
pub fn mark_delivered(tx: &mut Tx<'_>, ids: &[i64]) -> Result<()> {
    apply(tx, ids, WakeEvent::Deliver).map(|_| ())
}

/// 丢掉（人停、渲染不出来、没有这个 session）：pending / inflight → dropped，记下原因。
pub fn mark_dropped(tx: &mut Tx<'_>, id: i64, why: &str) -> Result<()> {
    apply(tx, &[id], WakeEvent::Drop)?;
    note_error(tx, &[id], why)
}

/// inflight 原样退回 pending（没尝试成：crewd 停、撞额度、会话退出时被打断）：退回 `claim` 记的那一次，不算失败。
pub fn requeue(tx: &mut Tx<'_>, ids: &[i64]) -> Result<()> {
    apply(tx, ids, WakeEvent::Requeue).map(|_| ())
}

/// 清理：删掉 `settled_before_ms` 之前就已 delivered / dropped 的行（M3c 的定时拍按保留期调，缺省 7 天）。
/// 删了之后同一 source 可以再入队——消息类有 6h 陈旧闸，带时刻 / 序号的 source 不会重复，所以安全。
pub fn prune(tx: &mut Tx<'_>, settled_before_ms: i64) -> Result<usize> {
    Ok(tx.conn().execute(
        "DELETE FROM wakes WHERE state IN ('delivered','dropped') AND settled_at IS NOT NULL AND settled_at<?1",
        [crewd_store::ts_from_ms(settled_before_ms)],
    )?)
}

/// 启动时：上次没确认的 inflight 全部退回 pending（spec §22.4「重启时 inflight 退回 pending」），走表里的 `Requeue`：
/// 退回 `claim` 记的那一次，所以这次重启不算一次尝试（C25：Task 9 改成算一次，让弄崩 crewd 的唤醒有上限）。
/// 只在启动时调（运行期的退回是 `requeue`、失败是 `ack_failed`）。语义是至少一次：上次可能已经投出去只是没来得及记，
/// 所以会再投一遍。delivered / dropped 不动。
pub fn reset_inflight(tx: &mut Tx<'_>) -> Result<usize> {
    let ids: Vec<i64> = {
        let mut st = tx
            .conn()
            .prepare("SELECT id FROM wakes WHERE state='inflight' ORDER BY id")?;
        st.query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?
    };
    requeue(tx, &ids)?;
    Ok(ids.len())
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
                _ => Err(CollabError::Invalid(format!(
                    "熔断台账的 id 不能为空、也不能含「→」：{self:?}"
                ))),
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
                        CollabError::Invalid(format!(
                            "wake_latches 里有认不出的行：scope={s:?} id={id:?}"
                        ))
                    })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{MIN, NOW, count, err, ok, ok_at, store};
    use crewd_store::Store;

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

    fn new_ids(s: &mut Store, srcs: &[&str]) -> Vec<i64> {
        ok(s, |tx| {
            let mut v = Vec::new();
            for src in srcs {
                if let Enqueued::New(id) = enqueue(tx, req("w-a1", src, WakePriority::Auto))? {
                    v.push(id);
                }
            }
            Ok(v)
        })
    }

    fn state_of(s: &Store, id: i64) -> (WakeState, u32) {
        let r = &by_ids(s.conn(), &[id]).unwrap()[0];
        (r.state, r.attempts)
    }

    fn lobby_cursor(s: &Store) -> Option<i64> {
        crate::cursors::cursor(s.conn(), "w-a1", "crew-a/lobby").unwrap()
    }

    /// 两条合成消息进大厅（游标能推到 2）。
    fn two_lobby_messages(s: &Store) {
        for id in ["m-1", "m-2"] {
            crate::testkit::msg(
                s,
                "crew-a/lobby",
                id,
                crewd_proto::SenderKind::Human,
                None,
                vec![],
            );
        }
    }

    /// 次数在 claim 时记（C24）；失败不再加，按库里的数问 `decide`；退回把 claim 那次退掉；退避改 `not_before`。
    #[test]
    fn lifecycle_counts_attempts_at_claim_and_fails_by_the_stored_count() {
        let mut s = store();
        let id = new_ids(&mut s, &["msg:1"])[0];
        assert_eq!(ok(&mut s, |tx| claim(tx, &[id])), [1]);
        assert!(
            due_for(s.conn(), "w-a1", NOW).unwrap().is_empty(),
            "inflight 不再算到期"
        );
        assert!(matches!(
            err(&mut s, |tx| claim(tx, &[id])),
            CollabError::Conflict(_)
        ));
        let seen = std::cell::RefCell::new(Vec::new());
        let failed = ok(&mut s, |tx| {
            ack_failed(tx, &[id], "输入框没就绪", |n| {
                seen.borrow_mut().push(n);
                Some(NOW + MIN)
            })
        });
        assert_eq!(seen.into_inner(), [1], "decide 拿到的是库里 claim 记的数");
        assert_eq!(
            failed,
            [Failed {
                id,
                attempts: 1,
                retry_at_ms: Some(NOW + MIN)
            }]
        );
        let r = &by_ids(s.conn(), &[id]).unwrap()[0];
        assert_eq!((r.state, r.attempts), (WakeState::Pending, 1), "失败不再加");
        assert_eq!(r.not_before_ms, NOW + MIN, "退避持久化，重启不会立刻重试");
        assert_eq!(r.last_error.as_deref(), Some("输入框没就绪"));
        assert!(due_for(s.conn(), "w-a1", NOW).unwrap().is_empty());
        assert_eq!(ok(&mut s, |tx| claim(tx, &[id])), [2]);
        ok(&mut s, |tx| requeue(tx, &[id]));
        assert_eq!(
            state_of(&s, id),
            (WakeState::Pending, 1),
            "原样退回：退掉这次 claim，不记失败"
        );
        assert_eq!(ok(&mut s, |tx| claim(tx, &[id])), [2]);
        assert_eq!(ok(&mut s, |tx| claim(tx, &[])), Vec::<u32>::new());
        ok(&mut s, |tx| mark_delivered(tx, &[id]));
        let row = &list(s.conn(), None, 10).unwrap()[0];
        assert_eq!((row.state, row.attempts), (WakeState::Delivered, 2));
        assert!(row.settled_at.is_some());
        let id2 = new_ids(&mut s, &["msg:2"])[0];
        ok(&mut s, |tx| {
            mark_dropped(tx, id2, "游标已越过：hook 注入过了")
        });
        let r2 = &by_ids(s.conn(), &[id2]).unwrap()[0];
        assert_eq!(r2.state, WakeState::Dropped);
        assert_eq!(r2.last_error.as_deref(), Some("游标已越过：hook 注入过了"));
        assert!(r2.settled_at.is_some());
    }

    /// 到上限放弃：`decide` 回 None → dropped（库里的数，不是调用方那份）。
    #[test]
    fn ack_failed_gives_up_when_decide_says_so() {
        let mut s = store();
        let ids = new_ids(&mut s, &["msg:1", "msg:2"]);
        ok(&mut s, |tx| claim(tx, &ids));
        ok(&mut s, |tx| ack_failed(tx, &ids, "x", |_| Some(NOW)));
        ok(&mut s, |tx| claim(tx, &ids[..1]));
        ok(&mut s, |tx| claim(tx, &ids[1..]));
        let out = ok(&mut s, |tx| {
            ack_failed(tx, &ids, "又没送到", |n| (n < 2).then_some(NOW))
        });
        assert_eq!(
            out.iter()
                .map(|f| (f.attempts, f.retry_at_ms))
                .collect::<Vec<_>>(),
            [(2, None), (2, None)]
        );
        for id in &ids {
            let r = &by_ids(s.conn(), &[*id]).unwrap()[0];
            assert_eq!((r.state, r.attempts), (WakeState::Dropped, 2));
            assert!(r.settled_at.is_some());
            assert_eq!(r.last_error.as_deref(), Some("又没送到"));
        }
    }

    /// 全有或全无（C24）：一批里有一行不是 inflight，整批不动、也不问 `decide`。
    #[test]
    fn ack_failed_with_one_bad_row_changes_nothing() {
        let mut s = store();
        let ids = new_ids(&mut s, &["msg:1", "msg:2", "msg:3"]);
        ok(&mut s, |tx| claim(tx, &ids[..2]));
        let before = by_ids(s.conn(), &ids).unwrap();
        let e = err(&mut s, |tx| {
            ack_failed(tx, &ids, "x", |_| panic!("不该问到这里"))
        });
        assert!(matches!(e, CollabError::Conflict(_)), "{e:?}");
        assert_eq!(by_ids(s.conn(), &ids).unwrap(), before);
        assert!(matches!(
            err(&mut s, |tx| ack_failed(tx, &[999], "x", |_| None)),
            CollabError::NotFound(_)
        ));
    }

    /// 送到 = 一个 Tx：进正文的 → delivered、随批的放回 pending（退回 claim 那次）、游标推到 cursor_targets。
    #[test]
    fn ack_delivered_settles_rows_releases_riders_and_advances_the_cursor_together() {
        let mut s = store();
        two_lobby_messages(&s);
        let ids = new_ids(&mut s, &["msg:1", "msg:2", "msg:3"]);
        ok(&mut s, |tx| claim(tx, &ids));
        let targets = vec![("crew-a/lobby".to_string(), 2)];
        ok(&mut s, |tx| {
            ack_delivered(tx, "w-a1", &ids[..2], Riders::Release, &ids[2..], &targets)
        });
        assert_eq!(state_of(&s, ids[0]), (WakeState::Delivered, 1));
        assert_eq!(state_of(&s, ids[1]), (WakeState::Delivered, 1));
        assert_eq!(
            state_of(&s, ids[2]),
            (WakeState::Pending, 0),
            "随批的放回、这次不算"
        );
        assert_eq!(lobby_cursor(&s), Some(2));
    }

    /// 人正在停这个 session：随批的跟 pending 一起丢，不放回。
    #[test]
    fn ack_delivered_can_drop_the_riders() {
        let mut s = store();
        let ids = new_ids(&mut s, &["msg:1", "msg:2"]);
        ok(&mut s, |tx| claim(tx, &ids));
        ok(&mut s, |tx| {
            ack_delivered(
                tx,
                "w-a1",
                &ids[..1],
                Riders::Drop("stopped"),
                &ids[1..],
                &[],
            )
        });
        let r = &by_ids(s.conn(), &ids[1..]).unwrap()[0];
        assert_eq!(
            (r.state, r.last_error.as_deref()),
            (WakeState::Dropped, Some("stopped"))
        );
    }

    /// 原子性（C24）：游标那一步出错（推到一个不存在的群，外键挡住）→ 整个 Tx 回滚：行还是 inflight、游标没动。
    #[test]
    fn ack_delivered_rolls_back_everything_when_the_cursor_step_fails() {
        let mut s = store();
        two_lobby_messages(&s);
        let ids = new_ids(&mut s, &["msg:1", "msg:2"]);
        ok(&mut s, |tx| claim(tx, &ids));
        let cursor_before = lobby_cursor(&s);
        let before = by_ids(s.conn(), &ids).unwrap();
        let targets = vec![
            ("crew-a/lobby".to_string(), 2),
            ("没有这个群".to_string(), 1),
        ];
        let _ = err(&mut s, |tx| {
            ack_delivered(tx, "w-a1", &ids[..1], Riders::Release, &ids[1..], &targets)
        });
        assert_eq!(by_ids(s.conn(), &ids).unwrap(), before, "行一行都没改");
        assert_eq!(lobby_cursor(&s), cursor_before, "游标也没动");
    }

    /// 一行不对（不是 inflight / 不是这个 session 的）：整批不动。
    #[test]
    fn ack_delivered_refuses_a_bad_row_without_touching_the_rest() {
        let mut s = store();
        let ids = new_ids(&mut s, &["msg:1", "msg:2"]);
        ok(&mut s, |tx| claim(tx, &ids[..1]));
        ok(&mut s, |tx| mark_dropped(tx, ids[1], "x"));
        let before = by_ids(s.conn(), &ids).unwrap();
        assert!(matches!(
            err(&mut s, |tx| ack_delivered(
                tx,
                "w-a1",
                &ids[..1],
                Riders::Release,
                &ids[1..],
                &[]
            )),
            CollabError::Conflict(_)
        ));
        assert!(matches!(
            err(&mut s, |tx| ack_delivered(
                tx,
                "w-a2",
                &ids[..1],
                Riders::Release,
                &[],
                &[]
            )),
            CollabError::Invalid(_)
        ));
        assert_eq!(by_ids(s.conn(), &ids).unwrap(), before);
    }

    /// 整批 claim 也是全有或全无。
    #[test]
    fn claim_with_one_non_pending_row_claims_nothing() {
        let mut s = store();
        let ids = new_ids(&mut s, &["msg:1", "msg:2"]);
        ok(&mut s, |tx| claim(tx, &ids[1..]));
        assert!(matches!(
            err(&mut s, |tx| claim(tx, &ids)),
            CollabError::Conflict(_)
        ));
        assert_eq!(state_of(&s, ids[0]), (WakeState::Pending, 0));
        assert_eq!(state_of(&s, ids[1]), (WakeState::Inflight, 1));
    }

    /// `apply` 与 `crewd_proto::wake_lifecycle::next` 逐格一致（4 状态 × 6 事件），次数按 `attempts_after`。
    #[test]
    fn apply_follows_the_shared_table_for_every_state_and_event() {
        for from in WakeState::ALL {
            for event in WakeEvent::ALL {
                let mut s = store();
                let id = new_ids(&mut s, &["msg:1"])[0];
                match from {
                    WakeState::Pending => {}
                    WakeState::Inflight => {
                        ok(&mut s, |tx| claim(tx, &[id]));
                    }
                    WakeState::Delivered => ok(&mut s, |tx| mark_delivered(tx, &[id])),
                    WakeState::Dropped => ok(&mut s, |tx| mark_dropped(tx, id, "x")),
                }
                let (_, attempts) = state_of(&s, id);
                let got = crate::run(&mut s, NOW, |tx| apply(tx, &[id], event));
                match lifecycle::next(from, event) {
                    Ok(to) => {
                        assert_eq!(got.unwrap().0, [lifecycle::attempts_after(event, attempts)]);
                        assert_eq!(state_of(&s, id).0, to, "{from:?} + {event:?}");
                    }
                    Err(_) => {
                        assert!(
                            matches!(got, Err(CollabError::Conflict(_))),
                            "{from:?} + {event:?}: {got:?}"
                        );
                        assert_eq!(state_of(&s, id), (from, attempts));
                    }
                }
            }
        }
    }

    #[test]
    fn reset_inflight_returns_everything_in_flight_to_pending() {
        let mut s = store();
        let ids = new_ids(&mut s, &["msg:1", "msg:2"]);
        ok(&mut s, |tx| claim(tx, &ids));
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
        let ids = new_ids(&mut s, &["msg:1", "msg:2", "msg:3"]);
        ok(&mut s, |tx| claim(tx, &ids[..1]));
        ok(&mut s, |tx| {
            ack_failed(tx, &ids[..1], "没就绪", |_| Some(NOW))
        });
        ok(&mut s, |tx| claim(tx, &ids[..1]));
        ok(&mut s, |tx| mark_delivered(tx, &ids[1..2]));
        ok(&mut s, |tx| mark_dropped(tx, ids[2], "游标已越过"));
        let before = by_ids(s.conn(), &ids).unwrap();
        assert_eq!(ok(&mut s, reset_inflight), 1, "只动 inflight 那一条");
        let after = by_ids(s.conn(), &ids).unwrap();
        assert_eq!(after[0].state, WakeState::Pending);
        assert_eq!(
            after[0].attempts, 1,
            "重启退回不算一次尝试（退掉这次 claim）"
        );
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
