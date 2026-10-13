//! 数据形状（spec §4.2 字段表）。只有数据与极小的取值助手；规则在 `invariants`、命令在 `book` 及其兄弟模块。
use crate::ids::{AskId, ChannelId, CrewId, EntryNo, HOUR, MINUTE, SessionId, Ts, WorkId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Open,
    Active,
    Blocked,
    Done,
    Dropped,
}

impl Status {
    pub fn is_terminal(self) -> bool {
        matches!(self, Status::Done | Status::Dropped)
    }
    pub fn is_live(self) -> bool {
        !self.is_terminal()
    }
}

/// 完成层级（spec §4.2 body 里的 local|public|production）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DoneLevel {
    #[default]
    Local,
    Public,
    Production,
}

/// body = 目标 + 完成层级 + 验收标准。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Body {
    pub goal: String,
    pub level: DoneLevel,
    pub acceptance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum Evidence {
    Commit(String),
    Url(String),
    Text(String),
}

impl Evidence {
    pub fn is_blank(&self) -> bool {
        match self {
            Evidence::Commit(s) | Evidence::Url(s) | Evidence::Text(s) => s.trim().is_empty(),
        }
    }
}

/// blockedOn 的一项：一条人类待确认，或另一个工作项。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "lowercase")]
pub enum Blocker {
    Human(AskId),
    Item(WorkId),
}

impl std::fmt::Display for Blocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Blocker::Human(h) => write!(f, "{h}"),
            Blocker::Item(w) => write!(f, "{w}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Worker,
    Captain,
    Admin,
}

/// 谁在做这次修改。机长 / 行政处是 session 的角色，由调用方（crewd）按机组认定后传进来。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Actor {
    Session {
        id: SessionId,
        role: Role,
    },
    /// Trusted runtime evidence. Captain/admin privileges are resolved per object crew.
    ScopedSession {
        id: SessionId,
        authority: AuthorityScope,
    },
    Human,
    /// crewd 自己（调度层 Jev、计时器、迁移）。
    System,
}

/// Supplied by the trusted runtime, never parsed from worker RPC parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityScope {
    pub home: CrewId,
    pub roles: BTreeMap<CrewId, Role>,
}

impl Actor {
    pub fn scoped(id: &str, authority: AuthorityScope) -> Actor {
        Actor::ScopedSession {
            id: SessionId::new(id),
            authority,
        }
    }
    pub fn home(&self) -> Option<&CrewId> {
        match self {
            Actor::ScopedSession { authority, .. } => Some(&authority.home),
            _ => None,
        }
    }
    pub fn role_in(&self, crew: &CrewId, organization: bool) -> Option<Role> {
        match self {
            Actor::ScopedSession { authority, .. } => {
                Some(authority.roles.get(crew).copied().unwrap_or(Role::Worker))
            }
            Actor::Session { role, .. } => Some(if organization { Role::Worker } else { *role }),
            _ => None,
        }
    }
    pub fn worker(id: &str) -> Actor {
        Actor::Session {
            id: SessionId::new(id),
            role: Role::Worker,
        }
    }
    pub fn captain(id: &str) -> Actor {
        Actor::Session {
            id: SessionId::new(id),
            role: Role::Captain,
        }
    }
    pub fn admin(id: &str) -> Actor {
        Actor::Session {
            id: SessionId::new(id),
            role: Role::Admin,
        }
    }
    pub fn session(&self) -> Option<&SessionId> {
        match self {
            Actor::Session { id, .. } | Actor::ScopedSession { id, .. } => Some(id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HowChanged {
    /// 自己认领（认领入口、建项时自己当负责人、领走无主项）。
    Claim,
    /// 原负责人发起、新负责人接受。
    Transfer,
    /// 机长 / 人 / 调度层指派。
    Assign,
    /// 负责人失联后重派。
    Lost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerChange {
    pub from: Option<SessionId>,
    pub to: SessionId,
    pub by: Actor,
    pub how: HowChanged,
    pub note: String,
    pub at: Ts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingTransfer {
    pub to: SessionId,
    pub requested_by: SessionId,
    pub note: String,
    pub at: Ts,
}

/// 最近一次回报（spec §4.3 注入里的「下一步」）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub text: String,
    pub at: Ts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DropKind {
    /// 事实性关闭（行政处也能做，必须带证据）。
    Duplicate,
    Superseded,
    /// 业务决定「不做了」（只有负责人 / 机长 / 人）。
    NotNeeded,
}

impl DropKind {
    pub fn is_factual(self) -> bool {
        matches!(self, DropKind::Duplicate | DropKind::Superseded)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DropReason {
    pub kind: DropKind,
    pub note: String,
    pub evidence: Option<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkItem {
    pub id: WorkId,
    pub title: String,
    pub body: Body,
    pub parent: Option<WorkId>,
    pub depends_on: Vec<WorkId>,
    pub sources: Vec<EntryNo>,
    pub owner: Option<SessionId>,
    pub owner_history: Vec<OwnerChange>,
    pub status: Status,
    pub blocked_on: Vec<Blocker>,
    pub next_check_at: Option<Ts>,
    pub missed_checks: u32,
    pub evidence: Option<Evidence>,
    pub drop_reason: Option<DropReason>,
    pub pending_transfer: Option<PendingTransfer>,
    pub channel: Option<ChannelId>,
    pub last_report: Option<Note>,
    /// 建这一项的 session（调度层 / 人建的为空）。无主的 request_help 项可由它撤回。
    pub created_by: Option<SessionId>,
    pub revision: u64,
    pub created_at: Ts,
    pub updated_at: Ts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryClosure {
    pub by: Actor,
    pub note: String,
    pub evidence: Option<Evidence>,
    pub at: Ts,
}

/// 第一层入口（#N）。原文在 M1 的 todos 表里，这里只记工作跟进需要的部分。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryTodo {
    pub no: EntryNo,
    pub registered_at: Ts,
    /// 机长 / 人的裁定关闭（或行政处带证据的事实关闭）。状态推导见 `views::EntryStatus`。
    pub closed: Option<EntryClosure>,
}

/// 「X 毫秒没回复就按 A 办」（spec §6；到点后照常经过 runner 自己的权限确认，§13 Q2）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultOption {
    pub text: String,
    pub after_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum AskState {
    Open,
    Answered {
        text: String,
        at: Ts,
    },
    Defaulted {
        at: Ts,
    },
    /// 提问者 / 机长 / 人撤回了（问没被回答就不要了）。
    Withdrawn {
        reason: String,
        at: Ts,
    },
}

/// 等人的升级档（spec §6：24h 置顶、72h 问「还要吗」）。
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WaitTier {
    #[default]
    Fresh,
    Pinned,
    StillWanted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanAsk {
    pub id: AskId,
    /// 这条问关于哪一项：卡住时开的问 = 被它卡住的那一项（后来别的项也可以挂到同一条问上）；
    /// `open_ask` 开的问可以不挂任何项（M3 的人类待确认台账也从这里拿 H 号）。
    pub item: Option<WorkId>,
    pub asked_by: Actor,
    pub question: String,
    pub default: Option<DefaultOption>,
    pub asked_at: Ts,
    pub state: AskState,
    /// 已经通知过的最高升级档。
    pub escalated: WaitTier,
    /// true = `open_ask` 直接开的问（不是卡住时开的）：只要还开着、关于的项还活着（或不关于任何项），
    /// 就进人的待确认清单；卡住时开的问只在此刻仍卡着活项时才进（没人等的问不打扰人）。
    pub standalone: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopKind {
    /// 被检查点叫醒过、之后没回报。
    Unanswered,
    /// active 项的检查时间已过、没改。
    Overdue,
    /// 名下 open 项依赖已齐，却没排下一次检查。
    ReadyUnscheduled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StopViolation {
    pub item: WorkId,
    pub kind: StopKind,
}

/// 一个 session 连续几次 idle 都没改善（spec §5 防死循环）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopStreak {
    pub last: Vec<StopViolation>,
    pub unimproved: u32,
    pub escalated: bool,
    /// 自上次「干净」以来叫了几次（不论违反集合变没变；到 `stop_rewake_budget` 就交行政处）。
    pub rewakes: u32,
}

/// 一个机组的全部工作跟进状态。M4b 把它落库 / 读回（`WorkBook::restore` 会整份校验）。
/// Object attribution lives inside the sole authoritative book, not a parallel ledger.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Organization {
    pub item_crews: BTreeMap<WorkId, CrewId>,
    pub ask_crews: BTreeMap<AskId, CrewId>,
    pub entry_crews: BTreeMap<EntryNo, CrewId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookState {
    pub crew: CrewId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization: Option<Organization>,
    /// Organization migration pins this to 3. Legacy books omit it so their
    /// original authority can be diagnosed before an explicit migration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parent_depth: Option<u8>,
    pub items: BTreeMap<WorkId, WorkItem>,
    pub entries: BTreeMap<EntryNo, EntryTodo>,
    pub asks: BTreeMap<AskId, HumanAsk>,
    /// 最后发出的 W 号 / H 号 / 事件序号；号码只增不复用。
    pub last_item: u32,
    pub last_ask: u32,
    pub last_seq: u64,
    /// 账本见过的最晚时间。命令用 max(传入的 now, clock)：墙钟往回跳不会让账本卡死（裁定 10）。
    pub clock: Ts,
    pub stop_streaks: BTreeMap<SessionId, StopStreak>,
}

impl BookState {
    pub fn item_crew(&self, id: WorkId) -> Option<&CrewId> {
        if let Some(org) = &self.organization {
            org.item_crews.get(&id)
        } else {
            self.items.contains_key(&id).then_some(&self.crew)
        }
    }
    pub fn ask_crew(&self, id: AskId) -> Option<&CrewId> {
        if let Some(org) = &self.organization {
            org.ask_crews.get(&id)
        } else {
            self.asks.contains_key(&id).then_some(&self.crew)
        }
    }
    pub fn entry_crew(&self, id: EntryNo) -> Option<&CrewId> {
        if let Some(org) = &self.organization {
            org.entry_crews.get(&id)
        } else {
            self.entries.contains_key(&id).then_some(&self.crew)
        }
    }
    pub fn empty(crew: CrewId) -> BookState {
        BookState {
            crew,
            organization: None,
            max_parent_depth: None,
            items: BTreeMap::new(),
            entries: BTreeMap::new(),
            asks: BTreeMap::new(),
            last_item: 0,
            last_ask: 0,
            last_seq: 0,
            clock: Ts(0),
            stop_streaks: BTreeMap::new(),
        }
    }
}

/// 可配的阈值。默认值来自 spec：检查上限 2h（§4.2）、失联 K=2（§5）、停下即检查 3 次（§5）、
/// 24h / 72h（§6）、巡检每 10 条消息 / 30min（§8.4）。`recheck_after_ms`（叫醒后没回报、多久再叫）由人拍板
/// 默认 15 分钟：配合 K=2，一个不回话的负责人约 30 分钟被判失联（M4a 裁定 Q3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub max_check_horizon_ms: i64,
    pub recheck_after_ms: i64,
    pub missed_checks_limit: u32,
    pub stop_check_limit: u32,
    /// 停下即检查：自上次「干净」以来最多再叫几次（不论违反集合变没变）；超了就交行政处。
    /// 挡住「每次只挪一项、集合每次都变」的来回摆动（复审 P12）。
    pub stop_rewake_budget: u32,
    pub pin_after_ms: i64,
    pub still_wanted_after_ms: i64,
    pub patrol_every_messages: u32,
    pub patrol_interval_ms: i64,
    /// 调用方的 now 比账本时间超前多少算「可疑前跳」（`WorkBook::clock_jump` 报给 M4b；命令照常执行）。
    pub max_clock_jump_ms: i64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_check_horizon_ms: 2 * HOUR,
            recheck_after_ms: 15 * MINUTE,
            missed_checks_limit: 2,
            stop_check_limit: 3,
            stop_rewake_budget: 6,
            pin_after_ms: 24 * HOUR,
            still_wanted_after_ms: 72 * HOUR,
            patrol_every_messages: 10,
            patrol_interval_ms: 30 * MINUTE,
            max_clock_jump_ms: 24 * HOUR,
        }
    }
}

/// 新建项引用别的项：已存在的 W 号，或同一批里排在它前面的第几条。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum ItemRef {
    Id(WorkId),
    Batch(usize),
}

/// 建项参数（认领入口、建项、卡住拆分共用）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crew_id: Option<CrewId>,
    pub title: String,
    pub body: Body,
    pub parent: Option<ItemRef>,
    pub depends_on: Vec<ItemRef>,
    pub sources: Vec<EntryNo>,
    pub owner: Option<SessionId>,
    pub channel: Option<ChannelId>,
    pub next_check_at: Option<Ts>,
}

impl NewItem {
    pub fn titled(title: &str) -> NewItem {
        NewItem {
            title: title.to_string(),
            ..NewItem::default()
        }
    }
    pub fn owned_by(mut self, owner: &str) -> NewItem {
        self.owner = Some(SessionId::new(owner));
        self
    }
    pub fn under(mut self, parent: ItemRef) -> NewItem {
        self.parent = Some(parent);
        self
    }
    pub fn after(mut self, dep: ItemRef) -> NewItem {
        self.depends_on.push(dep);
        self
    }
    pub fn from_entry(mut self, no: EntryNo) -> NewItem {
        self.sources.push(no);
        self
    }
    pub fn check_at(mut self, at: Ts) -> NewItem {
        self.next_check_at = Some(at);
        self
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_statuses() {
        let live: Vec<Status> = [
            Status::Open,
            Status::Active,
            Status::Blocked,
            Status::Done,
            Status::Dropped,
        ]
        .into_iter()
        .filter(|s| s.is_live())
        .collect();
        assert_eq!(live, [Status::Open, Status::Active, Status::Blocked]);
    }

    #[test]
    fn blank_evidence_is_detected() {
        assert!(Evidence::Text("  \n".into()).is_blank());
        assert!(Evidence::Commit(String::new()).is_blank());
        assert!(!Evidence::Url("https://x".into()).is_blank());
    }

    #[test]
    fn wait_tiers_are_ordered() {
        assert!(WaitTier::Fresh < WaitTier::Pinned);
        assert!(WaitTier::Pinned < WaitTier::StillWanted);
    }

    #[test]
    fn spec_defaults() {
        let c = Config::default();
        assert_eq!(c.max_check_horizon_ms, 2 * HOUR);
        assert_eq!(
            c.recheck_after_ms,
            15 * MINUTE,
            "裁定 Q3：15 分钟 × K=2 ≈ 30 分钟判失联"
        );
        assert_eq!(c.missed_checks_limit, 2);
        assert_eq!(c.max_clock_jump_ms, 24 * HOUR);
        assert_eq!(c.stop_check_limit, 3);
        assert_eq!(c.stop_rewake_budget, 6);
        assert_eq!(c.pin_after_ms, 24 * HOUR);
        assert_eq!(c.still_wanted_after_ms, 72 * HOUR);
        assert_eq!(c.patrol_every_messages, 10);
        assert_eq!(c.patrol_interval_ms, 30 * MINUTE);
    }

    #[test]
    fn wire_shapes_are_stable() {
        let b = serde_json::to_value(Blocker::Human(AskId(2))).unwrap();
        assert_eq!(b, serde_json::json!({"kind": "human", "id": "H2"}));
        let e = serde_json::to_value(Evidence::Commit("abc".into())).unwrap();
        assert_eq!(e, serde_json::json!({"kind": "commit", "value": "abc"}));
        let a = serde_json::to_value(Actor::captain("s1")).unwrap();
        assert_eq!(
            a,
            serde_json::json!({"kind": "session", "id": "s1", "role": "captain"})
        );
        assert_eq!(serde_json::to_value(Status::Blocked).unwrap(), "blocked");
        assert_eq!(
            serde_json::to_value(WaitTier::StillWanted).unwrap(),
            "still_wanted"
        );
    }
}
