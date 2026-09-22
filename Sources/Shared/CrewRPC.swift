import Foundation
import Combine

enum CrewRPC {
    static let capability = "crew-rpc-v1"
    static let resultEvent = "crewRPCResult"
    static let whiteboardChangedEvent = "crewWhiteboardChanged"

    enum Op {
        static let listCrews = "crew.listCrews"
        static let chiefLayer = "crew.chiefLayer"
        static let getCrew = "crew.getCrew"
        static let listWhiteboard = "crew.listCrewWhiteboard"
        static let listMembers = "crew.listCrewMembers"
        static let postMessage = "crew.postCrewMessage"
        static let subscribeWhiteboard = "crew.subscribeWhiteboard"

        static func isCrew(_ value: String) -> Bool { value.hasPrefix("crew.") }
    }

    struct CrewID: Codable { var crewId: String }
    struct PostMessage: Codable {
        var crewId: String
        var text: String
        var mentions: [CrewMention]
        var replyToId: String?
        var extraReferences: [CrewMessageReference]
    }
    struct PostResult: Codable { var warning: String? }
}

/// 结构化审批账本的 SessionProtocol RPC。客户端只传 Codable 值；只有 daemon
/// 持有并改写 `LocalApprovalStore`，iOS 从不直接碰 Mac 的 JSON 文件。
enum ApprovalRPC {
    static let capability = "approval-rpc-v1"
    static let resultEvent = "approvalRPCResult"
    static let changedEvent = "approvalChanged"

    enum Op {
        static let list = "approval.list"
        static let subscribe = "approval.subscribe"
        static let answer = "approval.answer"
        static let decide = "approval.decide"
        static func isApproval(_ value: String) -> Bool { value.hasPrefix("approval.") }
    }

    struct Scope: Codable {
        var crewId: String
        var sessionId: String
    }
    struct Answer: Codable {
        var crewId: String
        var sessionId: String
        var approvalId: String
        var reply: String
    }
    struct Decide: Codable {
        var crewId: String
        var sessionId: String
        var approvalId: String
        var decision: String
    }
    struct Empty: Codable {}
}

enum CrewRPCError: LocalizedError, Equatable {
    case notConnected
    case incompatibleServer
    case disconnected(String)
    case timedOut
    case remote(String)
    case invalidResponse
    case unsupported(String)

    var errorDescription: String? {
        switch self {
        case .notConnected: return "远端尚未连接"
        case .incompatibleServer: return "远端 daemon 不支持 crew 数据协议"
        case let .disconnected(reason): return "远端连接已断开：\(reason)"
        case .timedOut: return "远端请求超时"
        case let .remote(reason): return "远端请求失败：\(reason)"
        case .invalidResponse: return "远端返回了损坏的数据"
        case let .unsupported(reason): return reason
        }
    }
}

@MainActor
final class CrewRPCClient {
    private let link: any SessionMessageLink
    private let timeout: TimeInterval
    private let codec = SessionProtocolCodec()
    private var decoder = SessionFrameDecoder()
    private var helloWaiter: CheckedContinuation<Void, Error>?
    private var helloTimeout: Task<Void, Never>?
    private var pending: [String: (Result<Data, Error>) -> Void] = [:]
    private var timeoutTasks: [String: Task<Void, Never>] = [:]
    private var changeHandlers: [UUID: (String) -> Void] = [:]
    private var approvalChangeHandlers: [UUID: (String) -> Void] = [:]
    private var sessionListWaiters: [String: (Result<SessionList, Error>) -> Void] = [:]
    private var sessionListTimeouts: [String: Task<Void, Never>] = [:]
    private var latestSessions = SessionList(sessions: [])
    private var channels: [String: RemoteSessionChannel] = [:]
    private var channelByHandle: [UInt32: RemoteSessionChannel] = [:]
    private let generation: UInt64
    private var terminated = false
    private(set) var connected = false
    var pendingRequestCount: Int { pending.count + sessionListWaiters.count }
    var onDisconnect: ((Error) -> Void)?

    init(link: any SessionMessageLink, timeout: TimeInterval, generation: UInt64 = 0) {
        self.link = link
        self.timeout = timeout
        self.generation = generation
        link.onReceive = { [weak self] data in
            MainActor.assumeIsolated { self?.receive(data) }
        }
        link.onClose = { [weak self] in
            MainActor.assumeIsolated {
                self?.failAll(CrewRPCError.disconnected(
                    self?.link.terminalErrorDescription ?? "网络链路关闭"))
            }
        }
    }

    func connect() async throws {
        guard !connected else { return }
        guard !terminated else { throw CrewRPCError.disconnected("连接已经终止") }
        guard helloWaiter == nil else {
            throw CrewRPCError.disconnected("连接握手已经在进行中")
        }
        try await withCheckedThrowingContinuation { continuation in
            helloWaiter = continuation
            scheduleHelloTimeout()
            send(.hello(.init(protocolVersion: SessionProtocolVersion.current,
                              appBuild: "ios-remote", capabilities: [
                                CrewRPC.capability, ApprovalRPC.capability,
                                "terminal-bytes", "transcript-events",
                              ])))
        }
    }

    func close() {
        link.close()
        failAll(CrewRPCError.disconnected("本端已关闭"))
    }

    func request<Response: Decodable, Arguments: Encodable>(
        op: String, arguments: Arguments, as: Response.Type
    ) async throws -> Response {
        guard connected else { throw CrewRPCError.notConnected }
        let payload = try JSONEncoder().encode(arguments)
        let requestID = UUID().uuidString
        let data: Data = try await withCheckedThrowingContinuation { continuation in
            pending[requestID] = { continuation.resume(with: $0) }
            scheduleRequestTimeout(requestID)
            send(.control(.init(requestId: requestID, op: op, arguments: [
                "payload": .string(payload.base64EncodedString()),
            ])))
        }
        do { return try JSONDecoder().decode(Response.self, from: data) }
        catch { throw CrewRPCError.invalidResponse }
    }

    func request<Response: Decodable>(op: String, as: Response.Type) async throws -> Response {
        try await request(op: op, arguments: Empty(), as: Response.self)
    }

    func subscribe(crewID: String) {
        guard connected, let payload = try? JSONEncoder().encode(CrewRPC.CrewID(crewId: crewID))
        else { return }
        send(.control(.init(requestId: nil, op: CrewRPC.Op.subscribeWhiteboard,
                            arguments: ["payload": .string(payload.base64EncodedString())])))
    }

    func subscribeApprovals(crewID: String, sessionID: String) async throws {
        let _: ApprovalRPC.Empty = try await request(
            op: ApprovalRPC.Op.subscribe,
            arguments: ApprovalRPC.Scope(crewId: crewID, sessionId: sessionID),
            as: ApprovalRPC.Empty.self)
    }

    func listSessions() async throws -> SessionList {
        guard connected else { throw CrewRPCError.notConnected }
        let requestID = UUID().uuidString
        return try await withCheckedThrowingContinuation { continuation in
            sessionListWaiters[requestID] = { continuation.resume(with: $0) }
            sessionListTimeouts[requestID] = Task { @MainActor [weak self] in
                guard let self else { return }
                try? await Task.sleep(
                    nanoseconds: UInt64(max(self.timeout, 0.001) * 1_000_000_000))
                guard !Task.isCancelled,
                      let completion = self.sessionListWaiters.removeValue(forKey: requestID)
                else { return }
                self.sessionListTimeouts.removeValue(forKey: requestID)
                completion(.failure(CrewRPCError.timedOut))
            }
            send(.listSessions)
        }
    }

    func attach(_ channel: RemoteSessionChannel) {
        guard channels[channel.sessionID] !== channel else { return }
        channels[channel.sessionID] = channel
        channel.bind(client: self, generation: generation)
        send(.attach(.init(sessionId: channel.sessionID, cols: 0, rows: 0)))
    }

    func detach(_ channel: RemoteSessionChannel) {
        guard channels[channel.sessionID] === channel else { return }
        channels.removeValue(forKey: channel.sessionID)
        let handles = channelByHandle.compactMap { $0.value === channel ? $0.key : nil }
        for handle in handles {
            channelByHandle.removeValue(forKey: handle)
            send(.detach(.init(handle: handle)))
        }
        channel.didDetach(generation: generation)
    }

    func sendInput(handle: UInt32, bytes: [UInt8]) {
        guard connected else { return }
        send(.input(.init(handle: handle, bytes: bytes)))
    }

    func addApprovalChangeHandler(_ handler: @escaping (String) -> Void) -> UUID {
        let id = UUID(); approvalChangeHandlers[id] = handler; return id
    }

    func addChangeHandler(_ handler: @escaping (String) -> Void) -> UUID {
        let id = UUID(); changeHandlers[id] = handler; return id
    }
    func removeChangeHandler(_ id: UUID) { changeHandlers.removeValue(forKey: id) }
    func removeApprovalChangeHandler(_ id: UUID) {
        approvalChangeHandlers.removeValue(forKey: id)
    }

    private struct Empty: Codable {}

    private func scheduleHelloTimeout() {
        helloTimeout?.cancel()
        helloTimeout = Task { @MainActor [weak self] in
            guard let self else { return }
            try? await Task.sleep(nanoseconds: UInt64(max(timeout, 0.001) * 1_000_000_000))
            guard !Task.isCancelled, let waiter = self.helloWaiter else { return }
            self.helloWaiter = nil
            waiter.resume(throwing: CrewRPCError.timedOut)
            self.link.close()
        }
    }

    private func scheduleRequestTimeout(_ requestID: String) {
        timeoutTasks[requestID] = Task { @MainActor [weak self] in
            guard let self else { return }
            try? await Task.sleep(nanoseconds: UInt64(max(timeout, 0.001) * 1_000_000_000))
            guard !Task.isCancelled, let completion = self.pending.removeValue(forKey: requestID)
            else { return }
            self.timeoutTasks.removeValue(forKey: requestID)
            completion(.failure(CrewRPCError.timedOut))
        }
    }

    private func receive(_ data: Data) {
        guard !terminated else { return }
        do {
            for frame in try decoder.append(data) {
                if case let .snapshot(handle, seq, isLast, bytes) = frame {
                    channelByHandle[handle]?.receiveSnapshot(
                        seq: seq, isLast: isLast, bytes: bytes, generation: generation)
                    continue
                }
                guard let message = try codec.decodeDaemon(frame) else { continue }
                handle(message)
            }
        } catch {
            failAll(CrewRPCError.disconnected("协议数据损坏"))
            link.close()
        }
    }

    private func handle(_ message: SessionDaemonMessage) {
        guard !terminated else { return }
        switch message {
        case let .hello(hello):
            guard hello.protocolVersion == SessionProtocolVersion.current,
                  hello.capabilities.contains(CrewRPC.capability),
                  hello.capabilities.contains(ApprovalRPC.capability) else {
                let waiter = helloWaiter; helloWaiter = nil; helloTimeout?.cancel()
                waiter?.resume(throwing: CrewRPCError.incompatibleServer)
                link.close(); return
            }
            connected = true
            let waiter = helloWaiter; helloWaiter = nil; helloTimeout?.cancel()
            waiter?.resume()
        case let .sessions(value):
            latestSessions = value
            for summary in value.sessions {
                channels[summary.sessionId]?.apply(summary: summary, generation: generation)
            }
            let waiters = sessionListWaiters
            sessionListWaiters.removeAll()
            sessionListTimeouts.values.forEach { $0.cancel() }
            sessionListTimeouts.removeAll()
            waiters.values.forEach { $0(.success(value)) }
        case let .attached(value):
            guard let channel = channels[value.sessionId] else {
                send(.detach(.init(handle: value.handle)))
                return
            }
            let superseded = channelByHandle.compactMap {
                $0.value === channel && $0.key != value.handle ? $0.key : nil
            }
            for handle in superseded {
                channelByHandle.removeValue(forKey: handle)
                send(.detach(.init(handle: handle)))
            }
            channel.attach(handle: value.handle, generation: generation)
            channelByHandle[value.handle] = channel
        case let .state(value):
            channels[value.sessionId]?.apply(
                state: value.delta, sequence: value.stateSeq, generation: generation)
        case let .data(value):
            channelByHandle[value.handle]?.receiveTerminal(
                value.bytes, generation: generation)
        case let .resync(value):
            channelByHandle[value.handle]?.beginResync(generation: generation)
        case let .event(event) where event.kind == CrewRPC.resultEvent:
            guard let id = event.requestId,
                  let completion = pending.removeValue(forKey: id) else { return }
            timeoutTasks.removeValue(forKey: id)?.cancel()
            if case let .string(reason)? = event.fields["error"] {
                completion(.failure(CrewRPCError.remote(reason)))
            } else if case let .string(encoded)? = event.fields["payload"],
                      let data = Data(base64Encoded: encoded) {
                completion(.success(data))
            } else { completion(.failure(CrewRPCError.invalidResponse)) }
        case let .event(event) where event.kind == CrewRPC.whiteboardChangedEvent:
            guard case let .string(crewID)? = event.fields["crewId"] else { return }
            changeHandlers.values.forEach { $0(crewID) }
        case let .event(event) where event.kind == ApprovalRPC.resultEvent:
            guard let id = event.requestId,
                  let completion = pending.removeValue(forKey: id) else { return }
            timeoutTasks.removeValue(forKey: id)?.cancel()
            if case let .string(reason)? = event.fields["error"] {
                completion(.failure(CrewRPCError.remote(reason)))
            } else if case let .string(encoded)? = event.fields["payload"],
                      let data = Data(base64Encoded: encoded) {
                completion(.success(data))
            } else { completion(.failure(CrewRPCError.invalidResponse)) }
        case let .event(event) where event.kind == ApprovalRPC.changedEvent:
            guard case let .string(crewID)? = event.fields["crewId"] else { return }
            approvalChangeHandlers.values.forEach { $0(crewID) }
        case let .event(event):
            if case let .string(sessionID)? = event.fields["sessionId"] {
                channels[sessionID]?.receive(event: event, generation: generation)
            }
        case .pong: break
        }
    }

    private func failAll(_ error: Error) {
        guard !terminated else { return }
        terminated = true
        connected = false
        helloTimeout?.cancel(); helloTimeout = nil
        if let waiter = helloWaiter { helloWaiter = nil; waiter.resume(throwing: error) }
        let completions = pending; pending.removeAll()
        timeoutTasks.values.forEach { $0.cancel() }; timeoutTasks.removeAll()
        completions.values.forEach { $0(.failure(error)) }
        let lists = sessionListWaiters; sessionListWaiters.removeAll()
        sessionListTimeouts.values.forEach { $0.cancel() }; sessionListTimeouts.removeAll()
        lists.values.forEach { $0(.failure(error)) }
        channelByHandle.removeAll()
        channels.values.forEach { $0.disconnected(error.localizedDescription, generation: generation) }
        onDisconnect?(error)
    }

    private func send(_ message: SessionAppMessage) {
        guard let bytes = try? codec.encode(message) else { return }
        link.send(bytes)
    }
}

enum RemoteSessionConnectionState: Equatable {
    case connecting
    case connected
    case disconnected(String)
}

/// iOS viewer 的跨平台 session 门面。它不解析协议帧，只消费上面的单一 client
/// 分发出的 session 事件；generation 门保证旧链路迟到数据不会改写重连后的画面。
@MainActor
final class RemoteSessionChannel: ObservableObject {
    let sessionID: String
    @Published private(set) var summary: SessionSummary?
    @Published private(set) var terminalText = ""
    @Published private(set) var connectionState: RemoteSessionConnectionState = .connecting
    @Published private(set) var pendingApprovals: [ApprovalItem] = []
    @Published private(set) var transcriptText = ""

    private weak var client: CrewRPCClient?
    private var generation: UInt64 = 0
    private var handle: UInt32?
    private var snapshotBytes: [UInt8] = []
    private var terminalBytes: [UInt8] = []
    private var stateSequence: UInt64 = 0
    private let transcript = CodexTranscript()

    init(sessionID: String) { self.sessionID = sessionID }

    func bind(client: CrewRPCClient, generation: UInt64) {
        self.client = client
        self.generation = generation
        handle = nil
        snapshotBytes = []
        pendingApprovals = []
        connectionState = .connecting
    }

    func attach(handle: UInt32, generation: UInt64) {
        guard generation == self.generation else { return }
        self.handle = handle
        connectionState = .connected
    }

    func apply(summary: SessionSummary, generation: UInt64) {
        guard generation == self.generation else { return }
        self.summary = summary
        stateSequence = summary.stateSeq
    }

    func apply(
        state: SessionProtocolState, sequence: UInt64, generation: UInt64
    ) {
        guard generation == self.generation, sequence >= stateSequence else { return }
        stateSequence = sequence
        if let old = summary {
            summary = .init(sessionId: old.sessionId, stateSeq: sequence,
                            state: state, run: old.run)
        }
    }

    func receiveTerminal(_ bytes: [UInt8], generation: UInt64) {
        guard generation == self.generation else { return }
        terminalBytes.append(contentsOf: bytes)
        if terminalBytes.count > 512_000 { terminalBytes.removeFirst(terminalBytes.count - 512_000) }
        terminalText = Self.plainText(terminalBytes)
        connectionState = .connected
    }

    func receiveSnapshot(
        seq: UInt32, isLast: Bool, bytes: [UInt8], generation: UInt64
    ) {
        guard generation == self.generation else { return }
        if seq == 0 { snapshotBytes = [] }
        snapshotBytes.append(contentsOf: bytes)
        guard isLast else { return }
        terminalBytes = snapshotBytes
        snapshotBytes = []
        terminalText = Self.plainText(terminalBytes)
        connectionState = .connected
    }

    func beginResync(generation: UInt64) {
        guard generation == self.generation else { return }
        snapshotBytes = []
    }

    func receive(event: SessionEvent, generation: UInt64) {
        guard generation == self.generation,
              let notification = SessionCodexNotification(event),
              notification.sessionID == sessionID else { return }
        transcript.apply(method: notification.method, params: notification.foundationParams)
        transcriptText = transcript.items.isEmpty ? "" : CodexTranscriptText.render(
            items: transcript.items, maxLines: 10_000)
    }

    func setApprovals(_ approvals: [ApprovalItem], generation: UInt64) {
        guard generation == self.generation else { return }
        pendingApprovals = approvals
    }

    func disconnected(_ reason: String, generation: UInt64) {
        guard generation == self.generation else { return }
        handle = nil
        connectionState = .disconnected(reason)
    }

    func chooseTerminalDecision(optionIndex: Int) {
        guard let handle, let decision = summary?.state.pendingDecision,
              decision.numbered,
              decision.options.indices.contains(optionIndex) else { return }
        client?.sendInput(handle: handle, bytes: Array("\(optionIndex + 1)\r".utf8))
    }

    func detach() { client?.detach(self) }

    func didDetach(generation: UInt64) {
        guard generation == self.generation else { return }
        handle = nil
        pendingApprovals = []
        connectionState = .connecting
    }

    private static func plainText(_ bytes: [UInt8]) -> String {
        let string = String(decoding: bytes, as: UTF8.self)
        var result = ""
        var iterator = string.makeIterator()
        var escaping = false
        var csi = false
        while let character = iterator.next() {
            if escaping {
                if !csi, character == "[" { csi = true; continue }
                if csi, character.unicodeScalars.allSatisfy({ (0x40...0x7e).contains($0.value) }) {
                    escaping = false; csi = false
                } else if !csi { escaping = false }
                continue
            }
            if character == "\u{1b}" { escaping = true; continue }
            if character == "\r" { continue }
            if character == "\n" || character == "\t"
                || character.unicodeScalars.allSatisfy({ $0.value >= 0x20 }) {
                result.append(character)
            }
        }
        return result
    }
}
