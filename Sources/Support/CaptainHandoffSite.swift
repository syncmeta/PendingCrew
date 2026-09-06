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
        isViewer ? .forwardToOwner : .executeHere
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

/// 「本进程持有真的 run，可以就地跑一笔机长交接」的凭据。
///
/// 它存在的理由不是类型洁癖，是这个 bug 的形状：**「记得先问一句归属」是一条靠人
/// 记住的规矩**，而同一个文件里五处编排动作记住了、交接的两个 GUI 入口没记住。
/// 靠人记住的规矩挡不住下一个人 —— 所以把那句问话搬进 `executeCaptainHandoff` 的
/// **参数表**：拿不到这张票就调不动它，少一个参数编不过。新加一条交接入口的人
/// 不需要读到任何注释，编译器会把这个问题顶到他脸上。
///
/// **它盖不住什么，说清楚**：这张票挡的是「忘了问」，不是「问了但答错」——
/// 谁硬写一个 `claim(isViewer: false)` 照样拿得到票。想连那个也堵上就得让
/// `isViewer` 不可伪造（比如只能由 runner 自己提供），代价是把 runner 的类型拖进
/// 这个纯文件、这条判定就再也进不了 test bundle。这里选了能被单测的那一侧。
struct CaptainHandoffOwnership {
    private init() {}

    /// 造票的**唯一**入口。`nil` = 本进程只看得到镜像，这笔交接必须转交持有者。
    static func claim(isViewer: Bool) -> CaptainHandoffOwnership? {
        switch CaptainHandoffSite.decide(isViewer: isViewer) {
        case .executeHere: return CaptainHandoffOwnership()
        case .forwardToOwner: return nil
        }
    }
}
#endif
