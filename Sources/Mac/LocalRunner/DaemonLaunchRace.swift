#if os(macOS)
import Foundation

/// **「后台到底起成了没有」的赛跑**（2026-09-04 父机长定的判据第 1–4 条）。
///
/// 拉起 daemon 之后，我们并行等两件事，**谁先到算谁**：
///
/// - **首次协议握手** → 起成了。这是唯一一种「起成了」的证据 ——
///   「进程还在」只证明它没死，不证明它在服务。
/// - **子进程终止** → 起没成。**退出码 0 也算失败**：daemon 在拿不到编排锁 /
///   打不开锁文件时都是打一行原因然后 `exit(0)`，真机上实测过。
///
/// 两件都没发生就继续等，但**等待有明确上限**；到上限而进程还活着 = **不确定态**，
/// 走 fail-closed（不回退、也不继续无限「正在连接」）。
///
/// ## 为什么不是「睡一个宽限窗再看它还在不在」
///
/// 那是在赌时序 —— 窗给短了会把「正要退的」判成起成了，给长了每次启动都白等。
/// 而这一整期的教训正好就是别再赌时序：判据必须是**观测到的事件**（握上手了 /
/// 进程没了 / 等够了），不是「大概到点了」。
///
/// 纯判定 + 时间由调用方喂 —— 所以四种赛果都能用**可控假进程**当场测出来
/// （`ChildState` 与握手布尔值就是那个假进程）。
enum DaemonLaunchRace {

    /// 我们拉起的那个子进程现在的样子。
    ///
    /// daemon **不做 double-fork**（`SessionDaemonMain.run` 只 `setsid()` 然后
    /// `RunLoop.main.run()`），所以我们拉起的那个进程**就是 daemon 本身** ——
    /// 「它还在不在」是直接观测得到的事实，不是推断。
    /// （`setsid()` 只脱离会话/终端，不改父子关系；A1 需要的正是前者。）
    enum ChildState: Equatable {
        /// 这一轮我们**没有**拉起任何进程（锁上写着已经有 daemon 在跑，或者上一次
        /// 拉起来的那个还活着）。没有自己的子进程可看，于是赛跑里只剩「握手」与
        /// 「等够了」两件事 —— 而「等够了」在这条路上的结论不一样：该去问锁，
        /// 不是下「说不准」的结论。
        case notSpawned
        case alive
        /// 已经退了。退出码可能拿不到（被别人收走了），所以是 optional ——
        /// 但**有没有拿到不影响判定**：这里从不解析退出码。
        case exited(Int32?)
    }

    /// 我们这一侧那条链路走到哪了。
    ///
    /// ## 为什么这里是一个三态枚举，而不是一个 `Bool`
    ///
    /// 2026-09-04 读代码逮到的偏离：判据第 1 条要的是「等**首次协议握手**」，
    /// 而实现里拿的是「`UnixSocketTransport.connect` 成功」—— **「socket 连上了」
    /// 被当成了「握上手了」**。这两件事差着一整个握手：`SessionProtocolClient`
    /// 的 `isConnected` 只在真收到 `daemonHello`、且协议/能力协商**兼容**时才翻。
    ///
    /// 后果是一个**接受连接但不回话**的 daemon（卡死在 listen 之后、或半开链路）
    /// 会被判成「起成了」：赛跑当场收工、横幅被清掉，之后一直在
    /// 「连上 → 心跳超时 → 重连 → 又连上」之间打转，`applyFallback` 一次都不会被
    /// 调用 —— 于是「超限之后升级成可操作的错误」在这种形状上**永远不触发**。
    /// 界面上确实有字在变、也确实说了「没有回应」，**只是永远不会升级成一件人能
    /// 做的事**。那正是这一整期在消灭的东西的第四种穿法，也是最会骗人的一种。
    ///
    /// 所以这里**不收 `Bool`**：`Bool` 会让「用哪个信号」变成调用方随手一填的事，
    /// 而那正是出错的地方。换成三态之后，`socketOpen` 与 `handshaken` 在类型上就
    /// 分得开，填错会被编译器和测试一起挡下来。
    enum LinkState: Equatable {
        /// 还没开 socket，或者刚断了。
        case none
        /// socket 开着，hello 还没回来。**这不算赢。**
        case socketOpen
        /// 收到 `daemonHello` 且协商兼容 —— 这才叫握上手了。
        case handshaken
    }

    enum Outcome: Equatable {
        /// 两件都没发生，也没到上限 —— 接着等。
        case pending
        /// 握上手了 = 起成了。
        case handshake
        /// 握手之前它自己退了 = 没起成。**exit 0 也在这一支里。**
        case exitedBeforeHandshake(Int32?)
        /// 到上限了，我们拉起的那个进程还活着 = 说不准（fail-closed）。
        case timedOutStillAlive
        /// 到上限了，而这一轮我们**根本没拉过** —— 该去问锁（是谁占着、能不能取到），
        /// 由 §9.2 那张表决定继续重连还是给可操作错误。
        case timedOutWithoutSpawn
    }

    /// 一拍观测 → 结论。
    ///
    /// **顺序要紧**：先问握手。握手赢的话，哪怕同一拍里进程也退了也算起成了 ——
    /// 它已经服务过我们了，之后再退是另一回事（由心跳/断线那条路管）。
    static func step(link: LinkState,
                     child: ChildState,
                     elapsed: TimeInterval,
                     limit: TimeInterval) -> Outcome {
        // **只有 `handshaken` 算赢。** `socketOpen` 是本端单方面就能达成的事，
        // 拿它当「对端回过话」的证据，就是这一整期在消灭的那种静默的第四种穿法。
        if link == .handshaken { return .handshake }
        if case let .exited(code) = child { return .exitedBeforeHandshake(code) }
        if elapsed >= limit {
            return child == .notSpawned ? .timedOutWithoutSpawn : .timedOutStillAlive
        }
        return .pending
    }
}
#endif
