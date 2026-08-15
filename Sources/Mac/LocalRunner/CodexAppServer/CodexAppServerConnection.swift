#if os(macOS)
import Foundation

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
            let data = h.availableData
            guard !data.isEmpty else { return }
            Task { await self?.ingest(data) }
        }
        // Drain stderr too. codex writes tracing/diagnostics there; if nobody reads it,
        // the ~64KB kernel pipe buffer fills, codex blocks on the write, and the whole
        // app-server stalls — a "turn never completes" deadlock that only surfaces after
        // sustained output (a short turn stays under 64KB, which is why unit/one-turn
        // checks pass). Discarded in v1; we just need to keep the pipe empty.
        stderrPipe.fileHandleForReading.readabilityHandler = { h in _ = h.availableData }
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
        stdinPipe.fileHandleForWriting.write(Data((line + "\n").utf8))
    }

    private func handleTermination() async {
        await dispatcher.failAll(CodexRPCError.malformed("app-server terminated"))
        onTerminate?(process.terminationStatus)
    }

    func terminate() {
        if process.isRunning { process.terminate() }
        let pid = process.processIdentifier
        Task { await terminateTree(pid: pid, graceSeconds: 2.0) }
    }

    var isRunning: Bool { process.isRunning }
}
#endif
