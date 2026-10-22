import Foundation
import Combine

/// A projection into the existing frontend models. All authority remains in crewd.
@MainActor
final class CrewdPendingCrewBackend: PendingCrewBackend {
    let workspace: CrewdWorkspace
    private var changeToken: AnyCancellable?
    private var selectionToken: AnyCancellable?
    private var listeners: [UUID: AsyncStream<Void>.Continuation] = [:]
    private var messages: [String: [CrewdValue]] = [:]
    init(workspace: CrewdWorkspace) {
        self.workspace = workspace
        selectionToken = Publishers.CombineLatest3(workspace.$crewID, workspace.$channelID, workspace.$channels)
            .dropFirst().sink { [weak self] _ in
                guard let self else { return }
                let epoch = self.workspace.observationGeneration
                // @Published fires before assignment. Yield after the complete
                // response has installed both IDs and validated Channel rows.
                Task { @MainActor [weak self] in
                    guard let self, self.workspace.acceptsObservation(epoch),
                          !self.workspace.channelID.isEmpty,
                          self.workspace.channels.contains(where: { $0["id"].string == self.workspace.channelID }) else { return }
                    self.listeners.values.forEach { $0.yield(()) }
                }
            }
        changeToken = workspace.authorityEvents.sink { [weak self] event in
            guard event.0.hasPrefix("message.") || event.0.hasPrefix("session.") || event.0.hasPrefix("channel.") || event.0 == "events.lagged" else { return }
            guard let self else { return }
            let epoch = self.workspace.observationGeneration
            Task { @MainActor [weak self] in
                guard let self, self.workspace.acceptsObservation(epoch) else { return }
                self.listeners.values.forEach { $0.yield(()) }
            }
        }
    }
    private func call(_ method: String, _ fields: [String: CrewdValue] = [:]) async throws -> CrewdValue {
        if method.hasSuffix(".list") || method.hasSuffix(".get") {
            return try await workspace.readValue(method, fields.isEmpty ? .null : .object(fields))
        }
        return try await workspace.performValue(method, fields.isEmpty ? .null : .object(fields))
    }
    private func summary(_ row: CrewdValue) throws -> CrewSummary {
        guard let id = row["id"].string, let title = row["title"].string,
              let created = row["createdAt"].string, let updated = row["updatedAt"].string else {
            throw CrewdClientError(message: "机组回复缺少正式身份或时间，未接受投影。")
        }
        return CrewSummary(id: id, title: title, responsibleSubjectId: "human", runtimeLocation: workspace.dataDirectory == nil ? "remote_host" : "local_host",
            captainBotId: row["captainSessionId"].string, status: row["statusLine"].string,
            createdAt: created, updatedAt: updated, parentCrewIds: row["parentIds"].array.compactMap(\.string),
            captainAgentKind: row["captainExecutor"].string, machineId: workspace.dataDirectory == nil ? workspace.authorityRecordID.uuidString : nil, manuallyHiddenAt: row["hiddenAt"].string)
    }
    func listMySubjects() async throws -> [UserSubject] {
        [UserSubject(id: "human", kind: "byok", displayName: "人", role: nil, userId: nil)]
    }
    func listCrews() async throws -> [CrewSummary] {
        let rows = try await workspace.readValue("crew.list")
        workspace.crews = rows.array
        return try rows.array.filter { $0["management"] != .bool(true) }.map(summary)
    }
    func chiefLayer() async throws -> CrewSummary? {
        try workspace.crews.first { $0["management"] == .bool(true) }.map(summary)
    }
    func getCrew(_ crewId: String) async throws -> CrewDetail {
        let detail = try await call("crew.get", ["crewId": .string(crewId)])
        let crew = detail["crew"], projected = try summary(crew)
        func link(_ id: String) -> CrewDetail.CrewLink {
            .init(crewId: id, title: workspace.crews.first { $0["id"].string == id }?["title"].string ?? id, childShareBps: 0)
        }
        let captain = detail["sessions"].array.first { $0["id"] == crew["captainSessionId"] && $0["id"].string != nil }
        return CrewDetail(crew: .init(id: projected.id, title: projected.title,
            responsibleSubjectId: projected.responsibleSubjectId, runtimeLocation: projected.runtimeLocation,
            workingDirectory: crew["workingDirectory"].string, captainBotId: projected.captainBotId,
            status: projected.status, createdAt: projected.createdAt, updatedAt: projected.updatedAt,
            captainAgentKind: projected.captainAgentKind), parents: projected.parentCrewIds.map(link),
            children: crew["childIds"].array.compactMap(\.string).map(link), shares: [],
            captain: captain.flatMap { row in row["id"].string.map { .init(botId: $0, displayName: row["displayName"].string ?? "机长") } })
    }
    func createCrew(_ request: CreateCrewRequest) async throws -> CreateCrewResponse {
        guard let directory = request.workingDirectory, !directory.isEmpty else {
            throw CrewdClientError(message: "请先选择机组实际工作目录。", kind: .notSubmitted)
        }
        let result = try await call("crew.create", ["title": .string(request.title ?? "新机组"), "workingDirectory": .string(directory)])
        guard let id = result["id"].string else { throw CrewdClientError(message: "创建结果没有正式机组编号，须核对原连接。") }
        let captain = await workspace.startCaptainForCreatedCrew(id)
        return CreateCrewResponse(crewId: id, captainBotId: captain)
    }
    func attachParent(crewId: String, parentCrewId: String, childKeepsBps: Int) async throws {
        _ = try await call("crew.adopt", ["crewId": .string(crewId), "parentId": .string(parentCrewId)])
    }
    func detachParent(crewId: String, parentCrewId: String) async throws {
        _ = try await call("crew.release", ["crewId": .string(crewId), "fromParentId": .string(parentCrewId)])
    }
    func listCrewMembers(crewId: String) async throws -> CrewRoster {
        let detail = try await call("crew.get", ["crewId": .string(crewId)])
        let captain = detail["crew"]["captainSessionId"].string
        let rows = detail["sessions"].array.compactMap { row -> CrewMember? in
            guard let id = row["id"].string else { return nil }
            return CrewMember(id: id, memberKind: id == captain ? "captain" : "code_session", userId: nil,
                botId: id == captain ? id : nil, codeSessionId: id, displayName: row["displayName"].string ?? row["title"].string,
                role: row["role"].string, status: row["desired"].string ?? "unknown", representsCrewId: crewId,
                sessionStatus: row["desired"].string, createdAt: row["updatedAt"].string)
        }
        return CrewRoster(captainBotId: captain, members: rows + [CrewMember(id: LocalWhiteboardStore.localUserId,
            memberKind: "human", userId: LocalWhiteboardStore.localUserId, botId: nil, codeSessionId: nil,
            displayName: "人", role: nil, status: "active", representsCrewId: nil, sessionStatus: nil)])
    }
    private func selectedChannel(_ crew: String) throws -> String {
        guard workspace.crewID == crew, !workspace.channelID.isEmpty,
              workspace.channels.contains(where: { $0["id"].string == workspace.channelID }) else {
            throw CrewdClientError(message: "当前会话尚未由后台确认。", kind: .notSubmitted)
        }
        return workspace.channelID
    }
    func listCrewWhiteboard(crewId: String) async throws -> [CrewWhiteboardEntry] {
        let channel = try selectedChannel(crewId)
        let epoch = workspace.observationGeneration
        let response = try await call("message.list", ["channelId": .string(channel), "limit": .integer(1000)])
        guard !Task.isCancelled, workspace.acceptsObservation(epoch), try selectedChannel(crewId) == channel else { throw CancellationError() }
        let rows = response.array
        messages[crewId] = rows
        return try rows.map { row in
            guard let id = row["id"].string, let created = row["createdAt"].string else {
                throw CrewdClientError(message: "消息缺少正式身份，未接受时间线投影。")
            }
            let human = row["senderKind"] == .string("human")
            return CrewWhiteboardEntry(id: id, senderKind: human ? "user" : (row["senderKind"].string ?? "system"),
                senderSessionId: row["senderSessionId"].string, senderUserId: human ? LocalWhiteboardStore.localUserId : nil,
                senderBotId: nil, messageKind: "announcement", summary: row["text"].string, createdAt: created,
                payload: .init(text: row["text"].string), attachments: row["attachments"].array.compactMap { attachment in
                    guard let id = attachment["id"].string, let mime = attachment["mime"].string, let path = attachment["path"].string else { return nil }
                    // Preserve metadata, never interpret the computer's path as
                    // a phone/current-host file or a transferable URL.
                    return CrewAttachment(id: id, kind: nil, mime: mime,
                        size: attachment["size"].int, width: nil, height: nil,
                        url: "crewd-reference:" + (path.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? ""), filename: attachment["filename"].string)
                }, senderDisplayName: row["senderName"].string,
                senderMemberId: row["senderSessionId"].string, inReplyTo: row["inReplyTo"].string,
                mentions: row["mentions"].array.compactMap { m in m["kind"].string.map { CrewMention(kind: $0, targetId: m["targetId"].string) } }, category: row["category"].string)
        }
    }
    func postCrewMessage(crewId: String, text: String, mentions: [CrewMention], replyToId: String?,
        localAttachments: [LocalWhiteboardAttachment], extraReferences: [CrewMessageReference]) async throws -> String? {
        guard localAttachments.isEmpty else { throw CrewdClientError(message: "当前后台尚未开放附件提交；附件仍留在输入中。", kind: .notSubmitted) }
        guard extraReferences.isEmpty else { throw CrewdClientError(message: "请使用正式工作项关联入口。", kind: .notSubmitted) }
        let channel = try selectedChannel(crewId)
        var fields: [String: CrewdValue] = ["channelId": .string(channel), "text": .string(text),
            "mentions": .array(mentions.map { m in var value: [String: CrewdValue] = ["kind": .string(m.kind)]; if let id = m.targetId { value["targetId"] = .string(id) }; return .object(value) })]
        if let replyToId { fields["inReplyTo"] = .string(replyToId) }
        let context = CrewdDrafts.Context.message(crew: crewId, channel: channel, session: "")
        workspace.messageDrafts.set(text, for: context)
        _ = try await workspace.submitOriginalText(.object(fields), snapshot: workspace.messageDrafts.snapshot(for: context),clearPersonal:false)
        return nil
    }
    func whiteboardChanges(crewId: String) -> AsyncStream<Void> {
        AsyncStream { continuation in
            let id = UUID(); listeners[id] = continuation
            continuation.onTermination = { [weak self] _ in Task { @MainActor in self?.listeners[id] = nil } }
        }
    }
    func listModels() async throws -> [ModelCatalogEntry] { [] }
}
