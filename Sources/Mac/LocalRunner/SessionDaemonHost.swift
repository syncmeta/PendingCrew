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

    static func standard(
        support: URL = FileManager.default.urls(
            for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? FileManager.default.temporaryDirectory,
        logs: URL = FileManager.default.urls(
            for: .libraryDirectory, in: .userDomainMask).first?
            .appendingPathComponent("Logs", isDirectory: true)
            ?? FileManager.default.temporaryDirectory
    ) -> PendingCrewDaemonPaths {
        let dir = support.appendingPathComponent("PendingCrew", isDirectory: true)
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
                     lock: dir.appendingPathComponent("daemon.lock"),
                     registry: dir.appendingPathComponent("daemon.registry.json"),
                     log: logDir.appendingPathComponent("daemon.log"),
                     socketFallbackReason: reason)
    }
}

/// 单实例锁（§6.2 闸门 2）。
///
/// **拿锁必须在 unlink socket 文件之前**：反过来的话，第二个 daemon 会在发现自己
/// 抢不到锁**之前**就把第一个 daemon 正在听的那个 socket 文件删掉 —— 老 daemon
/// 还活着、还在 accept，但谁也连不上它了，而且没有任何报错。
/// `SessionDaemonHost.start()` 里的顺序是有意的，别调换。
final class SessionDaemonLock {
    private let fd: Int32

    private init(fd: Int32) { self.fd = fd }

    /// 拿到 = 本进程是唯一的 daemon；nil = 已经有一个在跑（附带它写下的 pid）。
    static func acquire(at url: URL) -> (lock: SessionDaemonLock?, holderPid: Int32?) {
        try? FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        let fd = open(url.path, O_CREAT | O_RDWR, 0o600)
        guard fd >= 0 else { return (nil, nil) }
        guard flock(fd, LOCK_EX | LOCK_NB) == 0 else {
            let holder = (try? String(contentsOf: url, encoding: .utf8))
                .flatMap { Int32($0.trimmingCharacters(in: .whitespacesAndNewlines)) }
            close(fd)
            return (nil, holder)
        }
        ftruncate(fd, 0)
        let pid = "\(ProcessInfo.processInfo.processIdentifier)\n"
        _ = pid.withCString { write(fd, $0, strlen($0)) }
        return (SessionDaemonLock(fd: fd), nil)
    }

    /// flock 随 fd 关闭而释放；进程被 SIGKILL 时由内核释放 —— 所以崩溃不会留下
    /// 一把没人持有的锁（这正是不用「写 pid 文件再判进程在不在」的理由）。
    deinit { flock(fd, LOCK_UN); close(fd) }
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
        case alreadyRunning(pid: Int32?)
        case lockUnavailable
        case listen(Error)

        var description: String {
            switch self {
            case let .alreadyRunning(pid):
                return "已经有一个 PendingCrew daemon 在跑"
                    + (pid.map { "（pid \($0)）" } ?? "") + "；本进程退出。"
            case .lockUnavailable:
                return "拿不到 daemon.lock（目录不可写？）；本进程退出。"
            case let .listen(error):
                return "socket 监听失败：\(error)"
            }
        }
    }

    let paths: PendingCrewDaemonPaths
    let log: SessionDaemonLog
    let server: SessionProtocolServer

    /// 往受影响 crew 的白板发一条（crewId, text）。默认落真白板；
    /// 测试注入它，免得单测往人的白板上写字。
    var onCrewNotice: (String, String) -> Void = { crewId, text in
        LocalWhiteboardStore.shared.appendSessionMessage(
            crewId: crewId, sessionId: "system", text: text,
            category: "progress", senderName: "系统")
    }

    private var lock: SessionDaemonLock?
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
        server = SessionProtocolServer(capabilities: capabilities, daemonBuild: build)
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

    /// 顺序是有意的，见 `SessionDaemonLock` 的注释：**先锁，后 unlink socket**。
    func start() throws {
        let acquired = SessionDaemonLock.acquire(at: paths.lock)
        guard let lock = acquired.lock else {
            if acquired.holderPid != nil || FileManager.default.fileExists(atPath: paths.lock.path) {
                throw StartError.alreadyRunning(pid: acquired.holderPid)
            }
            throw StartError.lockUnavailable
        }
        self.lock = lock

        log.write("=== daemon 启动 pid=\(ProcessInfo.processInfo.processIdentifier) "
            + "build=\(Self.currentBuild) protocol=\(SessionProtocolVersion.current) ===")
        if let reason = paths.socketFallbackReason { log.write("⚠️ socket 路径退路：\(reason)") }

        reapOrphansFromPreviousRun()

        do {
            let listener = try UnixSocketListener(path: paths.socket)
            listener.onAccept = { [weak self] link in
                MainActor.assumeIsolated { self?.server.accept(link: link) }
            }
            listener.start()
            self.listener = listener
            log.write("监听 \(paths.socket)")
        } catch {
            throw StartError.listen(error)
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
    /// 判据是 **`daemon.lock` 上的 flock 拿不拿得到**，不是「pid 文件里那个进程在不在」
    /// —— flock 随进程消失由内核释放，所以崩溃不会留下一把假锁；而 pid 文件会。
    static func runningDaemonPid(paths: PendingCrewDaemonPaths = .standard()) -> Int32? {
        let probe = SessionDaemonLock.acquire(at: paths.lock)
        guard probe.lock == nil else { return nil }   // 抢到了 = 没人在跑
        return probe.holderPid
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
#endif
