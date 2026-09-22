import Foundation

@MainActor
final class RemotePendingCrewBackend: PendingCrewBackend {
    let configuration: RemoteBackendConfiguration
    private let connector: any SessionMessageLinkConnecting
    private let requestTimeout: TimeInterval
    private var client: CrewRPCClient?
    private var connectTask: Task<Void, Error>?
    private var connectionGeneration: UInt64 = 0
    private var streams: [String: [UUID: AsyncStream<Void>.Continuation]] = [:]
    private var changeHandlerID: UUID?
    private var approvalChangeHandlerID: UUID?
    private var sessionChannels: [String: RemoteSessionChannel] = [:]
    private var crewBySession: [String: String] = [:]

    var pendingRequestCount: Int { client?.pendingRequestCount ?? 0 }
    var isConnected: Bool { client?.connected == true }

    init(configuration: RemoteBackendConfiguration, requestTimeout: TimeInterval = 10) {
        self.configuration = configuration
        self.requestTimeout = requestTimeout
        self.connector = ClosureSessionMessageLinkConnector {
            try SecureTCPTransport.connect(parameters: configuration.parameters())
        }
    }

    init(configuration: RemoteBackendConfiguration,
         connector: any SessionMessageLinkConnecting,
         requestTimeout: TimeInterval = 10) {
        self.configuration = configuration
        self.connector = connector
        self.requestTimeout = requestTimeout
    }

    func connect() async throws {
        if let client, client.connected { return }
        if let connectTask { return try await connectTask.value }
        connectionGeneration &+= 1
        let generation = connectionGeneration
        let task = Task { @MainActor [weak self] in
            guard let self else { throw CrewRPCError.disconnected("连接对象已释放") }
            try await self.performConnect(generation: generation)
        }
        connectTask = task
        do {
            try await task.value
            if connectionGeneration == generation { connectTask = nil }
        } catch {
            if connectionGeneration == generation { connectTask = nil }
            throw error
        }
    }

    private func performConnect(generation: UInt64) async throws {
        let newClient = CrewRPCClient(
            link: try connector.connect(), timeout: requestTimeout, generation: generation)
        guard generation == connectionGeneration else {
            newClient.close()
            throw CrewRPCError.disconnected("连接已被更新的一代取代")
        }
        client = newClient
        changeHandlerID = newClient.addChangeHandler { [weak self, weak newClient] crewID in
            guard let self, let newClient,
                  generation == self.connectionGeneration, self.client === newClient else { return }
            self.streams[crewID]?.values.forEach { $0.yield(()) }
        }
        approvalChangeHandlerID = newClient.addApprovalChangeHandler {
            [weak self, weak newClient] crewID in
            guard let self, let newClient,
                  generation == self.connectionGeneration, self.client === newClient else { return }
            Task { @MainActor [weak self, weak newClient] in
                guard let self, let newClient,
                      generation == self.connectionGeneration,
                      self.client === newClient else { return }
                await self.refreshApprovals(crewID: crewID, generation: generation)
            }
        }
        newClient.onDisconnect = { [weak self, weak newClient] _ in
            guard let self, let newClient,
                  generation == self.connectionGeneration, self.client === newClient else { return }
            self.client = nil
        }
        do {
            try await newClient.connect()
            guard generation == connectionGeneration, client === newClient else {
                newClient.close()
                throw CrewRPCError.disconnected("旧连接握手已作废")
            }
            streams.keys.forEach { newClient.subscribe(crewID: $0) }
            for channel in sessionChannels.values {
                newClient.attach(channel)
                if let crewID = crewBySession[channel.sessionID] {
                    try await newClient.subscribeApprovals(
                        crewID: crewID, sessionID: channel.sessionID)
                }
            }
            if !sessionChannels.isEmpty {
                _ = try await newClient.listSessions()
            }
            for crewID in Set(crewBySession.values) {
                await refreshApprovals(crewID: crewID, generation: generation)
            }
        } catch {
            newClient.close()
            if generation == connectionGeneration, client === newClient { client = nil }
            throw error
        }
    }

    func disconnect() {
        connectionGeneration &+= 1
        connectTask?.cancel(); connectTask = nil
        client?.close(); client = nil
    }

    func reconnect() async throws {
        disconnect()
        try await connect()
    }

    private func rpc<Response: Decodable, Arguments: Encodable>(
        _ op: String, _ arguments: Arguments, as: Response.Type
    ) async throws -> Response {
        if client?.connected != true { try await connect() }
        guard let client else { throw CrewRPCError.notConnected }
        return try await client.request(op: op, arguments: arguments, as: Response.self)
    }

    private func rpc<Response: Decodable>(_ op: String, as: Response.Type) async throws -> Response {
        if client?.connected != true { try await connect() }
        guard let client else { throw CrewRPCError.notConnected }
        return try await client.request(op: op, as: Response.self)
    }

    func listCrews() async throws -> [CrewSummary] {
        try await rpc(CrewRPC.Op.listCrews, as: [CrewSummary].self)
    }
    func chiefLayer() async throws -> CrewSummary? {
        let box: OptionalBox<CrewSummary> = try await rpc(CrewRPC.Op.chiefLayer, as: OptionalBox.self)
        return box.value
    }
    func getCrew(_ crewId: String) async throws -> CrewDetail {
        try await rpc(CrewRPC.Op.getCrew, CrewRPC.CrewID(crewId: crewId), as: CrewDetail.self)
    }
    func listCrewWhiteboard(crewId: String) async throws -> [CrewWhiteboardEntry] {
        try await rpc(CrewRPC.Op.listWhiteboard, CrewRPC.CrewID(crewId: crewId),
                      as: [CrewWhiteboardEntry].self)
    }
    func listCrewMembers(crewId: String) async throws -> CrewRoster {
        try await rpc(CrewRPC.Op.listMembers, CrewRPC.CrewID(crewId: crewId), as: CrewRoster.self)
    }
    func postCrewMessage(
        crewId: String, text: String, mentions: [CrewMention], replyToId: String?,
        localAttachments: [LocalWhiteboardAttachment],
        extraReferences: [CrewMessageReference]
    ) async throws -> String? {
        guard localAttachments.isEmpty else {
            throw CrewRPCError.unsupported("iOS 远端发送本批暂不支持附件")
        }
        let result: CrewRPC.PostResult = try await rpc(
            CrewRPC.Op.postMessage,
            CrewRPC.PostMessage(crewId: crewId, text: text, mentions: mentions,
                                replyToId: replyToId, extraReferences: extraReferences),
            as: CrewRPC.PostResult.self)
        return result.warning
    }

    func whiteboardChanges(crewId: String) -> AsyncStream<Void> {
        AsyncStream { continuation in
            let id = UUID()
            streams[crewId, default: [:]][id] = continuation
            client?.subscribe(crewID: crewId)
            continuation.onTermination = { [weak self] _ in
                Task { @MainActor in self?.streams[crewId]?.removeValue(forKey: id) }
            }
        }
    }

    // MARK: - Remote sessions and structured approvals

    func listRemoteSessions() async throws -> [SessionSummary] {
        if client?.connected != true { try await connect() }
        guard let client else { throw CrewRPCError.notConnected }
        return try await client.listSessions().sessions
    }

    func openSession(sessionID: String) async throws -> RemoteSessionChannel {
        let sessions = try await listRemoteSessions()
        guard let summary = sessions.first(where: { $0.sessionId == sessionID }) else {
            throw CrewRPCError.remote("远端 session 已不存在")
        }
        let channel = sessionChannels[sessionID] ?? RemoteSessionChannel(sessionID: sessionID)
        sessionChannels[sessionID] = channel
        client?.attach(channel)
        channel.apply(summary: summary, generation: connectionGeneration)
        if let crewID = summary.run?.crewId {
            crewBySession[sessionID] = crewID
            guard let client else { throw CrewRPCError.notConnected }
            try await client.subscribeApprovals(crewID: crewID, sessionID: sessionID)
        }
        if let crewID = summary.run?.crewId {
            await refreshApprovals(crewID: crewID, generation: connectionGeneration)
        }
        return channel
    }

    func closeSession(sessionID: String) {
        guard let channel = sessionChannels.removeValue(forKey: sessionID) else { return }
        crewBySession.removeValue(forKey: sessionID)
        client?.detach(channel)
    }

    func listApprovals(crewID: String, sessionID: String) async throws -> [ApprovalItem] {
        try await rpc(
            ApprovalRPC.Op.list,
            ApprovalRPC.Scope(crewId: crewID, sessionId: sessionID),
            as: [ApprovalItem].self)
    }

    func answerApproval(
        crewID: String, approvalID: String, reply: String
    ) async throws {
        guard let sessionID = sessionChannels.values.first(where: {
            $0.pendingApprovals.contains(where: { $0.id == approvalID })
        })?.sessionID else {
            throw CrewRPCError.remote("待决策不在当前远端 session 中")
        }
        let _: ApprovalRPC.Empty = try await rpc(
            ApprovalRPC.Op.answer,
            ApprovalRPC.Answer(crewId: crewID, sessionId: sessionID,
                               approvalId: approvalID, reply: reply),
            as: ApprovalRPC.Empty.self)
        await refreshApprovals(crewID: crewID, generation: connectionGeneration)
    }

    func decideApproval(
        crewID: String, approvalID: String, decision: String
    ) async throws {
        guard let sessionID = sessionChannels.values.first(where: {
            $0.pendingApprovals.contains(where: { $0.id == approvalID })
        })?.sessionID else {
            throw CrewRPCError.remote("待审批不在当前远端 session 中")
        }
        let _: ApprovalRPC.Empty = try await rpc(
            ApprovalRPC.Op.decide,
            ApprovalRPC.Decide(crewId: crewID, sessionId: sessionID,
                               approvalId: approvalID, decision: decision),
            as: ApprovalRPC.Empty.self)
        await refreshApprovals(crewID: crewID, generation: connectionGeneration)
    }

    private func refreshApprovals(crewID: String, generation: UInt64) async {
        let sessionIDs = crewBySession.filter { $0.value == crewID }.map(\.key)
        for sessionID in sessionIDs {
            do {
                let approvals = try await listApprovals(crewID: crewID, sessionID: sessionID)
                guard generation == connectionGeneration else { return }
                sessionChannels[sessionID]?.setApprovals(approvals, generation: generation)
            } catch {
                guard generation == connectionGeneration else { return }
                sessionChannels[sessionID]?.disconnected(
                    error.localizedDescription, generation: generation)
            }
        }
    }

    func listMySubjects() async throws -> [UserSubject] {
        throw CrewRPCError.unsupported("iOS 远端本批不提供 subject 管理")
    }
    func createCrew(_ request: CreateCrewRequest) async throws -> CreateCrewResponse {
        throw CrewRPCError.unsupported("iOS 远端本批不提供新建 crew")
    }
    func attachParent(crewId: String, parentCrewId: String, childKeepsBps: Int) async throws {
        throw CrewRPCError.unsupported("iOS 远端本批不提供修改 crew 层级")
    }
    func detachParent(crewId: String, parentCrewId: String) async throws {
        throw CrewRPCError.unsupported("iOS 远端本批不提供修改 crew 层级")
    }
    func listModels() async throws -> [ModelCatalogEntry] { [] }

    private struct OptionalBox<Value: Codable>: Codable { var value: Value? }
}
