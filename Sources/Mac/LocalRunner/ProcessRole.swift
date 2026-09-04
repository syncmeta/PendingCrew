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

    /// 总闸环境变量名。**默认（不设）= `daemon`**：所有权在常驻后台进程，
    /// GUI 退化成 viewer —— 这就是「关掉 / 更新 app 而 session 不断」。
    /// `inproc` 是**显式的退回开关**（设计 §9 那张表的「回退方式」列）。
    static let backendEnvKey = "PENDINGCREW_BACKEND"

    /// 纯判定（可单测）。优先级：helper argv > daemon argv > 总闸。
    ///
    /// **2026-09-04：默认从 `inproc` 翻成 daemon**（P5a 收尾）。翻之前不设总闸兜底
    /// 选 `.orchestrator`，理由是「没人管账比两个人管账更难发现」—— 唤醒器全不跑、
    /// session 静静地没人叫醒，正是最怕的静默失效。
    ///
    /// **那条理由在翻完之后不再成立**，所以兜底跟着改成 `.viewer`：viewer 连不上
    /// 后台时不会静默 —— 按 §9.2 那张表，要么后台真的在跑，要么**拿到独占编排锁
    /// 之后**临时本地接管（界面上一直挂着说明），要么给一条可操作的错误。
    /// 三条出口没有一条是「没人管账且没人知道」，见 `OrchestrationFallback`。
    ///
    /// 只有**恰好** `inproc` 才退回老路；拼错的值按新默认走（它是安全的那一侧：
    /// 会有一个后台起来管账，而不是两个进程一起管）。
    static func resolve(argv: [String], backendFlag: String?) -> ProcessRole {
        let helperFlags = ["--mcp-serve", "--mcp-hook", "--mcp-permission-hook", "--mcp-turn-hook"]
        if argv.contains(where: helperFlags.contains) { return .helper }
        if argv.contains("--daemon") { return .orchestrator }
        let flag = (backendFlag ?? "").trimmingCharacters(in: .whitespaces).lowercased()
        return flag == "inproc" ? .orchestrator : .viewer
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
        effective(requested: requested,
                  decision: OrchestrationGate.shared?.decision,
                  localFallbackActive: LocalOrchestrationFallback.shared.isActive)
    }

    /// 上面那条的**纯判定**版本。
    ///
    /// 拆出来不是为了好看：翻默认之后测试进程自己的 `requested` 就是 `.viewer` 了，
    /// 而这套判定要覆盖的恰恰是「`requested` 是 orchestrator 但没拿到锁」那种组合。
    /// 让它依赖环境的话，那几条会在测试进程里**静默 skip** —— 一条 skip 掉的测试
    /// 和一条没写的测试是同一个东西，而这里守的正好是「修一个双头顺手造出另一个」。
    static func effective(requested: ProcessRole,
                          decision: OrchestrationGate.Decision?,
                          localFallbackActive: Bool) -> ProcessRole {
        // §9.2 唯一允许的那一支：viewer 拿到了独占编排锁、且后台确实起不来，
        // 于是本进程**真的**成了这个数据根的编排者。`requested` 仍写着 `.viewer`
        // （身份不被改写），但「我该不该动共享账、该不该起长期定时器」这个问题
        // 的答案已经变了 —— 那正是 `effective` 存在的意义。
        if localFallbackActive { return .orchestrator }
        guard requested == .orchestrator else { return requested }
        switch decision {
        case .none, .some(.takeOver), .some(.notOrchestrator):
            return .orchestrator
        case .some(.followDaemon), .some(.conflict):
            return .viewer
        }
    }
}

/// **「本窗口临时接管了编排」这件事的唯一真值**（设计 §9.2）。
///
/// 它只有两个状态，而且**只能从「没接管」翻到「接管了」**，不能翻回来 ——
/// 这不是偷懒，是这一期最该守住的那条：回退期间本进程已经在养真的 agent 子进程
/// （它们是这个进程的孩子，交不给 daemon），半路把编排交还就是 §9.2 附加约束 2
/// 点名的「半停一半留 = 双头的另一种形状」。所以回到后台模式的唯一走法是**重开
/// app**，而界面上那条常驻横幅就是这么写的。
///
/// 接管的同时**持有编排锁**：锁在我们手上，任何一个 daemon 都起不来（它会当场
/// 拒绝启动），于是「第二个 host」在结构上不可能出现，不靠谁记得去检查。
/// **不是 `@MainActor`**：`ProcessRole.effective` 是 nonisolated 的（那几条
/// `precondition` 分布在各个类型的 `start` 里），所以这里自己上锁。
final class LocalOrchestrationFallback: @unchecked Sendable {
    static let shared = LocalOrchestrationFallback()

    private let lock = NSLock()
    /// 编排锁的持有句柄。**只存不用** —— 它存在的全部意义就是活着（释放即解锁）。
    private var handle: SessionOrchestratorLock.Handle?
    private var active = false
    private var takeoverReason: String?

    var isActive: Bool { lock.lock(); defer { lock.unlock() }; return active }
    var reason: String? { lock.lock(); defer { lock.unlock() }; return takeoverReason }

    /// 接管。`handle` 必须是**刚刚真的取到**的那把锁 —— 没有锁就没有资格，
    /// 这个方法不替调用方去取，免得「取锁」这件事有第二个入口。
    func takeOver(handle: SessionOrchestratorLock.Handle, reason: String) {
        lock.lock()
        defer { lock.unlock() }
        guard !active else { return }
        self.handle = handle
        self.active = true
        self.takeoverReason = reason
    }

    /// 单测还原用。生产上没有调用点 —— 见类型注释：接管之后不许翻回来。
    func resetForTesting() {
        lock.lock()
        defer { lock.unlock() }
        handle = nil
        active = false
        takeoverReason = nil
    }
}
#endif