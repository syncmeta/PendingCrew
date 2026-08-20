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

    /// 本进程的角色。第一次取用时算一次，之后固定。
    static let current: ProcessRole = resolve(
        argv: CommandLine.arguments,
        backendFlag: ProcessInfo.processInfo.environment[backendEnvKey])
}
#endif
