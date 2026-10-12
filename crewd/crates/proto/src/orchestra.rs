//! M3c 的客户端方法与事件（spec §22.4「客户端 RPC（M3c 新增）」，T10b 补充 §2.1）。方法常量都在
//! `crate::methods::ALL` 里（M3a 的规矩，`lib.rs`）；共享的 `SessionLive` 等类型在 `model`。
//! 服务端给客户端的 M3c 词表（`WakeKindView` / `WakeStateView` / `WakePriorityView` / `BlockerCodeView` /
//! `ReceiptStateView`）都带一个不进契约的 `Unknown`：以后加的词，旧客户端读成 `Unknown`，不至于整条解析失败（S9）。
//! 客户端给服务端的参数照旧严格（`deny_unknown_fields`）。
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use crate::Role;

pub mod methods {
    pub const SESSION_START: &str = "session.start";
    pub const SESSION_STOP: &str = "session.stop";
    pub const SESSION_RESUME: &str = "session.resume";
    pub const SESSION_INSPECT: &str = "session.inspect";
    pub const SESSION_INTERRUPT: &str = "session.interrupt";
    pub const SESSION_RESPOND_REQUEST: &str = "session.respondRunnerRequest";
    pub const SESSION_ANSWER: &str = "session.answer";
    pub const DECISION_LIST: &str = "decision.list";
    pub const SESSION_LIVE: &str = "session.live";
    pub const CREW_SET_CAPTAIN: &str = "crew.setCaptain";
    /// 排障：唤醒行 + 每条为什么还没出去 + 每个目标的近况。
    pub const WAKE_LIST: &str = "wake.list";
    /// 此刻的环路熔断（只有人能解）。
    pub const LOOP_LIST: &str = "loop.list";
    /// 解环路熔断。只有客户端（人）能调，agent 没有入口。
    pub const LOOP_RECOVER: &str = "loop.recover";
    pub const SETTINGS_GET: &str = "settings.get";
    pub const SETTINGS_SET: &str = "settings.set";
    /// 休眠（C26 / U6）：更新、关机之前由客户端调。回执之后 crewd 自己退出。
    pub const HIBERNATE: &str = "crewd.hibernate";
    /// 一条消息叫醒了谁、送到没有、对方在不在干（S54）。
    pub const MESSAGE_RECEIPTS: &str = "message.receipts";

    /// M3c 的全部方法（都在 `crate::methods::ALL` 里；dispatch 按它把请求交给编排）。
    pub const ALL: &[&str] = &[
        SESSION_START,
        SESSION_STOP,
        SESSION_RESUME,
        SESSION_LIVE,
        SESSION_INSPECT,
        SESSION_ANSWER,
        SESSION_RESPOND_REQUEST,
        SESSION_INTERRUPT,
        DECISION_LIST,
        CREW_SET_CAPTAIN,
        WAKE_LIST,
        LOOP_LIST,
        LOOP_RECOVER,
        SETTINGS_GET,
        SETTINGS_SET,
        HIBERNATE,
        MESSAGE_RECEIPTS,
    ];
}

/// `system.hello` 的 `capabilities`：会话管理（编排）在跑时这个 crewd 开着的能力（U1）。没有编排 = 空。
pub const CAPABILITIES: &[&str] = &[
    "orchestra",
    "session.answer",
    "session.respondRunnerRequest",
    "crewd.hibernate",
    "wake.explain",
    "message.receipts",
    "settings",
];

pub mod events {
    pub const SESSION_RUNNER_ITEM: &str = "session.runnerItem";
    pub const SESSION_RUNNER_PROFILE: &str = "session.runnerProfile";
    /// 某个 session 的唤醒行变了（`WakeChanged`）。主题 = `wake`。
    pub const WAKE_CHANGED: &str = "wake.changed";
    pub const TOPIC_WAKE: &str = "wake";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunnerItem {
    pub session_id: String,
    pub turn_id: String,
    pub item: serde_json::Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunnerProfile {
    pub session_id: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub fast: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionInspectParams {
    pub session_id: String,
    #[serde(default)]
    pub max_lines: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionAnswerParams {
    pub session_id: String,
    pub fingerprint: String,
    /// Zero-based index in decision.options; menu digits are executor output only.
    pub choice: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunnerRequestView {
    pub fingerprint: String,
    pub method: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionRespondRequestParams {
    pub session_id: String,
    pub fingerprint: String,
    pub response: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionInspection {
    pub live: crate::SessionLive,
    pub screen: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_request: Option<RunnerRequestView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionAnswerResult {
    pub cleared: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<crate::DecisionView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PendingDecisionView {
    pub session_id: String,
    pub crew_id: String,
    pub title: String,
    pub decision: crate::DecisionView,
    pub since_ms: i64,
}

/// 三态：键不在 = `None`（不改）；`null` = `Some(None)`（清掉）；值 = `Some(Some(v))`（设成它）。
fn double_option<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionStartParams {
    pub crew_id: String,
    pub role: Role,
    /// `claude` / `codex` / `acp:<id>`（M3c 里只有 claude 起得来）。
    pub executor: String,
    pub brief: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// 必填（控制者裁定 C8）：true = 在机组工作目录里新建 git worktree 并在那里起；false = 机组共享目录。
    pub isolation: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionStopParams {
    pub session_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionIdParams {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CrewSetCaptainParams {
    pub crew_id: String,
    pub session_id: String,
}

/// 全可选：`null` 当 `{}`（= 全部 session）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(extend("type" = ["object", "null"]))]
pub struct WakeListParams {
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LatchScopeView {
    /// 「来源 → 目标」这一条（`from` 必填）。
    Edge,
    /// 目标的非 session 来源（系统、它自己的定时）。
    Session,
    /// 一条因果链在精确来源与目标之间的熔断。
    Chain,
}

/// `loop.list` 的一行：一条环路熔断。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LatchView {
    pub scope: LatchScopeView,
    pub target_session_id: String,
    /// `edge` 或 `chain` 的来源（session id，或 `system`）；`session` 级的没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// `chain` 的不透明持久身份；旧两级省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
    /// 诊断短码（如 `no-progress:9/10min`）。
    pub reason: String,
    pub at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoopRecoverParams {
    pub scope: LatchScopeView,
    pub target_session_id: String,
    /// `edge` 或 `chain` 的来源（session id，或 `system`）；缺失由服务端拒绝。
    #[serde(default)]
    pub from: Option<String>,
    /// `chain` 必填，须与持久熔断的来源和目标精确匹配；旧两级不接受它。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
}

/// `loop.recover` 的结果：`false` = 本来就没有这条熔断（不算错）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoopRecovered {
    pub recovered: bool,
}

/// 设置的语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LocaleView {
    Zh,
    En,
}

/// `settings.get` / `settings.set` 的结果：crewd 的全部设置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// 给 agent 与群看的文字的语言（拒绝文字的语言由客户端按码自己定）。
    pub locale: LocaleView,
    /// 可选并发上限（§22.7 Q5：没有 = 不设；只挡自动拉起）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_live_sessions: Option<u32>,
    /// 休眠时等在干活的 session 到安全点的期限（秒；没有 = 默认 120；0 = 立即）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hibernate_deadline_secs: Option<u32>,
}

/// `settings.set`：键不在 = 不改；`null` = 清掉（回到默认）；值 = 设成它。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsPatch {
    /// `zh` / `en`；别的词、以及 `null`（语言没有「清掉」这回事，null 在这张表里只有一个意思）=
    /// -32602 `settings.bad_locale`。契约里写成不可为 null 的 `LocaleView`（修复轮 M5）。
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "LocaleView")]
    pub locale: Option<Option<String>>,
    /// ≥ 1；0 = -32602 `settings.max_live_min`。
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<u32>")]
    pub max_live_sessions: Option<Option<u32>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<u32>")]
    pub hibernate_deadline_secs: Option<Option<u32>>,
}

/// 全可选：`null` 当 `{}`（= 用设置里的期限）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(extend("type" = ["object", "null"]))]
pub struct HibernateParams {
    /// 在干活的最多等几秒（不给 = 设置里的，没设 = 120；0 = 立即打断）。
    #[serde(default)]
    pub deadline_secs: Option<u32>,
}

/// `crewd.hibernate` 的回执（之后 crewd 自己退出）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HibernateResult {
    /// 在安全点停下的（空闲的、期限内做完这一轮的、正在起被叫停的）。
    pub clean: Vec<String>,
    /// 到了期限还在干活、被打断之后停下的（恢复后要重跑被打断的命令）。
    pub interrupted: Vec<String>,
    /// 到期限之后的宽限也没退出的。
    pub failed: Vec<String>,
    /// 全部退出了。
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageReceiptsParams {
    pub message_id: String,
}

/// 这条消息对一个 session 的唤醒走到哪了。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptStateView {
    /// 排着（没轮到、被挡着——含输入框里有草稿，`wake.list` 的 `input_draft`；也包括随在途那一批拿走、但这一批不送的
    /// 窗口外行）。
    Queued,
    /// 正在送（已拿走，等回执）。已交给会话 ≠ 已敲进去：输入框里有人的草稿时会话挡着不写，一直是这一态，草稿清掉才写
    /// （T10d：queued; held while a draft is present）。
    Delivering,
    /// 送进了它的会话（或它已经从别处读到了）。
    Delivered,
    /// 不再送（人停了它、它的机组被移除、送了几次都没送到）。
    Dropped,
    /// 以后加的词（旧客户端读到的）。不在契约里。
    #[serde(other)]
    #[schemars(skip)]
    Unknown,
}

/// `message.receipts` 的一行：一条消息叫醒的一个 session（S54；N2 再加「开始了 / 回复了」）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub session_id: String,
    pub state: ReceiptStateView,
    /// 目标此刻在干活（不论这一行走到哪）。`state = delivered` 且 `working` = 「处理中」；排着的行带 `working`
    /// 只说明它在忙别的。
    pub working: bool,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WakeKindView {
    Message,
    Timer,
    AskAnswer,
    Restart,
    System,
    Checkpoint,
    StopCheck,
    /// 以后加的词。不在契约里。
    #[serde(other)]
    #[schemars(skip)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WakePriorityView {
    Auto,
    System,
    Human,
    /// 以后加的词。不在契约里。
    #[serde(other)]
    #[schemars(skip)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WakeStateView {
    Pending,
    Inflight,
    Delivered,
    Dropped,
    /// 以后加的词。不在契约里。
    #[serde(other)]
    #[schemars(skip)]
    Unknown,
}

/// pending 的唤醒此刻为什么还没出去（crewd-wake `explain::Blocker::code`，词逐字相同）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlockerCodeView {
    CausalCursorUnknown,
    CausalUnresolved,
    CausalBudget,
    /// crewd 在停机 / 休眠。
    ShuttingDown,
    /// 机组已被移除：不拉起，下一次尝试把它作废。
    CrewRemoved,
    /// 目标正在拉起。
    Starting,
    /// 还没到点（`untilMs`）。
    NotDue,
    /// 环路熔断挡着（`detail` = 哪一条；`loop.recover` 解）。
    Held,
    /// 有更高一档的到期唤醒先走。
    BehindHigherPriority,
    /// 上一笔投递还没回执。
    Inflight,
    /// 目标在干活（停下时投）。
    Working,
    /// 目标卡在待决策框上。
    Decision,
    /// 目标的输入框里有人留下的草稿（或分不清是不是）：不往里敲，等人清掉（`SessionLive.inputDraft`）。
    InputDraft,
    /// 目标正在被停掉。
    Stopping,
    /// 额度用尽、额度唤醒还没到点。
    Quota,
    /// 目标要人登录。
    AuthRequired,
    /// 目标起来了但不能用。
    LaunchFailed,
    /// 在等人答重启询问。
    AwaitingRestartAnswer,
    /// 额度还没恢复：自动唤醒不拉起它。
    QuotaNotReset,
    /// 可选并发上限满了（`detail` = 上限）。
    AtCap,
    /// 错开：这一刻之后再拉起（`untilMs`）。
    Stagger,
    /// 以后加的词。不在契约里。
    #[serde(other)]
    #[schemars(skip)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CausalChainHoldView {
    pub chain_id: String,
    pub from: String,
    pub target_session_id: String,
    pub hop: u32,
    pub reason: String,
    pub at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CausalChainCandidateView {
    pub chain_id: String,
    pub from: String,
    pub target_session_id: String,
    pub hop: u32,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CausalUnresolvedView {
    pub message_id: String,
    pub seq: i64,
    pub from: String,
    pub target_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CausalBlockageView {
    /// Missing cursor means the readonly scanner cannot establish an unread prefix.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cursor_unknown: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holds: Vec<CausalChainHoldView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<CausalUnresolvedView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hold_candidates: Vec<CausalChainCandidateView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CausalChannelView {
    pub channel_id: String,
    pub blockage: CausalBlockageView,
}

/// pending 的唤醒此刻为什么还没出去。没有它（`WakeView.blocker` 缺省 / null）= 此刻就有资格出去：已经从别处读到
/// （游标越过）的会在下一次投递时直接记 delivered，同一档超过一批上限的排在下一批——这两种都在下一轮里结掉，
/// 不在这里报。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WakeBlocker {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causal: Option<CausalBlockageView>,
    pub code: BlockerCodeView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 排障用的一行唤醒（`wake.list`）。`kind` / `priority` / `state` 的词同 `wakes` 表。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WakeView {
    pub id: i64,
    pub session_id: String,
    pub kind: WakeKindView,
    pub source: String,
    pub origin: String,
    pub priority: WakePriorityView,
    pub not_before: i64,
    pub state: WakeStateView,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// 只有 pending 的才有：此刻为什么还没出去。没有 = 此刻就有资格出去（已注入的、超出一批上限的在下一轮里结掉）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocker: Option<WakeBlocker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// 消息类唤醒的消息 id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

/// `wake.list` 里一个目标的近况（R9）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TargetView {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub causal_diagnostics: Vec<CausalChannelView>,
    pub session_id: String,
    pub running: bool,
    /// 在途那一批拿着的唤醒（在跑的投递，或正在起的那批）。
    pub inflight_wake_ids: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_id: Option<u64>,
    /// 写进它的会话之前还排着几项（投递 + nudge）。
    pub writes_queued: u32,
    /// 最近一次投递回执（`delivered` / `undelivered` / `abandoned` …）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WakeListResult {
    /// 最新的至多 200 条，新的在前。
    pub wakes: Vec<WakeView>,
    pub targets: Vec<TargetView>,
}

/// `wake.changed` 的数据：这一次改到的唤醒行与它们的消息（S54）。客户端据此刷新 `wake.list` / `message.receipts`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WakeChanged {
    pub session_id: String,
    pub wake_ids: Vec<i64>,
    pub message_ids: Vec<String>,
}

/// (名字, schema) 的一张表。
type Entries = Vec<(&'static str, serde_json::Value)>;

/// 登记进 `api_schema()` 的方法与事件：用 `api_schema` 的同一个生成器（`crate::sub`），类型只进顶层 `$defs`。
/// 返回 (方法 → {params, result}, 事件 → 负载)。
pub(crate) fn schema_entries(g: &mut schemars::SchemaGenerator) -> (Entries, Entries) {
    use crate::{m, many, sub};
    let methods = vec![
        (
            methods::SESSION_INTERRUPT,
            m(sub::<SessionIdParams>(g), sub::<crate::Empty>(g)),
        ),
        (
            methods::SESSION_RESPOND_REQUEST,
            m(
                sub::<SessionRespondRequestParams>(g),
                sub::<SessionAnswerResult>(g),
            ),
        ),
        (
            methods::SESSION_INSPECT,
            m(sub::<SessionInspectParams>(g), sub::<SessionInspection>(g)),
        ),
        (
            methods::SESSION_ANSWER,
            m(sub::<SessionAnswerParams>(g), sub::<SessionAnswerResult>(g)),
        ),
        (
            methods::DECISION_LIST,
            m(
                sub::<crate::NoParams>(g),
                many(sub::<PendingDecisionView>(g)),
            ),
        ),
        (
            methods::SESSION_START,
            m(sub::<SessionStartParams>(g), sub::<crate::Session>(g)),
        ),
        (
            methods::SESSION_STOP,
            m(sub::<SessionStopParams>(g), sub::<crate::Empty>(g)),
        ),
        (
            methods::SESSION_RESUME,
            m(sub::<SessionIdParams>(g), sub::<crate::Empty>(g)),
        ),
        (
            methods::SESSION_LIVE,
            m(sub::<SessionIdParams>(g), sub::<crate::SessionLive>(g)),
        ),
        (
            methods::CREW_SET_CAPTAIN,
            m(sub::<CrewSetCaptainParams>(g), sub::<crate::Empty>(g)),
        ),
        (
            methods::WAKE_LIST,
            m(sub::<WakeListParams>(g), sub::<WakeListResult>(g)),
        ),
        (
            methods::LOOP_LIST,
            m(sub::<crate::NoParams>(g), many(sub::<LatchView>(g))),
        ),
        (
            methods::LOOP_RECOVER,
            m(sub::<LoopRecoverParams>(g), sub::<LoopRecovered>(g)),
        ),
        (
            methods::SETTINGS_GET,
            m(sub::<crate::NoParams>(g), sub::<Settings>(g)),
        ),
        (
            methods::SETTINGS_SET,
            m(sub::<SettingsPatch>(g), sub::<Settings>(g)),
        ),
        (
            methods::HIBERNATE,
            m(sub::<HibernateParams>(g), sub::<HibernateResult>(g)),
        ),
        (
            methods::MESSAGE_RECEIPTS,
            m(sub::<MessageReceiptsParams>(g), many(sub::<Receipt>(g))),
        ),
    ];
    let events = vec![
        (events::WAKE_CHANGED, sub::<WakeChanged>(g)),
        (events::SESSION_RUNNER_ITEM, sub::<RunnerItem>(g)),
        (events::SESSION_RUNNER_PROFILE, sub::<RunnerProfile>(g)),
    ];
    (methods, events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn params_are_strict_and_vocab_is_the_table_words() {
        assert!(
            serde_json::from_value::<SessionStopParams>(
                json!({"sessionId":"s","reason":"r","x":1})
            )
            .is_err()
        );
        assert!(serde_json::from_value::<WakeListParams>(json!({})).is_ok());
        assert!(serde_json::from_value::<WakeListParams>(json!({"bogus":1})).is_err());
        assert_eq!(
            serde_json::to_value(WakeKindView::AskAnswer).unwrap(),
            json!("ask_answer")
        );
        assert_eq!(
            serde_json::to_value(LatchScopeView::Edge).unwrap(),
            json!("edge")
        );
        assert_eq!(
            serde_json::to_value(BlockerCodeView::BehindHigherPriority).unwrap(),
            json!("behind_higher_priority")
        );
        assert_eq!(
            serde_json::to_value(ReceiptStateView::Delivering).unwrap(),
            json!("delivering")
        );
        let p: LoopRecoverParams =
            serde_json::from_value(json!({"scope":"edge","targetSessionId":"w1","from":"cap"}))
                .unwrap();
        assert_eq!(p.from.as_deref(), Some("cap"));
        assert!(
            serde_json::from_value::<LoopRecoverParams>(
                json!({"scope":"bogus","targetSessionId":"w1"})
            )
            .is_err(),
            "客户端给的词照旧严格"
        );
    }

    #[test]
    fn chain_recovery_wire_is_additive_and_legacy_requests_omit_chain_identity() {
        let legacy = json!({"scope":"edge","targetSessionId":"w1","from":"cap"});
        let p: LoopRecoverParams = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(serde_json::to_value(p).unwrap(), legacy);
        let chain =
            json!({"scope":"chain","targetSessionId":"w1","from":"cap","chainId":"c1:opaque"});
        let p: LoopRecoverParams = serde_json::from_value(chain.clone()).unwrap();
        assert_eq!(p.scope, LatchScopeView::Chain);
        assert_eq!(serde_json::to_value(p).unwrap(), chain);
        // Parsing alone grants no authority: identity and route are checked by the human RPC.
        assert!(
            serde_json::from_value::<LoopRecoverParams>(json!({
                "scope":"chain","targetSessionId":"w1","from":"cap",
                "chainId":"c1:opaque","trusted":true
            }))
            .is_err()
        );
        let legacy_hold =
            json!({"scope":"session","targetSessionId":"w1","reason":"held","atMs":7});
        let hold: LatchView = serde_json::from_value(legacy_hold.clone()).unwrap();
        assert_eq!(serde_json::to_value(hold).unwrap(), legacy_hold);
    }

    /// S9：服务端给客户端的 M3c 词表，没见过的词读成 `Unknown`（旧客户端不至于整条解析失败）；契约里没有 `Unknown`。
    #[test]
    fn unknown_server_words_decode_to_unknown_and_the_contract_does_not_list_it() {
        let w = json!("added_in_a_later_crewd");
        assert_eq!(
            serde_json::from_value::<WakeKindView>(w.clone()).unwrap(),
            WakeKindView::Unknown
        );
        assert_eq!(
            serde_json::from_value::<WakePriorityView>(w.clone()).unwrap(),
            WakePriorityView::Unknown
        );
        assert_eq!(
            serde_json::from_value::<WakeStateView>(w.clone()).unwrap(),
            WakeStateView::Unknown
        );
        assert_eq!(
            serde_json::from_value::<BlockerCodeView>(w.clone()).unwrap(),
            BlockerCodeView::Unknown
        );
        assert_eq!(
            serde_json::from_value::<ReceiptStateView>(w).unwrap(),
            ReceiptStateView::Unknown
        );
        for (name, s) in [
            (
                "WakeKindView",
                serde_json::to_value(schemars::schema_for!(WakeKindView)).unwrap(),
            ),
            (
                "WakePriorityView",
                serde_json::to_value(schemars::schema_for!(WakePriorityView)).unwrap(),
            ),
            (
                "WakeStateView",
                serde_json::to_value(schemars::schema_for!(WakeStateView)).unwrap(),
            ),
            (
                "BlockerCodeView",
                serde_json::to_value(schemars::schema_for!(BlockerCodeView)).unwrap(),
            ),
            (
                "ReceiptStateView",
                serde_json::to_value(schemars::schema_for!(ReceiptStateView)).unwrap(),
            ),
        ] {
            let text = s.to_string();
            assert!(
                !text.contains("\"unknown\""),
                "{name} 的契约里不能有 Unknown：{text}"
            );
            assert!(text.contains('"'), "{name}: {text}");
        }
        // 一个整条 WakeView 里夹着新词照样读得出来。
        let v: WakeView = serde_json::from_value(json!({
            "id": 1, "sessionId": "w1", "kind": "brand_new", "source": "x", "origin": "human",
            "priority": "urgent", "notBefore": 0, "state": "parked", "attempts": 0,
            "blocker": {"code": "new_reason"}
        }))
        .unwrap();
        assert_eq!(
            (v.kind, v.priority, v.state, v.blocker.unwrap().code),
            (
                WakeKindView::Unknown,
                WakePriorityView::Unknown,
                WakeStateView::Unknown,
                BlockerCodeView::Unknown
            )
        );
    }

    /// `settings.set`：键不在 = 不改，`null` = 清掉，数 = 设成它（三态）。
    #[test]
    fn settings_patch_tells_absent_from_null_from_a_value() {
        let p: SettingsPatch = serde_json::from_value(json!({})).unwrap();
        assert_eq!(p, SettingsPatch::default());
        let p: SettingsPatch =
            serde_json::from_value(json!({"maxLiveSessions": null, "hibernateDeadlineSecs": 30}))
                .unwrap();
        assert_eq!(p.max_live_sessions, Some(None));
        assert_eq!(p.hibernate_deadline_secs, Some(Some(30)));
        assert_eq!(p.locale, None);
        let p: SettingsPatch = serde_json::from_value(json!({"locale": null})).unwrap();
        assert_eq!(
            p.locale,
            Some(None),
            "null 读得出来，由 settings.set 拒（settings.bad_locale）"
        );
        let s = serde_json::to_value(schemars::schema_for!(SettingsPatch)).unwrap();
        let locale = &s["properties"]["locale"];
        assert!(
            locale["$ref"].is_string()
                && locale.get("anyOf").is_none()
                && locale.get("type").is_none(),
            "契约里语言不可为 null：{locale}"
        );
        let max = &s["properties"]["maxLiveSessions"];
        assert!(
            max.to_string().contains("\"null\""),
            "上限可以 null（清掉）：{max}"
        );
        assert!(serde_json::from_value::<SettingsPatch>(json!({"maxLiveSessions": -1})).is_err());
        assert!(serde_json::from_value::<SettingsPatch>(json!({"bogus": 1})).is_err());
    }

    #[test]
    fn every_method_is_listed_once_and_all_are_in_the_client_table() {
        let mut v = methods::ALL.to_vec();
        v.sort();
        v.dedup();
        assert_eq!(v.len(), methods::ALL.len());
        for m in methods::ALL {
            assert!(
                crate::methods::ALL.contains(m),
                "{m} 不在 crate::methods::ALL 里"
            );
        }
        assert!(crate::events::TOPICS.contains(&events::TOPIC_WAKE));
        assert_eq!(
            events::WAKE_CHANGED.split('.').next(),
            Some(events::TOPIC_WAKE)
        );
    }
}
