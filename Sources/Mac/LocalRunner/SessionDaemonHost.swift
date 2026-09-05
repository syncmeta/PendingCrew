#if os(macOS)
import Foundation

/// 常驻后台进程用到的几条路径。
///
/// socket 放在 Application Support 里（与共享账本同一处，「清除本机所有数据」
/// 一并覆盖到）。**但 `sockaddr_un.sun_path` 只有 104 字节**，用户名够长时那条路径
/// 会顶穿 —— 那时退到 `/tmp` 下的短路径，并且**大声说出来**，不静默换地方。
struct PendingCrewDaemonPaths {
    var socket: String
    var lock: URL
    var registry: URL
    var log: URL
    /// 非空 = socket 用了退路，理由在这里。调用方应当把它写进日志。
    var socketFallbackReason: String?

    /// **锁、socket、registry 一律落在数据根下**（不是「Application Support 下」）。
    /// 这条不是审美：`PENDINGCREW_DATA_DIR` 挪走数据根之后，如果锁还钉在真目录上，
    /// 临时根里的 daemon 会和真 app 抢同一把锁 —— 那就白挪了，而且症状是「隔离
    /// 明明设了却还是打架」，比不隔离更难查。
    ///
    /// 日志刻意留在 `~/Library/Logs/PendingCrew/`（跟着数据根走的话，临时根那次
    /// 跑完连日志一起被删，而日志正是那种跑法唯一的观察窗）。
    /// **但覆盖数据根时换文件名**，见 `logFileName` —— 目录留在 Logs 下，
    /// 写的却不再是真人那份 `daemon.log`。
    static func standard(
        dataRoot: URL = PendingCrewDataRoot.url,
        logs: URL = FileManager.default.urls(
            for: .libraryDirectory, in: .userDomainMask).first?
            .appendingPathComponent("Logs", isDirectory: true)
            ?? FileManager.default.temporaryDirectory,
        dataRootIsOverridden: Bool = PendingCrewDataRoot.isOverridden
    ) -> PendingCrewDaemonPaths {
        let dir = dataRoot
        let logDir = logs.appendingPathComponent("PendingCrew", isDirectory: true)
        let preferred = dir.appendingPathComponent("daemon.sock").path
        var socket = preferred
        var reason: String?
        if preferred.utf8.count > UnixSocketTransport.maximumPathLength {
            socket = "/tmp/pendingcrew-\(getuid()).sock"
            reason = "首选路径 \(preferred.utf8.count) 字节，超过 sun_path 的 "
                + "\(UnixSocketTransport.maximumPathLength) —— 已退到 \(socket)"
        }
        return .init(socket: socket,
                     lock: dir.appendingPathComponent(SessionOrchestratorLock.fileName),
                     registry: dir.appendingPathComponent("daemon.registry.json"),
                     log: logDir.appendingPathComponent(
                         logFileName(dataRoot: dataRoot,
                                     dataRootIsOverridden: dataRootIsOverridden)),
                     socketFallbackReason: reason)
    }

    /// 日志文件名。**默认根照旧 `daemon.log`；覆盖根换一个由根路径决定的名字。**
    ///
    /// 两条相反的约束都要满足（各自都出过事）：
    /// - 日志**不跟着数据根走** —— 临时根跑完就删，连唯一的观察窗一起没了。
    /// - 日志**不写真人那份** —— 2026-09-04 的隔离冒烟把启动/连接/退出几十行混进了
    ///   用户的真 `daemon.log`。隔离做一半比不做更难查：人以为看的是自家后台，
    ///   其实混着一次实验。
    ///
    /// 名字由根路径原样折出来（不哈希）：两个不同的根一定得到两个不同的文件，
    /// 而且人一眼看得出这份日志是哪次跑的。
    static func logFileName(dataRoot: URL, dataRootIsOverridden: Bool) -> String {
        guard dataRootIsOverridden else { return "daemon.log" }
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-_."))
        let folded = String(dataRoot.standardizedFileURL.path.unicodeScalars.map {
            allowed.contains($0) ? Character($0) : "-"
        })
        // 留尾不留头：区分两个根的信息在末尾（`/tmp/rootA` vs `/tmp/rootB`）。
        let tail = String(folded.suffix(120))
        return "daemon-\(tail).log"
    }
}

/// daemon 的滚动日志（§8.5）。
///
/// app 退化成 viewer 之后，后台出问题**没有画面可看** —— 这个文件就是那时唯一的
/// 线索来源。它刻意简单：追加、按大小滚、留最近几份，没有等级、没有异步队列。
final class SessionDaemonLog {
    private let url: URL
    private let maximumBytes: Int
    private let keep: Int
    private let queue = DispatchQueue(label: "pendingcrew.daemon.log")

    init(url: URL, maximumBytes: Int = 2 * 1024 * 1024, keep: Int = 3) {
        self.url = url
        self.maximumBytes = maximumBytes
        self.keep = keep
        try? FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    }

    func write(_ line: String) {
        let stamped = Self.formatter.string(from: Date()) + " " + line + "\n"
        queue.async { [url, maximumBytes, keep] in
            let fm = FileManager.default
            if let size = (try? fm.attributesOfItem(atPath: url.path)[.size]) as? Int,
               size > maximumBytes {
                for index in stride(from: keep - 1, through: 1, by: -1) {
                    try? fm.removeItem(at: url.appendingPathExtension("\(index + 1)"))
                    try? fm.moveItem(at: url.appendingPathExtension("\(index)"),
                                     to: url.appendingPathExtension("\(index + 1)"))
                }
                try? fm.removeItem(at: url.appendingPathExtension("1"))
                try? fm.moveItem(at: url, to: url.appendingPathExtension("1"))
            }
            guard let data = stamped.data(using: .utf8) else { return }
            if let handle = try? FileHandle(forWritingTo: url) {
                defer { try? handle.close() }
                try? handle.seekToEnd()
                try? handle.write(contentsOf: data)
            } else {
                try? data.write(to: url)
            }
        }
    }

    private static let formatter: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()
}

/// **`--daemon` 进程的本体**（spec §9 P4 那一行）。
///
/// 它自己不做编排 —— 编排还是 `SessionHost` + `CrewSessionRunner`，**一份代码，
/// 两种模式**（§10）。这个类只负责「让那份编排跑在一个没有窗口的进程里」需要的四件事：
/// 单实例锁、socket 监听、崩溃善后（registry + 孤儿双重核对）、日志。
@MainActor
final class SessionDaemonHost {
    enum StartError: Error, CustomStringConvertible {
        /// 这个数据根已经有一个编排者了（**可能是另一个 daemon，也可能是 app 窗口** ——
        /// 2026-08-26 那次事故就是后者，见 `SessionOrchestratorLock`）。
        ///
        /// `holderIsDaemon` 把这两种分开，**因为它们的「期望状态成立了没有」不一样**：
        /// 占着的是另一个 daemon → 「有一个 daemon 在跑」这件事已经成立，本进程安静
        /// 退出是正确结局；占着的是 app 窗口 / 读不出是谁 → **一个 daemon 都没有**，
        /// 拉起方要的东西没拿到。退出码据此分岔（见 `DaemonExitCode`）。
        case alreadyOrchestrated(String, holderIsDaemon: Bool)
        /// 锁文件**根本打不开**（数据根不可写、目录建不出来之类）。
        ///
        /// **和上面那条不是一回事**：上面是「已经有人在编排」，这条是「我们连问都问
        /// 不出来」—— 期望状态没达成，而且多半要人去修目录权限。2026-09-04 真机上
        /// `chmod 500` 数据根复现到的就是这一条。
        case lockUnavailable(String)
        case listen(Error)

        var description: String {
            switch self {
            case let .alreadyOrchestrated(detail, _): return detail + "\n本进程退出。"
            case let .lockUnavailable(detail): return detail + "\n本进程退出。"
            case let .listen(error): return "socket 监听失败：\(error)"
            }
        }
    }

    let paths: PendingCrewDaemonPaths
    let log: SessionDaemonLog
    let server: SessionProtocolServer
    /// 见 `startIdleReclaimTimer()`。
    private var idleReclaimTimer: Timer?

    /// 往受影响 crew 的白板发一条（crewId, text）。默认落真白板；
    /// 测试注入它，免得单测往人的白板上写字。
    var onCrewNotice: (String, String) -> Void = { crewId, text in
        LocalWhiteboardStore.shared.appendSessionMessage(
            crewId: crewId, sessionId: "system", text: text,
            category: "progress", senderName: "系统")
    }

    private var lock: SessionOrchestratorLock.Handle?
    private var listener: UnixSocketListener?
    private var registry = SessionProcessRegistry()
    private let startedAt = Date()

    init(paths: PendingCrewDaemonPaths? = nil,
         capabilities: [String] = SessionDaemonHost.defaultCapabilities,
         build: String? = nil) {
        let paths = paths ?? .standard()
        let build = build ?? SessionDaemonHost.currentBuild
        self.paths = paths
        log = SessionDaemonLog(url: paths.log)
        server = SessionProtocolServer(
            capabilities: capabilities, daemonBuild: build,
            startedAt: startedAt.timeIntervalSince1970)
        server.onDiagnostic = { [log] line in log.write(line) }
    }

    static let defaultCapabilities = [
        "approval-mode", "launch-parameter-problem", "profile-switch", "screen-text",
        "terminal-bytes", "transcript-events",
    ]

    nonisolated static var currentBuild: String {
        let info = Bundle.main.infoDictionary
        let short = info?["CFBundleShortVersionString"] as? String ?? "?"
        let build = info?["CFBundleVersion"] as? String ?? "?"
        return "\(short)(\(build))"
    }

    /// **顺序是有意的：先锁，后 unlink socket。** 反过来的话，第二个 daemon 会在发现
    /// 自己抢不到锁**之前**就把第一个 daemon 正在听的那个 socket 文件删掉 —— 老 daemon
    /// 还活着、还在 accept，但谁也连不上它了，而且没有任何报错。别调换。
    func start() throws {
        let dataRoot = paths.lock.deletingLastPathComponent()
        let outcome = SessionOrchestratorLock.acquire(dataRoot: dataRoot, kind: "daemon")
        guard case let .acquired(handle) = outcome else {
            let detail = SessionOrchestratorLock.describe(outcome, dataRoot: dataRoot)
            if case let .heldBy(holder) = outcome {
                throw StartError.alreadyOrchestrated(
                    detail, holderIsDaemon: holder?.kind == "daemon")
            }
            throw StartError.lockUnavailable(detail)
        }
        self.lock = handle

        log.write("=== daemon 启动 pid=\(ProcessInfo.processInfo.processIdentifier) "
            + "build=\(Self.currentBuild) protocol=\(SessionProtocolVersion.current) ===")
        // 六条约束里的第 6 条：**启动时把数据根打出来**。这套隔离机制自己的失败形态
        // 是「悄悄跑在临时目录上」——人以为在动真数据、其实在动空壳，而所有操作都会成功。
        log.write(PendingCrewDataRoot.startupLine())
        if let reason = paths.socketFallbackReason { log.write("⚠️ socket 路径退路：\(reason)") }

        reapOrphansFromPreviousRun()

        do {
            let listener = try UnixSocketListener(path: paths.socket)
            listener.onAccept = { [weak self] link in
                MainActor.assumeIsolated { self?.server.accept(link: link) }
            }
            listener.start()
            self.listener = listener
            startIdleReclaimTimer()
            log.write("监听 \(paths.socket)")
        } catch {
            throw StartError.listen(error)
        }
    }

    /// 半开链路回收的节拍。**这一拍是 `daemonIdleTimeout` 唯一的消费者** —— 在它之前
    /// 那个常量声明了却没人读，于是「后台会回收半开连接」这件事从来没发生过。
    ///
    /// 周期取 `pingInterval`（10s）：viewer 本来就每 10s 发一次 ping，用同一个节拍扫，
    /// 最坏也就多留一个 ping 周期。**别取成 `daemonIdleTimeout`** —— 那样一条刚好在
    /// 扫描后一秒变哑的连接要等将近两倍时长才被回收。
    private func startIdleReclaimTimer() {
        idleReclaimTimer?.invalidate()
        idleReclaimTimer = Timer.scheduledTimer(
            withTimeInterval: SessionReconnectPolicy.pingInterval, repeats: true
        ) { [weak self] _ in
            MainActor.assumeIsolated { _ = self?.server.reclaimIdleConnections() }
        }
    }

    /// registry 里的一条：这个 session 的子进程是谁。
    struct RosterProcess: Equatable {
        var sessionId: String
        var crewId: String
        var pid: Int32
    }

    /// 让 registry 跟着编排侧的 roster 走。
    ///
    /// 为什么不在「起 session 的那一刻」写一条：拉起是异步的，那一刻 pid 常常还是 0；
    /// 而且漏写一条的代价是**那个子进程将来永远不会被回收**。跟着整份 roster 重建
    /// 既补得上后到的 pid，也不会因为某条路径忘了调用而漏。
    ///
    /// 收的是「已经摘干净的三元组」而不是 `CrewSessionRunner` —— 这个文件所在的
    /// `Sources/Mac/LocalRunner` 是编进单测 bundle 的那一块，不许依赖 app 的编排层。
    func updateRegistry(from roster: [RosterProcess]) {
        var next = SessionProcessRegistry(
            daemonPid: Int32(ProcessInfo.processInfo.processIdentifier),
            daemonStartedAt: startedAt)
        for item in roster {
            guard item.pid > 0,
                  let identity = SessionOrphanReaper.identity(forRunning: item.pid) else { continue }
            next.record(sessionId: item.sessionId, crewId: item.crewId, identity: identity)
        }
        guard next != registry else { return }
        registry = next
        guard let data = try? JSONEncoder().encode(next) else { return }
        try? data.write(to: paths.registry, options: .atomic)
    }

    /// 上一轮 daemon 没了之后留下的子进程（§8.2）。
    ///
    /// **每一条都双重核对，`pidReused` 一律不动手**，并且把「我没有动它」写进日志和
    /// 受影响 crew 的白板 —— 静默留一个孤儿和静默杀一个无辜进程一样查不出来。
    private func reapOrphansFromPreviousRun() {
        guard let data = try? Data(contentsOf: paths.registry),
              let previous = try? JSONDecoder().decode(SessionProcessRegistry.self, from: data),
              !previous.entries.isEmpty else { return }

        log.write("上一轮 daemon（pid \(previous.daemonPid)）留下 \(previous.entries.count) 条记录，开始核对")
        var interruptedByCrew: [String: [String]] = [:]
        var unresolved: [String] = []

        for entry in previous.entries {
            let decision = SessionOrphanReaper.decide(
                recorded: entry.identity,
                current: SessionOrphanReaper.probe(pid: entry.identity.pid))
            let text = SessionOrphanReaper.describe(
                sessionId: entry.sessionId, recorded: entry.identity, decision: decision)
            log.write(text)
            SessionOrphanReaper.apply(decision, recorded: entry.identity)
            switch decision {
            case .reap, .alreadyGone:
                interruptedByCrew[entry.crewId, default: []].append(entry.sessionId)
            case .pidReused:
                interruptedByCrew[entry.crewId, default: []].append(entry.sessionId)
                unresolved.append(text)
            }
        }

        // fail loud，不静默：受影响的每个 crew 白板上都要看得见。
        for (crewId, sessions) in interruptedByCrew {
            var text = "后台进程重启，\(sessions.count) 个 session 被中断："
                + sessions.map { "`\($0)`" }.joined(separator: "、") + "。"
            let mine = unresolved.filter { line in sessions.contains { line.contains($0) } }
            if !mine.isEmpty {
                text += "\n\n另有对不上账的 pid，**我没有动它们**：\n"
                    + mine.map { "- " + $0 }.joined(separator: "\n")
            }
            onCrewNotice(crewId, text)
        }
        try? FileManager.default.removeItem(at: paths.registry)
    }

    /// 停 daemon：先摘监听、再放锁。session 由调用方决定停不停 ——
    /// 这个方法本身**不碰任何 session**。
    func stop() {
        listener?.close()
        listener = nil
        log.write("=== daemon 退出 ===")
        lock = nil
    }
}

/// app 侧要跟一个**正在跑的** daemon 打交道时用的两件小事。
enum SessionDaemonControl {
    /// 有 daemon 在跑吗？在的话它的 pid 是多少。
    ///
    /// 判据是 **编排锁上的 flock 拿不拿得到 + 持有者自称是不是 daemon**，
    /// 不是「pid 文件里那个进程在不在」—— flock 随进程消失由内核释放，
    /// 所以崩溃不会留下一把假锁；而 pid 文件会。
    static func runningDaemonPid(paths: PendingCrewDaemonPaths = .standard()) -> Int32? {
        let dataRoot = paths.lock.deletingLastPathComponent()
        // **只认 daemon 那一种持有者。** 同一把锁现在也可能被 inproc 的 app 窗口
        // 拿着（那正是 2026-08-26 补上的那道闸），而「有没有 daemon 在跑」问的是
        // 另一件事 —— 分不清的话，`LocalDataReset` 会去 SIGTERM 一个 GUI 进程。
        guard let holder = SessionOrchestratorLock.currentHolder(dataRoot: dataRoot),
              holder.kind == "daemon" else { return nil }
        return holder.pid
    }

    /// 停掉正在跑的 daemon，等它真的放锁。
    ///
    /// **「清除本机所有数据」必须先走这一步再删目录**（§6.1）：不然 daemon 会把刚被
    /// 删掉的快照立刻重新写回来，用户看到的是「清了个寂寞」。
    @discardableResult
    static func stopRunningDaemon(
        paths: PendingCrewDaemonPaths = .standard(),
        timeout: TimeInterval = 8
    ) -> Bool {
        guard let pid = runningDaemonPid(paths: paths) else { return true }
        guard kill(pid, SIGTERM) == 0 else { return false }
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if runningDaemonPid(paths: paths) == nil { return true }
            usleep(100_000)
        }
        return false
    }
}

/// **CLI 的退出码：0 = 期望状态成立，非 0 = 没成立。**
///
/// ## ⚠️ 这些码是给**人和脚本**看的，**判据一律不许读它们**
///
/// 别顺手把 `ViewerSessionClient` 那条腿的判定改成解析退出码 —— 那正是这一期
/// 绕开的坑，而「退出码现在变准了」看起来像个足够好的理由。**它不是。**
///
/// 因果写全：daemon 对「锁被另一个 daemon 占着」和「锁文件打不开」**本来就可能
/// 同码**（2026-09-04 之前两者都是 exit 0，真机实测过），而且今后也不保证一定分得
/// 开——退出码是**约定**，不是**观测**。所以 app 侧「后台起没起成」必须由
/// **锁的观测**去分辨（见 `OrchestrationFallback.decide` 那张表），不是由退出码。
/// 这两件事的可靠性根本不在一个量级：锁是内核维护的、崩溃自动释放；退出码是
/// 一行 `exit(n)`，谁改一下都没人发现。
///
/// 换句话说：**退出码变准了，只是让人少猜一次，不构成任何契约。**
enum DaemonExitCode {
    static let ok: Int32 = 0
    static let failed: Int32 = 1
    /// 参数不对（对齐 `--daemon-attach` 已有的约定）。
    static let badUsage: Int32 = 2

    /// `PendingCrew --daemon` 启动失败 → 退出码。
    static func forDaemonStart(_ error: SessionDaemonHost.StartError) -> Int32 {
        switch error {
        case let .alreadyOrchestrated(_, holderIsDaemon):
            // 占着的是另一个 daemon → 「有一个 daemon 在跑」已经成立，本进程安静退出
            // 是正确结局。占着的是 app 窗口 / 读不出是谁 → **一个 daemon 都没有**，
            // 拉起方要的东西没拿到，报 0 就是骗它。
            return holderIsDaemon ? ok : failed
        case .lockUnavailable, .listen:
            return failed
        }
    }

    /// `PendingCrew --daemon-status` 探测失败 → 退出码。
    /// **说了「不可连接」就不许报成功** —— 否则 `if PendingCrew --daemon-status; then …`
    /// 会一路走进 then。
    static let statusProbeFailed: Int32 = failed
}
#endif