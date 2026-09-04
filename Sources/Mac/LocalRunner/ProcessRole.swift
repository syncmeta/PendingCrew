#if os(macOS)
import Foundation

/// 本进程在「前后端分离」里扮演的角色（spec `docs/internal/2026-08-19-backend-split-design.md` §6.2）。
///
/// 存在的理由只有一个：**防双头**。同一批共享账本（白板/Todo/账本）和同一批长期
/// 定时器（唤醒器/中继/额度轮询）必须只有一个所有者。这个枚举把「我有没有资格
/// 跑这些」变成一条可断言的事实，而不是靠每个人自觉。
///
/// 判定在进程启动时算一次、之后只读 —— 中途不切（切了就等于中途换所有者）。
enum ProcessRole: String {
    /// 长期职责的所有者：跑定时器、写编排性账目、养 agent 子进程。
    case orchestrator
    /// 只看不管：连上去显示、发指令，不持有任何长期定时器。
    case viewer
    /// MCP helper 短命子进程（跑完即退）。直写共享账本是它的正常工作，
    /// 但它不构成「第二个编排者」—— 它不长期存活、不持有定时器。
    case helper

    /// 总闸环境变量名。`inproc`（默认）= GUI 进程自己就是所有者；
    /// `daemon` = 所有权在常驻后台进程，GUI 退化成 viewer。
    static let backendEnvKey = "PENDINGCREW_BACKEND"

    /// 纯判定（可单测）。优先级：helper argv > daemon argv > 总闸。
    ///
    /// 兜底选 `.orchestrator` 而不是 `.viewer`：总闸拼错时，「没人管账」比
    /// 「两个人管账」更难发现 —— 唤醒器全不跑、session 静静地没人叫醒，
    /// 而那正是我们最怕的静默失效。
    static func resolve(argv: [String], backendFlag: String?) -> ProcessRole {
        let helperFlags = ["--mcp-serve", "--mcp-hook", "--mcp-permission-hook", "--mcp-turn-hook"]
        if argv.contains(where: helperFlags.contains) { return .helper }
        if argv.contains("--daemon") { return .orchestrator }
        let flag = (backendFlag ?? "").trimmingCharacters(in: .whitespaces).lowercased()
        return flag == "daemon" ? .viewer : .orchestrator
    }

    /// 本进程**想**当的角色 —— argv / 总闸算出来的意图。第一次取用时算一次，之后固定。
    ///
    /// ⚠️ **它不叫 `current` 是有原因的。** 叫 `current` 时它读起来像「当前实际角色」，
    /// 而那正是 2026-08-26 差点出事的那个误读：补上 app 侧编排闸门之后，
    /// 「想当编排者」和「当上了编排者」第一次可以不相等。问「我该不该动共享账、
    /// 该不该起长期定时器」时要问的是 `effective`，见下。
    ///
    /// 改名那一拍是**故意让编译器把每个调用点顶出来逐个定**的 —— 这比一份手工维护
    /// 的「哪些地方该改」名单硬：名单和它要防的东西不在同一个地方，方案一变就成了假的。
    static let requested: ProcessRole = resolve(
        argv: CommandLine.arguments,
        backendFlag: ProcessInfo.processInfo.environment[backendEnvKey])

    /// **过了编排闸门之后**的角色（`OrchestrationGate`）。
    ///
    /// 为什么要分成两个：`requested` 是 argv/总闸算出来的**意图**，而 2026-08-26 补上
    /// app 侧闸门之后多出了一种新状态 —— **`requested == .orchestrator` 但没拿到锁**
    /// （已经有一个 daemon 或另一个窗口在编排）。那个进程不是编排者，可它的
    /// `requested` 还写着 `.orchestrator`。
    ///
    /// **不分开的话会当场造出一个双头**：`CrewStore.ownsSharedControlChannel` 只看
    /// `requested`，于是这个「没拿到锁的 app」照样去排空共享控制通道 —— 那三条通道是
    /// 「一文件一命令、排空后删」的无锁模型，两边都排会让机长的 `start_session`
    /// 被随机一方吞掉，不报错、不重试、命令文件已经删了。**修一个双头的改动顺手
    /// 造出另一个双头**，正是这一期最该避免的形状。
    ///
    /// 闸门没装时（`--daemon` 进程、单测）退回 `requested`，行为与从前一致。
    static var effective: ProcessRole {
        guard requested == .orchestrator else { return requested }
        switch OrchestrationGate.shared?.decision {
        case .none, .some(.takeOver), .some(.notOrchestrator):
            return .orchestrator
        case .some(.followDaemon), .some(.conflict):
            return .viewer
        }
    }
}
#endif
