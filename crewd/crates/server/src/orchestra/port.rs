//! 编排读写数据的唯一出口。生产实现 `db::DbPort`（全部写经 M3a 冻结的 `crewd_collab` 函数；M3c 不建表），
//! 测试实现 `testkit::MemPort`。M3a 的实际名字只在 `db.rs` 里出现。
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use crewd_exec::health::Health;
use crewd_exec::resume::AgentIdLedger;
use crewd_proto::wake_lifecycle as lifecycle;
use crewd_term::PendingDecision;
use crewd_wake::plan::{ChannelCtx, MsgFacts};
use crewd_wake::{Latch, LatchScope, MessageRef, Ms, NewWake, QueuedWake, WakeState};

/// 休眠记录（C26）的形状就是 collab `hibernate` 存进 `meta` 的那一份（M3c 自己的功能，不是 M3a 的冻结接口）：
/// DbPort 原样读写，不另抄一份类型。
pub use crewd_collab::hibernate::{Hibernated, Record as HibernateRecord};
pub use crewd_proto::RestartReason;

/// 默认错误类型是 anyhow；带第二个参数时就是 std 的 `Result<T, E>`——`use super::port::*` 的模块里
/// 两种写法都照常用（控制者裁定 C16 / 预检 G-1：一参的别名会遮住 std `Result`，E0107）。
pub type Result<T, E = anyhow::Error> = std::result::Result<T, E>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desired {
    Running,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRow {
    pub id: String,
    pub crew_id: String,
    pub captain: bool,
    /// `claude` / `codex` / `acp:<id>`（§16）。
    pub executor: String,
    pub display_name: Option<String>,
    pub agent_session_id: Option<String>,
    /// 解析好的工作目录：`worktree` → session 自己的 → 机组的；都没有 = `None`（起不了）。
    pub workdir: Option<PathBuf>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub brief: Option<String>,
    pub desired: Desired,
    pub was_working: bool,
    pub last_active_at: Option<Ms>,
    pub resume_failed_at: Option<Ms>,
    pub awaiting_question: Option<String>,
    /// 最近一次「产出」（M3a `sessions.last_progress_at`：工具调用 / 工作项变化，`note_progress` 写；发言不写，C3）。
    pub last_progress_at: Option<Ms>,
    /// 它的机组已被移除（归档，N1 Task 9）：只读、不收新 session、起不来也不该被叫醒（spec §26.5 / §26.7）。
    pub crew_removed: bool,
}

impl SessionRow {
    /// 原样的显示名（没有就取 id 前 8 位）。**插进给 agent / 群看的文字之前必须过
    /// `crewd_collab::render::safe_name`**（控制者裁定 C12 / C20）。
    pub fn name(&self) -> String {
        self.display_name
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| self.id.chars().take(8).collect())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewSession {
    pub crew_id: String,
    pub captain: bool,
    pub executor: String,
    pub brief: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// crewd 为它新建的 git worktree 的绝对路径（`isolation: true`，控制者裁定 C8；`Core::start_new` 建好后填）；None = 共享机组目录。
    pub worktree: Option<PathBuf>,
}

/// 只改给了的列（§18 遗留：`sessions::upsert` 传 None 会抹掉会话号——这里不碰没给的列）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionPatch {
    pub desired: Option<Desired>,
    pub was_working: Option<bool>,
    pub last_active_at: Option<Ms>,
    pub resume_failed_at: Option<Option<Ms>>,
    /// None = 不改。**清不掉**：M3a 的 `sessions::set_profile` 是 `COALESCE(?, model)`，传 None 保留旧值，
    /// 所以这里没有「清掉」这一态（与 `SessionControl::set_profile(model?, effort?)` 的「没给 = 不改」同义）。
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// 唤醒行状态的唯一规则是 `crewd_proto::wake_lifecycle`（控制者裁定 C24；collab 的写函数与 MemPort 都只经它改状态）。
/// crewd-wake 是零依赖的纯判定 crate，自己有一份同名的 `WakeState`；两边只在这里对上。
pub fn lifecycle_state(s: WakeState) -> lifecycle::WakeState {
    match s {
        WakeState::Pending => lifecycle::WakeState::Pending,
        WakeState::Inflight => lifecycle::WakeState::Inflight,
        WakeState::Delivered => lifecycle::WakeState::Delivered,
        WakeState::Dropped => lifecycle::WakeState::Dropped,
    }
}

/// [`lifecycle_state`] 的反方向。
pub fn wake_state(s: lifecycle::WakeState) -> WakeState {
    match s {
        lifecycle::WakeState::Pending => WakeState::Pending,
        lifecycle::WakeState::Inflight => WakeState::Inflight,
        lifecycle::WakeState::Delivered => WakeState::Delivered,
        lifecycle::WakeState::Dropped => WakeState::Dropped,
    }
}

/// 不属于任何一批的结算（一批的结算走 [`Port::ack`]）。每种对应表里的一个事件，表不认 / 没有这一行 = 错误，
/// 整批不改（库在同一事务里回滚）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settle {
    /// 表里的 `Deliver`：pending / inflight → delivered（pending 的 = 游标已越过、已被看到；无附言的恢复在跑就算到）。
    Delivered,
    /// 表里的 `Drop`：pending / inflight → dropped（人停、渲染不出来、没有这个 session、起不来）。
    Dropped { error: String },
    /// 表里的 `Requeue`：inflight → pending，退回 `claim` 记的那一次（crewd 停、撞额度、拿走之后会话没了）。
    Requeue,
}

impl Settle {
    pub fn event(&self) -> lifecycle::WakeEvent {
        match self {
            Self::Delivered => lifecycle::WakeEvent::Deliver,
            Self::Dropped { .. } => lifecycle::WakeEvent::Drop,
            Self::Requeue => lifecycle::WakeEvent::Requeue,
        }
    }
}

/// Exact preparation and receipt identities. Memory variants implement the same minimum
/// cursor/revision/token contract in the unit-test Port; real DB identities remain opaque.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreparedSnapshot {
    Database(crewd_collab::cursor_reservations::Snapshot),
    CausalDatabase {
        reservation: crewd_collab::cursor_reservations::Snapshot,
        causes: crewd_collab::causal::CausalSnapshot,
    },
    #[cfg(test)]
    Memory {
        session: String,
        revision: u64,
        bases: Vec<(String, i64)>,
        surface: Vec<(String, bool)>,
        messages: Vec<(MsgFacts, String)>,
        causal: Option<
            Box<(
                crewd_collab::cursor_reservations::Snapshot,
                crewd_collab::causal::CausalSnapshot,
            )>,
        >,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BatchLease {
    Database(crewd_collab::cursor_reservations::Lease),
    CausalDatabase {
        claimed: Box<crewd_collab::causal::ClaimedOutput>,
        observed: Option<Box<crewd_collab::causal::ObservedSuccess>>,
    },
    #[cfg(test)]
    Memory {
        session: String,
        token: u64,
        generation: u64,
        claimed: Option<Box<crewd_collab::causal::ClaimedOutput>>,
        observed: Option<Box<crewd_collab::causal::ObservedSuccess>>,
    },
}
impl BatchLease {
    #[cfg(test)]
    pub(crate) fn same_memory_owner(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Memory {
                    session: a,
                    token: at,
                    generation: ag,
                    claimed: ac,
                    ..
                },
                Self::Memory {
                    session: b,
                    token: bt,
                    generation: bg,
                    claimed: bc,
                    ..
                },
            ) => a == b && at == bt && ag == bg && ac == bc,
            _ => false,
        }
    }
    pub(crate) fn causal_claimed(&self) -> Option<&crewd_collab::causal::ClaimedOutput> {
        match self {
            Self::CausalDatabase { claimed, .. } => Some(claimed),
            #[cfg(test)]
            Self::Memory { claimed, .. } => claimed.as_deref(),
            _ => None,
        }
    }
    pub(crate) fn causal_witness_mut(
        &mut self,
    ) -> Option<(
        &crewd_collab::causal::ClaimedOutput,
        &mut Option<Box<crewd_collab::causal::ObservedSuccess>>,
    )> {
        match self {
            Self::CausalDatabase { claimed, observed } => Some((claimed, observed)),
            #[cfg(test)]
            Self::Memory {
                claimed: Some(claimed),
                observed,
                ..
            } => Some((claimed, observed)),
            _ => None,
        }
    }
}
pub use crewd_collab::cursor_reservations::Decision;

/// 一次投递拿着的全部行（C24）：进正文的 `wakes`、随批的窗口外行 `riders`（M3a R6 `deferred`）、送到后各群游标推到哪。
/// `wakes` 与 `riders` 在投出去之前一起 [`Port::claim`]（pending → inflight，次数 +1）：一行要么 pending 要么被一批拿着，
/// 没有「看起来 pending、其实跟着一批」的行。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Batch {
    pub session: String,
    pub wakes: Vec<QueuedWake>,
    /// 渲染时落在窗口外的唤醒：不进正文；这批送到时放回 pending（[`Riders::Release`]），没送到时跟这批一起记失败。
    pub riders: Vec<QueuedWake>,
    /// 送到后各群游标推到哪（`Rendered::cursor_targets` = 批里**实际带上**的最大 seq）。只有积压时 `wakes` 可以为空而它非空。
    pub cursor_targets: Vec<(String, i64)>,
    pub prepared: Option<PreparedSnapshot>,
    pub lease: Option<BatchLease>,
}

impl Batch {
    /// 这批拿着的全部行（`wakes` 在前、`riders` 在后）。
    pub fn ids(&self) -> Vec<i64> {
        self.wakes
            .iter()
            .chain(&self.riders)
            .map(|w| w.id)
            .collect()
    }

    /// 有要投的东西：唤醒，或只有积压（游标推进点）。只有随批行的不算（它们只随一批走）。
    pub fn has_work(&self) -> bool {
        !self.wakes.is_empty() || !self.cursor_targets.is_empty()
    }
}

/// 送到时随批行怎么办。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Riders {
    /// 放回 pending（退回 claim 那一次），下一批带上。
    Release,
    /// 一起丢掉（人正在停这个 session：跟别的 pending 一样不再投）。
    Drop(String),
}

/// 一批的终局（[`Port::ack`]）。
#[derive(Clone, Debug, PartialEq)]
pub enum Ack {
    /// 送到了：`wakes` → delivered、`riders` 按这里办、游标推到 `cursor_targets`。
    Delivered { riders: Riders },
    /// 没送到：`wakes` 与 `riders` 全部按**库里的**次数（claim 时记的）判——`crewd_wake::queue::after_failure`
    /// 给了时刻就回 pending、`not_before` = 它；没给 = 到上限放弃（dropped）。
    /// `jitter` 是这一批抽的一个抖动值（`crewd_wake::queue::Jitter`，±20%），原样交给 `after_failure`。
    Failed { error: String, now: Ms, jitter: f64 },
}

/// [`Port::ack`] 提交之后的结果（没提交 = `Err`，什么都没改）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Acked {
    /// Exact old receipt did not own the current batch; no rows/cursor/origins changed.
    pub stale: bool,
    /// `Delivered`：要记进环路熔断的来源（批里进正文的唤醒的来源，去重、按出现先后）。
    pub origins: Vec<String>,
    /// `Failed`：还要再试的 (id, 新的 not_before)。
    pub retried: Vec<(i64, Ms)>,
    /// `Failed`：到上限放弃了的 id。
    pub gave_up: Vec<i64>,
}

/// [`Port::fail_inflight`] 提交之后的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reclaimed {
    /// 还要再试的 (id, 新的 not_before)。
    pub retried: Vec<(i64, Ms)>,
    /// 到上限放弃了的行（结算之后的样子，含 session）：boot 据此告警，**不**回头去封顶的 [`Port::list_wakes`] 里找
    /// （评审 Fix 1：一条崩了好几轮的唤醒 id 很旧，早就不在最新的 200 条里）。
    pub gave_up: Vec<QueuedWake>,
}

/// 系统消息发到哪、@ 谁。群主由 Port 按 M3a 的规则解析（大厅 / 人类群的群主 = 机组机长）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Audience {
    HumanChannel { crew: String, mention_human: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveView {
    pub session_id: String,
    pub running: bool,
    pub working: bool,
    pub health: Option<Health>,
    pub pending_decision: Option<PendingDecision>,
    pub awaiting_question: Option<String>,
    /// 输入框里挡着投递的草稿（T10d；`SessionLive.inputDraft`）。
    pub input_draft: Option<crewd_exec::session::HeldDraft>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    /// 渲染不出来、已被记 dropped 的唤醒 id。
    pub dropped: Vec<i64>,
    /// M3a R6：消息行里、消息落在这批窗口之外的（一个群一批只带最旧的 `UNREAD_CAP` 条）。**不进正文**；它们随这批
    /// 一起被 claim（[`Batch::riders`]，C24）：送到时放回 pending、下一批带上（`WakeBatch.deferred`）；没送到时跟这批
    /// 一起记一次失败、到上限放弃——只有积压的批不记账就会无限重投（Task 8 评审 (1)）。
    pub deferred: Vec<i64>,
    /// 投递成功后每个群的游标推到哪（M3a `WakeBatch::cursor_targets()` = 批里**实际带上**的最大 seq）。
    /// 可能在没有任何 `wake_ids` 的时候非空（只有积压的窗口），这样的批照样要投。
    pub cursor_targets: Vec<(String, i64)>,
    pub prepared: Option<PreparedSnapshot>,
}

/// 改设置（`settings.set`）：外层 None = 不改；`max_live` / `hibernate_deadline_secs` 里层 None = 清掉（回到默认）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsUpdate {
    pub lang: Option<crewd_wake::text::Lang>,
    pub max_live: Option<Option<usize>>,
    pub hibernate_deadline_secs: Option<Option<u32>>,
}

/// M3c 自己发的事件（M3a 的协作事件由 collab 发）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrchestraEvent {
    SessionChanged(LiveView),
    /// Codex 已完成的结构化条目，保持原始字段。
    RunnerItem {
        session: String,
        turn_id: String,
        item: serde_json::Value,
    },
    /// 服务端确认的档位；None 表示本次没有提供该字段。
    RunnerProfile {
        session: String,
        profile: crewd_exec::session::Profile,
    },
    /// 这个 session 的唤醒行变了：这一次改到的行（入队 / 拿走 / 送到 / 失败 / 作废）与它们的消息（S54）。
    WakeChanged {
        session: String,
        wake_ids: Vec<i64>,
        message_ids: Vec<String>,
    },
}

/// 消息类唤醒指向的消息 id（`msg:<id>` 的 id）；别的种类 = None。
pub fn message_id(w: &QueuedWake) -> Option<&str> {
    match w.kind {
        crewd_wake::WakeKind::Message => w.source.strip_prefix("msg:"),
        _ => None,
    }
}

/// 一次改动（这几行）的 `WakeChanged`：行 id 按出现先后、消息 id 去重。
pub fn wake_changed<'a>(
    session: &str,
    rows: impl IntoIterator<Item = &'a QueuedWake>,
) -> OrchestraEvent {
    let (mut wake_ids, mut message_ids) = (Vec::new(), Vec::<String>::new());
    for w in rows {
        wake_ids.push(w.id);
        if let Some(m) = message_id(w)
            && !message_ids.iter().any(|x| x == m)
        {
            message_ids.push(m.to_string());
        }
    }
    OrchestraEvent::WakeChanged {
        session: session.to_string(),
        wake_ids,
        message_ids,
    }
}

/// [`Port::list_wakes`] 至多给几条（最新的在前）：DbPort 落在 collab `wakes::list(.., limit)`（`ORDER BY id DESC LIMIT`）。
/// 它是给人看的近况（`wake.list`），**不能**拿来找「某几行」——要哪几行，让写它们的那个调用把它们带回来。
pub const WAKE_LIST_LIMIT: usize = 200;

pub trait Port: Send + Sync + 'static {
    // —— sessions 行
    fn session(&self, id: &str) -> impl Future<Output = Result<Option<SessionRow>>> + Send;
    fn sessions_desired_running(&self) -> impl Future<Output = Result<Vec<SessionRow>>> + Send;
    fn all_session_ids(&self) -> impl Future<Output = Result<Vec<String>>> + Send;
    fn insert_session(&self, s: NewSession) -> impl Future<Output = Result<SessionRow>> + Send;
    /// 机组的工作目录（`isolation: true` 建 worktree 的起点，C8）；没有 = None。
    fn crew_workdir(&self, crew: &str) -> impl Future<Output = Result<Option<PathBuf>>> + Send;
    fn patch_session(&self, id: &str, p: SessionPatch) -> impl Future<Output = Result<()>> + Send;
    /// 换机长；返回原机长（没有 / 就是它 → None）。
    fn set_captain(
        &self,
        crew: &str,
        session: &str,
    ) -> impl Future<Output = Result<Option<String>>> + Send;
    /// 机组此刻的机长（= 大厅群主）。
    fn captain(&self, crew: &str) -> impl Future<Output = Result<Option<String>>> + Send;
    /// 机组已被移除（归档；M3a `crews::is_archived`）。没有这个机组 = 错误。
    fn crew_removed(&self, crew: &str) -> impl Future<Output = Result<bool>> + Send;
    fn ledger(&self, session: &str) -> Arc<dyn AgentIdLedger>;
    fn worldview(&self, session: &str) -> impl Future<Output = Result<String>> + Send;
    fn mint_token(&self, session: &str) -> String;
    fn causal_fence(&self) -> Option<Arc<crate::causal_fence::CausalFence>> {
        None
    }
    /// 进程退出（含人停掉）后作废它的 helper 令牌（M3a `TokenTable::revoke`）：旧配置文件里的 token 不能再 attach。
    fn revoke_token(&self, session: &str);
    // —— 唤醒队列与熔断
    /// 插入成功 = 新行的 id；`UNIQUE(session, kind, source)` 撞了 = None；目标的机组已被移除（M3a `Enqueued::Skipped`）= None。
    fn enqueue(&self, w: NewWake) -> impl Future<Output = Result<Option<i64>>> + Send;
    /// 该 session 所有 `pending` 的唤醒（不论到没到点），优先级高的在前、同档按入队先后。
    fn pending(&self, session: &str) -> impl Future<Output = Result<Vec<QueuedWake>>> + Send;
    /// 人停：停止状态、全部旧行的持久化取消意图、pending 作废一个事务提交。inflight 保留，允许迟到真实送达；
    /// 之后旧行退回/失败/boot 必须作废，即使新工作已改 desired=Running。Err = 没提交，不能假装停成功。
    /// 返回本次作废的 pending 行（变化事件）；取消覆盖所有旧行，不只内存里的 batch。
    fn human_stop(&self, session: &str) -> impl Future<Output = Result<Vec<QueuedWake>>> + Send;
    /// 不属于一批的结算（见 [`Settle`]）：全部 id 一个事务，表不认 / 没有这一行 = 错误、整批不改。
    fn settle(&self, ids: &[i64], s: Settle, now: Ms) -> impl Future<Output = Result<()>> + Send;
    /// 投出去之前拿走一批（`wakes` + `riders`）：pending → inflight，次数在这里 +1（库记次数，C24）。一个事务、全有或全无。
    fn claim(&self, b: &Batch) -> impl Future<Output = Result<()>> + Send;
    /// Every actual rendered body, including zero wake IDs, must acquire exact ownership.
    fn claim_reserved(
        &self,
        b: &Batch,
        generation: u64,
    ) -> impl Future<Output = Result<Decision<BatchLease>>> + Send;
    /// Non-Delivered terminal fate plus exact lease release, one transaction.
    fn settle_batch(
        &self,
        b: &Batch,
        s: Settle,
        now: Ms,
    ) -> impl Future<Output = Result<Decision<()>>> + Send;

    /// 一批的终局，**一次调用、一个事务**（C24）：送到 = 行 + 随批行 + 游标一起提交；没送到 = 全部行一起退避或放弃。
    /// `Err` = 什么都没改（那几行仍是 inflight，由重启时的回收接走）。
    fn ack(&self, b: &Batch, a: Ack) -> impl Future<Output = Result<Acked>> + Send;
    /// 启动时：上次没确认的全部 inflight 按一次失败的尝试结算（C25；M3a `wakes::reset_inflight`）——次数不退回，
    /// 按库里的次数问 `crewd_wake::queue::after_restart(n, now)`：没到上限立刻回 pending（`not_before = now`），
    /// 到了上限放弃（dropped）。人停已取消的旧行只作废（不退次数，记 stopped），不列入 retried / gave_up；
    /// 其余记 `error`（last_error）。一个事务；真正到上限放弃的行原样带回（[`Reclaimed`]）。
    fn fail_inflight(&self, error: &str, now: Ms)
    -> impl Future<Output = Result<Reclaimed>> + Send;
    /// 有到期 pending 的 session：按其中最高优先级降序、再按最早一条的 id 升序。
    fn due_sessions(&self, now: Ms) -> impl Future<Output = Result<Vec<String>>> + Send;
    /// 晚于 `now` 的最早 `not_before`（已到期的不算，免得忙等）。
    fn next_not_before_after(&self, now: Ms) -> impl Future<Output = Result<Option<Ms>>> + Send;
    /// 最新的至多 [`WAKE_LIST_LIMIT`] 条（任何状态），新的在前。
    fn list_wakes(
        &self,
        session: Option<&str>,
    ) -> impl Future<Output = Result<Vec<QueuedWake>>> + Send;
    /// 一条消息给每个 session 入队的唤醒（任何状态，按入队先后；`message.receipts`，S54）。
    fn wakes_for_message(
        &self,
        message_id: &str,
    ) -> impl Future<Output = Result<Vec<QueuedWake>>> + Send;
    /// Readiness has already passed; install exact new episodes and notice atomically.
    fn prepare_causal_admission(
        &self,
        _session: &str,
        _now: Ms,
    ) -> impl Future<Output = Result<()>> + Send {
        async { anyhow::bail!("causal preparation model is unavailable") }
    }
    /// Read only current surface/unread causes: never initialize cursors or install episodes.
    fn causal_diagnostics(
        &self,
        _session: &str,
        _now: Ms,
    ) -> impl Future<Output = Result<Vec<(String, crewd_wake::explain::CausalBlockage)>>> + Send
    {
        async { anyhow::bail!("causal diagnostic model is unavailable") }
    }
    fn causal_holds(
        &self,
        _target: Option<&str>,
    ) -> impl Future<Output = Result<Vec<crewd_collab::causal::ChainHold>>> + Send {
        async { anyhow::bail!("causal hold model is unavailable") }
    }
    fn recover_chain(
        &self,
        _chain: &str,
        _from: &str,
        _target: &str,
        _action: &crewd_collab::causal::HumanRecoveryAction,
    ) -> impl Future<Output = Result<bool>> + Send {
        async { anyhow::bail!("causal recovery model is unavailable") }
    }
    fn latches(&self) -> impl Future<Output = Result<Vec<Latch>>> + Send;
    fn put_latch(&self, l: &Latch) -> impl Future<Output = Result<()>> + Send;
    fn clear_latch(&self, scope: LatchScope, id: &str) -> impl Future<Output = Result<()>> + Send;
    // —— 设置（M3a `collab::settings`）
    /// 可选并发上限（§22.7 Q5：缺省 None = 不设）。
    fn max_live(&self) -> impl Future<Output = Result<Option<usize>>> + Send;
    fn locale(&self) -> impl Future<Output = Result<crewd_wake::text::Lang>> + Send;
    /// 改设置（`settings.set`）：给了的才写，**一个事务**——库拒了其中一个（如上限 0）就什么都不改（修复轮 M4）。
    fn set_settings(&self, u: &SettingsUpdate) -> impl Future<Output = Result<()>> + Send;
    // —— 重启询问与默认方案（M3a `collab::restart` / `collab::asks`）
    /// 开一次「是否恢复这些 session」的询问（M3a 在相关根机组人类群各发一条 @人）。返回询问 id；名单空 = None。
    /// `reason = Hibernate`（C26）：公告说「上次休眠了 N 个」，休眠记录记下这条询问（collab `restart::open_with`）。
    fn open_restart(
        &self,
        session_ids: Vec<String>,
        reason: RestartReason,
    ) -> impl Future<Output = Result<Option<i64>>> + Send;
    /// 这条重启询问此刻的答复（M3a `restart::get(conn, id).answer`）：`Some(true)` = 恢复、`Some(false)` = 先不、None = 没答。
    fn restart_answer(&self, id: i64) -> impl Future<Output = Result<Option<bool>>> + Send;
    /// 各机组到点的默认方案按默认办（M3a `asks::apply_due_defaults`；控制者裁定由 M3c 的定时拍调）。
    fn apply_due_defaults(&self) -> impl Future<Output = Result<()>> + Send;
    fn enqueue_work_checkpoints(&self) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }
    /// 删掉 `before` 之前就已 delivered / dropped 的队列行，一次至多 `limit` 条（M3a `wakes::prune`；控制者裁定 C6：
    /// 清理归 M3c 的定时拍；S28：分批）。返回删了几条。
    fn prune_wakes(&self, before: Ms, limit: usize) -> impl Future<Output = Result<usize>> + Send;
    // —— 休眠（C26；collab `settings::hibernate_deadline_secs`、`hibernate::{get, put}`）
    /// 休眠期限的设置（秒）；None = 没设（用 `crewd_wake::policy::DEFAULT_HIBERNATE_DEADLINE_SECS`）。
    fn hibernate_deadline_secs(&self) -> impl Future<Output = Result<Option<u32>>> + Send;
    /// 最近一次休眠的记录。
    fn hibernate_record(&self) -> impl Future<Output = Result<Option<HibernateRecord>>> + Send;
    /// 整份写入（`prompt_id` 原样保留调用方给的）。
    fn put_hibernate(&self, r: &HibernateRecord) -> impl Future<Output = Result<()>> + Send;
    // —— 协作数据（M3a 的 Collab）
    /// 消息的规划要素 + 正文（正文只给 Arbiter）。
    fn message(
        &self,
        r: &MessageRef,
    ) -> impl Future<Output = Result<Option<(MsgFacts, String)>>> + Send;
    /// `now` 来自注入的 `Clock`：只取此刻还有效的收听（M3a `listens::active_for_channel(conn, ch, now_ms)` 按它滤）。
    /// 填法见控制者裁定 C22（T10a 补充 §2.2）：六个字段都要填（`dm_sessions` = dm 里的 session 各方，别的群为空）；
    /// 群里有 session 成员或收听者时 `live_sessions` 不能为空。
    fn channel_ctx(
        &self,
        channel: &str,
        now: Ms,
    ) -> impl Future<Output = Result<Option<ChannelCtx>>> + Send;
    /// 该 session 所在各群里游标之后的消息。
    fn unread(&self, session: &str) -> impl Future<Output = Result<Vec<MsgFacts>>> + Send;
    /// 游标只经 [`ack`](Port::ack) 往前推（跟送到同一个事务：游标是消息唤醒的幂等键）。
    fn cursor(&self, session: &str, channel: &str) -> impl Future<Output = Result<i64>> + Send;
    fn post_system(&self, to: Audience, text: String) -> impl Future<Output = Result<()>> + Send;
    /// 唤醒正文（`collab::render::{build_batch, wake_text}`）。渲染不出来的行（消息已删、问已撤回）由实现记 dropped，
    /// 不出现在正文里，并在 `Rendered::dropped` 里报回（调用方把它们从这一批里拿掉）；窗口外的消息行报在
    /// `Rendered::deferred`（调用方把它们从正文里拿掉、作为随批行一起 claim）；游标推进点在 `Rendered::cursor_targets`。
    fn render_wake(
        &self,
        session: &str,
        batch: &[QueuedWake],
    ) -> impl Future<Output = Result<Rendered>> + Send;
    /// 与 Claude Stop hook 共用收尾规则，回合号用于事务内重放去重。
    fn runner_turn_end(
        &self,
        session: &str,
        turn_id: &str,
        text: &str,
    ) -> impl Future<Output = Result<()>> + Send;
    fn publish(&self, e: OrchestraEvent);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Task 8b 评审 (4)：crewd-wake 的 `WakeState` 与共享表的 `WakeState` 两个方向互转是一一对应的（四个状态都过一遍）。
    #[test]
    fn wake_state_round_trips_through_the_shared_table() {
        for s in [
            WakeState::Pending,
            WakeState::Inflight,
            WakeState::Delivered,
            WakeState::Dropped,
        ] {
            assert_eq!(wake_state(lifecycle_state(s)), s);
        }
        for s in lifecycle::WakeState::ALL {
            assert_eq!(lifecycle_state(wake_state(s)), s);
        }
    }
}
