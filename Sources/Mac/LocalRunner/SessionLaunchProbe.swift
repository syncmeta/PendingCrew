#if os(macOS)
import Foundation

/// 拉起自检（#541）：把「后端对象建出来了」与「子进程真的跑起来了」分开。
///
/// 事故背景：worker-dd1bf4a3 经 `start_session` 排队拉起后子进程从未跑起来，
/// 但 `list_sessions` / 成员列表都显示「空闲」—— 因为后端一构造 `status` 就写死
/// `.running`，从不回头核实，而「空闲」正是 `running && !isWorking` 的推导结果。
/// 两条来路都会掉进这个洞：
/// - claude（PTY）：SwiftTerm 的 forkpty 分支**失败时静默 return**（不置
///   `running`、不回调 delegate）；且它的退出事件源 `activate()` 在
///   `setEventHandler` **之前**，秒退的子进程（如额度耗尽的 CLI 启动即退）
///   退出事件可能压根没人接 —— 两种情况都停在 `.running` 装空闲。
/// - codex（app-server）：握手 `catch` 把错误丢掉只留 `.exited(1)`（原因不可读）；
///   握手若卡住不返回，则永远停在 `.running` 装空闲。
///
/// 本判定是纯函数（可单测，不用真起进程）：后端定时喂当前观测量，拿到裁决后
/// 翻 health / status 并 fail-loud。
enum SessionLaunchVerdict: Equatable {
    /// 还在观察窗内且没有确凿坏消息 —— 继续观察，别急着报警。
    case pending
    /// fork/exec 就没起来（进程根本不存在）。
    case spawnFailed
    /// 起过、现在没了，但退出回调从没到过 —— 后端还以为自己在跑。
    case diedSilently
    /// 进程活着，但过了截止时刻一个字节都没吐 / 握手没完成 = 半死。
    case stalled
    /// 见过输出（或握手已完成）—— 正常，自检可以收工。
    case alive
}

/// 拉起自检的判定 + 文案（纯逻辑，`AgentTerminalSession` / `CodexAppServerBackend` 共用）。
enum SessionLaunchProbe {
    /// 「起来了但一直没输出」的截止时刻（秒）。claude TUI 正常在一两秒内就开始
    /// 重绘；codex 握手也是秒级。留 25s 是给冷启动 / 大仓库索引足够余量，
    /// 又不至于让机长干等太久才知道派出去的活没人干。
    static let firstOutputDeadline: TimeInterval = 25

    /// 自检轮询间隔（秒）。
    static let pollInterval: TimeInterval = 1

    /// - Parameters:
    ///   - spawned: 后端起进程那一刻自报的成功与否（claude=`process.running && pid>0`；
    ///     codex=`Process.run()` 没抛错）。
    ///   - processAlive: 现在进程还在不在（`kill(pid, 0) == 0`）。
    ///   - everAlive: **这一轮之前**有没有任何一次观测到它活着。调用方按轮次
    ///     累积（见下方「已死」与「还没生」的区分）。
    ///   - sawOutput: 见过第一手活迹没有（claude=收到过 PTY 字节；codex=握手拿到 threadId）。
    ///   - elapsed: 距拉起过了多久。
    static func verdict(
        spawned: Bool,
        processAlive: Bool,
        everAlive: Bool,
        sawOutput: Bool,
        elapsed: TimeInterval,
        deadline: TimeInterval = firstOutputDeadline
    ) -> SessionLaunchVerdict {
        // 没 spawn 起来是最硬的坏消息，优先于一切（此时 processAlive 必假，
        // 但不能让它被误判成 diedSilently —— 两者要给的错误文案不同）。
        if !spawned { return .spawnFailed }
        // 见过活迹 = 确定活着，自检收工。即便此刻进程恰好退出了，那也归正常的
        // 退出回调管（它是「跑完/跑挂了」，不是「没拉起来」）。
        if sawOutput { return .alive }
        // 活着、没输出：过了截止时刻才算半死，否则继续等。
        if processAlive { return elapsed >= deadline ? .stalled : .pending }

        // 进程此刻不在 —— **「已经死了」和「还没生出来」必须分开**，否则就是误报。
        // ① 曾观测到它活着 → 真的是起来后没了，而退出回调没把状态翻掉（调用方只
        //    在 status 仍 running 时问我们）→ 退出事件丢了，得自己认账。
        if everAlive { return .diedSilently }
        // ② 从没观测到活过 → 极可能只是**还没 fork 完**。codex 后端就踩过这个：
        //    看门狗 Task 排在 `connection.start()` 之前跑，第一轮 `isProcessRunning`
        //    必然是 false，于是每一个 codex session 刚拉起就被判「启动后立刻退出」，
        //    而进程随后好好地跑了一整场。误报比不报更坏 —— 人会去重起一个正在干活
        //    的 session。所以这里继续等，直到截止时刻仍没见它活过，才认「压根没起来」。
        return elapsed >= deadline ? .spawnFailed : .pending
    }

    /// 裁决是否终局（终局 = 自检可以停，且要 fail-loud）。
    static func isTerminal(_ v: SessionLaunchVerdict) -> Bool {
        switch v {
        case .pending, .alive: return false
        case .spawnFailed, .diedSilently, .stalled: return true
        }
    }

    /// 失败裁决 → 给人看的原因（白板警示 / 成员列表副行 / `lastStartError` 横幅同一份）。
    /// `alive`/`pending` 不该走到这里，返回 nil。
    static func failureDetail(
        _ verdict: SessionLaunchVerdict,
        kind: LocalCodingAgentKind,
        deadline: TimeInterval = firstOutputDeadline,
        underlying: String? = nil
    ) -> String? {
        let tool: String
        switch kind {
        case .claudeCode: tool = "Claude Code"
        case .codex: tool = "Codex"
        case .terminal: tool = "终端"
        }
        let tail = underlying.map { "（\($0)）" } ?? ""
        switch verdict {
        case .spawnFailed:
            return "\(tool) 子进程没能启动\(tail) —— 这个 session 从头到尾没跑起来，"
                + "活等于没派出去，请改派或排查后重起。"
        case .diedSilently:
            return "\(tool) 子进程启动后立刻退出\(tail) —— 常见于额度耗尽 / 未登录 / "
                + "工作目录不可用，请核实后重起，别把活挂在它身上。"
        case .stalled:
            return "\(tool) 起来了但 \(Int(deadline)) 秒内一个字都没输出\(tail) —— "
                + "半死状态（进程在、干不了活），请改派或重起这个 session。"
        case .alive, .pending:
            return nil
        }
    }
}
#endif
