//! 持久化唤醒队列（spec §22.4：取代只在内存里的 `CrewDeferredWakeQueue`）与熔断台账（§22.7 Q5）。
//! 这里只有读写；叫谁、何时投、准入与熔断判据都在 M3c（`crewd-wake` + `server::orchestra`）。
use crate::refusal::{Code, Refusal};
use crate::{Change, CollabError, Result, Tx};
use crewd_proto::wake_lifecycle as lifecycle;
use crewd_store::StoreError;
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

/// meta 里的永久、单调取消水位。wakes.id 是 AUTOINCREMENT：即使 prune 删光旧行也不复用 id。
/// 不随 resume / 新唤醒 / desired=running 清掉；因此旧 inflight 的结算失败也不会在后来的 crash 中复活。
/// 不是 schema 扩展（同 hibernate 使用现有 meta），没有时间戳比较/兼容分支。
const STOP_FLOOR_PREFIX: &str = "human_stop_wake_floor:";
pub const HUMAN_STOP_ERROR: &str = "stopped";

pub fn human_stop_floor(conn: &Connection, session: &str) -> Result<i64> {
    match crewd_store::meta_get(conn, &format!("{STOP_FLOOR_PREFIX}{session}"))? {
        None => Ok(0),
        Some(v) => v.parse::<i64>().ok().filter(|n| *n >= 0).ok_or_else(|| {
            CollabError::Store(StoreError::Invalid(format!(
                "human stop wake floor for {session} is unreadable: {v:?}"
            )))
        }),
    }
}

/// Explicit Human stop pauses automatic work reminders until a real resume.
/// A zero floor still records a stop before the first wake; absence differs from zero.
pub fn automatic_work_paused(conn: &Connection, session: &str) -> Result<bool> {
    let Some(s) = crewd_store::sessions::get(conn, session)? else {
        return Ok(false);
    };
    if s.desired != crewd_proto::Desired::Stopped {
        return Ok(false);
    }
    let recorded = crewd_store::meta_get(conn, &format!("{STOP_FLOOR_PREFIX}{session}"))?.is_some();
    if recorded {
        let _ = human_stop_floor(conn, session)?;
    }
    Ok(recorded)
}

/// 人停：停止状态 + 截至本事务的 wake-ID 取消意图 + 全部 pending 作废，同一个 Tx、全有或全无。
/// inflight 只记意图、不提前写终态：迟到的真实 Delivered 仍须原子确认行与游标；未确认的旧行后续退回/失败/boot
/// 都经共享 event_after_cancel 作废（含没有内存主人的行）。返回本次作废的 pending，给 Port 发布变化。
pub fn human_stop(tx: &mut Tx<'_>, session: &str) -> Result<Vec<WakeRow>> {
    crate::sessions::set_desired(tx, session, crewd_proto::Desired::Stopped)?;
    crate::sessions::set_working(tx, session, false)?;
    let floor: i64 = tx.conn().query_row(
        "SELECT COALESCE(MAX(id),0) FROM wakes WHERE session_id=?1",
        [session],
        |r| r.get(0),
    )?;
    let floor = floor.max(human_stop_floor(tx.conn(), session)?);
    crewd_store::meta_set(
        tx.conn(),
        &format!("{STOP_FLOOR_PREFIX}{session}"),
        &floor.to_string(),
    )?;
    let pending = due_for(tx.conn(), session, i64::MAX)?;
    for w in &pending {
        mark_dropped(tx, w.id, HUMAN_STOP_ERROR)?;
    }
    crate::cursor_reservations::cancel_active(tx, session)?;
    Ok(pending)
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

/// 一批行按同一个生命周期事件改状态（[`crewd_proto::wake_lifecycle`] 那张表是**唯一**的规则，C24）。
/// 先把每一行都核一遍（没有 = `NotFound`，表不认 = `Conflict`），全过了才写——一行不对整批不动
/// （调用方的 `Tx` 再兜一层回滚）。次数按 [`lifecycle::attempts_after`] 改；`Fail { retry_at: Some(t) }` 改 `not_before`；
/// 进终局的记 `settled_at`。返回每一行事件之后的次数（与 `ids` 同序）。
pub fn apply(tx: &mut Tx<'_>, ids: &[i64], event: WakeEvent) -> Result<Vec<u32>> {
    let mut planned = Vec::with_capacity(ids.len());
    for id in ids {
        let row: Option<(String, u32, String)> = tx
            .conn()
            .query_row(
                "SELECT state,attempts,session_id FROM wakes WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((state, attempts, session)) = row else {
            return Err(not_found(*id));
        };
        let from = WakeState::parse(&state).ok_or_else(|| {
            CollabError::Store(StoreError::Invalid(format!(
                "wake {id} has an unreadable state {state:?}"
            )))
        })?;
        let cancelled = *id <= human_stop_floor(tx.conn(), &session)?;
        let event = lifecycle::event_after_cancel(from, event, cancelled)
            .map_err(|e| bad_transition(*id, e))?;
        let to = lifecycle::next(from, event).map_err(|e| bad_transition(*id, e))?;
        let retry_at = match event {
            WakeEvent::Fail { retry_at } => retry_at,
            _ => None,
        };
        planned.push((
            *id,
            to,
            lifecycle::attempts_after(event, attempts),
            retry_at,
            cancelled && to == WakeState::Dropped,
        ));
    }
    for (id, to, attempts, retry_at, cancelled) in &planned {
        let settled = matches!(to, WakeState::Delivered | WakeState::Dropped);
        tx.conn().execute(
            "UPDATE wakes SET state=?2, attempts=?3, not_before=COALESCE(?4, not_before),
                    settled_at=CASE WHEN ?5 THEN ?6 ELSE settled_at END
              WHERE id=?1",
            params![id, to.as_str(), attempts, retry_at, settled, tx.now_ts()],
        )?;
        if *cancelled {
            note_error(tx, &[*id], HUMAN_STOP_ERROR)?;
        }
    }
    Ok(planned.into_iter().map(|(_, _, a, _, _)| a).collect())
}

fn not_found(id: i64) -> CollabError {
    CollabError::NotFound(Refusal::new(Code::WakeNotFound).arg("wake", id.to_string()))
}

/// 表不认的事件 → `wake.bad_transition`：`to` 是这个事件想去的状态（表认的话会到的那一个）。
fn bad_transition(id: i64, e: lifecycle::Illegal) -> CollabError {
    let to = match e.event {
        WakeEvent::Claim => WakeState::Inflight,
        WakeEvent::Deliver => WakeState::Delivered,
        WakeEvent::Fail { .. } | WakeEvent::Requeue => WakeState::Pending,
        WakeEvent::GiveUp | WakeEvent::Drop => WakeState::Dropped,
    };
    CollabError::Conflict(
        Refusal::new(Code::WakeBadTransition)
            .arg("wake", id.to_string())
            .arg("from", e.from.as_str())
            .arg("to", to.as_str()),
    )
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
    let before = by_ids(tx.conn(), &all)?;
    for r in &before {
        if r.session != session {
            // 调用方（M3c）自己的错，不是给人看的规则拒绝。
            return Err(CollabError::Store(StoreError::Invalid(format!(
                "wake {} belongs to session {}, not {session}",
                r.id, r.session
            ))));
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
    crate::checkpoints::delivered(
        tx,
        &before
            .into_iter()
            .filter(|r| wakes.contains(&r.id))
            .collect::<Vec<_>>(),
    )?;
    Ok(())
}

/// [`ack_failed`] 里一行的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failed {
    pub id: i64,
    /// 库里的次数（`claim` 记的）。
    pub attempts: u32,
    /// 再试的时刻（新的 `not_before`）；None = 到上限放弃或人停取消（见 `cancelled`）。
    pub retry_at_ms: Option<i64>,
    /// 人停的旧行：作废，不算重试或到上限，不触发 give-up 告警。
    pub cancelled: bool,
}

/// 一批没送到（终局失败）：每一行按**库里的**次数（`claim` 时记的，不是调用方手里那份）问 `decide`——
/// `Some(t)` = 回 pending、`not_before = t`；`None` = 放弃（dropped）。全部记下 `error`。一个 `Tx`、全有或全无：
/// 每一行都按**真正要用的那个事件**（`Fail` 或 `GiveUp`，`decide` 定的）核过表才写（Task 8b 评审 (3)：不拿 `GiveUp`
/// 当 `Fail` 的替身）；有一行不认整批不动。`decide` 是纯函数（`crewd_wake::queue::after_failure` 一类），
/// 对整批不动的那次调用没有副作用。已取消的旧行不问 decide：只作废、记 stopped、cancelled=true。
pub fn ack_failed(
    tx: &mut Tx<'_>,
    ids: &[i64],
    error: &str,
    decide: impl Fn(u32) -> Option<i64>,
) -> Result<Vec<Failed>> {
    let rows = by_ids(tx.conn(), ids)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(r) = rows.iter().find(|r| r.id == *id) else {
            return Err(not_found(*id));
        };
        let cancelled = r.id <= human_stop_floor(tx.conn(), &r.session)?;
        let retry_at_ms = if cancelled { None } else { decide(r.attempts) };
        lifecycle::event_after_cancel(r.state, failure_event(retry_at_ms), cancelled)
            .map_err(|e| bad_transition(*id, e))?;
        out.push(Failed {
            id: *id,
            attempts: r.attempts,
            retry_at_ms,
            cancelled,
        });
    }
    for f in &out {
        apply(tx, &[f.id], failure_event(f.retry_at_ms))?;
    }
    for f in &out {
        note_error(
            tx,
            &[f.id],
            if f.cancelled { HUMAN_STOP_ERROR } else { error },
        )?;
    }
    Ok(out)
}

/// `decide` 的答复对应的表事件：给了时刻 = `Fail`（回 pending），没给 = `GiveUp`（放弃）。
fn failure_event(retry_at_ms: Option<i64>) -> WakeEvent {
    match retry_at_ms {
        Some(t) => WakeEvent::Fail { retry_at: Some(t) },
        None => WakeEvent::GiveUp,
    }
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

/// 机组的 session 名下正在投递（inflight）的唤醒条数。`crews::archive` 见到非零就拒：投递方随后还要
/// `ack_delivered` / `ack_failed` / `requeue`，作废了它们会撞上 dropped。
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

/// inflight 原样退回 pending（没尝试成：crewd 停、撞额度、会话退出时被打断）：退回 `claim` 记的那一次，不算失败。
/// 人停水位内的旧行例外：经共享转移作废、不退次数，不能因为后来的新工作/停机把旧行重排。
pub fn requeue(tx: &mut Tx<'_>, ids: &[i64]) -> Result<()> {
    apply(tx, ids, WakeEvent::Requeue).map(|_| ())
}

/// 清理：删掉 `settled_before_ms` 之前就已 delivered / dropped 的行，**一次至多 `limit` 条**（结得最早的先删）——
/// M3c 的定时拍按保留期（缺省 7 天）分批调，积了很多也不会一口气删几十万行把库卡住（S28）。返回删了几条。
/// 删了之后同一 source 可以再入队——消息类有 6h 陈旧闸，带时刻 / 序号的 source 不会重复，所以安全。
/// 代价：`settled_at` 上没有索引，每次调用扫一遍已结的行再排序取前 `limit` 条——按预期规模（保留 7 天、一小时一次）
/// 可以接受；行数大到这一步显眼时再加 `(state, settled_at)` 索引（那是一次迁移）。
pub fn prune(tx: &mut Tx<'_>, settled_before_ms: i64, limit: u32) -> Result<usize> {
    crate::cursor_reservations::guard_prune(tx.conn())?;
    Ok(tx.conn().execute(
        "DELETE FROM wakes WHERE id IN (
           SELECT id FROM wakes
            WHERE state IN ('delivered','dropped') AND settled_at IS NOT NULL AND settled_at<?1
            ORDER BY settled_at, id LIMIT ?2)",
        params![crewd_store::ts_from_ms(settled_before_ms), limit],
    )?)
}

/// 启动时：上次没确认的 inflight 全部按「一次失败的尝试」结算（spec §22.4「重启时 inflight 退回 pending」；C25）：
/// 跟 [`ack_failed`] 同一条路——**不退回** `claim` 记的那一次，按库里的次数问 `decide`（M3c 给
/// `crewd_wake::queue::after_restart`：没到上限立刻再试、到了上限放弃），所以一条每次都把 crewd 弄崩的唤醒到上限就
/// 放弃，不无限重投。只在启动时调（运行期的退回是 `requeue`、失败是 `ack_failed`）。语义是至少一次：上次可能已经
/// 投出去只是没来得及记，所以会再投一遍。delivered / dropped / pending 不动；人停取消的旧 inflight 只作废，不重试/告警。
pub fn reset_inflight(
    tx: &mut Tx<'_>,
    error: &str,
    decide: impl Fn(u32) -> Option<i64>,
) -> Result<Vec<Failed>> {
    let ids: Vec<i64> = {
        let mut st = tx
            .conn()
            .prepare("SELECT id FROM wakes WHERE state='inflight' ORDER BY id")?;
        st.query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?
    };
    ack_failed(tx, &ids, error, decide)
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

/// 同一个来源（`kind` + `source`，如一条消息的 `msg:<id>`）给每个 session 入队的唤醒（任何状态），按入队先后。
/// M3c 的 `message.receipts` 读它（S54）：这条消息叫醒了谁、送到没有。
pub fn for_source(conn: &Connection, kind: WakeKind, source: &str) -> Result<Vec<WakeRow>> {
    rows(
        conn,
        &format!("SELECT {COLS} FROM wakes WHERE kind=?1 AND source=?2 ORDER BY id"),
        params![kind.as_str(), source],
    )
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
        /// 台账里的 id（`edge` = `from→to`，`session` = session id）。M3c 的熔断 id 只经这里与 [`Scope::parse`]
        /// 互转（控制者裁定 C21），不自己拼 / 拆分隔符。
        pub fn id(&self) -> String {
            match self {
                Scope::Edge { from, to } => format!("{from}→{to}"),
                Scope::Session(s) => s.clone(),
            }
        }

        /// `(scope, id)` → `Scope`；认不出（未知的 scope、空段、多个箭头）= None。与 [`Scope::id`] 往返一致。
        pub fn parse(scope: &str, id: &str) -> Option<Scope> {
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
                Scope::parse(&s, &id)
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
    fn for_source_lists_every_session_one_source_woke_in_order() {
        let mut s = store();
        for (session, source) in [("w-a1", "msg:m1"), ("w-a2", "msg:m1"), ("w-a1", "msg:m2")] {
            ok_at(&mut s, NOW, |tx| {
                enqueue(tx, req(session, source, WakePriority::Auto))
            });
        }
        let got: Vec<String> = for_source(s.conn(), WakeKind::Message, "msg:m1")
            .unwrap()
            .into_iter()
            .map(|r| r.session)
            .collect();
        assert_eq!(got, ["w-a1", "w-a2"]);
        assert!(
            for_source(s.conn(), WakeKind::Timer, "msg:m1")
                .unwrap()
                .is_empty(),
            "种类也要对上"
        );
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
                retry_at_ms: Some(NOW + MIN),
                cancelled: false,
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
        // 两个分支都要挡住（Task 8b 评审 (3)）：判「再试」（Fail）与判「放弃」（GiveUp）各按自己的事件核表。
        let deciders: [fn(u32) -> Option<i64>; 2] = [|_| Some(NOW), |_| None];
        for decide in deciders {
            let e = err(&mut s, |tx| ack_failed(tx, &ids, "x", decide));
            assert!(matches!(e, CollabError::Conflict(_)), "{e:?}");
            assert_eq!(by_ids(s.conn(), &ids).unwrap(), before);
        }
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
            CollabError::Store(StoreError::Invalid(_))
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
        let failed = ok(&mut s, |tx| reset_inflight(tx, "重启", |_| Some(NOW)));
        assert_eq!(failed.len(), 2);
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
        assert_eq!(ok(&mut s, |tx| prune(tx, NOW - 5 * MIN, 100)), 2);
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

    /// C25：重启回收**算一次尝试**（不退回 claim 那一次）：一条把 crewd 弄崩的唤醒到上限就放弃，不无限重投。
    /// 按库里的次数问 `decide`；delivered / dropped 不动。
    #[test]
    fn reset_inflight_counts_the_attempt_and_leaves_settled_rows() {
        let mut s = store();
        let ids = new_ids(&mut s, &["msg:1", "msg:2", "msg:3", "msg:4"]);
        ok(&mut s, |tx| claim(tx, &ids[..1]));
        ok(&mut s, |tx| {
            ack_failed(tx, &ids[..1], "没就绪", |_| Some(NOW))
        });
        ok(&mut s, |tx| claim(tx, &ids[..1]));
        ok(&mut s, |tx| claim(tx, &ids[3..]));
        ok(&mut s, |tx| mark_delivered(tx, &ids[1..2]));
        ok(&mut s, |tx| mark_dropped(tx, ids[2], "游标已越过"));
        let before = by_ids(s.conn(), &ids).unwrap();
        // 第 2 次的那条到了「上限」（这里上限 = 2）就放弃；第 1 次的那条立刻再试。
        let failed = ok(&mut s, |tx| {
            reset_inflight(tx, "crewd 重启", |n| (n < 2).then_some(NOW))
        });
        assert_eq!(
            failed,
            [
                Failed {
                    id: ids[0],
                    attempts: 2,
                    retry_at_ms: None,
                    cancelled: false,
                },
                Failed {
                    id: ids[3],
                    attempts: 1,
                    retry_at_ms: Some(NOW),
                    cancelled: false,
                },
            ],
            "只动 inflight 那两条，按库里的次数"
        );
        let after = by_ids(s.conn(), &ids).unwrap();
        assert_eq!((after[0].state, after[0].attempts), (WakeState::Dropped, 2));
        assert_eq!(after[0].last_error.as_deref(), Some("crewd 重启"));
        assert_eq!(
            (after[3].state, after[3].attempts),
            (WakeState::Pending, 1),
            "重启算一次尝试（不退回 claim 那一次）"
        );
        assert_eq!(after[1], before[1], "delivered 原样");
        assert_eq!(after[2], before[2], "dropped 原样");
    }

    /// S28：一次最多清 `limit` 条（定时拍里清，不能一口气删几十万行卡住库），先清结得最早的。
    #[test]
    fn prune_deletes_at_most_limit_rows_settled_earliest_first() {
        let mut s = store();
        let ids = new_ids(&mut s, &["msg:a", "msg:b", "msg:c"]);
        for (i, id) in ids.iter().enumerate() {
            ok_at(&mut s, NOW - (9 - i as i64) * MIN, |tx| {
                mark_delivered(tx, &[*id])
            });
        }
        assert_eq!(ok(&mut s, |tx| prune(tx, NOW, 2)), 2);
        let left: Vec<i64> = list(s.conn(), None, 10)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(left, [ids[2]], "结得最晚的那条留到下一次");
        assert_eq!(ok(&mut s, |tx| prune(tx, NOW, 2)), 1);
        assert_eq!(ok(&mut s, |tx| prune(tx, NOW, 2)), 0);
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
        assert_eq!(ok(&mut s, |tx| prune(tx, NOW - 5 * MIN, 100)), 1);
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

    /// C21：M3c 只经 `Scope::id` / `Scope::parse` 与台账 id 互转（不自己拆分隔符）；两边往返一致，
    /// 认不出的（未知 scope、没有箭头、多个箭头、空段）= None。session id 里可以有 `-`。
    #[test]
    fn scope_id_and_parse_round_trip_and_refuse_what_they_did_not_write() {
        for sc in [
            latches::Scope::Edge {
                from: "w-a1".into(),
                to: "w-a2".into(),
            },
            latches::Scope::Session("w-a1".into()),
        ] {
            let kind = match sc {
                latches::Scope::Edge { .. } => "edge",
                latches::Scope::Session(_) => "session",
            };
            assert_eq!(latches::Scope::parse(kind, &sc.id()), Some(sc.clone()));
        }
        assert_eq!(
            latches::Scope::Edge {
                from: "a".into(),
                to: "b".into()
            }
            .id(),
            "a→b"
        );
        for (kind, id) in [
            ("edge", "没有箭头"),
            ("edge", "a→b→c"),
            ("edge", "→b"),
            ("session", ""),
            ("session", "a→b"),
            ("other", "a"),
        ] {
            assert_eq!(latches::Scope::parse(kind, id), None, "{kind} {id}");
        }
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
