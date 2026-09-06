#if os(macOS)
import Foundation

/// 机长交接该在**哪个进程**里执行，以及有界启动循环每一轮该走哪一步。
///
/// 存在的理由：前后端分离之后，`CrewSessionRunner.runs` 在 GUI 进程里是 daemon
/// 那份 roster 的**镜像**，而不是事实。交接的核心动作（停旧 → 起新 → 确认新的真的
/// 在跑）全部是「对 run 做点什么，然后看 run 变成什么样」——**在镜像上做这件事，
/// 「看」永远比「做」晚一拍**。晚一拍还不是最坏的：`launchCaptainForHandoff` 每一轮
/// 开头都无条件停掉本 crew 全部 captain run，于是下一轮杀掉的正是上一轮刚起好的。
///
/// 同一个文件里其它编排动作（`stop` / `remove` / `applyProfileChange` /
/// `applyCodexApprovalMode` / `startCaptain` / `startForBrief`）都先问一句「我是不是
/// 只在看」再决定转交后台；交接自己的另外两条腿也都问了（MCP 那条走
/// `CrewStore.ownsSharedControlChannel`，重启续接那条被 `SessionHost.start` 的
/// `precondition(effective == .orchestrator)` 兜住）。**只有 GUI 那两个入口没问。**
/// 这个类型就是把那句问话变成一处有名字、能单测的判定，而不是散在各处的 `if isViewer`。
enum CaptainHandoffSite {
    /// 这一笔交接在本进程里该怎么处理。
    enum Decision: Equatable {
        /// 本进程持有真的 run：就地执行停旧 / 起新 / 回滚。
        case executeHere
        /// 本进程只看得到镜像：把**整笔**交接转交给持有 run 的进程。
        ///
        /// 注意是「整笔」而不是「其中的起新那一步」——只转发起新、留着确认在本地，
        /// 恰好就是今天这个 bug 的形状。
        case forwardToOwner
    }

    static func decide(isViewer: Bool) -> Decision {
        // TODO(Todo #101): 今天的实现就是这样——两个 GUI 入口谁都没问过归属，
        // 于是 viewer 里也就地执行。红测试钉的正是这一行。
        return .executeHere
    }

    /// 有界启动循环的一次尝试该走哪一步。
    ///
    /// 这是 `launchCaptainForHandoff` 那个 `for attempt in 0..<30` 的纯模型：
    /// 每一轮先停掉本进程**看得到**的 captain run，再请求起新，然后回头看本进程
    /// **看得到**的 roster 里有没有一个在跑的机长。
    /// 每一轮的固定动作是「先停掉看得到的占槽者，再请求起新」；这个判定回答的是
    /// **起完之后**该收工、该再来一轮，还是该放弃。
    enum Step: Equatable {
        /// 看到在跑的机长了，收工。
        case confirmed
        /// 没看到；睡一下再来一轮（下一轮开头照旧先停掉看得到的占槽者）。
        case retry
        /// 试满了，放弃。
        case exhausted
    }

    static func step(attempt: Int, maxAttempts: Int, rosterShowsRunningCaptain: Bool) -> Step {
        if rosterShowsRunningCaptain { return .confirmed }
        return attempt + 1 < maxAttempts ? .retry : .exhausted
    }

    /// 有界启动循环的轮数上限（与 `launchCaptainForHandoff` 共用同一个常量）。
    static let launchAttempts = 30
}
#endif
