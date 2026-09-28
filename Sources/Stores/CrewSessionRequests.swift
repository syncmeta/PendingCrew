import Foundation

/// `start_session` 命令排空后的一次待起会话请求。`MacRootView` 观察
/// `CrewStore.sessionSpawnRequests` 数组，逐条调 `runner.startForBrief`。
/// `listen` 命令排空后的一次收听登记请求（群聊收听;#465）。`off == true` 时
/// until/senders 无意义（撤销该 session 的收听）。
struct SessionListenRequest: Equatable {
    let crewId: String
    let sessionId: String
    let until: String?
    let senders: [String]?
    let off: Bool
}

/// `inspect_session` / `nudge_session` / `stop_session` 命令排空后的一次机长操作。
/// `stopReason != nil` = stop；否则 `input == nil` = inspect，非 nil = nudge。
/// `commandId` 用于写应答文件（helper 侧 long-poll `takeCommandResponse`）。
struct SessionOpsRequest: Equatable {
    let commandId: String
    let crewId: String
    let requesterSessionId: String?
    let targetSessionId: String
    let input: String?
    let stopReason: String?
}

/// `change_workdir` 命令排空后的一次待执行迁移。`confirm == false` = 只出预览。
/// `targetHint` 指本 crew 子树里的哪一个（nil = 本 crew）。
struct WorkdirChangeRequest: Equatable {
    let commandId: String
    /// 发起 crew —— 也是允许改动的**子树根**（不能拿它去动别的部门）。
    let crewId: String
    /// 发起的机长 session id：它自己不算「拦路的正在跑的 session」。
    let callerSessionId: String?
    let targetHint: String?
    let newPath: String
    let includeChildren: Bool
    let confirm: Bool
}

/// `set_profile` 命令排空后的一次待切换请求（session 自切模型/effort）。
struct SessionProfileChangeRequest: Equatable {
    let crewId: String
    let sessionId: String
    let model: String?
    let effort: String?
    var fastMode: Bool? = nil
}

/// `schedule_wakeup` 命令排空后的一次待登记唤醒。
struct SessionWakeupRequest: Equatable {
    let id: String
    let crewId: String
    let sessionId: String
    let fireAt: String   // ISO8601
    let note: String
    /// **督办租约**（人类 Todo #107，见 `SupervisionLease`）：这条唤醒替哪条计划
    /// 盯着。nil = 普通 `schedule_wakeup`。非 nil 时 runner 会换成确定性 id
    /// （同一笔委托最多一个在途唤醒）并走督办分支。
    var planNumber: Int? = nil
    /// 督办的基础间隔（秒），退避在它上面翻倍。
    var leaseBaseSeconds: Double? = nil
}

struct SessionSpawnRequest: Equatable {
    let crewId: String
    let brief: String
    /// nil = 随 crew `captainAgentKind`；"claude"/"codex" 覆盖。
    let runner: String?
    /// true = 独立 worktree；false/nil = 共享 crew 目录。
    let isolation: Bool
    /// 模型别名/slug；nil = 对应 runner 默认。
    var model: String? = nil
    /// thinking effort；nil = runner 默认。
    var effort: String? = nil
    /// 机长传的精简 title（≤18 字概括，作 session 显示名）；nil = 从 brief 兜底。
    var title: String? = nil
}

/// captain MCP 入队后交给 live runner 的明确二选一请求。
struct CaptainHandoffControlRequest: Equatable {
    let commandId: String
    /// 发起命令并提供授权的 crew；默认自交接时与 targetCrewId 相同。
    let sourceCrewId: String
    /// 真正执行持久 captain 与 live runner 切换的 crew。
    let targetCrewId: String
    let requesterSessionId: String?
    /// 非 nil = 现有成员模式；此时 runner 必须 nil，真实 kind 从会话账本读取。
    let targetSessionId: String?
    /// 非 nil = 新建模式；值只能为 claude/codex，targetSessionId 必须 nil。
    let runner: String?
    let model: String?
    let effort: String?
    let title: String?
    let openingBrief: String?
}
