//! 编排读写数据的唯一出口。生产实现 `db::DbPort`（全部写经 M3a 冻结的 `crewd_collab` 函数；M3c 不建表），
//! 测试实现 `testkit::MemPort`。M3a 的实际名字只在 `db.rs` 里出现。
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use crewd_exec::health::Health;
use crewd_exec::resume::AgentIdLedger;
use crewd_term::PendingDecision;
use crewd_wake::plan::{ChannelCtx, MsgFacts};
use crewd_wake::{Latch, LatchScope, MessageRef, Ms, NewWake, QueuedWake};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settle {
    Inflight,
    Delivered,
    /// 终局失败、还要再试：回 pending、次数 +1、`not_before = retry_at`（M3a `mark_failed(.., Some(retry_at))`，持久化退避）。
    Retry {
        attempts: u32,
        error: String,
        retry_at: Ms,
    },
    Dropped {
        error: String,
    },
    /// inflight → pending，次数不变（进程没了、重启）。
    Requeue,
}

/// 系统消息发到哪、@ 谁。群主由 Port 按 M3a 的规则解析（大厅 / 人类群的群主 = 机组机长）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Audience {
    /// 这个机组所属根机组的人类群，@人。
    HumanChannel {
        crew: String,
    },
    Lobby {
        crew: String,
        mention_owner: bool,
    },
    Channel {
        channel: String,
        mention_owner: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveView {
    pub session_id: String,
    pub running: bool,
    pub working: bool,
    pub health: Option<Health>,
    pub pending_decision: Option<PendingDecision>,
    pub awaiting_question: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    /// 渲染不出来、已被记 dropped 的唤醒 id。
    pub dropped: Vec<i64>,
    /// M3a R6：消息行里、消息落在这批窗口之外的（一个群一批只带最旧的 `UNREAD_CAP` 条）。**不进正文、不 settle**
    /// （仍是 pending，绝不 `mark_dropped`），投递成功、按 `cursor_targets` 推完游标后下一批带上（`WakeBatch.deferred`）。
    pub deferred: Vec<i64>,
    /// 投递成功后每个群的游标推到哪（M3a `WakeBatch::cursor_targets()` = 批里**实际带上**的最大 seq）。
    /// 可能在没有任何 `wake_ids` 的时候非空（只有积压的窗口），这样的批照样要投。
    pub cursor_targets: Vec<(String, i64)>,
}

/// M3c 自己发的事件（M3a 的协作事件由 collab 发）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrchestraEvent {
    SessionChanged(LiveView),
    WakeChanged { session: String },
}

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
    fn ledger(&self, session: &str) -> Arc<dyn AgentIdLedger>;
    fn worldview(&self, session: &str) -> impl Future<Output = Result<String>> + Send;
    fn mint_token(&self, session: &str) -> String;
    /// 进程退出（含人停掉）后作废它的 helper 令牌（M3a `TokenTable::revoke`）：旧配置文件里的 token 不能再 attach。
    fn revoke_token(&self, session: &str);
    // —— 唤醒队列与熔断
    /// 插入成功 = true；`UNIQUE(session, kind, source)` 撞了 = false。
    fn enqueue(&self, w: NewWake) -> impl Future<Output = Result<bool>> + Send;
    /// 该 session 所有 `pending` 的唤醒（不论到没到点）。
    fn pending(&self, session: &str) -> impl Future<Output = Result<Vec<QueuedWake>>> + Send;
    fn settle(&self, ids: &[i64], s: Settle, now: Ms) -> impl Future<Output = Result<()>> + Send;
    fn requeue_inflight(&self) -> impl Future<Output = Result<usize>> + Send;
    /// 有到期 pending 的 session：按其中最高优先级降序、再按最早一条的 id 升序。
    fn due_sessions(&self, now: Ms) -> impl Future<Output = Result<Vec<String>>> + Send;
    /// 晚于 `now` 的最早 `not_before`（已到期的不算，免得忙等）。
    fn next_not_before_after(&self, now: Ms) -> impl Future<Output = Result<Option<Ms>>> + Send;
    fn list_wakes(
        &self,
        session: Option<&str>,
    ) -> impl Future<Output = Result<Vec<QueuedWake>>> + Send;
    fn latches(&self) -> impl Future<Output = Result<Vec<Latch>>> + Send;
    fn put_latch(&self, l: &Latch) -> impl Future<Output = Result<()>> + Send;
    fn clear_latch(&self, scope: LatchScope, id: &str) -> impl Future<Output = Result<()>> + Send;
    // —— 设置（M3a `collab::settings`）
    /// 可选并发上限（§22.7 Q5：缺省 None = 不设）。
    fn max_live(&self) -> impl Future<Output = Result<Option<usize>>> + Send;
    fn set_max_live(&self, max: Option<usize>) -> impl Future<Output = Result<()>> + Send;
    fn locale(&self) -> impl Future<Output = Result<crewd_wake::text::Lang>> + Send;
    // —— 重启询问与默认方案（M3a `collab::restart` / `collab::asks`）
    /// 开一次「是否恢复这些 session」的询问（M3a 在相关根机组人类群各发一条 @人）。返回询问 id；名单空 = None。
    fn open_restart(
        &self,
        session_ids: Vec<String>,
    ) -> impl Future<Output = Result<Option<i64>>> + Send;
    /// 这条重启询问此刻的答复（M3a `restart::get(conn, id).answer`）：`Some(true)` = 恢复、`Some(false)` = 先不、None = 没答。
    fn restart_answer(&self, id: i64) -> impl Future<Output = Result<Option<bool>>> + Send;
    /// 各机组到点的默认方案按默认办（M3a `asks::apply_due_defaults`；控制者裁定由 M3c 的定时拍调）。
    fn apply_due_defaults(&self) -> impl Future<Output = Result<()>> + Send;
    /// 删掉 `before` 之前就已 delivered / dropped 的队列行（M3a `wakes::prune`；控制者裁定 C6：清理归 M3c 的定时拍）。
    fn prune_wakes(&self, before: Ms) -> impl Future<Output = Result<usize>> + Send;
    // —— 协作数据（M3a 的 Collab）
    /// 消息的规划要素 + 正文（正文只给 Arbiter）。
    fn message(
        &self,
        r: &MessageRef,
    ) -> impl Future<Output = Result<Option<(MsgFacts, String)>>> + Send;
    /// `now` 来自注入的 `Clock`：只取此刻还有效的收听（M3a `listens::active_for_channel(conn, ch, now_ms)` 按它滤）。
    /// 填法见控制者裁定 C22：`live_sessions` / `dm_peer` / `crew_captain` 都要填，有 session 时 `live_sessions` 不能为空。
    fn channel_ctx(
        &self,
        channel: &str,
        now: Ms,
    ) -> impl Future<Output = Result<Option<ChannelCtx>>> + Send;
    /// 该 session 所在各群里游标之后的消息。
    fn unread(&self, session: &str) -> impl Future<Output = Result<Vec<MsgFacts>>> + Send;
    fn cursor(&self, session: &str, channel: &str) -> impl Future<Output = Result<i64>> + Send;
    /// 只往前推。
    fn advance(
        &self,
        session: &str,
        channel: &str,
        seq: i64,
    ) -> impl Future<Output = Result<()>> + Send;
    fn post_system(&self, to: Audience, text: String) -> impl Future<Output = Result<()>> + Send;
    /// 唤醒正文（`collab::render::{build_batch, wake_text}`）。渲染不出来的行（消息已删、问已撤回）由实现记 dropped，
    /// 不出现在正文里，并在 `Rendered::dropped` 里报回（调用方把它们从这一批里拿掉）；窗口外的消息行报在
    /// `Rendered::deferred`（调用方也拿掉、但**不碰**它们的状态）；游标推进点在 `Rendered::cursor_targets`。
    fn render_wake(
        &self,
        session: &str,
        batch: &[QueuedWake],
    ) -> impl Future<Output = Result<Rendered>> + Send;
    fn publish(&self, e: OrchestraEvent);
}
