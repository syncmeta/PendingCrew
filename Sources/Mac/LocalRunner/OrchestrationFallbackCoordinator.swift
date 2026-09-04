#if os(macOS)
import Foundation

/// **连不上后台之后那一串动作的执行者**（设计 §9.2）。
///
/// 判据在 `OrchestrationFallback.decide`（纯函数），这里是**顺序**：什么时候才许
/// 去取锁、接管时那条 viewer 腿有没有真的停、没接管时锁有没有真的放回去。
///
/// ## 为什么它不长在 `ViewerSessionClient` 里
///
/// 那三件事任何一件做错，症状都不是「报错」而是**安静地坏**：
///
/// - **取锁太早**：会让我们**自己刚拉起来的那个 daemon 因为锁被占而当场退出**，
///   然后永远连不上，且看不出为什么 —— 我们亲手把自己的后路堵死。
/// - **接管后不停 viewer 腿**：下一次重连成功时就是「本地编排 + 连上的 daemon」
///   两个 host（§9.2 附加约束 2 点名的形状）。
/// - **没接管却把锁攥着**：真正的 daemon 从此起不来，我们自己造了个死结。
///
/// 三条都得有测试盯着，而 `ViewerSessionClient` 住在 `Sources/Mac/Services`、
/// **不进 test bundle**（它拖着半个 app）。所以顺序这一段搬到这里，动作全部
/// 由调用方注入 —— 于是 `OrchestrationFallbackCoordinatorTests` 能一条条钉住它。
///
/// ## ⚠️ 这里**故意没有**「交还编排」那一步 —— 别顺手补上
///
/// 你会注意到接管之后没有任何一条路回到 viewer 模式：连上 daemon 就把本地那套停掉、
/// 把所有权交还，看起来是显然该有的收尾。**它是故意不做的**，理由不是省事：
///
/// **回退期间本进程已经在养真的 agent 子进程 —— 它们是这个进程的孩子，交不给
/// daemon。** 没有一条路能把一个在跑的 PTY / app-server 子进程连同它的终端缓冲区
/// 和会话状态搬进另一个进程；能停的只有编排那半边。于是「交还」在今天只可能是
/// **半停一半留**，而那正是设计 §9.2 附加约束 2 亲口点名的**双头的另一种形状** ——
/// 补上它不是把契约做完整，是把契约破掉。
///
/// 而「任何路径不出现第二个 host」这条**已经满足了，且是结构性的**：接管时我们
/// 持有编排锁（任何 daemon 都会因此拒绝启动）并把 viewer 那条腿整个停掉，
/// 所以「本地编排 + 连上的 daemon」这个状态压根到不了 —— 不靠运行时谁记得去交还。
/// 代价只落在手感上：「重试」退化成「重开 app」，界面横幅上就是这么写的
/// （`OrchestrationNotice.localFallback`）。§9.2 里那句「可重试」本来就是给
/// **禁止接管的那几支**的，不是给回退支的。
///
/// 要做**在线**交还，前提是先有一条能把在跑的 agent 子进程交接出去的路。
/// 那条路不存在之前，这里少的不是一步收尾，而是一个不该有的双头。
/// （2026-09-04 父机长批准按此实现，并要求把这段理由写死在这里。）
@MainActor
final class OrchestrationFallbackCoordinator {

    /// 协调器要做的那几件事，全部由调用方给 —— 这里一个都不自己做，
    /// 免得「取锁」「停腿」各自又多出一个入口。
    struct Hooks {
        var acquireLock: (URL) -> SessionOrchestratorLock.Outcome
        /// 接管：把刚取到的那把锁交出去（**没有锁的接管就是无凭据的双头**）。
        var takeOver: (SessionOrchestratorLock.Handle, String) -> Void
        var stopViewerLeg: () -> Void
        var scheduleReconnect: () -> Void
    }

    private let dataRoot: URL
    private let hooks: Hooks

    init(dataRoot: URL, hooks: Hooks) {
        self.dataRoot = dataRoot
        self.hooks = hooks
    }

    /// 连不上之后走一遍：取观测量 → 问判据 → 执行。返回裁决供界面显示。
    @discardableResult
    func handle(spawn: OrchestrationFallback.Spawn,
                linkFailure: OrchestrationFallback.LinkFailure?)
        -> OrchestrationFallback.Decision {
        // **只在拉 daemon 明确失败、且对端一个字都没回过的时候才去取锁。**
        // 其余任何情形取锁都是在给自己下绊子（理由见类型注释第一条）。
        let lock: SessionOrchestratorLock.Outcome?
        if case .failed = spawn, linkFailure == nil {
            lock = hooks.acquireLock(dataRoot)
        } else {
            lock = nil
        }
        let decision = OrchestrationFallback.decide(
            lock: lock, spawn: spawn, linkFailure: linkFailure, dataRoot: dataRoot)

        guard case let .takeOverLocally(reason) = decision,
              case let .acquired(handle)? = lock else {
            // 没接管 —— `lock` 在这里出作用域，`Handle.deinit` 当场 `flock(LOCK_UN)`。
            // **绝不能攥着**：攥着会让真正的 daemon 永远起不来。
            hooks.scheduleReconnect()
            return decision
        }
        hooks.takeOver(handle, reason)
        // 接管之后**不再重连**：锁在我们手上（任何 daemon 都起不来），腿也停了，
        // 于是「第二个 host」在结构上不可能出现 —— 不靠谁记得去检查。
        hooks.stopViewerLeg()
        return decision
    }
}
#endif
