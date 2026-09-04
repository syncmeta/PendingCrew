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
        case alive
        /// 已经退了。退出码可能拿不到（被别人收走了），所以是 optional ——
        /// 但**有没有拿到不影响判定**：这里从不解析退出码。
        case exited(Int32?)
    }

    enum Outcome: Equatable {
        /// 两件都没发生，也没到上限 —— 接着等。
        case pending
        /// 握上手了 = 起成了。
        case handshake
        /// 握手之前它自己退了 = 没起成。**exit 0 也在这一支里。**
        case exitedBeforeHandshake(Int32?)
        /// 到上限了，进程还活着 = 说不准。
        case timedOutStillAlive
    }

    /// 一拍观测 → 结论。
    ///
    /// **顺序要紧**：先问握手。握手赢的话，哪怕同一拍里进程也退了也算起成了 ——
    /// 它已经服务过我们了，之后再退是另一回事（由心跳/断线那条路管）。
    static func step(handshakeSucceeded: Bool,
                     child: ChildState,
                     elapsed: TimeInterval,
                     limit: TimeInterval) -> Outcome {
        if handshakeSucceeded { return .handshake }
        if case let .exited(code) = child { return .exitedBeforeHandshake(code) }
        if elapsed >= limit { return .timedOutStillAlive }
        return .pending
    }
}
#endif
