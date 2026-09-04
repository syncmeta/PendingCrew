#if os(macOS)
import Combine
import Foundation

/// 协议的两端（spec `docs/internal/2026-08-19-backend-split-design.md` §4）。
///
/// P2 时这两个类型长在 `RemoteSessionBackend.swift` 里、且写死了 `InProcessTransport`。
/// P4 把它们搬出来并改成认 `SessionMessageLink` —— **同一段服务端代码，既服务同进程
/// 的那条链路，也服务 socket 上的 N 个 viewer**。§10 说的「同一套代码、两种传输」
/// 在这里是字面成立的：下面没有一处 `if 是不是 socket`，只有一处
/// `link.isSynchronous`，而它问的是「这条链路会不会跟不上」，不是「你是谁」。

/// daemon 侧：session 的唯一所有者，对 N 条链路提供协议服务。
@MainActor
final class SessionProtocolServer {
    /// 一条链路上的会话态。handle 空间是服务端全局的（`nextHandle` 单调递增），
    /// 所以不同 viewer 之间的 handle 天然不撞。
    private final class Connection {
        let link: any SessionMessageLink
        /// 每条连接各自推进：TCP/TLS/UDS 的 read 边界都不是消息边界。
        var frameDecoder = SessionFrameDecoder()
        var negotiatedCapabilities: [String] = []
        var handles: Set<UInt32> = []
        /// 只有**跟得上跟不上是个问题**的链路才需要背压队列（§5.4）。同进程直调
        /// 那条链路永远不会落后一拍——给它排队等于凭空造一个不存在的中间态。
        var queues: [UInt32: SessionAttachQueue] = [:]
        var pumpScheduled = false
        init(link: any SessionMessageLink) { self.link = link }
    }

    private final class Record {
        let backend: any SessionBackend
        var stateSeq: UInt64 = 0
        var handles: Set<UInt32> = []
        var observations: Set<AnyCancellable> = []
        var launchParameterProblem: SessionLaunchParameterProblem?
        init(backend: any SessionBackend) { self.backend = backend }
    }

    /// socket 上「已经交给内核但还没写出去」的高水位。超过它就先不再往下灌，
    /// 让积压落在 `SessionAttachQueue` 里 —— 那里才有「宁可重同步、绝不丢中间
    /// 一段」的正确处理，而 socket 缓冲区里没有。
    static let socketWriteHighWaterBytes = 1024 * 1024

    private let codec = SessionProtocolCodec()
    private let capabilities: [String]
    private let daemonBuild: String
    private let startedAt: Double
    private var records: [String: Record] = [:]
    private var connections: [ObjectIdentifier: Connection] = [:]
    private var sessionByHandle: [UInt32: String] = [:]
    private var connectionByHandle: [UInt32: ObjectIdentifier] = [:]
    private var nextHandle: UInt32 = 1
    private var pendingTerminalBytes: [String: [[UInt8]]] = [:]
    private var pendingEvents: [String: [SessionEvent]] = [:]

    /// 让 daemon 侧把 attach/detach/重同步记进滚动日志（§8.5）。
    var onDiagnostic: ((String) -> Void)?
    /// 这个 session 的编排身份（属于哪个 crew、机长还是 worker、当前档位…）。
    /// daemon 由编排层填；同进程桥不填 —— 那边 app 自己就持有 run。
    var runSummaryProvider: ((String) -> SessionRunSummary?)?
    /// viewer 发来的编排请求（起 session / 停 / 移除 / 切档位）。daemon 接上编排层；
    /// 同进程桥不接（那边 UI 直接调 runner）。
    var onOrchestrationRequest: ((SessionControl) -> Void)?

    var connectionCount: Int { connections.count }
    var sessionCount: Int { records.count }

    init(capabilities: [String], daemonBuild: String = "in-process",
         startedAt: Double = Date().timeIntervalSince1970) {
        self.capabilities = capabilities
        self.daemonBuild = daemonBuild
        self.startedAt = startedAt
    }

    // MARK: - 链路

    func accept(link: any SessionMessageLink) {
        let connection = Connection(link: link)
        let key = ObjectIdentifier(link)
        connections[key] = connection
        link.onReceive = { [weak self] data in
            MainActor.assumeIsolated { self?.receive(data, from: key) }
        }
        link.onClose = { [weak self] in
            MainActor.assumeIsolated { self?.dropConnection(key) }
        }
        onDiagnostic?("viewer 连入（当前 \(connections.count) 条）")
    }

    /// 对端消失 → 该 viewer 的 handle 全作废、停止推字节。**session 照常跑。**
    func dropConnection(_ key: ObjectIdentifier) {
        guard let connection = connections.removeValue(forKey: key) else { return }
        for handle in connection.handles {
            if let sessionId = sessionByHandle.removeValue(forKey: handle) {
                records[sessionId]?.handles.remove(handle)
            }
            connectionByHandle.removeValue(forKey: handle)
        }
        onDiagnostic?("viewer 断开（剩 \(connections.count) 条）；session 不受影响")
    }

    // MARK: - session 注册

    func register(sessionId: String, backend: any SessionBackend) {
        let record = Record(backend: backend)
        records[sessionId] = record
        observe(sessionId: sessionId, record: record)
    }

    func unregister(sessionId: String) {
        guard let record = records.removeValue(forKey: sessionId) else { return }
        for handle in record.handles {
            sessionByHandle.removeValue(forKey: handle)
            if let key = connectionByHandle.removeValue(forKey: handle) {
                connections[key]?.handles.remove(handle)
                connections[key]?.queues.removeValue(forKey: handle)
            }
        }
    }

    func backend(for sessionId: String) -> (any SessionBackend)? { records[sessionId]?.backend }

    // MARK: - 出站

    func publishTerminalBytes(sessionId: String, bytes: [UInt8]) {
        guard let record = records[sessionId] else { return }
        // 已注册但无人 attach 时按 §4.5 丢实时流；重连靠快照恢复，不重放增量。
        guard !record.handles.isEmpty else { return }
        for handle in record.handles.sorted() {
            guard let key = connectionByHandle[handle],
                  let connection = connections[key] else { continue }
            if let queue = connection.queues[handle] {
                queue.append(bytes)
                schedulePump(connection)
            } else {
                send(.data(.init(handle: handle, bytes: bytes)), on: connection)
            }
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
        broadcastToAttached(sessionId: sessionId, message: .event(event))
    }

    /// 主动往每条正在看的链路推一条事件（审批卡片之类）。
    func broadcastToAttached(sessionId: String, message: SessionDaemonMessage) {
        guard let record = records[sessionId] else { return }
        var seen: Set<ObjectIdentifier> = []
        for handle in record.handles.sorted() {
            guard let key = connectionByHandle[handle], seen.insert(key).inserted,
                  let connection = connections[key] else { continue }
            send(message, on: connection)
        }
    }

    // MARK: - 状态观察

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

    // MARK: - 入站

    private func receive(_ data: Data, from key: ObjectIdentifier) {
        guard let connection = connections[key] else { return }
        do {
            for frame in try connection.frameDecoder.append(data) {
                guard let message = try codec.decodeApp(frame) else { continue }
                receive(message, on: connection)
            }
        } catch {
            onDiagnostic?("协议字节流损坏，断开 viewer：\(error)")
            connection.link.close()
            dropConnection(key)
        }
    }

    private func receive(_ message: SessionAppMessage, on connection: Connection) {
        switch message {
        case let .hello(value):
            connection.negotiatedCapabilities = SessionCapabilities.negotiate(
                app: value.capabilities, daemon: capabilities)
            send(.hello(.init(protocolVersion: SessionProtocolVersion.current,
                              daemonBuild: daemonBuild,
                              capabilities: capabilities, sessionCount: records.count,
                              pid: Int32(ProcessInfo.processInfo.processIdentifier),
                              viewerCount: connections.count,
                              startedAt: startedAt)),
                 on: connection)
            onDiagnostic?("握手：app build=\(value.appBuild) protocol=\(value.protocolVersion)"
                + " 协商能力=\(connection.negotiatedCapabilities.joined(separator: ","))")
        case .listSessions:
            sendFullList(on: connection)
        case let .attach(value):
            attach(value, on: connection)
        case let .detach(value):
            detach(handle: value.handle, on: connection)
        case let .resize(value):
            terminal(for: value.handle)?.resizeTerminal(cols: value.cols, rows: value.rows)
        case let .input(value):
            guard connection.handles.contains(value.handle),
                  let sessionId = sessionByHandle[value.handle],
                  let backend = records[sessionId]?.backend else { return }
            if backend.kind == .codex {
                if value.bytes == [0x1b] { backend.interrupt() }
                else if let text = String(bytes: value.bytes, encoding: .utf8) { backend.send(text) }
            } else {
                (backend as? SessionProtocolTerminalControlling)?.sendRaw(value.bytes)
            }
        case let .control(value):
            if SessionOrchestrationOp.isOrchestration(value.op) {
                handleOrchestration(value)
            } else {
                handleControl(value, on: connection)
            }
        case let .ping(value):
            send(.pong(.init(nonce: value.nonce)), on: connection)
        }
    }

    private func attach(_ value: SessionAttach, on connection: Connection) {
        guard let record = records[value.sessionId] else { return }
        let handle = nextHandle
        nextHandle &+= 1
        record.handles.insert(handle)
        connection.handles.insert(handle)
        sessionByHandle[handle] = value.sessionId
        connectionByHandle[handle] = ObjectIdentifier(connection.link)
        // kind=2 不重复带尺寸；attach 自己的 cols/rows 就是这份快照的尺寸上下文。
        // 必须先 resize 权威终端再拍，否则首屏会按旧宽度序列化。
        //
        // **0 = 「我还不知道自己多大，别动你的尺寸」**（§5.5）。真分家之后 viewer
        // 是在窗口布局出来**之前**就要连上并拉 roster 的；那时报一个默认 80×25，
        // 等于每次重开 app 都把正在跑的 TUI 先按 80 列重排一次、给 agent 发一次
        // SIGWINCH，然后再排回去。没有窗口在看时保持最后一次的尺寸才是对的。
        if value.cols > 0, value.rows > 0,
           let terminal = record.backend as? SessionProtocolTerminalControlling {
            terminal.resizeTerminal(cols: value.cols, rows: value.rows)
        }
        let wantsSnapshot = record.backend.kind != .codex
            && connection.negotiatedCapabilities.contains("terminal-bytes")
        let snapshotProvider = record.backend as? SessionProtocolTerminalSnapshotProviding

        if wantsSnapshot, !connection.link.isSynchronous, let snapshotProvider {
            // 会跟不上的链路：快照与实时流全部过 `SessionAttachQueue`，由它保证
            // 「要么完整的字节流，要么一份干净的快照重来」。
            let queue = SessionAttachQueue {
                snapshotProvider.protocolTerminalSnapshot()
                    ?? .init(cols: value.cols, rows: value.rows, bytes: [])
            }
            queue.begin()
            connection.queues[handle] = queue
            send(.attached(.init(sessionId: value.sessionId, handle: handle,
                                 snapshotFrames: 0)), on: connection)
            schedulePump(connection)
        } else {
            let snapshotFrames: [SessionWireFrame]
            if wantsSnapshot, let snapshot = snapshotProvider?.protocolTerminalSnapshot() {
                snapshotFrames = SessionFrameEncoder.snapshotFrames(
                    handle: handle, serializedBytes: snapshot.bytes)
            } else {
                snapshotFrames = []
            }
            // advisory only：终止一律看每个 kind=2 帧自己的 isLast。
            send(.attached(.init(sessionId: value.sessionId, handle: handle,
                                 snapshotFrames: UInt32(snapshotFrames.count))), on: connection)
            for frame in snapshotFrames { send(frame, on: connection) }
        }

        if record.backend.kind == .codex,
           connection.negotiatedCapabilities.contains("transcript-events"),
           let history = record.backend as? SessionProtocolCodexHistoryProviding {
            for item in history.protocolCodexHistory {
                send(.event(.init(kind: "codexNotification", requestId: nil, fields: [
                    "sessionId": .string(value.sessionId),
                    "method": .string("item/completed"),
                    "params": .object(["item": .object(item.protocolWireItem)]),
                ])), on: connection)
            }
        }
        sendFullList(on: connection)
        for bytes in pendingTerminalBytes.removeValue(forKey: value.sessionId) ?? [] {
            publishTerminalBytes(sessionId: value.sessionId, bytes: bytes)
        }
        for event in pendingEvents.removeValue(forKey: value.sessionId) ?? [] {
            send(.event(event), on: connection)
        }
        onDiagnostic?("attach \(value.sessionId) → handle \(handle)"
            + "（\(value.cols)×\(value.rows)）")
    }

    private func detach(handle: UInt32, on connection: Connection) {
        guard connection.handles.remove(handle) != nil else { return }
        connection.queues.removeValue(forKey: handle)
        connectionByHandle.removeValue(forKey: handle)
        if let sessionId = sessionByHandle.removeValue(forKey: handle) {
            records[sessionId]?.handles.remove(handle)
            onDiagnostic?("detach \(sessionId)（handle \(handle)）；session 照跑")
        }
    }

    private func terminal(for handle: UInt32) -> SessionProtocolTerminalControlling? {
        guard let sessionId = sessionByHandle[handle] else { return nil }
        return records[sessionId]?.backend as? SessionProtocolTerminalControlling
    }

    private func handleControl(_ control: SessionControl, on connection: Connection) {
        guard case let .string(sessionId)? = control.arguments["sessionId"],
              let backend = records[sessionId]?.backend else { return }
        switch control.op {
        case "stop": backend.stop()
        case "clearQuotaHealth": backend.clearQuotaHealth()
        case "submitWake":
            guard let requestId = control.requestId,
                  case let .string(text)? = control.arguments["text"] else { return }
            Task { @MainActor [weak self, weak connection] in
                let result = await backend.submitWake(text)
                guard let self, let connection else { return }
                self.send(.event(.init(
                    kind: "wakeSubmitResult", requestId: requestId,
                    fields: [
                        "sessionId": .string(sessionId),
                        "result": .string(result == .accepted ? "accepted" : "retry"),
                    ])), on: connection)
            }
        case "applyProfileSwitch":
            guard let requestId = control.requestId,
                  case let .string(knobRaw)? = control.arguments["knob"],
                  case let .string(value)? = control.arguments["value"],
                  let knob = SessionProfileKnob(rawValue: knobRaw) else { return }
            Task { @MainActor [weak self, weak connection] in
                let result = await backend.applyProfileSwitch(.init(knob: knob, value: value))
                guard let self, let connection else { return }
                self.send(.event(result.protocolEvent(requestId: requestId)), on: connection)
            }
        case "screenText":
            guard let requestId = control.requestId,
                  case let .number(rawMaxLines)? = control.arguments["maxLines"] else { return }
            let maxLines = max(0, Int(rawMaxLines))
            let text = (backend as? SessionProtocolScreenTextProviding)?
                .screenText(maxLines: maxLines) ?? ""
            send(.event(.init(kind: "screenTextResult", requestId: requestId, fields: [
                "sessionId": .string(sessionId), "text": .string(text),
            ])), on: connection)
        case "updateApprovalsReviewer":
            guard let requestId = control.requestId,
                  case let .string(raw)? = control.arguments["reviewer"],
                  let reviewer = CodexProtocol.ApprovalsReviewer(rawValue: raw) else { return }
            guard let approval = backend as? SessionProtocolApprovalControlling else {
                send(.event(.controlResult(
                    kind: "approvalModeResult", requestId: requestId,
                    sessionId: sessionId, error: "backend 不支持审批模式切换")), on: connection)
                return
            }
            Task { @MainActor [weak self, weak connection] in
                do {
                    try await approval.updateProtocolApprovalsReviewer(reviewer)
                    guard let self, let connection else { return }
                    self.send(.event(.controlResult(
                        kind: "approvalModeResult", requestId: requestId,
                        sessionId: sessionId, error: nil)), on: connection)
                } catch {
                    guard let self, let connection else { return }
                    self.send(.event(.controlResult(
                        kind: "approvalModeResult", requestId: requestId,
                        sessionId: sessionId, error: error.localizedDescription)), on: connection)
                }
            }
        default:
            break // §4.4: unknown op is an additive capability, ignore without disconnecting.
        }
    }

    /// 不带 sessionId 的编排请求（起一个**还不存在**的 session、刷 roster）。
    /// `handleControl` 那条要求先找得到 backend，这类请求按定义找不到。
    private func handleOrchestration(_ control: SessionControl) {
        onOrchestrationRequest?(control)
    }

    // MARK: - 背压泵

    private func schedulePump(_ connection: Connection) {
        guard !connection.pumpScheduled else { return }
        connection.pumpScheduled = true
        DispatchQueue.main.async { [weak self, weak connection] in
            MainActor.assumeIsolated {
                guard let self, let connection else { return }
                connection.pumpScheduled = false
                self.pump(connection)
            }
        }
    }

    private func pump(_ connection: Connection) {
        guard connections[ObjectIdentifier(connection.link)] != nil,
              connection.link.isOpen else { return }
        var moreWork = false
        for (handle, queue) in connection.queues {
            while true {
                if connection.link.pendingWriteBytes >= Self.socketWriteHighWaterBytes {
                    // 灌不动了。积压留在 queue 里 —— 那里超限会重同步，socket 缓冲
                    // 区里超限只会无限膨胀。
                    moreWork = true
                    break
                }
                guard let frame = queue.next() else { break }
                switch frame {
                case .resync:
                    send(.resync(.init(handle: handle)), on: connection)
                    onDiagnostic?("重同步 handle \(handle)"
                        + "（累计 \(queue.diagnostics.resyncCount) 次）")
                case let .snapshot(seq, isLast, _, _, bytes):
                    send(.snapshot(handle: handle, seq: UInt32(seq),
                                   isLast: isLast, bytes: bytes), on: connection)
                case let .live(bytes):
                    send(.data(.init(handle: handle, bytes: bytes)), on: connection)
                }
            }
            if !queue.isEmpty { moreWork = true }
        }
        if moreWork { schedulePump(connection) }
    }

    // MARK: -

    private func publishState(
        sessionId: String, mutate: (inout SessionProtocolState) -> Void = { _ in }
    ) {
        guard let record = records[sessionId] else { return }
        record.stateSeq &+= 1
        var state = makeState(record.backend, launchParameterProblem: record.launchParameterProblem)
        mutate(&state)
        let message = SessionDaemonMessage.state(
            .init(sessionId: sessionId, stateSeq: record.stateSeq, delta: state))
        // 状态是**每个 session 一条单调序列**，所以每条链路都要收到同一条 delta ——
        // 只发给「正在看」的那条会让别的 viewer 的 stateSeq 出现跳号，然后它就会
        // 无谓地拉一次全量。
        for connection in connections.values { send(message, on: connection) }
    }

    /// roster 变了 → 全量推给每一条链路。
    func broadcastSessionList() {
        for connection in connections.values { sendFullList(on: connection) }
    }

    private func sendFullList(on connection: Connection) {
        let summaries = records.keys.sorted().compactMap { sessionId -> SessionSummary? in
            guard let record = records[sessionId] else { return nil }
            return .init(sessionId: sessionId, stateSeq: record.stateSeq,
                         state: makeState(record.backend,
                                          launchParameterProblem: record.launchParameterProblem),
                         run: runSummaryProvider?(sessionId))
        }
        send(.sessions(.init(sessions: summaries)), on: connection)
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

    private func send(_ message: SessionDaemonMessage, on connection: Connection) {
        guard let data = try? codec.encode(message) else { return }
        connection.link.send(data)
    }

    private func send(_ frame: SessionWireFrame, on connection: Connection) {
        guard let data = try? SessionFrameEncoder.encode(frame) else { return }
        connection.link.send(data)
    }
}

/// app 侧：`SessionBackend` 的第三个实现（`RemoteSessionBackend`）背后的那条链路。
@MainActor
final class SessionProtocolClient {
    private let link: any SessionMessageLink
    private let codec = SessionProtocolCodec()
    /// 连接级增量状态；read/write 回调边界不是帧边界。
    private var frameDecoder = SessionFrameDecoder()
    private let capabilities: [String]
    private let appBuild: String
    private var remotes: [String: RemoteSessionBackend] = [:]
    private var remoteByHandle: [UInt32: RemoteSessionBackend] = [:]
    private var negotiated: [String] = []
    private var pendingProfile: [String: CheckedContinuation<SessionProfileSwitchOutcome, Never>] = [:]
    private var pendingWakes: [String: CheckedContinuation<SessionWakeSubmission, Never>] = [:]
    private var synchronousResponses: [String: SessionEvent] = [:]
    private var pendingControls: [String: (Result<Void, Error>) -> Void] = [:]
    private lazy var stateReconciler = SessionStateReconciler(
        requestFullList: { [weak self] in self?.send(.listSessions) },
        apply: { [weak self] sessionId, _, state in
            self?.remotes[sessionId]?.apply(state: state)
        })

    /// 链路断了（socket 被对端关掉 / daemon 走了）。app 侧的重连策略挂在这里。
    var onLinkClosed: (() -> Void)?
    /// 全量列表到达。viewer 侧的 roster 镜像消费它。
    var onSessionList: ((SessionList) -> Void)?
    /// P5 `--daemon-status` 与版本横幅消费完整握手事实，不从本地 build 猜。
    var onDaemonHello: ((SessionDaemonHello) -> Void)?
    /// 没有对应 `RemoteSessionBackend` 的事件（viewer 侧 roster 之类）。
    var onUnroutedEvent: ((SessionEvent) -> Void)?

    /// 这条链路是否支持同一调用栈内的请求/应答。`screenText` 那条同步问答只在
    /// true 时成立 —— socket 上必须降级，不能假装读到了。
    var supportsSynchronousRequests: Bool { link.isSynchronous }
    private(set) var isConnected = false
    var negotiatedCapabilities: [String] { negotiated }

    init(link: any SessionMessageLink, capabilities: [String], appBuild: String = "in-process") {
        self.link = link
        self.capabilities = capabilities
        self.appBuild = appBuild
        link.onReceive = { [weak self] data in
            MainActor.assumeIsolated { self?.receive(data) }
        }
        link.onClose = { [weak self] in
            MainActor.assumeIsolated {
                self?.transportDisconnected()
                self?.onLinkClosed?()
            }
        }
    }

    func connect() {
        send(.hello(.init(protocolVersion: SessionProtocolVersion.current,
                          appBuild: appBuild, capabilities: capabilities)))
    }

    func transportDisconnected() {
        frameDecoder = SessionFrameDecoder()
        stateReconciler.resetForReconnect()
        remoteByHandle.removeAll()
        negotiated = []
        isConnected = false
        remotes.values.forEach { $0.transportDisconnected() }
    }

    func reconnect() {
        connect()
        send(.listSessions)
        for sessionId in remotes.keys.sorted() {
            guard let remote = remotes[sessionId] else { continue }
            send(.attach(.init(
                sessionId: sessionId,
                cols: remote.requestedTerminalSize.cols,
                rows: remote.requestedTerminalSize.rows)))
        }
    }

    func requestSessionList() { send(.listSessions) }

    /// viewer → daemon 的编排请求（起 / 停 / 移除 / 发文本 / 切档位）。
    func sendOrchestration(op: String, arguments: [String: SessionWireJSONValue]) {
        send(.control(.init(requestId: nil, op: op, arguments: arguments)))
    }

    /// §4.5 的心跳。`onLinkClosed` 只在**对端真的关了 socket** 时才响；半开连接
    /// （对端机器挂了、链路默默断了）没有 FIN，socket 会一直「open」着而一个字节
    /// 都不来。所以还要有这一层：10 秒一 ping，30 秒收不到 pong 就当断了。
    func noteHeartbeat() { lastPongAt = Date() }
    private(set) var lastPongAt = Date()
    func ping() { send(.ping(.init(nonce: nil))) }

    /// `announceViewport: false` = 「我还不知道自己多大」（见服务端 `attach` 那段）。
    /// 同进程桥照旧报默认视口 —— 那边 attach 时 core 本来就停在同一个默认值上。
    func attach(sessionId: String, kind: LocalCodingAgentKind,
                announceViewport: Bool = true) -> RemoteSessionBackend {
        let remote = RemoteSessionBackend(sessionId: sessionId, kind: kind, client: self)
        remotes[sessionId] = remote
        remote.updateConnection(capabilities: negotiated)
        send(.attach(.init(
            sessionId: sessionId,
            cols: announceViewport ? remote.requestedTerminalSize.cols : 0,
            rows: announceViewport ? remote.requestedTerminalSize.rows : 0)))
        return remote
    }

    func remote(for sessionId: String) -> RemoteSessionBackend? { remotes[sessionId] }

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

    func submitWake(sessionId: String, text: String) async -> SessionWakeSubmission {
        let requestId = UUID().uuidString
        return await withCheckedContinuation { continuation in
            pendingWakes[requestId] = continuation
            send(.control(.init(requestId: requestId, op: "submitWake", arguments: [
                "sessionId": .string(sessionId), "text": .string(text),
            ])))
            Task { @MainActor [weak self] in
                try? await Task.sleep(nanoseconds: 5_000_000_000)
                self?.pendingWakes.removeValue(forKey: requestId)?.resume(returning: .retry)
            }
        }
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

    /// **只在同步链路上成立。** socket 上没有「同一调用栈里拿到回应」这回事，
    /// 那时返回 nil，由调用方明说自己读不到，而不是编一段空白当成画面。
    func screenText(sessionId: String, maxLines: Int) -> String? {
        guard link.isSynchronous else { return nil }
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
        do {
            for frame in try frameDecoder.append(data) { receive(frame) }
        } catch {
            link.close()
            transportDisconnected()
            onLinkClosed?()
        }
    }

    private func receive(_ frame: SessionWireFrame) {
        if case let .snapshot(handle, seq, isLast, bytes) = frame {
            remoteByHandle[handle]?.receiveSnapshot(seq: seq, isLast: isLast, bytes: bytes)
            return
        }
        guard let message = try? codec.decodeDaemon(frame) else { return }
        switch message {
        case let .hello(value):
            guard case let .compatible(caps) = SessionCompatibility.evaluate(
                appProtocolVersion: SessionProtocolVersion.current,
                daemonProtocolVersion: value.protocolVersion,
                appCapabilities: capabilities, daemonCapabilities: value.capabilities) else { return }
            negotiated = caps
            isConnected = true
            onDaemonHello?(value)
            remotes.values.forEach { $0.updateConnection(capabilities: caps) }
        case let .sessions(value):
            for summary in value.sessions {
                guard remotes[summary.sessionId] != nil else { continue }
                stateReconciler.receiveFull(summary)
            }
            onSessionList?(value)
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
            if value.kind == "wakeSubmitResult", let requestId = value.requestId,
               let continuation = pendingWakes.removeValue(forKey: requestId) {
                let result: SessionWakeSubmission
                if case .string("accepted")? = value.fields["result"] {
                    result = .accepted
                } else {
                    result = .retry
                }
                continuation.resume(returning: result)
            } else if value.kind == "profileSwitchResult", let requestId = value.requestId,
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
            } else if case let .string(sessionId)? = value.fields["sessionId"],
                      let remote = remotes[sessionId] {
                remote.receiveEvent(value)
            } else {
                onUnroutedEvent?(value)
            }
        case let .resync(value):
            // 丢掉半截快照的累积再重来一遍 —— 不重置的话，重同步的 seq 0 会撞上
            // 上一份没发完的分片，那条 guard 会把整份新快照静默丢掉。
            remoteByHandle[value.handle]?.beginResync()
            send(.listSessions)
        case .pong:
            lastPongAt = Date()
        }
    }

    private func send(_ message: SessionAppMessage) {
        guard let data = try? codec.encode(message) else { return }
        link.send(data)
    }
}

/// 「session 往哪儿发布」这一个接缝。
///
/// 它存在的理由只有一条：**编排代码（`CrewSessionRunner`）在 inproc 和 daemon 两种
/// 模式下必须是同一份**（§10）。两种模式的差别只有两点，都收在这个协议后面：
///
/// 1. 这个进程里有没有窗口 —— 有窗口才造 `TerminalMirrorView`（AppKit）；
///    daemon 里一个 NSView 都不该有。
/// 2. 这个进程里有没有 viewer —— inproc 时 app 自己就是 viewer，`expose` 回一个
///    `RemoteSessionBackend` 给 run 用；daemon 里 viewer 在另一个进程，回 nil，
///    run 直接持有 direct backend。
@MainActor
protocol SessionProtocolPublishing: AnyObject {
    /// true = 本进程没有窗口（daemon）。后端据此选无画面内核而不是门面。
    var isHeadless: Bool { get }
    func terminalOutputSink(sessionId: String) -> ([UInt8]) -> Void
    func codexNotificationSink(sessionId: String) -> (String, [String: Any]) -> Void
    /// 把 session 交给协议服务端。返回值是「本进程里的 viewer 该拿的那个后端」——
    /// daemon 里没有 viewer，所以是 nil。
    func expose(sessionId: String, backend: any SessionBackend) -> RemoteSessionBackend?
    /// run 被移除 → 服务端也该忘掉它。daemon 一跑就是几周，不忘等于无界增长。
    func retire(sessionId: String)
}

/// daemon 侧的发布口：直接落在 socket 服务端上。
@MainActor
final class DaemonSessionPublisher: SessionProtocolPublishing {
    let isHeadless = true
    private let server: SessionProtocolServer

    init(server: SessionProtocolServer) { self.server = server }

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

    func expose(sessionId: String, backend: any SessionBackend) -> RemoteSessionBackend? {
        server.register(sessionId: sessionId, backend: backend)
        return nil          // 这个进程里没有窗口，没人要 viewer 侧的后端
    }

    func retire(sessionId: String) { server.unregister(sessionId: sessionId) }
}

/// viewer 能请求 daemon 做的编排动作。
///
/// 它们与 `handleControl` 里那批**刻意分开**：那批都是「对某个已经存在的 backend
/// 做点什么」，而这批是「让编排层做点什么」—— 起一个还不存在的 session 按定义
/// 找不到 backend，走那条路会被那句 `guard let backend` 静默丢掉。
enum SessionOrchestrationOp {
    static let startSession = "orchestration.startSession"
    static let stopRun = "orchestration.stopRun"
    static let removeRun = "orchestration.removeRun"
    static let sendText = "orchestration.sendText"
    static let interrupt = "orchestration.interrupt"
    static let profileChange = "orchestration.profileChange"
    static let approvalMode = "orchestration.approvalMode"

    static let all = [startSession, stopRun, removeRun, sendText, interrupt,
                      profileChange, approvalMode]

    static func isOrchestration(_ op: String) -> Bool { op.hasPrefix("orchestration.") }
}

/// 协议版本。**改它需要在 PR 里写明为什么不可避免**（§4.4）——「新增消息」「新增
/// 字段」「新增能力」三种都不许 +1，只有「改帧头布局 / 改字段语义 / 删字段」才算。
enum SessionProtocolVersion {
    static let current = 1
}
#endif
