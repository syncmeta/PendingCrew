#if os(macOS)
import AppKit
import Combine
import Foundation

private let inProcessProtocolCapabilities = [
    "approval-mode", "inspect-output", "launch-parameter-problem", "profile-switch",
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

extension AgentTerminalSession: SessionProtocolTerminalControlling {
    func resizeTerminal(cols: Int, rows: Int) {
        core.resize(cols: cols, rows: rows)
        core.noteViewportChange()
    }
}

extension PlainTerminalSession: SessionProtocolTerminalControlling {
    func sendRaw(_ bytes: [UInt8]) { core.sendRaw(bytes) }
    func resizeTerminal(cols: Int, rows: Int) {
        core.resize(cols: cols, rows: rows)
        core.noteViewportChange()
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
        }
    }

    func supportsCapability(_ capability: String) -> Bool {
        SessionCapabilities.supports(capability, in: negotiatedCapabilities)
    }

    func send(_ text: String) {
        client.sendControl(sessionId: sessionId, op: "sendText",
                           arguments: ["text": .string(text)])
    }

    func interrupt() { client.sendControl(sessionId: sessionId, op: "interrupt") }
    func stop() { client.sendControl(sessionId: sessionId, op: "stop") }
    func clearQuotaHealth() { client.sendControl(sessionId: sessionId, op: "clearQuotaHealth") }

    func applyProfileSwitch(_ cmd: SessionProfileSwitchCommand) async -> SessionProfileSwitchOutcome {
        await client.applyProfileSwitch(sessionId: sessionId, command: cmd)
    }

    func sendRaw(_ bytes: [UInt8]) {
        guard let handle else { return }
        client.sendInput(handle: handle, bytes: bytes)
    }

    func resizeTerminal(cols: Int, rows: Int) {
        guard let handle else { return }
        client.resize(handle: handle, cols: cols, rows: rows)
    }

    fileprivate func attach(handle: UInt32) { self.handle = handle }

    fileprivate func updateConnection(capabilities: [String]) {
        negotiatedCapabilities = capabilities
        isProtocolConnected = true
    }

    fileprivate func apply(state: SessionProtocolState) {
        status = state.status.sessionStatus
        isWorking = state.isWorking
        displayIsTyping = state.displayIsTyping
        health = state.health?.health
        pendingDecision = state.pendingDecision.map {
            PendingTerminalDecision(prompt: $0.prompt, options: $0.options)
        }
    }

    fileprivate func receiveTerminal(_ bytes: [UInt8]) {
        lastTerminalFrameBytes = bytes
        terminalView?.remoteLastOutputAt = Date()
        terminalView?.feedFromCore(bytes[...])
    }

    fileprivate func receiveEvent(_ event: SessionEvent) {
        guard event.kind == "codexNotification",
              case let .string(method)? = event.fields["method"],
              case let .object(params)? = event.fields["params"] else { return }
        transcript?.apply(method: method, params: params.mapValues(\.foundationObject))
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
}

@MainActor
private final class InProcessSessionProtocolServer {
    private final class Record {
        let backend: any SessionBackend
        var stateSeq: UInt64 = 0
        var handles: Set<UInt32> = []
        var observations: Set<AnyCancellable> = []
        init(backend: any SessionBackend) { self.backend = backend }
    }

    private let transport: InProcessTransport
    private let codec = SessionProtocolCodec()
    private let capabilities: [String]
    private var records: [String: Record] = [:]
    private var sessionByHandle: [UInt32: String] = [:]
    private var nextHandle: UInt32 = 1

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
        for handle in record.handles.sorted() {
            send(.data(.init(handle: handle, bytes: bytes)))
        }
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
            terminal(for: value.handle)?.sendRaw(value.bytes)
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
        case "sendText":
            if case let .string(text)? = control.arguments["text"] { backend.send(text) }
        case "interrupt": backend.interrupt()
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
        default:
            break // §4.4: unknown op is an additive capability, ignore without disconnecting.
        }
    }

    private func publishState(
        sessionId: String, mutate: (inout SessionProtocolState) -> Void = { _ in }
    ) {
        guard let record = records[sessionId] else { return }
        record.stateSeq &+= 1
        var state = makeState(record.backend)
        mutate(&state)
        send(.state(.init(sessionId: sessionId, stateSeq: record.stateSeq,
                          delta: state)))
    }

    private func sendFullList() {
        let summaries = records.keys.sorted().compactMap { sessionId -> SessionSummary? in
            guard let record = records[sessionId] else { return nil }
            return .init(sessionId: sessionId, stateSeq: record.stateSeq,
                         state: makeState(record.backend))
        }
        send(.sessions(.init(sessions: summaries)))
    }

    private func makeState(_ backend: any SessionBackend) -> SessionProtocolState {
        .init(status: .init(backend.status), isWorking: backend.isWorking,
              displayIsTyping: backend.displayIsTyping,
              health: backend.health.map(SessionHealthWire.init),
              pendingDecision: backend.pendingDecision.map {
                  .init(prompt: $0.prompt, options: $0.options)
              }, kind: backend.kind.rawValue, launchParameterProblem: nil, scrollState: nil)
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
    private lazy var stateReconciler = SessionStateReconciler(
        requestFullList: { [weak self] in self?.send(.listSessions) },
        apply: { [weak self] _, state in
            guard let self else { return }
            // The reconciler's public callback lacks sessionId by design; routing is
            // installed immediately around each receive below.
            self.stateReceiver?(state)
        })
    private var stateReceiver: ((SessionProtocolState) -> Void)?

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

    func attach(sessionId: String, kind: LocalCodingAgentKind) -> RemoteSessionBackend {
        let remote = RemoteSessionBackend(sessionId: sessionId, kind: kind, client: self)
        remotes[sessionId] = remote
        remote.updateConnection(capabilities: negotiated)
        send(.attach(.init(sessionId: sessionId, cols: 80, rows: 24)))
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
                guard let remote = remotes[summary.sessionId] else { continue }
                stateReceiver = { [weak remote] state in remote?.apply(state: state) }
                stateReconciler.receiveFull(summary)
                stateReceiver = nil
            }
        case let .attached(value):
            guard let remote = remotes[value.sessionId] else { return }
            remote.attach(handle: value.handle)
            remoteByHandle[value.handle] = remote
        case let .state(value):
            guard let remote = remotes[value.sessionId] else { return }
            stateReceiver = { [weak remote] state in remote?.apply(state: state) }
            stateReconciler.receiveDelta(
                sessionId: value.sessionId, stateSeq: value.stateSeq, state: value.delta)
            stateReceiver = nil
        case let .data(value): remoteByHandle[value.handle]?.receiveTerminal(value.bytes)
        case let .event(value):
            if value.kind == "profileSwitchResult", let requestId = value.requestId,
               let continuation = pendingProfile.removeValue(forKey: requestId) {
                continuation.resume(returning: .init(protocolEvent: value))
            } else {
                remotes.values.forEach { $0.receiveEvent(value) }
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
