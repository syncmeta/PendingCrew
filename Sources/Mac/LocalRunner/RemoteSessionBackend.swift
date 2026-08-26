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


extension AgentTerminalSession: SessionProtocolScreenTextProviding {
    func screenText(maxLines: Int) -> String { core.screenText(maxLines: maxLines) }
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

extension CodexAppServerBackend: SessionProtocolScreenTextProviding, SessionProtocolApprovalControlling {
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
final class RemoteSessionBackend: ObservableObject, SessionBackend {
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
    private var handle: UInt32?
    private unowned let client: InProcessSessionProtocolClient

    init(sessionId: String, kind: LocalCodingAgentKind, client: InProcessSessionProtocolClient) {
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
        guard let handle else { return }
        client.resize(handle: handle, cols: cols, rows: rows)
        refreshScrollState(userInitiated: false)
    }

    func scrollTerminal(toPosition position: Double) {
        terminalView?.scroll(toPosition: max(0, min(1, position)))
        refreshScrollState(userInitiated: true)
    }

    fileprivate func attach(handle: UInt32) { self.handle = handle }

    fileprivate func updateConnection(capabilities: [String]) {
        negotiatedCapabilities = capabilities
        isProtocolConnected = true
    }

    fileprivate func transportDisconnected() {
        handle = nil
        negotiatedCapabilities = []
        isProtocolConnected = false
    }

    fileprivate func apply(state: SessionProtocolState) {
        status = state.status.sessionStatus
        isWorking = state.isWorking
        displayIsTyping = state.displayIsTyping
        health = state.health?.health
        pendingDecision = state.pendingDecision.map {
            PendingTerminalDecision(prompt: $0.prompt, options: $0.options)
        }
        launchParameterProblem = state.launchParameterProblem?.problem
    }

    fileprivate func receiveTerminal(_ bytes: [UInt8]) {
        lastTerminalFrameBytes = bytes
        terminalView?.remoteLastOutputAt = Date()
        terminalView?.feedFromCore(bytes[...])
        refreshScrollState(userInitiated: false)
    }

    fileprivate func receiveEvent(_ event: SessionEvent) {
        guard event.kind == "codexNotification",
              case let .string(eventSessionId)? = event.fields["sessionId"],
              eventSessionId == sessionId,
              case let .string(method)? = event.fields["method"],
              case let .object(params)? = event.fields["params"] else { return }
        transcript?.apply(method: method, params: params.mapValues(\.foundationObject))
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
final class InProcessSessionProtocolBridge {
    private let transport: InProcessTransport
    private let server: InProcessSessionProtocolServer
    private let client: InProcessSessionProtocolClient

    init(appCapabilities: [String] = inProcessProtocolCapabilities,
         daemonCapabilities: [String] = inProcessProtocolCapabilities) {
        let transport = InProcessTransport()
        self.transport = transport
        server = InProcessSessionProtocolServer(
            transport: transport, capabilities: daemonCapabilities)
        client = InProcessSessionProtocolClient(
            transport: transport, capabilities: appCapabilities)
        client.connect()
    }

    func expose(sessionId: String, backend: any SessionBackend) -> RemoteSessionBackend {
        server.register(sessionId: sessionId, backend: backend)
        return client.attach(sessionId: sessionId, kind: backend.kind)
    }

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
        server.viewerDisconnected()
        client.transportDisconnected()
    }

    func reconnectViewer() {
        transport.reconnect()
        client.reconnect()
    }
}

@MainActor
private final class InProcessSessionProtocolServer {
    private final class Record {
        let backend: any SessionBackend
        var stateSeq: UInt64 = 0
        var handles: Set<UInt32> = []
        var observations: Set<AnyCancellable> = []
        var launchParameterProblem: SessionLaunchParameterProblem?
        init(backend: any SessionBackend) { self.backend = backend }
    }

    private let transport: InProcessTransport
    private let codec = SessionProtocolCodec()
    private let capabilities: [String]
    private var records: [String: Record] = [:]
    private var sessionByHandle: [UInt32: String] = [:]
    private var nextHandle: UInt32 = 1
    private var pendingTerminalBytes: [String: [[UInt8]]] = [:]
    private var pendingEvents: [String: [SessionEvent]] = [:]

    init(transport: InProcessTransport, capabilities: [String]) {
        self.transport = transport
        self.capabilities = capabilities
        transport.receiveFromApp = { [weak self] data in
            MainActor.assumeIsolated { self?.receive(data) }
        }
    }

    func register(sessionId: String, backend: any SessionBackend) {
        let record = Record(backend: backend)
        records[sessionId] = record
        observe(sessionId: sessionId, record: record)
    }

    func publishTerminalBytes(sessionId: String, bytes: [UInt8]) {
        guard let record = records[sessionId] else { return }
        // 已注册但无人 attach 时按 §4.5 丢实时流；重连靠快照恢复，不重放增量。
        guard !record.handles.isEmpty else { return }
        for handle in record.handles.sorted() {
            send(.data(.init(handle: handle, bytes: bytes)))
        }
    }

    func acceptTerminalBytes(sessionId: String, bytes: [UInt8]) {
        guard records[sessionId] != nil else {
            pendingTerminalBytes[sessionId, default: []].append(bytes)
            return
        }
        publishTerminalBytes(sessionId: sessionId, bytes: bytes)
    }

    func acceptCodexNotification(sessionId: String, method: String, params: [String: Any]) {
        guard let paramsValue = SessionWireJSONValue(params) else { return }
        let event = SessionEvent(kind: "codexNotification", requestId: nil, fields: [
            "sessionId": .string(sessionId), "method": .string(method), "params": paramsValue,
        ])
        guard let record = records[sessionId] else {
            pendingEvents[sessionId, default: []].append(event)
            return
        }
        // 与 PTY data 相同：断线期间不积压增量，重连后由快照/全量状态恢复。
        guard !record.handles.isEmpty else { return }
        send(.event(event))
    }

    func viewerDisconnected() {
        sessionByHandle.removeAll()
        records.values.forEach { $0.handles.removeAll() }
    }

    private func observe(sessionId: String, record: Record) {
        func changed(_ mutate: @escaping (inout SessionProtocolState) -> Void) {
            MainActor.assumeIsolated {
                self.publishState(sessionId: sessionId, mutate: mutate)
            }
        }
        // @Published emits in willSet. Use the emitted value as an override instead
        // of rereading the backend (which would serialize the previous value).
        record.backend.statusPublisher.sink { value in
            changed { $0.status = .init(value) }
        }.store(in: &record.observations)
        record.backend.isWorkingPublisher.sink { value in
            changed { $0.isWorking = value }
        }.store(in: &record.observations)
        record.backend.displayIsTypingUpdates.sink { value in
            changed { $0.displayIsTyping = value }
        }.store(in: &record.observations)
        record.backend.healthPublisher.sink { value in
            changed { $0.health = value.map(SessionHealthWire.init) }
        }.store(in: &record.observations)
        record.backend.pendingDecisionUpdates.sink { value in
            changed { state in
                state.pendingDecision = value.map { .init(prompt: $0.prompt, options: $0.options) }
            }
        }.store(in: &record.observations)
        if let source = record.backend as? SessionProtocolLaunchProblemProviding {
            source.protocolLaunchParameterProblems.sink { [weak self, weak record] value in
                guard let self, let record else { return }
                MainActor.assumeIsolated {
                    record.launchParameterProblem = value
                    self.publishState(sessionId: sessionId) {
                        $0.launchParameterProblem = .init(value)
                    }
                }
            }.store(in: &record.observations)
        }
    }

    private func receive(_ data: Data) {
        guard let message = try? codec.decodeApp(data) else { return }
        switch message {
        case .hello:
            send(.hello(.init(protocolVersion: 1, daemonBuild: "in-process",
                              capabilities: capabilities, sessionCount: records.count,
                              pid: Int32(ProcessInfo.processInfo.processIdentifier))))
        case .listSessions:
            sendFullList()
        case let .attach(value):
            guard let record = records[value.sessionId] else { return }
            let handle = nextHandle
            nextHandle &+= 1
            record.handles.insert(handle)
            sessionByHandle[handle] = value.sessionId
            send(.attached(.init(sessionId: value.sessionId, handle: handle, snapshotFrames: 0)))
            sendFullList()
            for bytes in pendingTerminalBytes.removeValue(forKey: value.sessionId) ?? [] {
                publishTerminalBytes(sessionId: value.sessionId, bytes: bytes)
            }
            for event in pendingEvents.removeValue(forKey: value.sessionId) ?? [] {
                send(.event(event))
            }
            if let terminal = record.backend as? SessionProtocolTerminalControlling {
                terminal.resizeTerminal(cols: value.cols, rows: value.rows)
            }
        case let .detach(value):
            if let sessionId = sessionByHandle.removeValue(forKey: value.handle) {
                records[sessionId]?.handles.remove(value.handle)
            }
        case let .resize(value):
            terminal(for: value.handle)?.resizeTerminal(cols: value.cols, rows: value.rows)
        case let .input(value):
            guard let sessionId = sessionByHandle[value.handle],
                  let backend = records[sessionId]?.backend else { return }
            if backend.kind == .codex {
                if value.bytes == [0x1b] { backend.interrupt() }
                else if let text = String(bytes: value.bytes, encoding: .utf8) { backend.send(text) }
            } else {
                (backend as? SessionProtocolTerminalControlling)?.sendRaw(value.bytes)
            }
        case let .control(value):
            handleControl(value)
        case let .ping(value):
            send(.pong(.init(nonce: value.nonce)))
        }
    }

    private func terminal(for handle: UInt32) -> SessionProtocolTerminalControlling? {
        guard let sessionId = sessionByHandle[handle] else { return nil }
        return records[sessionId]?.backend as? SessionProtocolTerminalControlling
    }

    private func handleControl(_ control: SessionControl) {
        guard case let .string(sessionId)? = control.arguments["sessionId"],
              let backend = records[sessionId]?.backend else { return }
        switch control.op {
        case "stop": backend.stop()
        case "clearQuotaHealth": backend.clearQuotaHealth()
        case "applyProfileSwitch":
            guard let requestId = control.requestId,
                  case let .string(knobRaw)? = control.arguments["knob"],
                  case let .string(value)? = control.arguments["value"],
                  let knob = SessionProfileKnob(rawValue: knobRaw) else { return }
            Task { @MainActor [weak self] in
                let result = await backend.applyProfileSwitch(.init(knob: knob, value: value))
                self?.send(.event(result.protocolEvent(requestId: requestId)))
            }
        case "screenText":
            guard let requestId = control.requestId,
                  case let .number(rawMaxLines)? = control.arguments["maxLines"] else { return }
            let maxLines = max(0, Int(rawMaxLines))
            let text = (backend as? SessionProtocolScreenTextProviding)?
                .screenText(maxLines: maxLines) ?? ""
            send(.event(.init(kind: "screenTextResult", requestId: requestId, fields: [
                "sessionId": .string(sessionId), "text": .string(text),
            ])))
        case "updateApprovalsReviewer":
            guard let requestId = control.requestId,
                  case let .string(raw)? = control.arguments["reviewer"],
                  let reviewer = CodexProtocol.ApprovalsReviewer(rawValue: raw) else { return }
            guard let approval = backend as? SessionProtocolApprovalControlling else {
                send(.event(.controlResult(
                    kind: "approvalModeResult", requestId: requestId,
                    sessionId: sessionId, error: "backend 不支持审批模式切换")))
                return
            }
            Task { @MainActor [weak self] in
                do {
                    try await approval.updateProtocolApprovalsReviewer(reviewer)
                    self?.send(.event(.controlResult(
                        kind: "approvalModeResult", requestId: requestId,
                        sessionId: sessionId, error: nil)))
                } catch {
                    self?.send(.event(.controlResult(
                        kind: "approvalModeResult", requestId: requestId,
                        sessionId: sessionId, error: error.localizedDescription)))
                }
            }
        default:
            break // §4.4: unknown op is an additive capability, ignore without disconnecting.
        }
    }

    private func publishState(
        sessionId: String, mutate: (inout SessionProtocolState) -> Void = { _ in }
    ) {
        guard let record = records[sessionId] else { return }
        record.stateSeq &+= 1
        var state = makeState(record.backend, launchParameterProblem: record.launchParameterProblem)
        mutate(&state)
        send(.state(.init(sessionId: sessionId, stateSeq: record.stateSeq,
                          delta: state)))
    }

    private func sendFullList() {
        let summaries = records.keys.sorted().compactMap { sessionId -> SessionSummary? in
            guard let record = records[sessionId] else { return nil }
            return .init(sessionId: sessionId, stateSeq: record.stateSeq,
                         state: makeState(record.backend,
                                          launchParameterProblem: record.launchParameterProblem))
        }
        send(.sessions(.init(sessions: summaries)))
    }

    private func makeState(
        _ backend: any SessionBackend,
        launchParameterProblem: SessionLaunchParameterProblem? = nil
    ) -> SessionProtocolState {
        .init(status: .init(backend.status), isWorking: backend.isWorking,
              displayIsTyping: backend.displayIsTyping,
              health: backend.health.map(SessionHealthWire.init),
              pendingDecision: backend.pendingDecision.map {
                  .init(prompt: $0.prompt, options: $0.options)
              }, kind: backend.kind.rawValue,
              launchParameterProblem: launchParameterProblem.map(SessionLaunchParameterProblemWire.init),
              scrollState: nil)
    }

    private func send(_ message: SessionDaemonMessage) {
        guard let data = try? codec.encode(message) else { return }
        transport.sendFromDaemon(data)
    }
}

@MainActor
final class InProcessSessionProtocolClient {
    private let transport: InProcessTransport
    private let codec = SessionProtocolCodec()
    private let capabilities: [String]
    private var remotes: [String: RemoteSessionBackend] = [:]
    private var remoteByHandle: [UInt32: RemoteSessionBackend] = [:]
    private var negotiated: [String] = []
    private var pendingProfile: [String: CheckedContinuation<SessionProfileSwitchOutcome, Never>] = [:]
    private var synchronousResponses: [String: SessionEvent] = [:]
    private var pendingControls: [String: (Result<Void, Error>) -> Void] = [:]
    private lazy var stateReconciler = SessionStateReconciler(
        requestFullList: { [weak self] in self?.send(.listSessions) },
        apply: { [weak self] sessionId, _, state in
            self?.remotes[sessionId]?.apply(state: state)
        })

    init(transport: InProcessTransport, capabilities: [String]) {
        self.transport = transport
        self.capabilities = capabilities
        transport.receiveFromDaemon = { [weak self] data in
            MainActor.assumeIsolated { self?.receive(data) }
        }
    }

    func connect() {
        send(.hello(.init(protocolVersion: 1, appBuild: "in-process", capabilities: capabilities)))
    }

    func transportDisconnected() {
        stateReconciler.resetForReconnect()
        remoteByHandle.removeAll()
        negotiated = []
        remotes.values.forEach { $0.transportDisconnected() }
    }

    func reconnect() {
        connect()
        send(.listSessions)
        for sessionId in remotes.keys.sorted() {
            send(.attach(.init(sessionId: sessionId, cols: 80, rows: 25)))
        }
    }

    func attach(sessionId: String, kind: LocalCodingAgentKind) -> RemoteSessionBackend {
        let remote = RemoteSessionBackend(sessionId: sessionId, kind: kind, client: self)
        remotes[sessionId] = remote
        remote.updateConnection(capabilities: negotiated)
        send(.attach(.init(sessionId: sessionId, cols: 80, rows: 25)))
        return remote
    }

    func sendInput(handle: UInt32, bytes: [UInt8]) {
        send(.input(.init(handle: handle, bytes: bytes)))
    }

    func resize(handle: UInt32, cols: Int, rows: Int) {
        send(.resize(.init(handle: handle, cols: cols, rows: rows)))
    }

    func sendControl(sessionId: String, op: String,
                     arguments: [String: SessionWireJSONValue] = [:]) {
        var arguments = arguments
        arguments["sessionId"] = .string(sessionId)
        send(.control(.init(requestId: nil, op: op, arguments: arguments)))
    }

    func applyProfileSwitch(
        sessionId: String, command: SessionProfileSwitchCommand
    ) async -> SessionProfileSwitchOutcome {
        let requestId = UUID().uuidString
        return await withCheckedContinuation { continuation in
            pendingProfile[requestId] = continuation
            send(.control(.init(requestId: requestId, op: "applyProfileSwitch", arguments: [
                "sessionId": .string(sessionId), "knob": .string(command.knob.rawValue),
                "value": .string(command.value),
            ])))
        }
    }

    func screenText(sessionId: String, maxLines: Int) -> String? {
        let requestId = UUID().uuidString
        send(.control(.init(requestId: requestId, op: "screenText", arguments: [
            "sessionId": .string(sessionId), "maxLines": .number(Double(maxLines)),
        ])))
        guard let event = synchronousResponses.removeValue(forKey: requestId),
              case let .string(text)? = event.fields["text"] else { return nil }
        return text
    }

    func updateApprovalsReviewer(
        sessionId: String, reviewer: CodexProtocol.ApprovalsReviewer
    ) async throws {
        let requestId = UUID().uuidString
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            pendingControls[requestId] = { continuation.resume(with: $0) }
            send(.control(.init(requestId: requestId, op: "updateApprovalsReviewer", arguments: [
                "sessionId": .string(sessionId), "reviewer": .string(reviewer.rawValue),
            ])))
        }
    }

    private func receive(_ data: Data) {
        guard let message = try? codec.decodeDaemon(data) else { return }
        switch message {
        case let .hello(value):
            guard case let .compatible(caps) = SessionCompatibility.evaluate(
                appProtocolVersion: 1, daemonProtocolVersion: value.protocolVersion,
                appCapabilities: capabilities, daemonCapabilities: value.capabilities) else { return }
            negotiated = caps
            remotes.values.forEach { $0.updateConnection(capabilities: caps) }
        case let .sessions(value):
            for summary in value.sessions {
                guard remotes[summary.sessionId] != nil else { continue }
                stateReconciler.receiveFull(summary)
            }
        case let .attached(value):
            guard let remote = remotes[value.sessionId] else { return }
            remote.attach(handle: value.handle)
            remoteByHandle[value.handle] = remote
        case let .state(value):
            guard remotes[value.sessionId] != nil else { return }
            stateReconciler.receiveDelta(
                sessionId: value.sessionId, stateSeq: value.stateSeq, state: value.delta)
        case let .data(value): remoteByHandle[value.handle]?.receiveTerminal(value.bytes)
        case let .event(value):
            if value.kind == "profileSwitchResult", let requestId = value.requestId,
               let continuation = pendingProfile.removeValue(forKey: requestId) {
                continuation.resume(returning: .init(protocolEvent: value))
            } else if value.kind == "approvalModeResult", let requestId = value.requestId,
                      let completion = pendingControls.removeValue(forKey: requestId) {
                if case let .string(error)? = value.fields["error"] {
                    completion(.failure(SessionProtocolControlError.failed(error)))
                } else {
                    completion(.success(()))
                }
            } else if value.kind == "screenTextResult", let requestId = value.requestId {
                synchronousResponses[requestId] = value
            } else {
                if case let .string(sessionId)? = value.fields["sessionId"] {
                    remotes[sessionId]?.receiveEvent(value)
                }
            }
        case .resync:
            send(.listSessions)
        case .pong:
            break
        }
    }

    private func send(_ message: SessionAppMessage) {
        guard let data = try? codec.encode(message) else { return }
        transport.sendFromApp(data)
    }
}

private extension SessionWireStatus {
    init(_ status: SessionStatus) {
        switch status { case .running: self = .running; case let .exited(code): self = .exited(code) }
    }
    var sessionStatus: SessionStatus {
        switch self { case .running: return .running; case let .exited(code): return .exited(code) }
    }
}

private extension SessionHealthWire {
    init(_ health: CrewSessionHealth) { self.init(kind: health.kind.rawValue, detail: health.detail) }
    var health: CrewSessionHealth? {
        guard let kind = CrewSessionHealth.Kind(rawValue: kind) else { return nil }
        return .init(kind: kind, detail: detail)
    }
}

private extension SessionLaunchParameterProblemWire {
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

private extension SessionEvent {
    static func controlResult(
        kind: String, requestId: String, sessionId: String, error: String?
    ) -> SessionEvent {
        var fields: [String: SessionWireJSONValue] = ["sessionId": .string(sessionId)]
        if let error { fields["error"] = .string(error) }
        return .init(kind: kind, requestId: requestId, fields: fields)
    }
}

private extension SessionProfileSwitchOutcome {
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

private extension SessionWireJSONValue {
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
