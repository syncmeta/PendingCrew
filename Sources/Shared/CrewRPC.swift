import Foundation

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
    private var terminated = false
    private(set) var connected = false
    var pendingRequestCount: Int { pending.count }

    init(link: any SessionMessageLink, timeout: TimeInterval) {
        self.link = link
        self.timeout = timeout
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
                              appBuild: "ios-remote", capabilities: [CrewRPC.capability])))
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

    func addChangeHandler(_ handler: @escaping (String) -> Void) -> UUID {
        let id = UUID(); changeHandlers[id] = handler; return id
    }
    func removeChangeHandler(_ id: UUID) { changeHandlers.removeValue(forKey: id) }

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
        do {
            for frame in try decoder.append(data) {
                guard let message = try codec.decodeDaemon(frame) else { continue }
                handle(message)
            }
        } catch {
            failAll(CrewRPCError.disconnected("协议数据损坏"))
            link.close()
        }
    }

    private func handle(_ message: SessionDaemonMessage) {
        switch message {
        case let .hello(hello):
            guard !terminated else { return }
            guard hello.protocolVersion == SessionProtocolVersion.current,
                  hello.capabilities.contains(CrewRPC.capability) else {
                let waiter = helloWaiter; helloWaiter = nil; helloTimeout?.cancel()
                waiter?.resume(throwing: CrewRPCError.incompatibleServer)
                link.close(); return
            }
            connected = true
            let waiter = helloWaiter; helloWaiter = nil; helloTimeout?.cancel()
            waiter?.resume()
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
        default: break
        }
    }

    private func failAll(_ error: Error) {
        terminated = true
        connected = false
        helloTimeout?.cancel(); helloTimeout = nil
        if let waiter = helloWaiter { helloWaiter = nil; waiter.resume(throwing: error) }
        let completions = pending; pending.removeAll()
        timeoutTasks.values.forEach { $0.cancel() }; timeoutTasks.removeAll()
        completions.values.forEach { $0(.failure(error)) }
    }

    private func send(_ message: SessionAppMessage) {
        guard let bytes = try? codec.encode(message) else { return }
        link.send(bytes)
    }
}
