#if os(macOS)
import AppKit
import Combine
import Foundation

private let inProcessProtocolCapabilities = [
    "approval-mode", "launch-parameter-problem", "profile-switch", "screen-text",
    "terminal-bytes", "transcript-events",
]

struct TerminalSize: Equatable {
    var cols: Int
    var rows: Int
}

/// Server-side optional terminal operations. It deliberately does not widen
/// `SessionBackend`: structured and PTY backends keep one shared lifecycle API,
/// while the protocol server asks for raw hot-path operations only when present.
@MainActor
protocol SessionProtocolTerminalControlling: AnyObject {
    func sendRaw(_ bytes: [UInt8])
    func resizeTerminal(cols: Int, rows: Int)
}

@MainActor
protocol SessionProtocolScreenTextProviding: AnyObject {
    func screenText(maxLines: Int) -> String
}

/// `SessionBackend` 外的可选读能力。P2 不扩大生命周期协议；调用方通过它同时覆盖
/// P1 直连回退和 `RemoteSessionBackend`，远端实现仍会发 `control.screenText`。
@MainActor
enum SessionAuthoritativeScreenText {
    static func read(from backend: any SessionBackend, maxLines: Int) -> String? {
        (backend as? SessionProtocolScreenTextProviding)?.screenText(maxLines: maxLines)
    }
}

@MainActor
protocol SessionProtocolTerminalSnapshotProviding: AnyObject {
    func protocolTerminalSnapshot() -> TerminalSnapshotEncoder.Snapshot?
}

@MainActor
protocol SessionProtocolCodexHistoryProviding: AnyObject {
    var protocolCodexHistory: [CodexThreadItem] { get }
}

/// 唤醒回执可选读能力：只给有结构化 Codex transcript 的后端实现，避免把
/// 生命周期协议扩大成所有 PTY 后端都要伪造活动序号。
@MainActor
protocol SessionWakeActivityProviding: AnyObject {
    var wakeActivityRevision: UInt64 { get }
}

@MainActor
protocol SessionProtocolApprovalControlling: AnyObject {
    func updateProtocolApprovalsReviewer(_ reviewer: CodexProtocol.ApprovalsReviewer) async throws
}

@MainActor
protocol SessionProtocolLaunchProblemProviding: AnyObject {
    var protocolLaunchParameterProblems: AnyPublisher<SessionLaunchParameterProblem, Never> { get }
}

extension AgentTerminalSession: SessionProtocolTerminalControlling {
    func resizeTerminal(cols: Int, rows: Int) {
        core.resize(cols: cols, rows: rows)
        core.noteViewportChange()
    }
}


extension AgentTerminalSession: SessionProcessIdentifying {
    var agentProcessIdentifier: Int32 { core.process?.shellPid ?? 0 }
}

extension PlainTerminalSession: SessionProcessIdentifying {
    var agentProcessIdentifier: Int32 { core.process?.shellPid ?? 0 }
}

extension AgentTerminalSession: SessionProtocolScreenTextProviding {
    func screenText(maxLines: Int) -> String { core.screenText(maxLines: maxLines) }
}

extension AgentTerminalSession: SessionProtocolTerminalSnapshotProviding {
    func protocolTerminalSnapshot() -> TerminalSnapshotEncoder.Snapshot? { core.snapshot() }
}

extension AgentTerminalSession: SessionProtocolLaunchProblemProviding {
    var protocolLaunchParameterProblems: AnyPublisher<SessionLaunchParameterProblem, Never> {
        launchParameterProblems
    }
}

extension PlainTerminalSession: SessionProtocolTerminalControlling {
    func sendRaw(_ bytes: [UInt8]) { core.sendRaw(bytes) }
    func resizeTerminal(cols: Int, rows: Int) {
        core.resize(cols: cols, rows: rows)
        core.noteViewportChange()
    }
}


extension PlainTerminalSession: SessionProtocolScreenTextProviding {
    func screenText(maxLines: Int) -> String { core.screenText(maxLines: maxLines) }
}

extension PlainTerminalSession: SessionProtocolTerminalSnapshotProviding {
    func protocolTerminalSnapshot() -> TerminalSnapshotEncoder.Snapshot? { core.snapshot() }
}

extension CodexAppServerBackend: SessionProcessIdentifying {
    var agentProcessIdentifier: Int32 { cachedAgentProcessIdentifier }
}

extension CodexAppServerBackend: SessionProtocolScreenTextProviding,
    SessionProtocolApprovalControlling, SessionProtocolCodexHistoryProviding,
    SessionWakeActivityProviding {
    var protocolCodexHistory: [CodexThreadItem] { transcript.items }
    var wakeActivityRevision: UInt64 { transcript.activityRevision }

    func screenText(maxLines: Int) -> String {
        let items = transcript.items.suffix(max(0, maxLines))
        guard !items.isEmpty else { return "（transcript 为空）" }
        return items.map { item in
            switch item.kind {
            case let .userMessage(text): return "[输入] \(text.prefix(200))"
            case let .agentMessage(text, _): return "[回复] \(text.prefix(300))"
            case let .reasoning(summary, content):
                return "[思考] \((summary ?? content ?? "…").prefix(200))"
            case let .plan(text): return "[计划] \(text.prefix(200))"
            case let .commandExecution(command):
                return "[命令] \(command.command.prefix(160))"
                    + (command.exitCode.map { " → exit \($0)" } ?? "")
            case let .fileChange(change): return "[改文件] \(change.summary ?? change.status ?? "?")"
            case let .toolCall(name, status): return "[工具] \(name) \(status ?? "")"
            case let .webSearch(query): return "[搜索] \(query ?? "")"
            case let .unknown(type): return "[\(type)]"
            }
        }.joined(separator: "\n")
    }

    func updateProtocolApprovalsReviewer(_ reviewer: CodexProtocol.ApprovalsReviewer) async throws {
        try await updateApprovalsReviewer(reviewer)
    }
}

extension CodexThreadItem {
    /// 把 daemon 内存里的 reduced transcript 还原成 app 已经会消费的
    /// `item/completed` 形状。它只跨 app/viewer 重启；daemon 重启仍按 §8.6 不保证。
    var protocolWireItem: [String: SessionWireJSONValue] {
        var item: [String: SessionWireJSONValue] = ["id": .string(id)]
        func put(_ key: String, _ value: String?) {
            if let value { item[key] = .string(value) }
        }
        switch kind {
        case let .userMessage(text):
            item["type"] = .string("userMessage"); item["text"] = .string(text)
        case let .agentMessage(text, phase):
            item["type"] = .string("agentMessage"); item["text"] = .string(text)
            put("phase", phase)
        case let .reasoning(summary, content):
            item["type"] = .string("reasoning")
            put("summary", summary); put("content", content)
        case let .plan(text):
            item["type"] = .string("plan"); item["text"] = .string(text)
        case let .commandExecution(command):
            item["type"] = .string("commandExecution")
            item["command"] = .string(command.command)
            put("cwd", command.cwd); put("status", command.status)
            put("aggregatedOutput", command.aggregatedOutput)
            if let exitCode = command.exitCode { item["exitCode"] = .number(Double(exitCode)) }
            item["commandActions"] = .array(command.actions.map { action in
                var fields: [String: SessionWireJSONValue] = [
                    "type": .string(action.kind.rawValue),
                ]
                if let value = action.command { fields["command"] = .string(value) }
                if let value = action.name { fields["name"] = .string(value) }
                if let value = action.path { fields["path"] = .string(value) }
                if let value = action.query { fields["query"] = .string(value) }
                return .object(fields)
            })
        case let .fileChange(change):
            item["type"] = .string("fileChange")
            put("status", change.status); put("summary", change.summary)
        case let .toolCall(name, status):
            item["type"] = .string("dynamicToolCall"); item["name"] = .string(name)
            put("status", status)
        case let .webSearch(query):
            item["type"] = .string("webSearch"); put("query", query)
        case let .unknown(type):
            item["type"] = .string(type)
        }
        return item
    }
}

enum SessionProtocolControlError: LocalizedError {
    case unsupported(String)
    case failed(String)

    var errorDescription: String? {
        switch self {
        case let .unsupported(message), let .failed(message): return message
        }
    }
}

/// §9 P2 的一行回退开关。false = P1 直连 backend；true = 同进程但全过协议。
enum SessionBackendRouting {
    static let usesProtocolTransport = true
}

@MainActor
final class RemoteSessionBackend: ObservableObject, SessionBackend,
    SessionProtocolScreenTextProviding, SessionWakeActivityProviding {
    let sessionId: String
    let kind: LocalCodingAgentKind
    let terminalView: TerminalMirrorView?
    let transcript: CodexTranscript?

    @Published private(set) var status: SessionStatus = .running
    var statusPublisher: Published<SessionStatus>.Publisher { $status }
    var isBusy: Bool { kind == .codex && isWorking }
    @Published private(set) var isWorking = false
    var isWorkingPublisher: Published<Bool>.Publisher { $isWorking }
    @Published private(set) var displayIsTyping = false
    var displayIsTypingUpdates: AnyPublisher<Bool, Never> { $displayIsTyping.eraseToAnyPublisher() }
    @Published private(set) var health: CrewSessionHealth?
    var healthPublisher: Published<CrewSessionHealth?>.Publisher { $health }
    @Published private(set) var pendingDecision: PendingTerminalDecision?
    var pendingDecisionUpdates: AnyPublisher<PendingTerminalDecision?, Never> {
        $pendingDecision.eraseToAnyPublisher()
    }
    @Published private(set) var launchParameterProblem: SessionLaunchParameterProblem?
    var launchParameterProblems: AnyPublisher<SessionLaunchParameterProblem, Never> {
        $launchParameterProblem.compactMap { $0 }.eraseToAnyPublisher()
    }
    @Published private(set) var scrollState = AgentTerminalSession.ScrollState()

    private(set) var isProtocolConnected = false
    private(set) var negotiatedCapabilities: [String] = []
    private(set) var lastTerminalFrameBytes: [UInt8] = []
    private(set) var lastCompletedSnapshotBytes: [UInt8] = []
    private(set) var completedSnapshotCount = 0
    var wakeActivityRevision: UInt64 { transcript?.activityRevision ?? 0 }
    private(set) var requestedTerminalSize = TerminalSize(cols: 80, rows: 25)
    private var handle: UInt32?
    private var snapshotBytes: [UInt8] = []
    private var nextSnapshotSequence: UInt32 = 0
    private unowned let client: SessionProtocolClient

    init(sessionId: String, kind: LocalCodingAgentKind, client: SessionProtocolClient) {
        self.sessionId = sessionId
        self.kind = kind
        self.client = client
        if kind == .codex {
            terminalView = nil
            transcript = CodexTranscript()
        } else {
            let mirror = TerminalMirrorView(frame: .zero)
            terminalView = mirror
            transcript = nil
            mirror.getTerminal().resize(
                cols: requestedTerminalSize.cols, rows: requestedTerminalSize.rows)
            mirror.terminalDelegate = mirror
            if kind == .terminal { mirror.useNativeScroller() }
            mirror.onSendBytes = { [weak self] bytes in self?.sendRaw(bytes) }
            mirror.onResize = { [weak self] cols, rows in self?.resizeTerminal(cols: cols, rows: rows) }
            mirror.onScroll = { [weak self] userInitiated in
                self?.refreshScrollState(userInitiated: userInitiated)
            }
        }
    }

    func supportsCapability(_ capability: String) -> Bool {
        SessionCapabilities.supports(capability, in: negotiatedCapabilities)
    }

    func send(_ text: String) {
        sendRaw(Array(text.utf8))
        // Preserve AgentSessionCore.send's paste-vs-key timing at the app side:
        // body and Enter are two input frames, never one JSON/control message.
        if kind == .claudeCode {
            Task { @MainActor [weak self] in
                try? await Task.sleep(nanoseconds: 200_000_000)
                guard self?.status == .running else { return }
                self?.sendRaw([0x0d])
            }
        }
    }

    func interrupt() { sendRaw(kind == .terminal ? [0x03] : [0x1b]) }
    func stop() { client.sendControl(sessionId: sessionId, op: "stop") }
    func clearQuotaHealth() { client.sendControl(sessionId: sessionId, op: "clearQuotaHealth") }

    func applyProfileSwitch(_ cmd: SessionProfileSwitchCommand) async -> SessionProfileSwitchOutcome {
        await client.applyProfileSwitch(sessionId: sessionId, command: cmd)
    }

    func updateApprovalsReviewer(_ reviewer: CodexProtocol.ApprovalsReviewer) async throws {
        guard supportsCapability("approval-mode") else {
            throw SessionProtocolControlError.unsupported("daemon 不支持运行态审批模式切换")
        }
        try await client.updateApprovalsReviewer(sessionId: sessionId, reviewer: reviewer)
    }

    func screenText(maxLines: Int) -> String {
        guard supportsCapability("screen-text") else { return "（daemon 不支持读取输出）" }
        return client.screenText(sessionId: sessionId, maxLines: maxLines) ?? "（输出为空）"
    }

    func sendRaw(_ bytes: [UInt8]) {
        guard let handle else { return }
        client.sendInput(handle: handle, bytes: bytes)
    }

    func resizeTerminal(cols: Int, rows: Int) {
        requestedTerminalSize = .init(cols: cols, rows: rows)
        guard let handle else { return }
        client.resize(handle: handle, cols: cols, rows: rows)
        refreshScrollState(userInitiated: false)
    }

    func scrollTerminal(toPosition position: Double) {
        terminalView?.scroll(toPosition: max(0, min(1, position)))
        refreshScrollState(userInitiated: true)
    }

    func attach(handle: UInt32) { self.handle = handle }

    func updateConnection(capabilities: [String]) {
        negotiatedCapabilities = capabilities
        isProtocolConnected = true
    }

    func transportDisconnected() {
        handle = nil
        negotiatedCapabilities = []
        isProtocolConnected = false
    }

    func apply(state: SessionProtocolState) {
        status = state.status.sessionStatus
        isWorking = state.isWorking
        displayIsTyping = state.displayIsTyping
        health = state.health?.health
        pendingDecision = state.pendingDecision.map {
            PendingTerminalDecision(prompt: $0.prompt, options: $0.options)
        }
        launchParameterProblem = state.launchParameterProblem?.problem
    }

    func receiveTerminal(_ bytes: [UInt8]) {
        lastTerminalFrameBytes = bytes
        terminalView?.remoteLastOutputAt = Date()
        terminalView?.feedFromCore(bytes[...])
        refreshScrollState(userInitiated: false)
    }

    /// 收到 `resync` —— 手上这半截快照作废，等新的 seq 0。不清的话，新快照的
    /// seq 0 会撞上旧累积的 `nextSnapshotSequence`，那条顺序 guard 会把整份新快照
    /// 静默丢掉，画面从此停在重同步那一刻。
    func beginResync() {
        snapshotBytes = []
        nextSnapshotSequence = 0
    }

    func receiveSnapshot(seq: UInt32, isLast: Bool, bytes: [UInt8]) {
        if seq == 0 {
            snapshotBytes = []
            nextSnapshotSequence = 0
            terminalView?.getTerminal().resize(
                cols: requestedTerminalSize.cols, rows: requestedTerminalSize.rows)
        }
        guard seq == nextSnapshotSequence else {
            snapshotBytes = []
            nextSnapshotSequence = 0
            return
        }
        snapshotBytes.append(contentsOf: bytes)
        nextSnapshotSequence &+= 1
        guard isLast else { return }

        lastCompletedSnapshotBytes = snapshotBytes
        completedSnapshotCount += 1
        terminalView?.feedFromCore(snapshotBytes[...])
        snapshotBytes = []
        nextSnapshotSequence = 0
        refreshScrollState(userInitiated: false)
    }

    func receiveEvent(_ event: SessionEvent) {
        guard event.kind == "codexNotification",
              case let .string(eventSessionId)? = event.fields["sessionId"],
              eventSessionId == sessionId,
              case let .string(method)? = event.fields["method"],
              case let .object(params)? = event.fields["params"] else { return }
        transcript?.apply(method: method, params: params.mapValues(\.foundationObject))
        // codex notification 比下一份 state snapshot 更早到 app。立刻镜像 turn
        // 生命周期，避免这段窗口里 inspect_session / 状态点谎报“空闲”。
        if method == "turn/started" {
            isWorking = true
            displayIsTyping = true
        } else if method == "turn/completed" {
            isWorking = false
            displayIsTyping = false
        }
    }

    private func refreshScrollState(userInitiated: Bool) {
        guard let terminalView else { return }
        let next = AgentTerminalSession.ScrollState(
            canScroll: terminalView.canScroll,
            position: terminalView.scrollPosition,
            thumbSize: Double(terminalView.scrollThumbsize),
            userScrollTick: scrollState.userScrollTick + (userInitiated ? 1 : 0))
        if next != scrollState { scrollState = next }
    }
}

@MainActor
final class InProcessSessionProtocolBridge: SessionProtocolPublishing {
    /// 同进程：有窗口，照旧造门面 + mirror。
    let isHeadless = false

    private let transport: InProcessTransport
    private let appLink: InProcessSessionLink
    private let daemonLink: InProcessSessionLink
    private let server: SessionProtocolServer
    private let client: SessionProtocolClient

    init(appCapabilities: [String] = inProcessProtocolCapabilities,
         daemonCapabilities: [String] = inProcessProtocolCapabilities) {
        let transport = InProcessTransport()
        self.transport = transport
        appLink = InProcessSessionLink(transport: transport, side: .app)
        daemonLink = InProcessSessionLink(transport: transport, side: .daemon)
        server = SessionProtocolServer(capabilities: daemonCapabilities)
        client = SessionProtocolClient(link: appLink, capabilities: appCapabilities)
        // accept / init 会各自把 onReceive 装到链路上；两条链路互不覆盖对方的回调。
        server.accept(link: daemonLink)
        client.connect()
    }

    func expose(sessionId: String, backend: any SessionBackend) -> RemoteSessionBackend? {
        exposeAttached(sessionId: sessionId, backend: backend)
    }

    /// 同进程一定拿得到 viewer 侧后端 —— 协议里那个签名是 optional，只是为了让
    /// daemon（没有 viewer）也能实现同一个接缝。调用方明知自己在同进程时用这个。
    @discardableResult
    func exposeAttached(sessionId: String, backend: any SessionBackend) -> RemoteSessionBackend {
        server.register(sessionId: sessionId, backend: backend)
        return client.attach(sessionId: sessionId, kind: backend.kind)
    }

    func retire(sessionId: String) { server.unregister(sessionId: sessionId) }

    func publishTerminalBytes(sessionId: String, bytes: [UInt8]) {
        server.publishTerminalBytes(sessionId: sessionId, bytes: bytes)
    }

    func terminalOutputSink(sessionId: String) -> ([UInt8]) -> Void {
        { [weak server] bytes in
            MainActor.assumeIsolated {
                server?.acceptTerminalBytes(sessionId: sessionId, bytes: bytes)
            }
        }
    }

    func codexNotificationSink(sessionId: String) -> (String, [String: Any]) -> Void {
        { [weak server] method, params in
            MainActor.assumeIsolated {
                server?.acceptCodexNotification(
                    sessionId: sessionId, method: method, params: params)
            }
        }
    }

    func disconnectViewer() {
        transport.disconnect()
        daemonLink.peerDisconnected()
        appLink.peerDisconnected()
    }

    func reconnectViewer() {
        transport.reconnect()
        server.accept(link: daemonLink)
        client.reconnect()
    }
}

extension SessionWireStatus {
    init(_ status: SessionStatus) {
        switch status { case .running: self = .running; case let .exited(code): self = .exited(code) }
    }
    var sessionStatus: SessionStatus {
        switch self { case .running: return .running; case let .exited(code): return .exited(code) }
    }
}

extension SessionHealthWire {
    init(_ health: CrewSessionHealth) { self.init(kind: health.kind.rawValue, detail: health.detail) }
    var health: CrewSessionHealth? {
        guard let kind = CrewSessionHealth.Kind(rawValue: kind) else { return nil }
        return .init(kind: kind, detail: detail)
    }
}

extension SessionLaunchParameterProblemWire {
    init(_ problem: SessionLaunchParameterProblem) {
        switch problem {
        case let .modelUnrecognized(value, quote):
            self.init(kind: "modelUnrecognized", value: value, quote: quote)
        case let .effortIgnored(value, quote):
            self.init(kind: "effortIgnored", value: value, quote: quote)
        }
    }

    var problem: SessionLaunchParameterProblem? {
        switch kind {
        case "modelUnrecognized": return .modelUnrecognized(value: value, quote: quote)
        case "effortIgnored": return .effortIgnored(value: value, quote: quote)
        default: return nil
        }
    }
}

extension SessionEvent {
    static func controlResult(
        kind: String, requestId: String, sessionId: String, error: String?
    ) -> SessionEvent {
        var fields: [String: SessionWireJSONValue] = ["sessionId": .string(sessionId)]
        if let error { fields["error"] = .string(error) }
        return .init(kind: kind, requestId: requestId, fields: fields)
    }
}

extension SessionProfileSwitchOutcome {
    func protocolEvent(requestId: String) -> SessionEvent {
        let pair: (String, String?)
        switch self {
        case let .applied(detail): pair = ("applied", detail)
        case let .rejected(detail): pair = ("rejected", detail)
        case .noConfirmation: pair = ("noConfirmation", nil)
        case .neverIdle: pair = ("neverIdle", nil)
        case .unsupported: pair = ("unsupported", nil)
        }
        var fields: [String: SessionWireJSONValue] = ["outcome": .string(pair.0)]
        if let detail = pair.1 { fields["detail"] = .string(detail) }
        return .init(kind: "profileSwitchResult", requestId: requestId, fields: fields)
    }

    init(protocolEvent event: SessionEvent) {
        let detail: String
        if case let .string(value)? = event.fields["detail"] { detail = value } else { detail = "" }
        guard case let .string(outcome)? = event.fields["outcome"] else { self = .unsupported; return }
        switch outcome {
        case "applied": self = .applied(detail)
        case "rejected": self = .rejected(detail)
        case "noConfirmation": self = .noConfirmation
        case "neverIdle": self = .neverIdle
        default: self = .unsupported
        }
    }
}

extension SessionWireJSONValue {
    /// Codable ↔ 线上 JSON 值。控制帧的 `arguments` 是 `[String: SessionWireJSONValue]`，
    /// 而我们要送的东西（`SessionConfig` 之类）本来就是 Codable —— 中间不再手抄字段。
    static func encoding<T: Encodable>(_ value: T) -> SessionWireJSONValue? {
        guard let data = try? JSONEncoder().encode(value),
              let object = try? JSONSerialization.jsonObject(with: data) else { return nil }
        return SessionWireJSONValue(object)
    }

    func decoding<T: Decodable>(_ type: T.Type) -> T? {
        guard let data = try? JSONSerialization.data(withJSONObject: foundationObject) else {
            return nil
        }
        return try? JSONDecoder().decode(type, from: data)
    }

    init?(_ value: Any) {
        switch value {
        case let value as String: self = .string(value)
        case let value as Bool: self = .bool(value)
        case let value as NSNumber: self = .number(value.doubleValue)
        case let value as [String: Any]:
            var object: [String: SessionWireJSONValue] = [:]
            for (key, child) in value {
                guard let converted = SessionWireJSONValue(child) else { return nil }
                object[key] = converted
            }
            self = .object(object)
        case let value as [Any]:
            var array: [SessionWireJSONValue] = []
            for child in value {
                guard let converted = SessionWireJSONValue(child) else { return nil }
                array.append(converted)
            }
            self = .array(array)
        case _ as NSNull: self = .null
        default: return nil
        }
    }

    var foundationObject: Any {
        switch self {
        case let .string(value): return value
        case let .number(value): return value
        case let .bool(value): return value
        case let .object(value): return value.mapValues(\.foundationObject)
        case let .array(value): return value.map(\.foundationObject)
        case .null: return NSNull()
        }
    }
}
#endif
