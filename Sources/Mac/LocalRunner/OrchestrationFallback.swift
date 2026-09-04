#if os(macOS)
import Foundation

/// **翻默认之后「后台起不来」那一天的硬契约**
/// （设计 §9.2，2026-09-04 由父机长拍板）。
///
/// ## 为什么需要它
///
/// 总闸默认切成 daemon 之后，GUI 进程的身份是 viewer。viewer 连不上时有两种走法，
/// **两种都能翻车，而且翻法相反**：
///
/// - **只连不退**：用户双击图标，界面在、什么都不动、不报错 —— 这条线上**最贵**的
///   翻车形状，也正是这一整期在修的那种静默。
/// - **连不上就自己接管**：如果那边其实**有**一个 daemon 在编排（只是这次没连上），
///   就当场造出双头 —— 两个进程写同一批账、同一批唤醒发两遍，事后极难定位。
///
/// 所以不是二选一，是**按「能不能确定没有别人在编排」分岔**。判据用现成的
/// `SessionOrchestratorLock.Outcome`，不另造一套真值来源。
///
/// ## 表本身（每一行都有一条会红的测试盯着）
///
/// | 观测到的 | 允许回退本地编排？ |
/// |---|---|
/// | `acquired` 且拉 daemon **确实失败** | ✅ 允许（**唯一**允许的一支） |
/// | `heldBy(holder)` 且 `holder.kind == "daemon"` | ❌ 那边真的在，继续重连 |
/// | `heldBy(nil)` —— 锁被占着但读不出是谁 | ❌ **归属不明 = 不许接管** |
/// | `heldBy(holder)` 且 kind 不是 daemon | ❌ 冲突，摆到用户面前 |
/// | `unavailable(reason)` —— 锁文件打不开 | ❌ 拿不到锁就没资格当唯一所有者 |
/// | 连上了但**协议不兼容** | ❌ 可操作错误（能力集取交集是另一回事） |
/// | 连上了但 **attach / 握手失败** | ❌ 可操作错误 + 重试 |
///
/// **这条契约不许被「让用户少看见一个错误」的理由削弱** —— 静默本地接管带来的双头，
/// 症状是账被两个进程交替覆盖、唤醒发两遍，比一个说得清的错误横幅贵得多。
///
/// ## 纯判定，时间与 I/O 都在外面
///
/// 输入是三样已经观测到的事实，输出是一个动作。于是上表每一行都能当场断言，
/// 而且**把任何一行改成「允许」，对应那条测试立刻红**（`OrchestrationFallbackTests`
/// 里有一条专门的负向对照说明这件事）。
enum OrchestrationFallback {

    /// viewer 连不上后台的那一刻，允许做什么。
    enum Decision: Equatable {
        /// 继续退避重连。**界面上仍要看得出「还没连上」**，只是不构成接管的理由。
        case keepConnecting(String)
        /// 回退本地编排 —— 表里**唯一**允许的一支。
        case takeOverLocally(String)
        /// 既不接管也不假装正常：可操作的错误 + 重试 / 诊断入口。
        case refuse(String)

        var reason: String {
            switch self {
            case let .keepConnecting(r), let .takeOverLocally(r), let .refuse(r): return r
            }
        }
    }

    /// 拉起 daemon 这一步的结果。
    enum Spawn: Equatable {
        /// 还没试（比如锁上写着已经有 daemon 在跑）。
        case notAttempted
        /// 进程起来了。**起来了 ≠ 连得上** —— 那是下一步的事，不构成接管的理由。
        case launched
        /// **明确失败**：连进程都没起来。这是允许接管那一支的必要条件之一。
        case failed(String)
    }

    /// 连上之后才可能出现的失败。它们**一律不构成接管的理由** ——
    /// 能回话的对端说明那边有东西在，接管就是双头。
    enum LinkFailure: Equatable {
        case protocolIncompatible(String)
        case handshakeFailed(String)
        case attachFailed(String)

        var detail: String {
            switch self {
            case let .protocolIncompatible(d):
                return "连上了后台，但协议对不上：\(d)\n"
                    + "多半是新旧版本混跑。请更新到同一版，或停掉旧的后台再重试。"
            case let .handshakeFailed(d):
                return "连上了后台的 socket，但握手没完成：\(d)\n"
                    + "后台可能卡住了。可以先 `PendingCrew --daemon-status` 问一下实况，再重试。"
            case let .attachFailed(d):
                return "连上了后台，但取不到 session 的画面：\(d)\n请重试；仍不行就把后台停掉重起。"
            }
        }
    }

    /// - Parameters:
    ///   - lock: 本进程**这一刻**试着取编排锁的结果。`nil` = 还没到取锁那一步
    ///     （比如 daemon 刚起来、还在等它监听）。**只在拉 daemon 明确失败之后才去
    ///     取锁** —— 抢在前面取会让我们自己拉起的那个 daemon 因为锁被占而退出。
    ///   - spawn: 拉起 daemon 的结果。
    ///   - linkFailure: 链路层已经发生的失败（nil = 没有）。
    static func decide(lock: SessionOrchestratorLock.Outcome?,
                       spawn: Spawn,
                       linkFailure: LinkFailure?,
                       dataRoot: URL) -> Decision {
        // 1) **链路层的失败压过一切。** 对端刚才回过话 = 那边有东西在，
        //    不管这一刻锁在谁手上都不许接管。顺序不能挪到锁后面：锁**可能**恰好
        //    到手（daemon 崩在握手之后、锁刚被内核释放），那一瞬接管就是双头。
        if let linkFailure { return .refuse(linkFailure.detail) }

        // 2) 还没到取锁那一步 —— 比如 daemon 刚被拉起来、还在等它开始监听。
        //    「进程起来了」不是「连得上」，但也**不是**接管的理由，继续退避重连。
        guard let lock else { return .keepConnecting("正在连接后台进程…") }

        switch lock {
        case .acquired:
            // 锁到手 = 确定没有别人在编排。**但还不够** —— 必须同时确认
            // 拉 daemon 确实失败，否则我们会在它正要起来的那一瞬把位置占掉。
            guard case let .failed(reason) = spawn else {
                return .keepConnecting("正在连接后台进程…")
            }
            return .takeOverLocally(
                "后台起不来（\(reason)），已临时由本窗口接管编排。\n"
                + "本窗口现在持有 \(dataRoot.path) 的编排锁，所以不会有第二个进程同时管账；"
                + "**关掉这个窗口，正在跑的 session 也会跟着停**。"
                + "要回到后台模式：重开 PendingCrew。")

        case let .heldBy(holder?) where holder.kind == "daemon":
            // 那边真的在编排（它自称 daemon、而且真的在 socket 上听）。
            // 这次没连上是链路的事，继续退避重连 —— 接管就是当场双头。
            return .keepConnecting(
                "后台进程正在运行（pid \(holder.pid)），本窗口继续重连。")

        case .heldBy(nil):
            // **整张表的重点。** 「读不出是谁」最像「那大概没人在管，我来吧」，
            // 而它恰恰最危险：读不出不等于没有。归属不明 = 一律不接管。
            return .refuse(
                "另一个进程正占着 \(dataRoot.path) 的编排锁，但锁文件里**读不出它是谁** —— "
                + "归属不明，本窗口不接管编排（接管就可能变成两个进程同时管账）。\n"
                + "用 `lsof \(dataRoot.appendingPathComponent(SessionOrchestratorLock.fileName).path)` "
                + "查出占着的 pid；确认它该退出就停掉它，然后重试。")

        case .heldBy:
            // 锁被一个**不听 socket** 的东西占着（另一个 inproc 窗口、崩到一半的进程）。
            return .refuse(
                SessionOrchestratorLock.describe(lock, dataRoot: dataRoot)
                + "\n本窗口既不接管编排也不连过去 —— 那边不听 socket，连过去只会得到一个"
                + "永远连不上的窗口。停掉它之后重试。")

        case let .unavailable(reason):
            // 拿不到锁就没资格当唯一所有者（锁自己的注释里就是这么写的）。
            return .refuse(
                "取不到 \(dataRoot.path) 的编排锁：\(reason)\n"
                + "拿不到锁就没资格当唯一所有者，所以本窗口不接管编排。"
                + "多半是数据目录的权限/磁盘问题，修好之后重试。")
        }
    }
}
#endif
