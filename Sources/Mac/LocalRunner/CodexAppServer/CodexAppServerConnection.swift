#if os(macOS)
import Foundation

/// `FileHandle.readabilityHandler` stays readable at EOF on macOS. If the handler merely
/// returns after `availableData` yields an empty buffer, Foundation immediately invokes it
/// again and one dead pipe consumes a CPU core forever. The daemon owns many long-lived
/// connections, so those cores accumulate until fresh PTY reads are starved and Claude blocks
/// during terminal setup. EOF therefore has to disarm the handler in the same callback.
enum CodexPipeReadability {
    static func drain(_ handle: FileHandle, consume: (Data) -> Void) {
        let data = handle.availableData
        guard !data.isEmpty else {
            handle.readabilityHandler = nil
            return
        }
        consume(data)
    }
}

/// 往子进程 stdin 写一行。
///
/// **必须用会抛的 `write(contentsOf:)`，不能用 `write(_:)`。** 后者是 ObjC 的
/// `-[NSFileHandle writeData:]`：对端没了的时候它抛的是 `NSFileHandleOperationException`
/// —— 一个 Swift 的 `try` / `catch` **永远接不住**的 ObjC 异常，于是整个 daemon
/// 被未捕获异常打死，那一刻所有 crew 的 session 一起断。
/// 2026-09-05 07:50:03Z 真发生过一次，5 个 crew 同时掉 session
/// （`crashes/2026-09-05T07-50-03Z.log`，栈是 boot → startFreshThread → request → 这里）。
///
/// 所以调用方那句 `try` 不是装饰：`write(contentsOf:)` 把 EPIPE 变成一个普通的 Swift
/// 错误，`CodexAppServerBackend.boot` 那个 `catch` 就能把它翻成 `reportLaunchFailure`
/// —— 这个 session 标异常、白板上说一句，而不是拖着整个 daemon 陪葬。
///
/// 形状照抄仓库里已有的两处正确孪生（同样是往子进程 stdin 写一行）：
/// `QuotaCenter.swift` / `ModelCatalogCenter.swift` 的 `send(_:)`。**别发明第三种。**
///
/// **换 API 还不够，SIGPIPE 也得自己关掉 —— 这条是实测出来的，不是推的。**
/// 单独量过（`swiftc` 起一个裸 Foundation 进程，关掉管道读端再写）：
///   * SIGPIPE 保持默认 → `write(contentsOf:)` **根本不返回**，进程被信号打死，
///     exit 141（128+13）。**没有异常、没有日志、什么都不留。**
///   * `signal(SIGPIPE, SIG_IGN)` 之后 → 抛 `NSCocoaErrorDomain 512`，接得住。
/// 2026-09-05 那次之所以还留下了一份 NSException 崩溃日志，是因为 daemon 那时跑在
/// AppKit 里、SIGPIPE 恰好是被忽略的 —— 那是**借来的**、没人写在纸上的前提。
/// P5b 正要把后台做成不带 AppKit 的常驻 agent：那一刻这个前提会无声消失，
/// 而回来的崩溃比原来更难查（连异常日志都没有了）。
/// 所以这份保证归这个函数自己拿着，不靠进程里碰巧有谁替它关过。
enum CodexPipeWrite {
    /// 进程级、一次性、幂等。放在这里而不是 main 入口，是为了让「往管道写不会被信号
    /// 打死」这件事**跟着写操作走**、并且能被单测直接钉住 —— 写在入口就只剩一句注释，
    /// 而注释拦不住把入口改掉的人。
    private static let sigpipeIgnored: Void = { signal(SIGPIPE, SIG_IGN) }()

    static func line(_ line: String, to handle: FileHandle) throws {
        _ = sigpipeIgnored
        try handle.write(contentsOf: Data((line + "\n").utf8))
    }
}

/// Owns the `codex app-server` child process and its stdio pipes. Encodes our
/// requests via CodexRPCMessage, routes incoming lines through CodexRPCDispatcher.
/// The hard logic (framing, two-id routing, early-response buffering) is unit-tested
/// in the codec/dispatcher; this type is the thin Process/pipe wrapper, verified
/// end-to-end on the real machine.
actor CodexAppServerConnection {
    private let process = Process()
    private let stdinPipe = Pipe()
    private let stdoutPipe = Pipe()
    private let stderrPipe = Pipe()
    private let dispatcher = CodexRPCDispatcher()
    private var nextId = 0
    private var readBuffer = Data()
    private var onTerminate: ((Int32?) -> Void)?

    init(executable: String, argv: [String], cwd: String, env: [String: String]) {
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = argv                  // SessionConfig.argv() → ["app-server"] (+ -c …)
        process.currentDirectoryURL = URL(fileURLWithPath: cwd)
        process.environment = env                  // env guard already stripped OPENAI_API_KEY for codex
        process.standardInput = stdinPipe
        process.standardOutput = stdoutPipe
        process.standardError = stderrPipe
    }

    /// Spawn + initialize handshake. Wire the streaming + server-request callbacks.
    func start(onServerRequest: @escaping (Int, String, [String: Any]) -> Void,
               onNotification: @escaping (String, [String: Any]) -> Void,
               onTerminate: ((Int32?) -> Void)? = nil) async throws {
        self.onTerminate = onTerminate
        await dispatcher.setServerRequestHandler(onServerRequest)
        await dispatcher.setNotificationHandler(onNotification)
        stdoutPipe.fileHandleForReading.readabilityHandler = { [weak self] h in
            CodexPipeReadability.drain(h) { data in
                Task { await self?.ingest(data) }
            }
        }
        // Drain stderr too. codex writes tracing/diagnostics there; if nobody reads it,
        // the ~64KB kernel pipe buffer fills, codex blocks on the write, and the whole
        // app-server stalls — a "turn never completes" deadlock that only surfaces after
        // sustained output (a short turn stays under 64KB, which is why unit/one-turn
        // checks pass). Discarded in v1; we just need to keep the pipe empty.
        stderrPipe.fileHandleForReading.readabilityHandler = { h in
            CodexPipeReadability.drain(h) { _ in }
        }
        process.terminationHandler = { [weak self] _ in
            Task { await self?.handleTermination() }
        }
        try process.run()
        _ = try await request(method: "initialize",
                              params: CodexProtocol.initializeParams(clientName: "PendingCrew", version: "1.0"))
        try notify(method: "initialized", params: [:])
    }

    /// 子进程还在不在（拉起自检 #541 用）——`Process.isRunning` 在 spawn 前也是
    /// false，所以自检只在 `start()` 之后问它。
    var isProcessRunning: Bool { process.isRunning }

    private func ingest(_ data: Data) async {
        readBuffer.append(data)
        while let nl = readBuffer.firstIndex(of: 0x0a) {
            let lineData = readBuffer[readBuffer.startIndex..<nl]
            readBuffer.removeSubrange(readBuffer.startIndex...nl)
            guard let line = String(data: lineData, encoding: .utf8),
                  !line.trimmingCharacters(in: .whitespaces).isEmpty else { continue }
            if let msg = try? CodexRPCMessage.classify(line: line) { try? await dispatcher.handle(msg) }
        }
    }

    @discardableResult
    func request(method: String, params: [String: Any]) async throws -> Any? {
        let id = nextId; nextId += 1
        async let response: Any? = dispatcher.awaitResponse(id: id)
        try writeLine(CodexRPCMessage.encodeRequest(id: id, method: method, params: params))
        return try await response
    }

    func notify(method: String, params: [String: Any]) throws {
        try writeLine(CodexRPCMessage.encodeNotification(method: method, params: params))
    }

    func respond(serverId: Int, result: [String: Any]) throws {
        try writeLine(CodexRPCMessage.encodeResponse(id: serverId, result: result))
    }

    func respondError(serverId: Int, code: Int, message: String) throws {
        try writeLine(CodexRPCMessage.encodeError(id: serverId, code: code, message: message))
    }

    private func writeLine(_ line: String) throws {
        try CodexPipeWrite.line(line, to: stdinPipe.fileHandleForWriting)
    }

    private func handleTermination() async {
        await dispatcher.failAll(CodexRPCError.malformed("app-server terminated"))
        onTerminate?(process.terminationStatus)
    }

    func terminate() {
        stdoutPipe.fileHandleForReading.readabilityHandler = nil
        stderrPipe.fileHandleForReading.readabilityHandler = nil
        if process.isRunning { process.terminate() }
        let pid = process.processIdentifier
        Task { await terminateTree(pid: pid, graceSeconds: 2.0) }
    }

    var isRunning: Bool { process.isRunning }

    /// daemon 的 registry 要记「谁是我的子进程」（前后端分离 §8.2）。
    /// 未拉起时 Foundation 给 0。
    var processIdentifier: Int32 { process.processIdentifier }
}
#endif
