import Foundation
import Combine
import Network
#if os(macOS)
import AppKit
#elseif os(iOS)
import UIKit
#endif

struct CrewdFirstCaptainReply: Equatable {
    enum Phase: Equatable {
        case starting
        case registered(String)
        case launched(String)
    }

    let crewID: String
    let channelID: String
    let messageID: String
    let firstSequence: Int
    var phase: Phase = .starting

    init?(crewID: String, channelID: String, messageID: String, firstSequence: Int) {
        guard !crewID.isEmpty,
              crewID.utf8.allSatisfy({ (48...57).contains($0) || (65...90).contains($0) ||
                  (97...122).contains($0) || $0 == 45 || $0 == 95 }),
              ["\(crewID)/human", "\(crewID)/lobby"].contains(channelID),
              UUID(uuidString: messageID)?.uuidString.lowercased() == messageID,
              firstSequence > 0, firstSequence < Int.max else { return nil }
        self.crewID = crewID
        self.channelID = channelID
        self.messageID = messageID
        self.firstSequence = firstSequence
    }

    var startupBrief: String {
        """
        本机组首次启动时，先用 mcp__crew__read_channel（channel="\(channelID)", before=\(firstSequence + 1), limit=1）读取原群首条人类消息，确认读到 #\(firstSequence)。
        回复这条消息时，用 mcp__crew__post，同时填写 channel="\(channelID)" 和 reply_to="\(messageID)"。若原消息未读到或原群回复失败，请如实说明，勿改发大厅、其他群或私聊。结果不明时先核对原群，勿重复发。
        这段指引只用于原首条消息。恢复已有会话时，不要因看到这段指引而重新接单或再次回复。
        之后读取本机组群聊与待办，回应人类需求；没有任务时待命。
        """
    }

    func isReply(channel: String?, senderKind: String?, sender: String?, sequence: Int?, inReplyTo: String?) -> Bool {
        let captain: String
        switch phase {
        case .starting: return false
        case .registered(let id), .launched(let id): captain = id
        }
        return channel == channelID && senderKind == "captain" && sender == captain &&
            inReplyTo == messageID && sequence.map { $0 > firstSequence } == true
    }
}

@MainActor
final class CrewdWorkspace: ObservableObject {
    let client = CrewdClient()
    let authorityEvents = PassthroughSubject<(String, CrewdValue), Never>()
    var originalJournalRequired = false
    private var originalJournal: CrewdPresentationJournal?
    private var preparingJournal: UUID?
    private var journalFailureConnection: UUID?
    var originalJournalReady: Bool { originalJournalBound && acceptsObservation(generation) }
    /// Stop closes execution, but the same authenticated scope can still look
    /// up its original receipts without reopening admission.
    private var originalJournalBound: Bool {
        guard originalJournal != nil, let scope = presentationScope,
              preparingJournal == nil, !retired, client.readyForWork else { return false }
        if client.capabilities.contains("command.receipts"), scope.databaseID != client.runtimeIdentity["dbId"].string { return false }
        if dataDirectory == nil {
            let current = client.authorityBinding
            guard scope.authorityID == current["authorityId"].string,
                  scope.databaseID == current["dbId"].string,
                  scope.grantID == current["grantId"].string,
                  scope.grantRevision == current["grantRevision"].string else { return false }
        }
        return true
    }
    @Published private(set) var durableUnknown: [CrewdPresentationJournal.Prepared] = []
    @Published private(set) var originalFirstMessageHistory: [CrewdPresentationJournal.Prepared] = []
    @Published private(set) var presentationScope: CrewdPresentationJournal.Scope?
    var presentationNamespace: String {
        let scope = presentationScope
        return authorityRecordID.uuidString + "/" + (scope?.authorityID ?? "unconfirmed") + "/" + (scope?.databaseID ?? "unconfirmed") + "/" + (scope?.grantRevision ?? "local")
    }

    lazy var publicCommands = CrewdPublicCommands(
        binding: { [weak self] in
            guard let self else { throw CrewdClientError(message: "原连接已释放，未提交命令。", kind: .notSubmitted) }
            return try self.publicCommandBinding(for: self.client)
        },
        ownsDaemon: { [weak self] in self?.hasVerifiedOwnedLocalHelper == true },
        call: { [weak self] method, params in
            guard let self else { throw CrewdClientError(message: "原连接已释放，未发送命令。", kind: .notSubmitted) }
            return try await self.client.call(method, params)
        },
        prepare: { [weak self] envelope, crew in
            guard let journal = self?.originalJournal else { throw CrewdClientError(message: "原命令历史尚未就绪，未发送。", kind: .notSubmitted) }
            try journal.validateUnusedCodexInputs(envelope.command)
            var firstMessageExecutor: String?
            if envelope.operation == .firstMessage, let draft = self?.originalPendingCrewDraft {
                guard envelope.params["crew"]["title"].string == draft.title,
                      envelope.params["crew"]["workingDirectory"].string == draft.workingDirectory,
                      envelope.params["crew"]["parentId"].string == draft.parentCrewId,
                      envelope.params["text"].string == draft.firstMessage.trimmingCharacters(in: .whitespacesAndNewlines) else {
                    throw CrewdClientError(message: "首发输入已变化，尚未提交；原草稿仍保留。", kind: .notSubmitted)
                }
                firstMessageExecutor = draft.executor
            }
            let record: CrewdPresentationJournal.Prepared
            if envelope.operation == .workCloseWithReceipt {
                record = try journal.prepareWorkClose(method:"command.submit",params:envelope.value,crew:crew)
            } else {
                record = try journal.prepare(method:"command.submit",params:envelope.value,crew:crew,firstMessageExecutor:firstMessageExecutor)
            }
            if envelope.params["codexInputs"] != .null || envelope.params["inputs"] != .null{self?.codexInputs.refreshProjection()}
            return record
        },
        finish: { [weak self] record, state, detail in
            guard let journal = self?.originalJournal else { throw CrewdClientError(message: "原命令历史尚未就绪。") }
            try journal.finish(record, state: state, detail: detail)
            self?.originalFirstMessageHistory = try journal.firstMessageRecords()
            if state == "unknown" { self?.durableUnknown = try journal.unknownRecords() }
        }, allowsSubmission: { [weak self] in self?.stopping == false })
    func makeTerminalCommands(for connection: CrewdClient) -> CrewdPublicCommands {
        CrewdPublicCommands(binding: { [weak self, weak connection] in
            guard let self, let connection else { throw CrewdClientError(message: "原终端连接已关闭，尚未提交。", kind: .notSubmitted) }
            return try self.publicCommandBinding(for: connection)
        }, ownsDaemon: { false }, call: { [weak connection] method, params in
            guard let connection else { throw CrewdClientError(message: "原终端连接已关闭，尚未提交。", kind: .notSubmitted) }
            return try await connection.call(method, params)
        }, prepare: { [weak self] envelope, crew in
            guard let journal = self?.originalJournal else { throw CrewdClientError(message: "原终端历史未就绪，未发送。", kind: .notSubmitted) }
            return try journal.prepare(method: "command.submit", params: envelope.value, crew: crew)
        }, finish: { [weak self] record, state, detail in
            guard let journal = self?.originalJournal else { throw CrewdClientError(message: "原终端历史未就绪。") }
            try journal.finish(record, state: state, detail: detail)
            if state == "unknown" { self?.durableUnknown = try journal.unknownRecords() }
        }, allowsSubmission: { [weak self] in self?.stopping == false })
    }
    func publicCommandBinding(for connection: CrewdClient) throws -> CrewdPublicCommands.Binding {
        guard !retired, client.connected, connection.connected,
              (!originalJournalRequired || originalJournalReady || (stopping && originalJournalBound)),
              connection.capabilities.contains("command.receipts"),
              let scope = presentationScope,
              let db = connection.runtimeIdentity["dbId"].string, !db.isEmpty,
              scope.databaseID == db,
              connection.runtimeIdentity["dbId"] == client.runtimeIdentity["dbId"],
              connection.runtimeIdentity["runtimeBindingId"] == client.runtimeIdentity["runtimeBindingId"],
              let instance = connection.runtimeIdentity["runtimeBindingId"].string else {
            throw CrewdClientError(message: "当前连接尚未具备真实公开命令与数据库资格；尚未提交。", kind: .notSubmitted)
        }
        return .init(scope: scope, databaseID: db, runtimeBindingID: instance, connectionEpoch: connection.connectionEpoch)
    }
    /// A dedicated authenticated observer/control socket owns its watches and
    /// leases. Closing it cannot stop the main business connection or team.
    func openTerminalConnection() async throws -> CrewdClient {
        guard client.capabilities.contains("session.terminal") else { throw CrewdClientError(message: "当前后台尚未开放实际终端机制。", kind: .notSubmitted) }
        let epoch = generation
        let observer = CrewdClient()
        do {
            try await connector(observer)
            guard acceptsObservation(epoch) else { throw CancellationError() }
            _ = try publicCommandBinding(for: observer)
            return observer
        } catch { observer.disconnect(); throw error }
    }
    var hasVerifiedOwnedLocalHelper: Bool {
        #if os(macOS)
        guard let owned = backend, owned.isRunning, !retired, client.connected,
              let birth = ownedChildIdentity, let binding = ownedIPCBinding, binding.generation == generation,
              owned.processIdentifier == birth.pid,
              (try? CrewdKernelProcessIdentity.ownChild(birth.pid)) == birth,
              (try? client.verifiedLocalPeerIdentity()) == binding.peer, binding.peer == birth,
              client.runtimeIdentity["dbId"].string == binding.dbID,
              client.runtimeIdentity["runtimeBindingId"].string == binding.runtimeBindingID else { return false }
        return true
        #else
        return false
        #endif
    }

    private final class WeakTerminal {
        weak var value: CrewdPublicTerminal?
        init(_ value: CrewdPublicTerminal) { self.value = value }
    }
    private var terminalDisplays: [WeakTerminal] = []
    func registerTerminalDisplay(_ display: CrewdPublicTerminal) {
        terminalDisplays.removeAll { $0.value == nil }
        terminalDisplays.append(WeakTerminal(display))
    }
    private func closeTerminalDisplays() {
        terminalDisplays.removeAll { $0.value == nil }
        for display in terminalDisplays { display.value?.deactivate() }
        controlTargets.removeAll()
    }
    lazy var workStore = CrewdWorkStore { [weak self] method, params in
        guard let self, !self.retired else { throw CrewdClientError(message: "连接已切换，旧作用域不能提交。", kind: .notSubmitted) }
        return try await self.submitRPC(method, params)
    }
    lazy var codexInputs=CrewdCodexInputStore(origin:{[weak self] in
        guard let self,!self.retired,self.client.readyForWork,self.client.capabilities.contains("session.codex"),self.originalJournalReady,let scope=self.presentationScope else{return nil}
        return .init(scope:scope,generation:self.observationGeneration,connection:self.client.connectionEpoch,crew:self.crewID)
    },rpc:{[weak self] method,params in guard let self,!self.retired,self.client.readyForWork,!self.stopping else{throw CrewdClientError(message:"原输入连接已关闭，未提交。",kind:.notSubmitted)};return try await self.client.call(method,params)},journal:{[weak self] in guard let self,let journal=self.originalJournal else{throw CrewdPublicWire.invalid()};return journal})
    lazy var notifications=CrewdNotificationStore(origin:{[weak self] in
        guard let self,!self.retired,self.client.readyForWork,self.client.capabilities.contains("notification.nativeApns"),self.originalJournalBound,let scope=self.presentationScope else{return nil}
        let remote=self.dataDirectory==nil && scope.grantID?.isEmpty==false && scope.grantRevision?.isEmpty==false && !self.stopping
        return .init(scope:scope,observation:self.generation,connection:self.client.connectionEpoch,remoteRecipient:remote)
    },rpc:{[weak self] method,params in
        guard let self,!self.retired,self.client.readyForWork else{throw CrewdClientError(message:"原通知连接已关闭。",kind:.notSubmitted)}
        return try await self.client.call(method,params)
    },journal:{[weak self] in
        guard let self,self.originalJournalBound,let journal=self.originalJournal else{throw CrewdClientError(message:"原通知历史尚未就绪，未发送。",kind:.notSubmitted)};return journal
    })
    lazy var artifacts=CrewdArtifactStore(origin:{[weak self] in
        guard let self,!self.retired,self.client.readyForWork,self.originalJournalReady,
              self.client.capabilities.contains(CrewdArtifactWire.capability),let scope=self.presentationScope else {return nil}
        return .init(scope:scope,observation:self.observationGeneration,connection:self.client.connectionEpoch,crew:self.crewID)
    },rpc:{[weak self] method,params in
        guard let self,!self.retired,self.client.readyForWork,self.originalJournalReady,
              self.client.capabilities.contains(CrewdArtifactWire.capability),method != CrewdArtifactWire.submit || !self.stopping else {
            throw CrewdClientError(message:"原产物连接已变化，未提交。",kind:.notSubmitted)
        }
        return try await self.client.call(method,params)
    },journal:{[weak self] in
        guard let self,let journal=self.originalJournal,self.originalJournalReady else {throw CrewdManagedDirectoryWire.Problem.unavailable}
        return journal
    },available:{[weak self] in self?.client.capabilities.contains(CrewdArtifactWire.capability)==true},
    historyAvailable:{[weak self] in self?.client.capabilities.contains(CrewdArtifactHistoryWire.capability)==true})
    lazy var adminAskGroups = CrewdAdminAskGroupsStore(origin: { [weak self] in
        guard let self, !self.retired, self.client.readyForWork,
              self.client.capabilities.contains(CrewdAdminAskGroupsWire.capability),
              let scope = self.presentationScope, scope.databaseID != nil else { return nil }
        return .init(scope: scope, observation: self.observationGeneration,
                     connection: self.client.connectionEpoch, crew: self.crewID)
    }, rpc: { [weak self] method, params in
        guard let self, !self.retired, self.client.readyForWork,
              self.client.capabilities.contains(CrewdAdminAskGroupsWire.capability),
              method == CrewdAdminAskGroupsWire.list else { throw CrewdAdminAskGroupsWire.Problem.unavailable }
        return try await self.client.call(method, params)
    }, available: { [weak self] in self?.client.capabilities.contains(CrewdAdminAskGroupsWire.capability) == true })
    lazy var managedTablesV2=CrewdManagedTableV2Store(origin:{[weak self] in
        guard let self,!self.retired,self.client.readyForWork,self.originalJournalReady,
              self.client.capabilities.contains(CrewdManagedTableV2Wire.capability) || self.client.capabilities.contains(CrewdManagedTableV2Wire.linksCapability),let scope=self.presentationScope else {return nil}
        return .init(scope:scope,observation:self.observationGeneration,connection:self.client.connectionEpoch,crew:self.crewID)
    },rpc:{[weak self] method,params in
        guard let self,!self.retired,self.client.readyForWork,self.originalJournalReady,
              ![CrewdManagedTableV2Wire.submit,CrewdManagedTableV2Wire.linkSubmit].contains(method) || !self.stopping else {
            throw CrewdClientError(message:"原表格或关联连接已变化，未提交。",kind:.notSubmitted)
        }
        return try await self.client.call(method,params)
    },journal:{[weak self] in
        guard let self,self.originalJournalReady,let ledger=self.originalJournal else {throw CrewdManagedDirectoryWire.Problem.unavailable};return ledger
    },tablesAvailable:{[weak self] in self?.client.capabilities.contains(CrewdManagedTableV2Wire.capability)==true},
      linksAvailable:{[weak self] in self?.client.capabilities.contains(CrewdManagedTableV2Wire.linksCapability)==true})
    lazy var managedTablesV3=CrewdManagedTableV3Store(origin:{[weak self] in
        guard let self,!self.retired,self.client.readyForWork,self.originalJournalReady,
              self.client.capabilities.contains(CrewdManagedTableV3Wire.capability),let scope=self.presentationScope else{return nil}
        return .init(scope:scope,observation:self.observationGeneration,connection:self.client.connectionEpoch,crew:self.crewID)
    },rpc:{[weak self] method,params in
        guard let self,!self.retired,self.client.readyForWork,self.originalJournalReady,
              self.client.capabilities.contains(CrewdManagedTableV3Wire.capability),CrewdManagedTableV3Wire.methods.contains(method),
              method != CrewdManagedTableV3Wire.submit || !self.stopping else{throw CrewdClientError(message:"原表格连接已变化，未提交。",kind:.notSubmitted)}
        return try await self.client.call(method,params)
    },journal:{[weak self] in
        guard let self,self.originalJournalReady,let ledger=self.originalJournal else{throw CrewdManagedDirectoryWire.Problem.unavailable};return ledger
    },available:{[weak self] in self?.client.capabilities.contains(CrewdManagedTableV3Wire.capability)==true})
    /// Each search presentation owns its cursor/tickets; it never owns mutation receipts.
    func makeOrganizationSearchStore()->CrewdOrganizationSearchStore {
        CrewdOrganizationSearchStore(origin:{[weak self] in
            guard let self,!self.retired,self.client.readyForWork,let scope=self.presentationScope,scope.databaseID != nil,
                  self.client.capabilities.contains(CrewdOrganizationSearchWire.capability) else{return nil}
            return .init(scope:scope,observation:self.observationGeneration,connection:self.client.connectionEpoch,crew:self.crewID)
        },rpc:{[weak self] method,params in
            guard let self,!self.retired,self.client.readyForWork,self.client.capabilities.contains(CrewdOrganizationSearchWire.capability),
                  [CrewdOrganizationSearchWire.search,CrewdOrganizationSearchWire.expand].contains(method) else{throw CrewdOrganizationSearchWire.Problem.unavailable}
            return try await self.client.call(method,params)
        },available:{[weak self] in self?.client.capabilities.contains(CrewdOrganizationSearchWire.capability)==true},
          knowledgeAvailable:{[weak self] in self?.client.capabilities.contains(CrewdManagedTableV2Wire.knowledgeCapability)==true})
    }
    struct ManagedBusinessNavigation:Identifiable {
        let id=UUID()
        let target:CrewdManagedTableV2Wire.Target
        let crew:String
        let connection:UUID
        let authority:UUID
    }
    @Published private(set) var managedBusinessNavigation:ManagedBusinessNavigation?
    var currentManagedBusinessNavigation:ManagedBusinessNavigation? {
        guard let value=managedBusinessNavigation,!retired,client.readyForWork,value.connection==client.connectionEpoch,value.authority==authorityRecordID,value.crew==crewID else {return nil};return value
    }
    func consumeManagedBusinessNavigation(_ id:UUID) {if managedBusinessNavigation?.id==id {managedBusinessNavigation=nil}}
    func openManagedBusinessTarget(_ target:CrewdManagedTableV2Wire.Target,metadataCrewId:String?=nil,presentationCrewId:String?=nil,current:@MainActor ()->Bool) async throws {
        let scope=try await managedTablesV2.scope(stillCurrent:current)
        var value=try await managedTablesV2.resolve(target,scope:scope,current:current)
        if let metadataCrewId {
            guard target.kind == .artifact,let reference=target.reference else {throw CrewdManagedDirectoryWire.Problem.invalid}
            _=try await managedTablesV2.links(scope,source:.init(kind:"artifact",reference:reference,metadataCrewId:metadataCrewId),includeAdapters:false,limit:1,current:current)
        }
        if let presentationCrewId {
            guard crewID.isEmpty,metadataCrewId==nil,[CrewdManagedTableV2Wire.TargetKind.artifact,.human].contains(target.kind) else {throw CrewdManagedDirectoryWire.Problem.invalid}
            let rows=try await readValue("crew.list",.object(["includeRemoved":.bool(true)]))
            guard current(),managedTablesV2.owns(scope),case .array(let crews)=rows,
                  crews.contains(where:{$0["id"].string?.utf8.elementsEqual(presentationCrewId.utf8)==true}) else {throw CrewdManagedDirectoryWire.Problem.stale}
            // The extra presentation read must not replace the target's current ACL check.
            value=try await managedTablesV2.resolve(target,scope:scope,current:current)
        }
        guard current(),managedTablesV2.owns(scope) else {throw CrewdManagedDirectoryWire.Problem.stale}
        if let metadataCrewId {selectCrew(metadataCrewId)}
        else if let presentationCrewId {selectCrew(presentationCrewId)}
        if crewID.isEmpty,[CrewdManagedTableV2Wire.TargetKind.table,.row,.decision,.risk,.lesson].contains(target.kind) {
            guard let owner=value.ownerCrewId,!owner.isEmpty else {throw CrewdManagedDirectoryWire.Problem.invalid}
            selectCrew(owner)
        }
        guard !crewID.isEmpty || ![CrewdManagedTableV2Wire.TargetKind.artifact,.human].contains(target.kind) else {throw CrewdClientError(message:"请选择一个机组作为查看范围。",kind:.notSubmitted)}
        if [.crew,.session,.channel,.message,.work,.requirement,.ask].contains(target.kind) {
            guard let crew=value.ownerCrewId,!crew.isEmpty else {throw CrewdManagedDirectoryWire.Problem.invalid}
            selectCrew(crew)
            switch target.kind {
            case .crew:originalSection="Todo"
            case .session:selectSession(target.idValue!);originalSection="消息"
            case .channel:selectChannel(target.idValue!);originalSection="消息"
            case .message:guard let channel=value.messageChannelId,!channel.isEmpty else {throw CrewdManagedDirectoryWire.Problem.invalid};selectChannel(channel);originalSection="消息"
            case .work:originalSection="进展"
            case .ask:originalSection="真人Todo"
            default:originalSection="Todo"
            }
        } else if target.kind == .human {originalSection="真人Todo"}
        else {originalSection="文档"}
        managedBusinessNavigation = .init(target:target,crew:crewID,connection:client.connectionEpoch,authority:authorityRecordID)
    }
    func makeManagedLinkKnowledgeReader(crew:String)->CrewdKnowledgeStore {
        let observation=observationGeneration,connection=client.connectionEpoch,caller=crewID,scope=presentationScope
        @MainActor func current(_ w:CrewdWorkspace)->Bool {w.acceptsObservation(observation) && w.client.connectionEpoch==connection && w.crewID==caller && w.presentationScope==scope && w.originalJournalReady && w.client.capabilities.contains(CrewdManagedTableV2Wire.knowledgeCapability)}
        return CrewdKnowledgeStore(origin:{[weak self] in guard let self,let scope,current(self) else {return nil};return .init(scope:scope,observation:observation,connection:connection,crew:crew)},rpc:{[weak self] method,params in
            guard let self,current(self),["data.workspace.get","data.object.list","data.object.get","data.query"].contains(method),
                  (method=="data.workspace.get" ? params["crewId"]:params["context"]["crewId"]) == .string(crew) else {throw CrewdKnowledgeStore.Failure.unavailable}
            return try await self.client.call(method,params)
        },journal:{[weak self] in guard let self,current(self),let ledger=self.originalJournal else {throw CrewdKnowledgeStore.Failure.unavailable};return ledger},readOnlyPresentation:true)
    }

    lazy var managedDirectory = CrewdManagedDirectoryStore(origin: { [weak self] in
        guard let self, !self.retired, self.client.readyForWork,
              self.client.capabilities.contains(CrewdManagedDirectoryWire.capability),
              self.originalJournalReady, let scope = self.presentationScope else { return nil }
        return .init(scope: scope, observation: self.observationGeneration,
                     connection: self.client.connectionEpoch, crew: self.crewID)
    }, rpc: { [weak self] method, params in
        guard let self, !self.retired, self.client.readyForWork, self.originalJournalReady,
              self.client.capabilities.contains(CrewdManagedDirectoryWire.capability),
              !["managedObject.submit",CrewdManagedMessageWire.record].contains(method) || !self.stopping else {
            throw CrewdClientError(message:"原组织目录连接已变更，未提交。", kind:.notSubmitted)
        }
        return try await self.client.call(method, params)
    }, journal: { [weak self] in
        guard let self, self.originalJournalReady, let journal = self.originalJournal
        else { throw CrewdManagedDirectoryWire.Problem.unavailable }
        return journal
    }, decisionsAvailable: { [weak self] in
        self?.client.capabilities.contains(CrewdManagedDirectoryWire.decisionsCapability) == true
    }, riskLessonsAvailable: { [weak self] in
        self?.client.capabilities.contains(CrewdManagedDirectoryWire.riskLessonsCapability) == true
    }, structuredDecisionsAvailable: { [weak self] in
        self?.client.capabilities.contains(CrewdStructuredAskWire.capability) == true
    }, messageSourcesAvailable: { [weak self] in
        self?.client.capabilities.contains(CrewdManagedMessageWire.capability) == true
    })
    lazy var knowledge: CrewdKnowledgeStore = CrewdKnowledgeStore(origin: { [weak self] in
        guard let self, self.client.capabilities.contains("data"), self.originalJournalReady,
              self.acceptsObservation(self.observationGeneration), let scope = self.presentationScope else { return nil }
        return .init(scope: scope, observation: self.observationGeneration, connection: self.client.connectionEpoch, crew: self.crewID)
    }, rpc: { [weak self] method, params in
        guard let self, self.client.capabilities.contains("data"), self.originalJournalReady,
              self.acceptsObservation(self.observationGeneration) else { throw CrewdKnowledgeStore.Failure.unavailable }
        // This adapter owns data UUIDs and pure lookup. Never wrap them in a second mutation envelope.
        return try await self.client.call(method, params)
    }, journal: { [weak self] in
        guard let self, self.originalJournalReady, let journal = self.originalJournal else { throw CrewdKnowledgeStore.Failure.unavailable }
        return journal
    }, nativeNavigation: { [weak self] reference, context in
        guard let self, reference.kind == "session", reference.namespace == context.namespace,
              self.knowledge.workspace?.context == context, context.crewId == self.crewID,
              self.acceptsObservation(self.observationGeneration), self.client.connected,
              self.sessions.contains(where: { $0["id"].string == reference.id }) else { return false }
        self.knowledge.invalidate()
        self.selectSession(reference.id)
        self.knowledgeNavigation = UUID()
        return true
    })
    @Published private(set) var linkedKnowledgeCrew=""
    lazy var linkedKnowledge: CrewdKnowledgeStore = CrewdKnowledgeStore(origin: { [weak self] in
        guard let self, self.client.capabilities.contains("data"), self.originalJournalReady,
              self.acceptsObservation(self.observationGeneration), let scope = self.presentationScope else { return nil }
        return .init(scope: scope, observation: self.observationGeneration, connection: self.client.connectionEpoch, crew: self.linkedKnowledgeCrew)
    }, rpc: { [weak self] method, params in
        guard let self, self.client.capabilities.contains("data"), self.originalJournalReady,
              self.acceptsObservation(self.observationGeneration) else { throw CrewdKnowledgeStore.Failure.unavailable }
        // This adapter owns data UUIDs and pure lookup. Never wrap them in a second mutation envelope.
        return try await self.client.call(method, params)
    }, journal: { [weak self] in
        guard let self, self.originalJournalReady, let journal = self.originalJournal else { throw CrewdKnowledgeStore.Failure.unavailable }
        return journal
    }, nativeNavigation: { [weak self] reference, context in
        guard let self, reference.kind == "session", reference.namespace == context.namespace,
              self.knowledge.workspace?.context == context, context.crewId == self.crewID,
              self.acceptsObservation(self.observationGeneration), self.client.connected,
              self.sessions.contains(where: { $0["id"].string == reference.id }) else { return false }
        self.linkedKnowledge.invalidate()
        self.selectSession(reference.id)
        self.knowledgeNavigation = UUID()
        return true
    })
    private struct KnowledgeLinkPickerRoute: Equatable {
        let owner: UUID
        let crew: String
        let channel: String
        let source: String
    }
    private var knowledgeLinkPickerRoute: KnowledgeLinkPickerRoute?
    private func acceptsKnowledgeLinkPicker(_ route: KnowledgeLinkPickerRoute) -> Bool {
        acceptsObservation(observationGeneration) && client.capabilities.contains("data") && originalJournalReady &&
            !route.channel.isEmpty && crewID == route.crew && channelID == route.channel &&
            channelKnowledge.crew == route.crew && channelKnowledge.channel == route.channel &&
            crews.contains(where:{$0["id"] == .string(route.source)})
    }
    lazy var knowledgeLinkPicker = CrewdKnowledgeStore(origin:{ [weak self] in
        guard let self, let route = self.knowledgeLinkPickerRoute, self.acceptsKnowledgeLinkPicker(route),
              let scope = self.presentationScope else { return nil }
        return .init(scope:scope,observation:self.observationGeneration,connection:self.client.connectionEpoch,crew:route.source)
    },rpc:{ [weak self] method,params in
        guard let self, let route = self.knowledgeLinkPickerRoute, self.acceptsKnowledgeLinkPicker(route),
              ["data.workspace.get","data.object.list"].contains(method),
              (method == "data.workspace.get" ? params["crewId"] : params["context"]["crewId"]) == .string(route.source) else {
            throw CrewdKnowledgeStore.Failure.unavailable
        }
        return try await self.client.call(method,params)
    },journal:{ [weak self] in
        guard let self,self.originalJournalReady,let journal=self.originalJournal else { throw CrewdKnowledgeStore.Failure.unavailable }
        return journal
    })
    func makeWorkDocumentReader(crew: String) -> CrewdKnowledgeStore {
        let epoch = observationGeneration
        let connection = client.connectionEpoch
        let readScope = workStore.readScope
        let scope = presentationScope
        func current(_ workspace: CrewdWorkspace) -> Bool {
            workspace.acceptsObservation(epoch) && workspace.client.connectionEpoch == connection &&
                workspace.crewID == crew && workspace.workStore.readScope == readScope &&
                workspace.presentationScope == scope && workspace.originalJournalReady &&
                workspace.client.capabilities.contains("data.documentWorkLinks")
        }
        return CrewdKnowledgeStore(origin: { [weak self] in
            guard let self, let scope, current(self) else { return nil }
            return .init(scope: scope, observation: epoch, connection: connection, crew: crew)
        }, rpc: { [weak self] method, params in
            guard let self, current(self),
                  ["data.workspace.get", "data.object.list", "data.object.get"].contains(method),
                  (method == "data.workspace.get" ? params["crewId"] : params["context"]["crewId"]) == .string(crew) else {
                throw CrewdKnowledgeStore.Failure.unavailable
            }
            return try await self.client.call(method, params)
        }, journal: { [weak self] in
            guard let self, current(self), let journal = self.originalJournal else {
                throw CrewdKnowledgeStore.Failure.unavailable
            }
            return journal
        })
    }
    @Published private(set) var knowledgeNavigation = UUID()
    lazy var resources=CrewdResources { [weak self] method,params in
        guard let self, !self.retired else {throw CrewdClientError(message: "连接已切换，旧作用域不能提交。", kind: .notSubmitted)}
        return try await self.submitRPC(method,params)
    }
    lazy var channelResources=CrewdResources { [weak self] method,params in
        guard let self,!self.retired else{throw CrewdClientError(message:"连接已切换，旧作用域不能提交。",kind:.notSubmitted)}
        return try await self.submitRPC(method,params)
    }
    @Published var knowledgeLinkDrafts=CrewdChannelKnowledgeDrafts()
    lazy var channelKnowledge=CrewdChannelKnowledgeStore { [weak self] method,params in
        guard let self,!self.retired else{throw CrewdClientError(message:"原知识连接已切换。",kind:.notSubmitted)}
        return try await self.submitRPC(method,params)
    }
    @Published var todoMutationDrafts = CrewdTodoMutationDrafts()
    lazy var channelWork=CrewdChannelWorkStore { [weak self] method,params in
        guard let self, !self.retired else{throw CrewdClientError(message: "连接已切换，旧作用域不能提交。", kind: .notSubmitted)};return try await self.submitRPC(method,params)
    }
    @Published var channelLinkDrafts=CrewdChannelLinkDrafts()
    @Published var resourceDrafts=CrewdResourceDrafts()
    private var workDirty = true
    @Published var crews: [CrewdValue] = []
    @Published var sessions: [CrewdValue] = []
    @Published var channels: [CrewdValue] = []
    @Published private(set) var channelSummaries: [CrewdValue] = []
    @Published private(set) var crewAvatarMembers: [String:[CrewdValue]] = [:]
    @Published private(set) var crewActivity: [String: CrewdValue] = [:]
    @Published private(set) var activityRead: Set<String> = []
    private var activityDirty = true
    private var activityAffected: Set<String> = []
    private var activityChannelCrews: [String: String] = [:]
    struct TextSubmission:Identifiable {let id:UUID;let method:String;let params:CrewdValue;let snapshot:CrewdDrafts.Snapshot}
    @Published private(set) var uncertainTextSubmissions:[TextSubmission]=[]
    var uncertainTextSnapshots:[CrewdDrafts.Snapshot] {uncertainTextSubmissions.map(\.snapshot)}
    struct UncertainSessionStart { let request: UUID?; let snapshot: CrewdDrafts.Snapshot }
    @Published private(set) var uncertainSessionStartRecords: [UncertainSessionStart] = []
    var uncertainSessionStarts: [CrewdDrafts.Snapshot] { uncertainSessionStartRecords.map(\.snapshot) }
    func recordUncertainSessionStart(_ snapshot: CrewdDrafts.Snapshot, error: Error) {
        guard CrewdClientError.isUncertainSubmission(error) else { return }
        let request = (error as? CrewdClientError)?.data["requestId"].string.flatMap(UUID.init(uuidString:))
        let observation = request.flatMap { publicCommands.observations[$0] }
        let matches = observation?.envelope.operation == .sessionStart &&
            observation?.prepared.scope == presentationScope &&
            observation?.envelope.command["params"]["crewId"].string == snapshot.context.crew &&
            observation?.envelope.command["params"]["brief"].string == snapshot.text
        uncertainSessionStartRecords.append(.init(request: matches ? request : nil, snapshot: snapshot))
    }
    struct OriginalFirstMessage: Identifiable {
        enum Phase { case creating, created, posting, refused, unknown, accepted }
        let id: UUID // presentation identity, never a server request receipt
        let draftID: String
        let title: String
        let directory: String
        let text: String
        var parentCrewId: String? = nil
        var executor: String = "codex"
        var crewID: String?
        var phase: Phase
        var failure: String?
    }
    @Published var originalFirstMessages: [OriginalFirstMessage] = []
    @Published var originalPendingCrewDraft: PendingCrewDraft?
    @Published var crewCreationDrafts = CrewdCrewCreationDrafts()
    @Published var messageDrafts = CrewdDrafts()
    private var personalProjectionObserver:AnyCancellable?
    private var personalEditingScope:CrewPersonalDraftService.EditingScope?
    @Published private(set) var personalDraftContexts:[String:CrewdDataContext]=[:]
    var personalDrafts:CrewPersonalDraftService{.shared}
    private(set) var composerDrafts = CrewComposerDraftStore()
    @Published var requirementDrafts = CrewdRequirementDrafts()
    @Published var composerRequirementDrafts = CrewdRequirementDrafts()
    @Published var planDrafts = CrewdPlanDrafts()
    @Published var humanDrafts = CrewdHumanDrafts()
    @Published private(set) var knownSessions: [String:CrewdValue] = [:]
    @Published var pendingAsks: [CrewdValue] = []
    @Published var restartPrompt: CrewdValue = .null
    @Published var messages: [CrewdValue] = []
    @Published private(set) var controlTargets: [String: CrewdCommandTarget] = [:]
    @Published private(set) var executorProfiles: [CrewdExecutorProfile] = []
    @Published private(set) var executorProfilesFailure: String?
    @Published var selectedExecutor = "codex"
    private var executorProfilesQuery = UUID()
    private var executorProfilesRead: (crew: String, generation: UUID, connection: UUID, query: UUID, task: Task<Void, Never>)?
    private var executorProfilesBinding: (generation: UUID, connection: UUID, crew: String)?
    func executorProfile(_ executor: String) -> CrewdExecutorProfile? {
        guard let binding = executorProfilesBinding, acceptsObservation(binding.generation),
              binding.connection == client.connectionEpoch, binding.crew == crewID else { return nil }
        return executorProfiles.first { $0.executor == executor && $0.offeredInOriginalUI }
    }
    func requireExecutor(_ executor: String, crew: String) throws {
        guard !stopping, crew == crewID, let profile = executorProfile(executor), profile.canStart else {
            throw CrewdClientError(message: executorProfile(executor)?.unavailableDescription ?? "当前执行能力尚未核对，未发送启动请求。", kind: .notSubmitted)
        }
    }
    func profileForSession(_ id: String) -> CrewdExecutorProfile? {
        guard let row = sessions.first(where: { $0["id"].string == id }) ?? knownSessions[id],
              row["crewId"].string == crewID, let executor = row["executor"].string else { return nil }
        return executorProfile(executor)
    }
    private func requireSessionExecutor(_ id: String) throws {
        guard let profile = profileForSession(id) else {
            throw CrewdClientError(message: "原会话的执行能力尚未在当前机组核对；未提交恢复。", kind: .notSubmitted)
        }
        try requireExecutor(profile.executor, crew: crewID)
    }
    private func clearExecutorProfiles() {
        executorProfilesQuery = UUID()
        executorProfiles = []; executorProfilesFailure = nil; executorProfilesBinding = nil; executorProfilesRead = nil
    }
    func refreshExecutorProfiles() async {
        let epoch = generation, connection = client.connectionEpoch, crew = crewID
        guard acceptsObservation(epoch), !crew.isEmpty else { clearExecutorProfiles(); return }
        // The explicit create/start path and ordinary read-only sync can ask at
        // the same time. Share one read instead of invalidating each other.
        if let read = executorProfilesRead, read.crew == crew, read.generation == epoch, read.connection == connection {
            await read.task.value; return
        }
        let query = UUID(); executorProfilesQuery = query
        let task = Task { [weak self] in
            guard let self else { return }
            await self.loadExecutorProfiles(crew: crew, epoch: epoch, connection: connection, query: query)
        }
        executorProfilesRead = (crew, epoch, connection, query, task)
        await task.value
        if executorProfilesRead?.query == query { executorProfilesRead = nil }
    }
    private func loadExecutorProfiles(crew: String, epoch: UUID, connection: UUID, query: UUID) async {
        do {
            let response = try await client.call("executor.list", .object(["crewId": .string(crew)]))
            guard acceptsObservation(epoch), connection == client.connectionEpoch, crew == crewID, query == executorProfilesQuery else { return }
            let profiles = try CrewdExecutorProfile.list(response)
            executorProfiles = profiles; executorProfilesFailure = nil
            executorProfilesBinding = (epoch, connection, crew)
        } catch {
            guard acceptsObservation(epoch), connection == client.connectionEpoch, crew == crewID, query == executorProfilesQuery else { return }
            clearExecutorProfiles(); executorProfilesFailure = "无法读取当前后台的执行能力：" + error.localizedDescription
        }
    }
    @Published var crewID = ""
    @Published var sessionID = ""
    @Published var channelID = ""
    @Published var originalObservedUnread: [String: Int] = [:]
    private var observedMessageIDs = Set<String>()
    @Published var originalSection = "消息"
    @Published var inspection: CrewdValue = .null
    @Published var runnerItems: [CrewdValue] = []
    @Published var history = ""
    @Published var status = "未连接"
    @Published var error: String?
    @Published var busy = false
    @Published private(set) var stopping = false
    /// Immutable cache/draft namespace. The pairing transport must separately
    /// authenticate the peer; a namespace UUID is never authentication evidence.
    let authorityRecordID: UUID
    let dataDirectory: URL?
    typealias Connector = @MainActor (CrewdClient) async throws -> Void
    private let connector: Connector
    private var retired = false
    var isRetired: Bool { retired }
    private var generation = UUID()
    var observationGeneration: UUID { generation }
    var observationSelection: UUID { selection }
    func acceptsObservation(_ epoch: UUID) -> Bool {
        !retired && !sleeping && client.readyForWork && generation == epoch
    }
    private var selection = UUID()
    private var sessionDirectoryQuery = UUID()
    private var crewDirectoryQuery = UUID()
    private var sync: Task<Void, Never>?
    private var reconnect: Task<Void, Never>?
    private var reconnectID: UUID?
    private var remoteRecoveryPending = false
    private var lastRecoveryWake: Date?
    private var recoveryActivation: AnyCancellable?
    #if os(macOS)
    private var recoveryWake: AnyCancellable?
    #endif
    private var recoveryPath: NWPathMonitor?
    private var previousPathSatisfied: Bool?
    #if os(macOS)
    private var backend: Process?
    @Published private var ownedCodexNetworkRouteStatus: CrewdCodexConfiguration.NetworkRouteStatus?
    private var ownedChildIdentity: CrewdKernelProcessIdentity?
    private struct OwnedIPCBinding {
        let generation: UUID
        let peer: CrewdKernelProcessIdentity
        let dbID: String
        let runtimeBindingID: String
    }
    private var ownedIPCBinding: OwnedIPCBinding?
    private var ownedLifecycle: CrewdOwnedLocalLifecycle?
    #endif
    private var again = false
    private var recoverNeeded = false
    private var sleeping = false
    private var booting = false

    #if os(macOS)
    convenience init(dataDirectory: URL, authorityRecordID: UUID = UUID()) {
        self.init(authorityRecordID: authorityRecordID, dataDirectory: dataDirectory, connector: { client in
            try await client.connect(path: dataDirectory.appendingPathComponent("crewd.sock").path)
        })
    }
    #endif
    /// A local folder chooser starts inside the selected authority's private
    /// presentation root. Remote paths are opaque server paths, never a local
    /// filesystem chooser or a fallback to Documents/CrewGround.
    func localDirectoryPickerRoot() throws -> URL {
        guard !retired, client.connected, let dataDirectory else {
            throw CrewdClientError(message: "请明确填写所连接电脑上的完整工作目录，不能浏览本机目录。", kind: .notSubmitted)
        }
        return try CrewdPresentationJournal.prepareUICache(directory: dataDirectory)
    }

    /// One instance per paired authority. Switching authorities retires this
    /// instance and creates another; public W/#/H numbers never share caches.
    convenience init(authorityRecordID: UUID, connect: @escaping Connector) {
        self.init(authorityRecordID: authorityRecordID, dataDirectory: nil, connector: connect)
    }
    private init(authorityRecordID: UUID, dataDirectory: URL?, connector: @escaping Connector) {
        self.authorityRecordID = authorityRecordID
        self.dataDirectory = dataDirectory
        self.connector = connector
        client.onManualDisconnect = { [weak self] in
            self?.remoteRecoveryPending=false;self?.reconnect?.cancel();self?.reconnectID=nil
            if let self,self.dataDirectory == nil,!self.retired {self.status="连接已断开"}
        }
        client.onEvent = { [weak self] name, data in
            guard let self, !self.retired else { return }
            self.artifacts.handleEvent(name,data,connection:self.client.connectionEpoch)
            self.managedTablesV2.handleEvent(name,data,connection:self.client.connectionEpoch)
            self.adminAskGroups.handleEvent(name,data,connection:self.client.connectionEpoch)
            self.managedTablesV3.handleEvent(name,data,connection:self.client.connectionEpoch)
            switch name {
            case "artifact.changed","artifact.historyChanged","links.changed",CrewdAdminAskGroupsWire.changed,CrewdAdminAskGroupsWire.invalidated:return
            case "onto.changed":
                if self.client.capabilities.contains("managedObjects.events.v1") {
                    self.managedDirectory.handleEvent(name,data,connection:self.client.connectionEpoch)
                }
                return
            case "events.lagged", "crew.changed", "crew.purged", "channel.changed", "channel.purged":
                self.managedDirectory.handleEvent(name,data,connection:self.client.connectionEpoch)
            default: break
            }
            if name == "message.appended" { self.observeFirstCaptainReply(data) }
            if name == "message.appended", let id = data["id"].string, self.observedMessageIDs.insert(id).inserted,
               let sender = data["senderSessionId"].string, sender != self.sessionID {
                self.originalObservedUnread[sender, default: 0] += 1
            }
            self.knowledge.handleEvent(name, data)
            self.linkedKnowledge.handleEvent(name,data)
            if self.knowledgeLinkPickerRoute != nil { self.knowledgeLinkPicker.handleEvent(name,data) }
            self.channelKnowledge.handleEvent(name,data)
            self.authorityEvents.send((name, data))
            if name=="personalDraft.changed" || name=="events.lagged"{CrewPersonalDraftService.shared.coreChanged(self)}
            self.resources.handleEvent(name,data)
            self.channelResources.handleEvent(name,data)
            self.channelWork.handleEvent(name,data)
            if name == "document.changed" || name == "viewFolder.changed" || name == "channel.workChanged" { return }
            if name.hasPrefix("message.") {
                if let channel=data["channelId"].string,let crew=self.activityChannelCrews[channel] {
                    if !self.activityDirty || !self.activityAffected.isEmpty {self.activityAffected.insert(crew)}
                } else {self.activityAffected.removeAll()}
                self.activityDirty=true
            } else if name.hasPrefix("crew.") || name == "events.lagged" {self.activityDirty=true;self.activityAffected.removeAll()}
            if name.hasPrefix("work.") || name.hasPrefix("ask.") || name == "todo.attributesChanged" || name.hasPrefix("crew.") || name == "events.lagged" { self.workDirty = true }
            if name == "session.runnerItem", data["sessionId"].string == self.sessionID {
                let item = data["item"]
                if let id = item["id"].string, let index = self.runnerItems.firstIndex(where: {$0["id"].string == id}) { self.runnerItems[index] = item }
                else { self.runnerItems.append(item); if self.runnerItems.count > 200 { self.runnerItems.removeFirst() } }
            } else if name == "system.shutdown" {
                self.closeTerminalDisplays()
                self.knowledge.invalidate(); self.linkedKnowledge.invalidate(); self.closeKnowledgeLinkPicker(); self.channelKnowledge.invalidate(); self.resources.invalidateConnection(); self.channelResources.invalidateConnection(); self.channelWork.invalidateConnection(); self.workStore.invalidateConnection(); self.sleeping = true; self.status = "后台已休眠"; self.reconnect?.cancel(); self.reconnectID = nil
                self.again = false; self.recoverNeeded = false; self.sync?.cancel()
            } else { self.scheduleSync(recoverHistory: name == "events.lagged") }
        }
        client.onDisconnect = { [weak self] in
            self?.closeTerminalDisplays()
            self?.clearExecutorProfiles()
            guard let self, !self.retired, !self.stopping, !self.booting, !self.sleeping, self.reconnectID == nil else { return }
            self.knowledge.invalidate(); self.linkedKnowledge.invalidate(); self.closeKnowledgeLinkPicker(); self.channelKnowledge.invalidate(); self.resources.invalidateConnection(); self.channelResources.invalidateConnection(); self.channelWork.invalidateConnection(); self.workStore.invalidateConnection(); self.workDirty = true
            self.error = self.client.lastError
            if self.dataDirectory == nil {
                self.remoteRecoveryPending = self.client.recoveryDisposition == .transport
                guard self.remoteRecoveryPending else {self.status="连接已中断，请核对电脑身份或授权";return}
            }
            self.beginAutomaticReconnect(delays:[1,2,4,8,8])
        }
        if dataDirectory == nil { observeRemoteRecovery() }
    }

    private func recoverableRemoteFailure(_ error:Error)->Bool {
        if error is CancellationError || error is CrewdRemoteError {return false}
        if let network=error as? NWError {return CrewdClient.recovery(for:network) == .transport}
        return error is CrewdClientError && client.recoveryDisposition == .transport
    }
    private func beginAutomaticReconnect(delays:[Int]) {
        guard !retired,!stopping,!booting,!sleeping,reconnectID == nil else{return}
        status="连接中断，正在重新连接"
        let attempt=UUID();reconnectID=attempt
        reconnect=Task { [weak self] in
            defer {if self?.reconnectID == attempt {self?.reconnectID=nil}}
            for delay in delays {
                if delay > 0 {try? await Task.sleep(for:.seconds(delay))}
                guard !Task.isCancelled,let self,!self.retired,!self.stopping,!self.sleeping,self.reconnectID == attempt else{return}
                if self.dataDirectory == nil && (!self.remoteRecoveryPending || self.client.recoveryDisposition != .transport) {self.remoteRecoveryPending=false;return}
                do {try await self.connectOnly();self.remoteRecoveryPending=false;return}
                catch {
                    guard !Task.isCancelled,!self.retired,!self.sleeping,self.reconnectID == attempt else{return}
                    self.error=error.localizedDescription
                    if self.dataDirectory == nil && !self.recoverableRemoteFailure(error) {
                        self.remoteRecoveryPending=false;self.status="连接已中断，请核对电脑身份或授权";return
                    }
                }
            }
            if self?.reconnectID == attempt {self?.status="连接已中断"}
        }
    }
    /// A wake grants one bounded read-only connection burst to this original
    /// remote workspace. It never selects a different record or replays work.
    func recoverRemoteTransportIfNeeded() {
        guard dataDirectory == nil,!retired,!stopping,!sleeping,!booting,!client.readyForWork,reconnectID == nil,remoteRecoveryPending else{return}
        guard client.recoveryDisposition == .transport else{remoteRecoveryPending=false;return}
        let wait=lastRecoveryWake.map{max(0,Int(ceil(30-Date().timeIntervalSince($0))))} ?? 0
        lastRecoveryWake=Date().addingTimeInterval(TimeInterval(wait))
        beginAutomaticReconnect(delays:[wait,1,2,4,8])
    }
    private func observeRemoteRecovery() {
        #if os(macOS)
        let active=NSApplication.didBecomeActiveNotification
        recoveryWake=NSWorkspace.shared.notificationCenter.publisher(for:NSWorkspace.didWakeNotification).sink { [weak self] _ in
            Task { @MainActor [weak self] in self?.recoverRemoteTransportIfNeeded() }
        }
        #elseif os(iOS)
        let active=UIApplication.didBecomeActiveNotification
        #endif
        recoveryActivation=NotificationCenter.default.publisher(for:active).sink { [weak self] _ in
            Task { @MainActor [weak self] in self?.recoverRemoteTransportIfNeeded() }
        }
        let monitor=NWPathMonitor();recoveryPath=monitor
        monitor.pathUpdateHandler={ [weak self] path in
            let satisfied=path.status == .satisfied
            Task { @MainActor [weak self] in
                guard let self,!self.retired else{return}
                let restored=self.previousPathSatisfied == false && satisfied
                self.previousPathSatisfied=satisfied
                if restored {self.recoverRemoteTransportIfNeeded()}
            }
        }
        monitor.start(queue:DispatchQueue(label:"pendingcrew.remote.recovery"))
    }

    /// Retain unknown inputs in this old scope for inspection; never replay them
    /// into the next authority or let a late reconnect reactivate a retired one.
    #if os(macOS)
    private var retiredManagementTarget: CrewdRetiredManagement.Target?
    private let retiredStopChannel = CrewdRetiredStopChannel()
    #endif
    func retire() {
        #if os(macOS)
        if managementRole == "owner", dataDirectory != nil, client.connected,
           (try? client.verifiedLocalPeerIdentity()) != nil,
           let database = client.runtimeIdentity["dbId"].string,
           let instance = client.runtimeIdentity["runtimeBindingId"].string {
            retiredManagementTarget = .init(database: database, instance: instance)
        }
        #endif
        closeTerminalDisplays()
        clearExecutorProfiles()
        retired = true; sleeping = true; generation = UUID(); selection = UUID()
        remoteRecoveryPending=false;recoveryActivation?.cancel();recoveryActivation=nil;recoveryPath?.cancel();recoveryPath=nil
        #if os(macOS)
        recoveryWake?.cancel();recoveryWake=nil
        #endif
        sync?.cancel(); sync = nil; reconnect?.cancel(); reconnect = nil; reconnectID = nil
        again = false; recoverNeeded = false
        knowledge.invalidate(); linkedKnowledge.invalidate(); closeKnowledgeLinkPicker(); channelKnowledge.invalidate(); resources.invalidateConnection(); channelResources.invalidateConnection(); channelWork.invalidateConnection(); workStore.invalidateConnection()
        client.retire(); status = "已切换连接"
    }
    func connect() async {
        guard !retired, !booting, !stopping else { return }
        #if os(macOS)
        if let dataDirectory { await connectLocal(dataDirectory); return }
        #endif
        booting = true; defer { booting = false }
        reconnect?.cancel(); reconnectID = nil; sleeping = false; error = nil; status = "正在连接"
        remoteRecoveryPending=false
        do { try await connectOnly() }
        catch { if !(error is CancellationError), !retired, !Task.isCancelled { self.error = error.localizedDescription; status = "连接失败";if dataDirectory == nil {remoteRecoveryPending=recoverableRemoteFailure(error)} } }
    }
    #if os(macOS)
    /// Explicit local launch uses an independent root; no legacy importer runs.
    private func connectLocal(_ dataDirectory: URL) async {
        guard !booting else { return }
        booting = true; defer { booting = false }
        reconnect?.cancel(); reconnectID = nil; sleeping = false; error = nil; status = "正在连接"
        let legacy = (ProcessInfo.processInfo.environment["PENDINGCREW_LEGACY_DATA_DIR"] ?? ProcessInfo.processInfo.environment["PENDINGCREW_DATA_DIR"]).map { URL(fileURLWithPath: $0) } ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/PendingCrew")
        let candidatePath = dataDirectory.standardizedFileURL.resolvingSymlinksInPath().path
        let legacyPath = legacy.standardizedFileURL.resolvingSymlinksInPath().path
        guard candidatePath != legacyPath, !candidatePath.hasPrefix(legacyPath + "/") else {
            error = "首版试用须使用独立数据目录，不能与旧版共用。"; status = "连接未建立"; return
        }
        do { try await connectOnly(); return } catch {
            guard !sleeping, !Task.isCancelled else { return }
            if let failure = error as? CrewdClientError, failure.kind == .backendHandshake {
                #if PENDINGCREW_LOCAL_MAC
                self.error = "当前运行尚未能连接。若上次运行还未结束，请稍后从「电脑 → 本机」重连；不会自动恢复会话。\n" + failure.localizedDescription
                status = "尚未连接，可稍后重连"
                #else
                self.error = failure.localizedDescription; status = "后台协议连接失败"
                #endif
                return
            }
            guard backend?.isRunning != true else { self.error = error.localizedDescription; status = "后台连接失败"; return }
        }
        do {
            let env = ProcessInfo.processInfo.environment
            let program = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/crewd")
            guard FileManager.default.isExecutableFile(atPath: program.path) else { throw CrewdClientError(message: "未找到配套后台程序，请使用完整的应用包。") }
            // Fake providers are ordinary explicit validation inputs, never a product requirement.
            let codex = CrewdCodexConfiguration.program(environment: env)
            let codexRoute = CrewdCodexConfiguration.networkRoute(program:codex,environment:env)
            let p = Process(); p.executableURL = program
            p.arguments = CrewdCodexConfiguration.launchArguments(directory:dataDirectory,program:codex,networkRoute:codexRoute)
            p.environment = CrewdCodexConfiguration.childEnvironment(env)
            // crewd owns a lock and its sessions. GUI reconnect never imports data or replays tasks.
            let logs = dataDirectory.appendingPathComponent("client-backend.log")
            if !FileManager.default.fileExists(atPath: dataDirectory.path) {
                try FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            }
            if !FileManager.default.fileExists(atPath: logs.path) { FileManager.default.createFile(atPath: logs.path, contents: nil, attributes: [.posixPermissions: 0o600]) }
            let log = try FileHandle(forWritingTo: logs); try log.seekToEnd()
            p.standardOutput = log; p.standardError = log
            try p.run(); backend = p
            ownedCodexNetworkRouteStatus = codexRoute.status
            // A failed birth observation leaves the helper unconfirmed; never
            // infer ownership from the launch path or an advertised PID.
            ownedChildIdentity = try? CrewdKernelProcessIdentity.ownChild(p.processIdentifier)
            try log.close()
            for _ in 0..<30 {
                try await Task.sleep(for: .milliseconds(100))
                do { try await connectOnly(); await claimManagement(); return } catch { if sleeping || Task.isCancelled { return } }
                if !p.isRunning { throw CrewdClientError(message: "后台启动失败，请查看连接信息中的日志。") }
            }
            throw CrewdClientError(message: "后台启动后未能建立连接，请检查后台日志。")
        } catch {
            guard !sleeping, !Task.isCancelled else { return }
            self.error = error.localizedDescription; status = "连接失败"
        }
    }

    /// Known only for the locally launched Process currently verified over IPC.
    var currentLocalCodexNetworkRouteStatus: CrewdCodexConfiguration.NetworkRouteStatus? {
        hasVerifiedOwnedLocalHelper ? ownedCodexNetworkRouteStatus : nil
    }

    /// Business retirement never transfers ownership. Only the independently
    /// captured original-child lifecycle may reconnect for bounded owned Quit.
    @Published private(set) var managementRole = "observer"
    @Published private(set) var managementOwnerPresent = false
    @Published private(set) var previousExit = "clean"
    func readManagementStatus() async {
        guard dataDirectory != nil, client.connected else { return }
        do { applyManagement(try await client.call("lifecycle.status", .object([:]))) }
        catch { managementRole = "observer"; self.error = "无法核对当前本机管理资格。" }
    }
    private func applyManagement(_ value: CrewdValue) {
        managementRole = value["role"].string ?? "observer"
        managementOwnerPresent = value["ownerPresent"] == .bool(true)
        previousExit = value["previousExit"].string ?? "clean"
    }
    func claimManagement() async {
        guard dataDirectory != nil, client.connected else { return }
        do { applyManagement(try await client.call("lifecycle.claim", .object([:]))) }
        catch { self.error = "当前管理权不可取得；保留观察连接。"; await readManagementStatus() }
    }
    func releaseManagement() async throws {
        guard dataDirectory != nil, client.connected else { return }
        let status = try await client.call("lifecycle.status", .object([:]))
        applyManagement(status)
        if managementRole == "owner" { applyManagement(try await client.call("lifecycle.release", .object([:]))) }
    }
    var canForceManagedStop: Bool {
        guard dataDirectory != nil else { return false }
        if retired { return retiredManagementTarget != nil }
        return client.readyForWork && managementRole == "owner" && (try? client.verifiedLocalPeerIdentity()) != nil
    }
    @discardableResult func exitOwnedLocalHelper(mode: CrewdQuitMode) async throws -> Bool {
        guard let directory = dataDirectory else { return false }
        stopping = true; sleeping = true
        reconnect?.cancel(); sync?.cancel(); closeTerminalDisplays(); knowledge.invalidate(); closeKnowledgeLinkPicker()
        status = mode == .direct ? "正在强制停止" : "正在叫他们停工"
        if retired {
            guard let target = retiredManagementTarget else { return false } // Former observer, no management authority.
            return try await retiredStopChannel.exit(target: target, mode: mode) {
                let next = CrewdClient()
                do {
                    try await next.connect(path: directory.appendingPathComponent("crewd.sock").path)
                    _ = try next.verifiedLocalPeerIdentity()
                    guard let database = next.runtimeIdentity["dbId"].string,
                          let instance = next.runtimeIdentity["runtimeBindingId"].string else { throw CrewdRetiredManagement.Failure.unconfirmed }
                    let actual = CrewdRetiredManagement.Target(database: database, instance: instance)
                    let epoch = next.connectionEpoch
                    return .init(target: actual, call: { method, params in
                        guard next.connected, next.connectionEpoch == epoch,
                              next.runtimeIdentity["dbId"].string == actual.database,
                              next.runtimeIdentity["runtimeBindingId"].string == actual.instance else { throw CrewdRetiredManagement.Failure.unconfirmed }
                        _ = try next.verifiedLocalPeerIdentity()
                        return try await next.call(method, params, timeout: method == "lifecycle.exit" && params["mode"].string == "safePause" ? .seconds(140) : .seconds(30))
                    }, close: { next.retire() })
                } catch { next.retire(); throw error }
            }
        }
        // Current authenticated management scope, independent of any historic GUI Process handle.
        guard client.connected, (try? client.verifiedLocalPeerIdentity()) != nil else {
            throw CrewdClientError(message: "当前本机后台认证连接不可用，尚未提交退出。")
        }
        applyManagement(try await client.call("lifecycle.status", .object([:])))
        // A concurrent observer exits its own UI only; it cannot stop another owner's work.
        guard managementRole == "owner" else { return false }
        sleeping = true; reconnect?.cancel(); sync?.cancel()
        do {
            let reply = try await client.call("lifecycle.exit", .object([
                "mode": .string(mode == .direct ? "direct" : "safePause"), "timeoutMs": .integer(120000)
            ]), timeout: mode == .direct ? .seconds(30) : .seconds(140))
            guard reply["status"].string == "stopped" else {
                throw CrewdClientError(message: "后台未在期限内确认停止；原操作保留待核对，应用仍保持打开。")
            }
        } catch { self.error = error.localizedDescription; throw error }
        return true
    }
    func quitOwnedLocalHelper() async throws {
        guard let owned = backend, owned.isRunning else { return }
        guard let lifecycle = ownedLifecycle, lifecycle.matches(owned, birth: ownedChildIdentity) else {
            throw CrewdClientError(message: "本 App 的后台尚无原内核连接与就绪历史的关闭资格；保留待确认。", kind: .notSubmitted)
        }
        sleeping = true; reconnect?.cancel(); sync?.cancel()
        do { try await lifecycle.quit(crew: crewID) }
        catch {
            if !retired, backend === owned { sleeping = false }
            throw error
        }
        // A late original close cannot clear a replacement Process or scope.
        if backend === owned, !owned.isRunning {
            backend = nil; ownedChildIdentity = nil; ownedIPCBinding = nil; ownedCodexNetworkRouteStatus = nil
            if ownedLifecycle === lifecycle { ownedLifecycle = nil }
        }
    }
    private func captureOwnedLifecycle() {
        guard hasVerifiedOwnedLocalHelper, originalJournalReady,
              let owned = backend, let birth = ownedChildIdentity,
              let journal = originalJournal, let directory = dataDirectory,
              let origin = try? publicCommandBinding(for: client) else { return }
        if ownedLifecycle?.matches(owned, birth: birth) == true { return }
        // Capture only while the original business connection/journal is ready.
        // This object exposes no crew, message, terminal or remote write API.
        ownedLifecycle = try? CrewdOwnedLocalLifecycle.live(owned: owned, birth: birth,
                                                           origin: origin, journal: journal, directory: directory)
    }
    private func associateOwnedIPC(_ epoch: UUID) {
        guard acceptsObservation(epoch) else { return }
        ownedIPCBinding = nil
        guard let process = backend, process.isRunning, let birth = ownedChildIdentity,
              process.processIdentifier == birth.pid,
              (try? CrewdKernelProcessIdentity.ownChild(birth.pid)) == birth,
              let peer = try? client.verifiedLocalPeerIdentity(), peer == birth,
              let db = client.runtimeIdentity["dbId"].string, !db.isEmpty, db.utf8.count <= 256,
              let instance = client.runtimeIdentity["runtimeBindingId"].string, UUID(uuidString: instance) != nil else { return }
        ownedIPCBinding = OwnedIPCBinding(generation: epoch, peer: peer, dbID: db, runtimeBindingID: instance)
        // Either journal readiness or verified peer association may arrive first.
        captureOwnedLifecycle()
    }
    #endif
    private func connectOnly() async throws {
        try Task.checkCancellation()
        guard !retired else { throw CrewdClientError(message: "连接已切换，旧作用域不能重新提交。", kind: .notSubmitted) }
        clearExecutorProfiles()
        firstCaptainReplies.removeAll()
        knowledge.invalidate(); linkedKnowledge.invalidate(); closeKnowledgeLinkPicker(); channelKnowledge.invalidate(); resources.invalidateConnection(); channelResources.invalidateConnection(); channelWork.invalidateConnection(); workStore.invalidateConnection(); workDirty = true; activityDirty = true
        closeTerminalDisplays()
        generation = UUID(); let current = generation; sync?.cancel(); sync = nil
        #if os(macOS)
        ownedIPCBinding = nil
        #endif
        try await connector(client)
        guard generation == current, !retired, !Task.isCancelled, !sleeping else {
            if retired { client.disconnect() }
            throw CancellationError()
        }
        workStore.configureCapabilities(client.capabilities)
        resources.configureCapabilities(client.capabilities)
        channelResources.configureCapabilities(client.capabilities)
        channelWork.configureCapabilities(client.capabilities)
        guard generation == current, !Task.isCancelled, !sleeping else { throw CancellationError() }
        error = nil
        status = "已连接 · crewd \(client.version)"
        scheduleSync(recoverHistory: true)
        await sync?.value
        guard generation == current, !Task.isCancelled, !sleeping else { throw CancellationError() }
        guard client.connected else { throw CrewdClientError(message: client.lastError ?? "后台连接已中断。") }
        #if os(macOS)
        associateOwnedIPC(current)
        await readManagementStatus()
        #endif
    }

    func refreshSessionDirectory(_ crew: String) async throws {
        guard !retired else { throw CrewdClientError(message: "连接作用域已关闭，尚未提交。", kind: .notSubmitted) }
        let current = generation, selected = selection, query = UUID()
        sessionDirectoryQuery = query
        let response = try await client.call("session.list", .object(["crewId": .string(crew)]))
        guard !retired, current == generation, selected == selection, sessionDirectoryQuery == query,
              crewID == crew, !Task.isCancelled else { throw CancellationError() }
        guard case .array(let rows) = response else { throw CrewdClientError(message: "成员列表回复格式无效。") }
        sessions = rows
        for row in rows { if let id = row["id"].string { knownSessions[id] = row } }
    }

    func refreshCrewDirectory() async throws {
        guard !retired else { throw CrewdClientError(message: "连接作用域已关闭，尚未提交。", kind: .notSubmitted) }
        let current = generation, selected = selection, query = UUID()
        crewDirectoryQuery = query
        let response = try await client.call("crew.list")
        guard !retired, current == generation, selected == selection, crewDirectoryQuery == query,
              !Task.isCancelled else { throw CancellationError() }
        guard case .array(let rows) = response else { throw CrewdClientError(message: "机组列表回复格式无效。") }
        crews = rows
    }

    func selectCrew(_ id: String) {
        guard crewID != id else { return }
        knowledge.invalidate(); linkedKnowledge.invalidate(); linkedKnowledgeCrew=""; closeKnowledgeLinkPicker(); channelKnowledge.selectChannel(crew:id,channel:""); workStore.selectCrew(id); resources.selectCrew(id); channelResources.selectScope(crew:id,channel:""); workDirty = true
        channelWork.selectChannel(crew:id,channel:"")
        clearExecutorProfiles(); selectedExecutor = "codex"
        selection = UUID(); crewID = id; sessionID = ""; channelID = ""; sessions = []; channels = []; messages = []; inspection = .null; runnerItems = []; history = ""
        scheduleSync(recoverHistory: true)
    }
    func readKnowledgeLinkSource(owner:UUID,crew:String,channel:String,source:String) async throws {
        let route = KnowledgeLinkPickerRoute(owner:owner,crew:crew,channel:channel,source:source)
        guard acceptsKnowledgeLinkPicker(route) else { throw CrewdClientError(message:"源机组或当前群不可读。",kind:.notSubmitted) }
        if knowledgeLinkPickerRoute != route { knowledgeLinkPickerRoute=route; knowledgeLinkPicker.invalidate() }
        await knowledgeLinkPicker.refreshViewCatalog(includeArchived:false)
        guard !Task.isCancelled, knowledgeLinkPickerRoute == route, acceptsKnowledgeLinkPicker(route) else { throw CancellationError() }
    }
    func closeKnowledgeLinkPicker(owner:UUID?=nil) {
        guard let route=knowledgeLinkPickerRoute,owner == nil || route.owner == owner else { return }
        knowledgeLinkPickerRoute=nil; knowledgeLinkPicker.invalidate()
    }
    func openLinkedKnowledge(_ item:CrewdChannelKnowledgeItem)async throws {
        guard !retired,item.isReadableActiveView,!item.source.context.crewId.isEmpty,
              channelKnowledge.items.contains(item) else{throw CrewdClientError(message:"本群知识引用已变化，请重新读取。",kind:.notSubmitted)}
        let epoch=observationGeneration,route=selection
        if linkedKnowledgeCrew != item.source.context.crewId {linkedKnowledge.invalidate();linkedKnowledgeCrew=item.source.context.crewId}
        await linkedKnowledge.refresh(loadViews:false)
        guard acceptsObservation(epoch),selection == route,let context=linkedKnowledge.workspace?.context,
              CrewdChannelKnowledgeReference(context:context,reference:item.source.reference).identity == item.id else{throw CrewdClientError(message:"知识来源上下文已变化，未将历史引用当作当前正文。")}
        await linkedKnowledge.open(item.source.reference)
        guard acceptsObservation(epoch),selection == route,
              linkedKnowledge.detail?.object.identity == item.source.reference.id else{throw CrewdClientError(message:"原知识正文未能完整读取。")}
    }
    func selectConversation(_ id: String) {
        selectChannel(id)
        if let summary = channelSummaries.first(where: {$0["channel"]["id"].string == id}), summary["channel"]["kind"].string == "dm",
           let member = summary["members"].array.first(where: {$0["kind"].string == "session"})?["id"].string { selectSession(member) }
    }
    var selectedMembers: [CrewdValue] { channelSummaries.first(where: {$0["channel"]["id"].string == channelID})?["members"].array ?? [] }
    func selectSession(_ id: String) { guard sessionID != id else { return }; selection = UUID(); sessionID = id; inspection = .null; runnerItems = []; history = ""; scheduleSync(recoverHistory: true) }
    /// Called once after an explicitly created crew has a confirmed identity.
    /// A startup failure must not turn a successful creation into a retry that
    /// creates a second crew. No appearance/reconnect hook calls this method.
    @Published private(set) var captainStartFailures: [String: String] = [:]
    @Published private var firstCaptainReplies: [String: CrewdFirstCaptainReply] = [:]
    private var openingCaptains = Set<String>()
    func firstCaptainReply(in crew: String) -> CrewdFirstCaptainReply? {
        guard !retired, !sleeping, client.readyForWork, crewID == crew,
              let progress = firstCaptainReplies[channelID], progress.crewID == crew else { return nil }
        return progress
    }
    private func observeFirstCaptainReply(_ message: CrewdValue) {
        guard let channel = message["channelId"].string, let progress = firstCaptainReplies[channel],
              progress.isReply(channel: channel, senderKind: message["senderKind"].string,
                               sender: message["senderSessionId"].string, sequence: message["seq"].int,
                               inReplyTo: message["inReplyTo"].string) else { return }
        firstCaptainReplies[channel] = nil
    }
    @discardableResult func startCaptainForCreatedCrew(_ id: String, executor: String = "codex", brief: String = "读取本机组群聊与待办，回应人类需求；没有任务时待命。",
                                                     firstMessage: CrewdFirstCaptainReply? = nil) async -> String? {
        guard !openingCaptains.contains(id), !isRetired else { return nil }
        openingCaptains.insert(id)
        let epoch = observationGeneration, connection = client.connectionEpoch
        let firstMessage = firstMessage.flatMap { $0.crewID == id ? $0 : nil }
        if let firstMessage, acceptsObservation(epoch) { firstCaptainReplies[firstMessage.channelID] = firstMessage }
        defer {
            openingCaptains.remove(id)
            if let firstMessage, acceptsObservation(epoch), client.connectionEpoch == connection,
               firstCaptainReplies[firstMessage.channelID]?.phase == .starting {
                firstCaptainReplies[firstMessage.channelID] = nil
            }
        }
        func recordCaptain(_ captain: String, launched: Bool) {
            guard !captain.isEmpty, let firstMessage, acceptsObservation(epoch), client.connectionEpoch == connection,
                  var progress = firstCaptainReplies[firstMessage.channelID] else { return }
            progress.phase = launched ? .launched(captain) : .registered(captain)
            firstCaptainReplies[firstMessage.channelID] = progress
            if crewID == id && channelID == firstMessage.channelID { messages.forEach(observeFirstCaptainReply) }
        }
        selectCrew(id)
        let route = selection
        await refreshExecutorProfiles()
        guard acceptsObservation(epoch), connection == client.connectionEpoch, selection == route, crewID == id else { return nil }
        do {
            try requireExecutor(executor, crew: id)
            let roster = try await readValue("session.list", .object(["crewId": .string(id)]))
            guard acceptsObservation(epoch), connection == client.connectionEpoch, selection == route, crewID == id else { return nil }
            guard case .array(let rows) = roster else { throw CrewdClientError(message: "机长名册未能确认。", kind: .notSubmitted) }
            if let captain = rows.first(where: { $0["role"].string == "captain" }) {
                captainStartFailures[id] = nil
                if let session = captain["id"].string { recordCaptain(session, launched: false) }
                return captain["id"].string
            }
            guard !hasOriginalUnknown(method: "session.start", crew: id) else {
                throw CrewdClientError(message: "机长原启动结果尚未确认，请核对原请求。", kind: .notSubmitted)
            }
            let result = try await performValue("session.start", .object([
                "crewId": .string(id), "role": .string("captain"), "executor": .string(executor),
                "brief": .string(firstMessage?.startupBrief ?? brief), "isolation": .bool(false)]))
            guard acceptsObservation(epoch), client.connectionEpoch == connection else { return nil }
            captainStartFailures[id] = nil
            if let session = result["id"].string {
                recordCaptain(session, launched: (result["birth"]["pid"].int ?? 0) > 0)
            }
            return result["id"].string
        } catch {
            if acceptsObservation(epoch), connection == client.connectionEpoch,
               firstMessage != nil || (selection == route && crewID == id) {
                let message = "机组及已有消息仍保留；机长尚未确认启动：" + error.localizedDescription
                captainStartFailures[id] = message
                if selection == route && crewID == id { self.error = message }
            }
            return nil
        }
    }

    func selectChannel(_ id: String) { if channelID != id {closeKnowledgeLinkPicker()};selection = UUID(); channelID = id;channelWork.selectChannel(crew:crewID,channel:id);channelResources.selectScope(crew:crewID,channel:id);channelKnowledge.selectChannel(crew:crewID,channel:id);linkedKnowledge.invalidate(); messages = []; scheduleSync() }

    func scheduleSync(recoverHistory: Bool = false) {
        guard !sleeping, client.connected else { return }
        again = true
        if recoverHistory { recoverNeeded = true; runnerItems = []; history = "" }
        guard sync == nil else { return }
        let current = generation
        sync = Task { [weak self] in
            guard let self, self.generation == current, !Task.isCancelled else { return }
            repeat {
                self.again = false
                let recover = self.recoverNeeded; self.recoverNeeded = false
                do { try await self.refresh(recoverHistory: recover) }
                catch { if self.generation == current && !self.sleeping && !Task.isCancelled { self.error = error.localizedDescription } }
            } while self.again && !self.sleeping && self.generation == current && !Task.isCancelled
            if self.generation == current { self.sync = nil }
        }
    }

    private func refresh(recoverHistory: Bool) async throws {
        guard !sleeping, !Task.isCancelled else { return }
        let current = generation, selected = selection, crewQuery = UUID()
        crewDirectoryQuery = crewQuery
        let newCrews = try await client.call("crew.list")
        guard current == generation, !sleeping, !Task.isCancelled else { return }
        guard selected == selection, crewDirectoryQuery == crewQuery else { again = true; return }
        crews = newCrews.array
        if activityDirty {
            activityDirty = false
            // Only message/crew changes and reconnect request this cached snapshot,
            // never picker changes or runner-item renders. No UI body performs I/O.
            do {
                var snapshot = crewActivity, read = activityRead
                let affected=activityAffected; activityAffected.removeAll()
                for row in crews {
                    guard let id = row["id"].string else { continue }
                    if !affected.isEmpty && !affected.contains(id) {continue}
                    let list = try await client.call("channel.list", .object(["crewId": .string(id)]))
                    guard current == generation, !Task.isCancelled else { activityDirty=true; return }
                    if let lobby=list.array.first(where:{$0["channel"]["kind"].string == "lobby"}) {crewAvatarMembers[id]=lobby["members"].array}
                    var latest: CrewdValue?
                    for summary in list.array {
                        guard let channel = summary["channel"]["id"].string else { continue }
                        activityChannelCrews[channel]=id
                        let messages = try await client.call("message.list", .object(["channelId": .string(channel), "limit": .integer(1)]))
                        guard current == generation, !Task.isCancelled else { activityDirty=true; return }
                        if let message = messages.array.last, (message["createdAt"].string ?? "") > (latest?["createdAt"].string ?? "") { latest = message }
                    }
                    snapshot[id] = latest; read.insert(id)
                }
                crewActivity = snapshot; activityRead = read
            } catch { activityDirty = true }
        }
        let asks = try await client.call("ask.list")
        guard !sleeping, !Task.isCancelled, current == generation, selected == selection else { again = true; return }
        pendingAsks = asks.array
        let restart = try await client.call("restart.pending")
        guard !sleeping, !Task.isCancelled, current == generation, selected == selection else { again = true; return }
        restartPrompt = restart
        if !crews.contains(where: {$0["id"].string == crewID}) { crewID = crews.first?["id"].string ?? "" }
        let crew = crewID
        knowledge.synchronizeOrigin(); workStore.selectCrew(crew); resources.selectCrew(crew)
        if workDirty {
            workDirty = false
            do { try await workStore.refresh() } catch { /* WorkStore retains typed failures; first-edition messaging stays usable. */ }
            guard current == generation, selected == selection, !sleeping, !Task.isCancelled else { return }
        }
        guard !crew.isEmpty else { clearExecutorProfiles(); return }
        await refreshExecutorProfiles()
        guard current == generation, selected == selection, crew == crewID, !sleeping, !Task.isCancelled else { return }
        let sessionQuery = UUID()
        sessionDirectoryQuery = sessionQuery
        let newSessions = try await client.call("session.list", .object(["crewId": .string(crew)]))
        guard !sleeping, !Task.isCancelled, current == generation, selected == selection,
              crewID == crew, sessionDirectoryQuery == sessionQuery else { again = true; return }
        let newChannels = try await client.call("channel.list", .object(["crewId": .string(crew)]))
        guard !sleeping, !Task.isCancelled, current == generation, selected == selection,
              crewID == crew, sessionDirectoryQuery == sessionQuery else { again = true; return }
        for row in newSessions.array {if let id=row["id"].string {knownSessions[id]=row}}
        sessions = newSessions.array; channelSummaries = newChannels.array; channels = newChannels.array.map { $0["channel"] }
        let dmPeer = selectedMembers.contains(where: {$0["kind"].string == "session" && $0["id"].string == sessionID}) && channels.first(where: {$0["id"].string == channelID})?["kind"].string == "dm"
        if !dmPeer && !sessions.contains(where: {$0["id"].string == sessionID}) { sessionID = sessions.first?["id"].string ?? ""; runnerItems = []; history = "" }
        if !channels.contains(where: {$0["id"].string == channelID}) { channelID = CrewdConversationProjection.selected(channels, requested: channelID) }
        channelWork.selectChannel(crew:crew,channel:channelID);channelResources.selectScope(crew:crew,channel:channelID);channelKnowledge.selectChannel(crew:crew,channel:channelID)
        if personalDraftContexts[crew]?.dbId != client.runtimeIdentity["dbId"].string{await qualifyPersonalDraftContext(crew)}
        let session = sessionID, channel = channelID
        if !session.isEmpty {
            let view = try await client.call("session.inspect", .object(["sessionId": .string(session), "maxLines": .number(200)]))
            guard !sleeping, !Task.isCancelled, current == generation, selected == selection, sessionID == session else { again = true; return }
            inspection = view
            if recoverHistory || runnerItems.isEmpty { history = view["screen"].string ?? "" }
        }
        if !channel.isEmpty {
            let rows = try await client.call("message.list", .object(["channelId": .string(channel), "limit": .number(200)]))
            guard !sleeping, !Task.isCancelled, current == generation, selected == selection, channelID == channel else { again = true; return }
            messages = rows.array
            messages.forEach(observeFirstCaptainReply)
        }
    }

    func prepareOriginalJournal(root: URL) async {
        let epoch = generation, connection = client.connectionEpoch, runtime = client.runtimeIdentity, binding = client.authorityBinding
        guard originalJournalRequired, originalJournal == nil, preparingJournal != connection,
              journalFailureConnection != connection, client.readyForWork, !retired else { return }
        preparingJournal = connection
        defer {
            if preparingJournal == connection { preparingJournal = nil }
            #if os(macOS)
            if acceptsObservation(epoch), client.connectionEpoch == connection { captureOwnedLifecycle() }
            #endif
        }
        do {
            let identity: CrewdValue
            if dataDirectory != nil { identity = try await readValue("remote.status")["identity"] }
            else { identity = client.authorityBinding }
            guard acceptsObservation(epoch), client.connectionEpoch == connection, client.runtimeIdentity == runtime, client.authorityBinding == binding else { throw CancellationError() }
            let localDirectory: String?
            if let directory = dataDirectory {
                guard let resolved = realpath(directory.path, nil) else { throw CrewdPublicWire.invalid() }
                defer { free(resolved) }
                guard String(cString: resolved) == directory.path else { throw CrewdPublicWire.invalid() }
                localDirectory = directory.path
            } else { localDirectory = nil }
            let scope = CrewdPresentationJournal.Scope(namespace: authorityRecordID,
                localDirectory: localDirectory,
                authorityID: identity["authorityId"].string, databaseID: client.runtimeIdentity["dbId"].string ?? identity["dbId"].string,
                grantID: identity["grantId"].string, grantRevision: identity["grantRevision"].string)
            let journal = try CrewdPresentationJournal(root: root, scope: scope)
            let unknown = try journal.unknownRecords()
            let history = try journal.firstMessageRecords()
            if client.capabilities.contains("command.receipts") { try publicCommands.restore(unknown) }
            guard acceptsObservation(epoch), client.connectionEpoch == connection, client.runtimeIdentity == runtime, client.authorityBinding == binding else { throw CancellationError() }
            try workStore.restoreLegacyIntents(unknown.filter {
                $0.method == "adminCase.legacyConfirm" && $0.scope == scope
            }.map(\.params))
            personalDraftContexts=personalDraftContexts.filter{$0.value.dbId == scope.databaseID}
            originalJournal = journal; presentationScope = scope; durableUnknown = unknown
            do { try restoreRequirementCreates(in:unknown,journal:journal) }
            catch { self.error = error.localizedDescription }
            originalFirstMessageHistory = history; journalFailureConnection = nil
            composerDrafts = CrewComposerDraftStore.durable(file: journal.composerDraftFile(namespace: presentationNamespace), namespace: presentationNamespace)
            for entry in composerDrafts.entries.values {
                if let crew = entry.crew, let channel = entry.channel {
                    messageDrafts.set(entry.draft.text, for: .message(crew: crew, channel: channel, session: ""))
                }
            }
            if let failure = composerDrafts.storageError { self.error = failure }

        } catch {
            if acceptsObservation(epoch), client.connectionEpoch == connection {
                journalFailureConnection = connection
                let message = "原前端提交历史未就绪：" + error.localizedDescription
                if self.error != message { self.error = message }
            }
        }
    }
    private func isReadOnly(_ method: String) -> Bool {
        if method == CrewdAdminAskGroupsWire.list { return true }
        if CrewdManagedTableV3Wire.readMethods.contains(method){return true}
        if [CrewdOrganizationSearchWire.search,CrewdOrganizationSearchWire.expand].contains(method) {return true}
        if [CrewdManagedTableV2Wire.tableGet,CrewdManagedTableV2Wire.tableList,CrewdManagedTableV2Wire.rows,CrewdManagedTableV2Wire.rowGet,CrewdManagedTableV2Wire.lookup,CrewdManagedTableV2Wire.links,CrewdManagedTableV2Wire.linkLookup,CrewdManagedTableV2Wire.resolveTarget].contains(method) {return true}
        if [CrewdArtifactHistoryWire.status,CrewdArtifactHistoryWire.lookup].contains(method) {return true}
        if [CrewdArtifactWire.preview,CrewdArtifactWire.list,CrewdArtifactWire.get,CrewdArtifactWire.backlinks,CrewdArtifactWire.subject,CrewdArtifactWire.lookup].contains(method) {return true}
        if [CrewdManagedMessageWire.source,CrewdManagedMessageWire.objects,CrewdManagedMessageWire.lookup].contains(method) { return true }

        if ["adminCase.legacyList", "adminCase.legacyGet", "adminCase.legacyPreview", "adminCase.legacyReceipt", "adminCase.similarRequirements", "adminCase.requirementReviews",
            "adminCase.workDuplicates", "adminCase.workDuplicateHistory", "ask.resolveCheck", CrewdStructuredAskWire.method, "work.coordinationStatus",
            "todo.rulingCheck", "todo.rulingResult", CrewdRequirementCreateWire.lookupMethod,
            CrewdChannelResponsibilityPage.method, CrewdMessageDeliveryExplanation.method].contains(method) { return true }
        if ["command.lookup", "command.listUnknown", "session.controlTarget", "session.terminal.watch"].contains(method) { return true }
        return method.hasSuffix(".list") || method.hasSuffix(".get") || method.hasSuffix(".inspect") || method.hasSuffix(".live") || method.hasSuffix(".history") || method.hasSuffix(".pending") || method.hasSuffix(".receipts") || method.hasSuffix(".usage") || ["remote.status", "remote.grants", "system.hello", "events.subscribe", "todo.attributesGet", "resource.requestLookup", "channel.knowledgeList", "session.codex.models", "session.codex.tools", "session.codex.permissionProfiles", "session.codex.inputLookup", "session.claude.taskGet", "session.claude.recoveryInspect"].contains(method)
    }
    private func submitRPC(_ method: String, _ params: CrewdValue) async throws -> CrewdValue {
        if isReadOnly(method) {
            do {
                if method == "adminCase.legacyReceipt" {
                    return try await lookupOriginalLegacyConfirmation(params)
                }
                return try await client.call(method, params)
            }
            catch {
                // Cancelling a projection read is not an uncertain submission.
                // Mutation paths below retain their original Unknown semantics.
                if Task.isCancelled { throw CancellationError() }
                throw error
            }
        }
        guard !stopping else { throw CrewdClientError(message: "正在停工，不再接受新执行。", kind: .notSubmitted) }
        if method == "todo.create" { return try await submitRequirementCreate(params) }
        let legacyCloseCrew: String?
        if method == "work.close" {
            guard !client.capabilities.contains(CrewdFactualWorkCloseWire.operation.rawValue),
                  !workStore.offersFactualWorkClose, let raw = params["id"].string, let id = CrewdWorkID(raw,kind:.plan),
                  let crew = workStore.plans[id]?.crew, !crew.isEmpty,
                  try refreshWorkCloseOriginals(raw,crew:crew).isEmpty else {
                throw CrewdClientError(message:"请核对当前计划，使用原回执关闭入口或查询已有原请求。",kind:.notSubmitted)
            }
            legacyCloseCrew = crew
        } else { legacyCloseCrew = nil }
        if ["ask.answer","ask.withdraw"].contains(method) {
            guard !workStore.offersAskResolution,
                  originalAskRecords(crew:params["crewId"].string,ask:params["ask"].string).isEmpty else {
                throw CrewdClientError(message:"请在原问题复核界面处理，或核对已有原请求。",kind:.notSubmitted)
            }
        }
        if method == "session.start" { try requireExecutor(params["executor"].string ?? "", crew: params["crewId"].string ?? crewID) }
        if method == "session.resume" {
            let id=params["sessionId"].string ?? params["target"]["sessionId"].string
            let exactClaudeSession=id.flatMap{profileForSession($0)?.backend == "existingClaudePty" ? $0:nil}
            let covered = try await claudeRecoveryCoverage(session: exactClaudeSession, params: params)
            guard !hasOriginalUnknown(method: method, crew: crewID, session:exactClaudeSession, excludingRequests: covered) else { throw CrewdClientError(message: "原恢复结果尚未确认，请只读核对原请求，不会重发。", kind: .notSubmitted) }
            try requireSessionExecutor(params["sessionId"].string ?? params["target"]["sessionId"].string ?? "")
        }
        if CrewdPublicOperation(rawValue:method)?.isCodex == true,!client.capabilities.contains("command.receipts") {
            throw CrewdClientError(message:"当前后台缺少正式命令回执，Codex 控制尚未提交。",kind:.notSubmitted)
        }
        if client.capabilities.contains("command.receipts"), let operation = CrewdPublicOperation(rawValue: method) {
            let commandParams = try await explicitControlParameters(operation, params)
            var receipt = try await submitPublicCommand(operation, params: commandParams, crew: params["crewId"].string ?? crewID)
            if [.sessionStart, .sessionResume, .sessionStop, .sessionInterrupt].contains(operation) || operation.isCodex {
                receipt = try await observeSessionControl(receipt)
            }
            guard !receipt.historical, ![.reserved, .admitted, .dispatched, .unknown].contains(receipt.phase) else {
                throw CrewdClientError(message: "原命令已有耐久身份，但外部结果尚未确认；请只读核对，不会重发。", data: receipt.value)
            }
            if [.suppressed, .rejected].contains(receipt.phase) {
                let result = receipt.value["result"]
                let refusal = result["refusal"].string ?? result["refusal"]["refusal"].string
                let reason = refusal == "session.worktree.not_repo"
                    ? "独立工作树需要已提交的 Git 仓库；可选择共享工作目录后重新启动。"
                    : (refusal ?? "当前权限或代际已改变。")
                throw CrewdClientError(message: "原命令未执行：" + reason, code: -32009, data: result)
            }
            let result = receipt.value["result"]
            if operation == .sessionStart {
                guard let id = result["sessionId"].string else { throw CrewdClientError(message: "会话分配回执缺少正式身份；保留原请求核对。") }
                guard case .object(var fields) = result else { throw CrewdClientError(message: "会话回执结构不完整。") }
                fields["id"] = .string(id) // Existing presentation model only, no minted ID.
                return .object(fields)
            }
            return result
        }
        guard !originalJournalRequired || originalJournalReady else { throw CrewdClientError(message: "原提交历史尚未可靠读取；本次尚未发送。", kind: .notSubmitted) }
        guard client.connected else { throw CrewdClientError(message: "当前连接未就绪；本次尚未发送。", kind: .notSubmitted) }
        let journal = originalJournal
        let legacyIntent: CrewdLegacyIntent?
        if method == "adminCase.legacyConfirm" {
            guard originalJournalReady, journal != nil else {
                throw CrewdClientError(message: "历史核验请求尚不能耐久保存，本次尚未发送。", kind: .notSubmitted)
            }
            legacyIntent = try CrewdLegacyIntent(prepared: params)
        } else {
            legacyIntent = nil
        }
        if Self.resourceMutationMethods.contains(method) {
            guard let journal=originalJournal else{throw CrewdClientError(message:"原资源提交历史尚未可靠读取，本次尚未发送。",kind:.notSubmitted)}
            do{return try await journal.performResourceMutation(method,params:params,crew:params["crewId"].string ?? crewID,call:{try await self.client.call($0,$1)})}
            catch{durableUnknown=(try? journal.unknownRecords()) ?? durableUnknown;throw error}
        }
        let prepared: CrewdPresentationJournal.Prepared?
        do {
            let crew = legacyCloseCrew ?? params["crewId"].string ?? crewID
            if method == "work.close" { prepared = try journal?.prepareWorkClose(method:method,params:params,crew:crew) }
            else { prepared = try journal?.prepare(method:method,params:params,crew:crew) }
        }
        catch { throw CrewdClientError(message: "本次提交快照未能耐久保存，尚未发送后台操作：" + error.localizedDescription, kind: .notSubmitted) }
        do {
            let result = try await client.call(method, params)
            try legacyIntent?.validate(result)
            if ["crew.create", "session.start"].contains(method) {
                guard let id = result["id"].string, !id.isEmpty else { throw CrewdClientError(message: "创建回复缺少正式编号，原提交结果待确认。") }
            }
            if method == "work.close" {
                if let prepared, let journal { try journal.observeLegacyWorkClose(prepared,result:result,expectedCrew:legacyCloseCrew) }
                else { try CrewdLegacyWorkClose.validate(result,params:params,expectedCrew:legacyCloseCrew) }
            } else if let prepared { try journal?.finish(prepared, state: "replyObserved") }
            return result
        } catch {
            if let prepared {
                let uncertain = CrewdClientError.isUncertainSubmission(error)
                do {
                    if method == "work.close" { try journal?.finishLegacyWorkClose(prepared,error:error) }
                    else { try journal?.finish(prepared, state: uncertain ? "unknown" : "refused", detail: error.localizedDescription) }
                } catch { self.error = "原请求返回结果与历史未能同时确认，保留提交快照。" }
                if let journal, journal === originalJournal { do { durableUnknown = try journal.unknownRecords() } catch { self.error = error.localizedDescription } }
            }
            throw error
        }
    }
    private func restoreRequirementCreates(in records: [CrewdPresentationJournal.Prepared], journal: CrewdPresentationJournal) throws {
        let intents = try journal.requirementCreateIntents(in:records)
        let snapshots: [CrewdRequirementDrafts.Snapshot] = intents.compactMap { intent in
            guard let record = intent.records.first else { return nil }
            let original = intent.original
            return .init(crew:original.crew,text:original.text,requestID:original.request,revision:record.id,channel:original.channel)
        }
        requirementDrafts.restoreUncertain(snapshots)
        composerRequirementDrafts.restoreUncertain(snapshots)
    }

    private func submitRequirementCreate(_ params: CrewdValue) async throws -> CrewdValue {
        let original: CrewdRequirementCreateWire.Original
        do { original = try .init(params) }
        catch { throw CrewdClientError(message:"原需求创建参数不符，尚未提交。",kind:.notSubmitted) }
        guard !Task.isCancelled, original.crew == crewID, original.crew == workStore.selectedCrew,
              original.channel.isEmpty || original.channel == channelID else {
            throw CrewdClientError(message:"创建需求的机组或群已变化，尚未提交。",kind:.notSubmitted)
        }
        guard originalJournalReady, let journal = originalJournal else {
            throw CrewdClientError(message:"原需求提交历史尚未就绪，尚未提交。",kind:.notSubmitted)
        }
        let epoch = generation, route = selection, connection = client.connectionEpoch, scope = presentationScope
        @MainActor func current() -> Bool {
            acceptsObservation(epoch) && selection == route && client.connectionEpoch == connection &&
                presentationScope == scope && originalJournal === journal
        }
        let prepared: CrewdPresentationJournal.Prepared
        do { prepared = try journal.prepareRequirementCreate(params,original:original) }
        catch {
            if let records = try? journal.unknownRecords() {
                durableUnknown = records
                try? restoreRequirementCreates(in:records,journal:journal)
            }
            throw CrewdClientError(message:error.localizedDescription,kind:.notSubmitted)
        }
        do {
            let value = try await client.call("todo.create",params)
            guard current(), !Task.isCancelled else { throw CancellationError() }
            try original.validateSubmission(value)
            try journal.finish(prepared,state:"replyObserved")
            return value
        } catch {
            let unsent = (error as? CrewdClientError)?.kind == .notSubmitted
            do { try journal.finishRequirementCreate(prepared,notSubmitted:unsent,detail:error.localizedDescription) }
            catch { if current() { self.error = "原需求回执未能保全，请只读核对原请求。" } }
            if acceptsObservation(epoch), client.connectionEpoch == connection, presentationScope == scope,
               originalJournal === journal, let records = try? journal.unknownRecords() {
                durableUnknown = records
                try? restoreRequirementCreates(in:records,journal:journal)
            }
            if unsent { throw error }
            throw CrewdClientError(message:error.localizedDescription)
        }
    }

    func recoverRequirementCreate(_ snapshot: CrewdRequirementDrafts.Snapshot,
                                  stillCurrent: @MainActor () -> Bool) async throws -> CrewdRequirementCreateWire.Receipt? {
        guard stillCurrent(), originalJournalReady, let journal = originalJournal, workStore.supportsRequirementCreateLookup,
              snapshot.crew == crewID, snapshot.crew == workStore.selectedCrew,
              snapshot.channel.isEmpty || snapshot.channel == channelID else {
            throw CrewdClientError(message:"当前连接不能只读核对这份原需求。原记录仍保留。",kind:.notSubmitted)
        }
        let epoch = generation, route = selection, connection = client.connectionEpoch, scope = presentationScope
        @MainActor func current() -> Bool {
            stillCurrent() && acceptsObservation(epoch) && selection == route && client.connectionEpoch == connection &&
                presentationScope == scope && originalJournal === journal && !Task.isCancelled
        }
        let intents = try journal.requirementCreateIntents(in:journal.unknownRecords())
        guard let intent = intents.first(where:{ $0.original.request == snapshot.requestID }),
              intent.original.crew == snapshot.crew, intent.original.channel == snapshot.channel,
              intent.original.text.utf8.elementsEqual(snapshot.text.utf8) else { throw CrewdRequirementCreateWire.invalid() }
        let receipt = try await journal.lookupRequirementCreate(intent,stillCurrent:current,read:workStore.requirementCreateLookup)
        guard current() else { throw CancellationError() }
        if receipt != nil {
            let records = try journal.unknownRecords()
            durableUnknown = records
            try restoreRequirementCreates(in:records,journal:journal)
            workDirty = true; scheduleSync()
        }
        return receipt
    }

    private func lookupOriginalLegacyConfirmation(_ params: CrewdValue) async throws -> CrewdValue {
        let epoch = observationGeneration
        let connection = client.connectionEpoch
        let crew = crewID
        let journal = originalJournal
        let record = durableUnknown.first {
            $0.scope == journal?.scope && $0.crew == crew &&
            $0.method == "adminCase.legacyConfirm" && $0.params["requestId"] == params["requestId"]
        }
        let result = try await client.call("adminCase.legacyReceipt", params)
        guard acceptsObservation(epoch), client.connectionEpoch == connection, crewID == crew else { throw CancellationError() }
        if let journal, let record, journal === originalJournal {
            try CrewdLegacyIntent(prepared: record.params).validate(result)
            try journal.acknowledge(record, detail: "只读核对原历史行政确认回执")
            durableUnknown = try journal.unknownRecords()
        }
        return result
    }
    /// Normal session controls first return dispatched. Follow only that original
    /// receipt while the explicit click is in progress; never submit a second
    /// command or infer launch success from the roster's desired state.
    private func observeSessionControl(_ initial: CrewdPublicReceipt) async throws -> CrewdPublicReceipt {
        var receipt = initial
        let deadline = ContinuousClock.now.advanced(by: .seconds(5))
        while !receipt.historical, [.reserved, .admitted, .dispatched].contains(receipt.phase), ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(100))
            guard let observation = publicCommands.observations[initial.requestID],
                  let current = try await publicCommands.lookup(observation) else { break }
            receipt = current
        }
        if receipt.phase != initial.phase, !receipt.historical,
           let observation = publicCommands.observations[initial.requestID],
           [.committed, .launched, .confirmed, .written, .suppressed, .rejected].contains(receipt.phase) {
            try acknowledgeOriginalCommand(observation.prepared, receipt: receipt,
                detail: "本次明确会话操作期间只读核对原请求终局；没有重放。")
        }
        return receipt
    }

    /// Original explicit controls supplied only a session ID. Resolve that
    /// closed legacy shape through a current read; never replace a caller's
    /// already typed CAS target or infer a target from a cached session row.
    private func explicitControlParameters(_ operation: CrewdPublicOperation, _ params: CrewdValue) async throws -> CrewdValue {
        guard [.sessionResume, .sessionInterrupt, .sessionStop].contains(operation),
              case .object(let fields) = params else { return params }
        let legacyKeys: Set<String> = operation == .sessionStop ? ["sessionId", "reason"] : ["sessionId"]
        guard Set(fields.keys) == legacyKeys else { return params } // Typed/mixed shapes retain strict canonical validation.
        guard let id = fields["sessionId"]?.string, !id.isEmpty else {
            throw CrewdClientError(message: "原会话身份不完整；控制尚未提交。", kind: .notSubmitted)
        }
        let epoch = generation, restoreEpoch = knowledge.workspace?.context.restoreEpoch
        let origin = try publicCommandBinding(for: client)
        let target: CrewdCommandTarget
        do { target = try await refreshControlTarget(id) }
        catch { throw CrewdClientError(message: "当前会话代际未能读取；控制尚未提交：" + error.localizedDescription, kind: .notSubmitted) }
        guard !stopping, acceptsObservation(epoch),
              (try? publicCommandBinding(for: client)) == origin,
              knowledge.workspace?.context.restoreEpoch == restoreEpoch else {
            throw CrewdClientError(message: "读取期间连接或恢复范围已改变；控制尚未提交。", kind: .notSubmitted)
        }
        if operation == .sessionStop { return .object(["target": target.value, "reason": fields["reason"]!]) }
        return target.value
    }
    func submitPublicCommand(_ operation: CrewdPublicOperation, params: CrewdValue, crew: String, request: UUID = UUID(), validateReceipt:((CrewdPublicReceipt)throws->Void)?=nil) async throws -> CrewdPublicReceipt {
        guard !stopping else { throw CrewdClientError(message: "正在停工，不再接受新执行。", kind: .notSubmitted) }
        if operation == .patrolStart || operation == .workDuplicateClose {
            guard client.capabilities.contains(operation.rawValue), client.readyForWork,
                  !crew.isEmpty, crew == crewID, params["crewId"] == .string(crew),
                  !hasOriginalUnknown(method: operation.rawValue, crew: crew),
                  !publicCommands.observations.values.contains(where: {
                      publicCommands.pending.contains($0.id) && $0.envelope.operation == operation &&
                          $0.envelope.params["crewId"] == .string(crew)
                  }) else {
                let action = operation == .patrolStart ? "启动手动巡检" : "确认重复工作关闭"
                throw CrewdClientError(message: "当前机组无法" + action + "，或已有原请求待核对。", kind: .notSubmitted)
            }
        }
        if operation == .askResolve || operation == .askInboxSchedule {
            let supported = operation == .askResolve ? workStore.supportsAskResolution : workStore.supportsAskInboxSchedule
            guard supported, originalJournalReady, let journal = originalJournal else {
                throw CrewdClientError(message:"当前连接尚未具备问题处理的原请求回执资格。",kind:.notSubmitted)
            }
            do { durableUnknown = try journal.unknownRecords() }
            catch { throw CrewdClientError(message:"无法核对原问题提交历史，本次尚未发送。",kind:.notSubmitted) }
            guard
                  let ask = params["ask"].string, !crew.isEmpty, params["crewId"] == .string(crew),
                  originalAskRecords(crew:crew,ask:ask).isEmpty,
                  !publicCommands.observations.values.contains(where:{
                      publicCommands.pending.contains($0.id) && [.askResolve,.askInboxSchedule].contains($0.envelope.operation) &&
                          $0.envelope.params["crewId"] == .string(crew) && $0.envelope.params["ask"] == .string(ask)
                  }) else { throw CrewdClientError(message:"此问题已有原请求待核对，或当前连接尚未具备回执资格。",kind:.notSubmitted) }
        }
        if operation == .workCloseWithReceipt {
            guard workStore.supportsFactualWorkClose, client.readyForWork,
                  client.capabilities.contains(operation.rawValue),
                  originalJournalReady, originalJournal != nil,
                  params["crewId"] == .string(crew), let id = params["id"].string,
                  try refreshWorkCloseOriginals(id,crew:crew).isEmpty,
                  !publicCommands.observations.values.contains(where: {
                      publicCommands.pending.contains($0.id) && $0.envelope.operation == .workCloseWithReceipt &&
                          $0.envelope.params["id"] == .string(id) &&
                          $0.envelope.params["crewId"].string?.utf8.elementsEqual(crew.utf8) == true
                  }) else {
                throw CrewdClientError(message:"当前连接无法保存原关闭回执，或此计划已有原请求待核对。",kind:.notSubmitted)
            }
        }
        if operation == .requirementRuleClose {
            guard workStore.supportsRequirementRuling, originalJournalReady, let journal = originalJournal,
                  !crew.isEmpty, crew == crewID, crew == workStore.selectedCrew,
                  params["crewId"] == .string(crew), let id = params["id"].string else {
                throw CrewdClientError(message:"当前连接或机组尚未具备需求裁定资格。",kind:.notSubmitted)
            }
            do { durableUnknown = try journal.unknownRecords() }
            catch { throw CrewdClientError(message:"无法核对原裁定历史，本次尚未发送。",kind:.notSubmitted) }
            guard originalRequirementRulingRecords(crew:crew,id:id).isEmpty,
                  !publicCommands.observations.values.contains(where:{
                      publicCommands.pending.contains($0.id) && $0.envelope.operation == .requirementRuleClose &&
                          $0.envelope.params["crewId"] == .string(crew) && $0.envelope.params["id"] == .string(id)
                  }) else { throw CrewdClientError(message:"该需求已有原裁定请求待核对。",kind:.notSubmitted) }
        }
        if operation.isCodex {
            let session=params["target"]["sessionId"].string
            guard client.capabilities.contains("session.codex"),session != nil else{throw CrewdClientError(message:"当前后台未接入 Codex 会话控制。",kind:.notSubmitted)}
            let unresolved=durableUnknown.contains{record in
                isCurrentOriginalObservation(record) && record.params["command"]["operation"] == .string(operation.rawValue) && record.params["command"]["params"]["target"]["sessionId"].string == session
            } || publicCommands.observations.values.contains{row in
                publicCommands.pending.contains(row.id) && row.envelope.operation == operation && row.envelope.params["target"]["sessionId"].string == session
            }
            guard !unresolved else{throw CrewdClientError(message:"该会话此项原控制尚未确认，请核对原请求，不会新发。",kind:.notSubmitted)}
        }
        if operation == .sessionStart { try requireExecutor(params["executor"].string ?? "", crew: crew) }
        if operation == .sessionResume {
            let id=params["sessionId"].string ?? params["target"]["sessionId"].string
            let exactClaudeSession=id.flatMap{profileForSession($0)?.backend == "existingClaudePty" ? $0:nil}
            let covered = try await claudeRecoveryCoverage(session: exactClaudeSession, params: params)
            guard !hasOriginalUnknown(method: operation.rawValue, crew: crewID, session:exactClaudeSession, excludingRequests: covered) else { throw CrewdClientError(message: "原恢复结果尚未确认，请只读核对原请求，不会重发。", kind: .notSubmitted) }
            try requireSessionExecutor(id ?? "")
        }
        _ = try publicCommandBinding(for: client)
        let envelope: CrewdPublicCommandEnvelope
        do { envelope = try CrewdPublicCommandEnvelope(request: request, binding: client.runtimeIdentity, operation: operation, params: params) }
        catch { throw CrewdClientError(message: "命令参数尚未满足正式契约，未发送：" + error.localizedDescription, kind: .notSubmitted) }
        do { return try await publicCommands.submit(envelope, crew: crew,validateReceipt:validateReceipt) }
        catch {
            // Retain the exact submitted identity even when the reply is lost.
            // A pre-send failure has no submitted observation and stays unlinked.
            if publicCommands.observations[request] != nil {
                let original = error as? CrewdClientError
                var fields: [String: CrewdValue] = [:]
                if let original, case .object(let data) = original.data { fields = data }
                fields["requestId"] = .string(request.uuidString)
                throw CrewdClientError(message: error.localizedDescription, kind: original?.kind ?? .general,
                    code: original?.code, data: .object(fields))
            }
            throw error
        }
    }
    func originalAskResolutionRecords(crew: String? = nil, ask: String? = nil) -> [CrewdPresentationJournal.Prepared] {
        originalAskRecords(crew:crew,ask:ask,operations:[.askResolve])
    }

    func originalAskRecords(crew: String? = nil, ask: String? = nil,
                            operations: Set<CrewdPublicOperation> = [.askResolve,.askInboxSchedule]) -> [CrewdPresentationJournal.Prepared] {
        durableUnknown.filter { record in
            let command = record.params["command"], params = command["params"]
            return isCurrentOriginalObservation(record) && record.method == "command.submit" &&
                command["operation"].string.flatMap(CrewdPublicOperation.init(rawValue:)).map(operations.contains) == true &&
                (crew == nil || params["crewId"].string == crew) && (ask == nil || params["ask"].string == ask)
        }
    }
    func submitAskResolution(_ review: CrewdAskResolutionWire.Review, disposition: CrewdAskResolutionWire.Disposition,
                             text: String, selectedCrew: String, scope: UUID) async throws -> CrewdPublicReceipt {
        guard !Task.isCancelled, !selectedCrew.isEmpty, crewID == selectedCrew, workStore.selectedCrew == selectedCrew,
              workStore.readScope == scope else { throw CancellationError() }
        let params = try review.parameters(disposition,text:text)
        let receipt = try await submitPublicCommand(.askResolve,params:params,crew:review.crewID) { receipt in
            if receipt.phase == .committed { try CrewdAskResolutionWire.validateResult(receipt.value["result"],params:params,review:review) }
        }
        if receipt.phase == .committed { workDirty = true; scheduleSync() }
        return receipt
    }
    var humanInboxSelection: UUID { selection }

    func submitAskInboxSchedule(_ shown: CrewdAskResolutionWire.Review, untilMs: Int?,
                                selectedCrew: String, scope: UUID, stillCurrent: @escaping @MainActor () -> Bool) async throws -> CrewdPublicReceipt? {
        let epoch = observationGeneration, connection = client.connectionEpoch, route = selection, origin = presentationScope
        func current() -> Bool {
            stillCurrent() && !Task.isCancelled && !stopping && acceptsObservation(epoch) && selection == route &&
                client.connectionEpoch == connection && presentationScope == origin && originalJournalReady &&
                workStore.supportsAskInboxSchedule && workStore.readScope == scope &&
                !selectedCrew.isEmpty && crewID == selectedCrew && workStore.selectedCrew == selectedCrew
        }
        guard current(), let id = CrewdWorkID(shown.askID,kind:.human), let inbox = shown.inbox else { throw CancellationError() }
        let fresh = try await workStore.humanResolutionCheck(id,crew:shown.crewID)
        guard current() else { throw CancellationError() }
        guard fresh.sameResolution(as:shown), fresh.inbox?.sameState(as:inbox) == true else { return nil }
        let params = try CrewdAskInboxWire.parameters(fresh,untilMs:untilMs,nowMs:CrewdAskInboxWire.nowMs())
        let receipt = try await submitPublicCommand(.askInboxSchedule,params:params,crew:fresh.crewID) { _ in
            guard current() else { throw CancellationError() }
        }
        if receipt.phase == .committed { workDirty = true; scheduleSync() }
        return receipt
    }

    func recoverAskInboxSchedule(_ id: UUID, stillCurrent: () -> Bool) async throws -> CrewdPublicReceipt? {
        let epoch = observationGeneration, route = selection, origin = presentationScope
        let connection = client.connectionEpoch, readScope = workStore.readScope
        func current() -> Bool {
            stillCurrent() && !Task.isCancelled && acceptsObservation(epoch) && selection == route &&
                client.connectionEpoch == connection && presentationScope == origin && workStore.readScope == readScope
        }
        guard current(), let record = originalAskRecords(operations:[.askInboxSchedule]).first(where:{ $0.id == id }) else {
            throw CancellationError()
        }
        let receipt = try await lookupOriginalCommand(record)
        guard current() else { throw CancellationError() }
        if let receipt, [.committed,.rejected,.suppressed].contains(receipt.phase) {
            try acknowledgeOriginalCommand(record,receipt:receipt)
            if receipt.phase == .committed { workDirty = true; scheduleSync() }
        }
        return receipt
    }

    func recoverAskResolution(_ id: UUID, stillCurrent: @MainActor () -> Bool = { true }) async throws -> CrewdPublicReceipt? {
        guard stillCurrent(), !Task.isCancelled else { throw CancellationError() }
        guard let record = originalAskResolutionRecords().first(where:{ $0.id == id }) else { throw CrewdPublicWire.invalid() }
        let receipt = try await lookupOriginalCommand(record)
        guard stillCurrent(), !Task.isCancelled else { throw CancellationError() }
        if let receipt, [.committed,.rejected,.suppressed].contains(receipt.phase) {
            try acknowledgeOriginalCommand(record,receipt:receipt)
            if receipt.phase == .committed { workDirty = true; scheduleSync() }
        }
        return receipt
    }
    func originalWorkCloseRecords(_ id: String, crew: String, in records: [CrewdPresentationJournal.Prepared]? = nil) -> [CrewdPresentationJournal.Prepared] {
        (records ?? durableUnknown).filter { record in
            isCurrentOriginalObservation(record) &&
                CrewdFactualWorkCloseWire.matchesOriginal(method:record.method,params:record.params,journalCrew:record.crew,id:id,crew:crew)
        }
    }

    func refreshWorkCloseOriginals(_ id: String, crew: String) throws -> [CrewdPresentationJournal.Prepared] {
        guard originalJournalReady || !originalJournalRequired else {
            throw CrewdClientError(message:"原关闭记录尚未读取。",kind:.notSubmitted)
        }
        if let journal = originalJournal {
            do { durableUnknown = try journal.unknownRecords() }
            catch { throw CrewdClientError(message:"无法读取原关闭记录。",kind:.notSubmitted) }
        }
        return originalWorkCloseRecords(id,crew:crew)
    }

    func submitFactualWorkClose(_ params: CrewdValue, stillCurrent: () -> Bool) async throws -> CrewdPublicReceipt {
        guard let crew = params["crewId"].string, let raw = params["id"].string,
              let id = CrewdWorkID(raw, kind: .plan), stillCurrent(), !Task.isCancelled else { throw CancellationError() }
        let route = selection, epoch = observationGeneration, connection = client.connectionEpoch
        let origin = presentationScope, scope = workStore.readScope
        let plan = try await workStore.detail(id)
        guard stillCurrent(), !Task.isCancelled, selection == route, observationGeneration == epoch,
              client.connectionEpoch == connection, presentationScope == origin, workStore.readScope == scope else {
            throw CancellationError()
        }
        guard plan.crew.utf8.elementsEqual(crew.utf8), plan.value["revision"] == params["expectedRevision"],
              !plan.frozen, !["done", "dropped"].contains(plan.status) else {
            throw CrewdClientError(message:"计划已变化。请核对当前版本和凭据后再确认。",kind:.notSubmitted)
        }
        let receipt = try await submitPublicCommand(.workCloseWithReceipt, params:params, crew:crew)
        workDirty = true
        scheduleSync()
        return receipt
    }

    func acknowledgeFactualWorkClose(_ receipt: CrewdPublicReceipt, lookedUp: CrewdPresentationJournal.Prepared? = nil) throws {
        guard receipt.phase == .committed, !receipt.historical || lookedUp != nil,
              let original = publicCommands.observations[receipt.requestID],
              original.envelope.operation == .workCloseWithReceipt,
              lookedUp == nil || lookedUp?.id == original.prepared.id else { throw CrewdPublicWire.invalid() }
        try acknowledgeOriginalCommand(original.prepared, receipt:receipt, detail:"已在计划详情核对原关闭回执。")
        workDirty = true
        scheduleSync()
    }

    func recoverFactualWorkClose(_ original: CrewdPresentationJournal.Prepared, id: String, crew: String,
                                 stillCurrent: () -> Bool) async throws -> CrewdPublicReceipt? {
        let route = selection, epoch = observationGeneration, connection = client.connectionEpoch
        let origin = presentationScope, scope = workStore.readScope
        @MainActor func current() -> Bool {
            stillCurrent() && !Task.isCancelled && selection == route && observationGeneration == epoch &&
                client.connectionEpoch == connection && presentationScope == origin && workStore.readScope == scope
        }
        guard current(), original.method == "command.submit",
              original.params["command"]["operation"] == .string(CrewdFactualWorkCloseWire.operation.rawValue),
              original.params["command"]["params"]["crewId"] == .string(crew),
              try refreshWorkCloseOriginals(id,crew:crew).contains(where: { $0.id == original.id }) else { throw CrewdPublicWire.invalid() }
        let receipt = try await lookupOriginalCommand(original)
        guard current() else { throw CancellationError() }
        return receipt
    }

    func originalRequirementRulingRecords(crew: String, id: String? = nil) -> [CrewdPresentationJournal.Prepared] {
        durableUnknown.filter { record in
            let command = record.params["command"], params = command["params"]
            return isCurrentOriginalObservation(record) && record.method == "command.submit" &&
                command["operation"] == .string(CrewdPublicOperation.requirementRuleClose.rawValue) &&
                params["crewId"] == .string(crew) && (id == nil || params["id"].string == id)
        }
    }
    func submitRequirementRuling(_ review: CrewdRequirementRulingWire.Review, note: String, scope: UUID) async throws -> CrewdPublicReceipt {
        guard !Task.isCancelled, review.crew == crewID, workStore.readScope == scope else { throw CancellationError() }
        let receipt = try await submitPublicCommand(.requirementRuleClose,params:try review.parameters(note:note),crew:review.crew)
        if receipt.phase == .committed { workDirty = true; scheduleSync() }
        return receipt
    }
    func recoverRequirementRuling(_ original: CrewdPresentationJournal.Prepared, crew: String, id: String, scope: UUID) async throws -> CrewdPublicReceipt? {
        func current() -> Bool {
            !Task.isCancelled && crewID == crew && workStore.selectedCrew == crew && workStore.readScope == scope
        }
        guard current(), originalRequirementRulingRecords(crew:crew,id:id).contains(where:{ $0.id == original.id }) else {
            throw CrewdPublicWire.invalid()
        }
        let receipt = try await lookupOriginalCommand(original)
        guard current() else { throw CancellationError() }
        if let receipt, [.committed,.rejected,.suppressed].contains(receipt.phase) {
            try acknowledgeOriginalCommand(original,receipt:receipt)
            if receipt.phase == .committed { workDirty = true; scheduleSync() }
        }
        return receipt
    }
    @discardableResult func refreshControlTarget(_ id: String) async throws -> CrewdCommandTarget {
        guard client.capabilities.contains("command.receipts") else { throw CrewdClientError(message: "当前后台尚无正式代际控制。", kind: .notSubmitted) }
        let epoch = generation
        let value = try await readValue("session.controlTarget", .object(["sessionId": .string(id)]))
        guard acceptsObservation(epoch) else { throw CancellationError() }
        let target = try CrewdCommandTarget(value)
        guard target.sessionID == id else { throw CrewdClientError(message: "控制目标返回其它会话，未接受。") }
        controlTargets[id] = target; return target
    }
    func personalDraftKey(_ context:CrewdDrafts.Context)->AppleDraftKey? {
        guard let source=personalDraftContexts[context.crew] else{return nil}
        if client.connected,let observed=client.runtimeIdentity["dbId"].string,observed != source.dbId{return nil}
        let kind:String,destination:AppleDraftKey.Destination
        if context.kind == .initialTask{kind="initialTask";destination = .channel(context.channel.isEmpty ? context.crew+"/lobby":context.channel)}
        else if !context.session.isEmpty{kind="sessionMessage";destination = .originalSession(context.session)}
        else{kind="channelMessage";destination = .channel(context.channel)}
        return .init(authorityID:source.authorityId,databaseID:source.dbId,crewID:context.crew,destination:destination,kind:kind)
    }
    func bindPersonalProjection(){
        guard personalProjectionObserver == nil else{return}
        let service=personalDrafts
        personalEditingScope=service.editingScope()
        personalProjectionObserver=service.$projectionID.dropFirst().sink{[weak self] projection in
            guard let self else{return}
            if let old=self.personalEditingScope {
                for snapshot in self.messageDrafts.allSnapshots where snapshot.context.kind == .initialTask || !snapshot.context.session.isEmpty {
                    if let key=self.personalDraftKey(snapshot.context){service.checkpoint(.init(text:snapshot.text),key:key,scope:old)}
                }
            }
            self.messageDrafts = .init()
            self.personalEditingScope = .init(owner:service.coordinator.current,projection:projection)
        }
    }
    func checkpointComposer(_ payload:CrewComposerDraftStore.Draft,crew:String,channel:String,scope:CrewPersonalDraftService.EditingScope?=nil){
        guard let key=personalDraftKey(.message(crew:crew,channel:channel,session:"")) else{return}
        personalDrafts.checkpoint(payload,key:key,scope:scope)
    }
    func setPersonalInput(_ text:String,context:CrewdDrafts.Context,scope:CrewPersonalDraftService.EditingScope?=nil){
        if scope.map({personalDrafts.isCurrent($0)}) ?? true {messageDrafts.set(text,for:context)}
        if let key=personalDraftKey(context){personalDrafts.checkpoint(.init(text:text),key:key,scope:scope)}
    }
    func personalInputSnapshot(_ snapshot:CrewdDrafts.Snapshot)->CrewPersonalDraftService.Snapshot? {
        personalDraftKey(snapshot.context).flatMap{personalDrafts.snapshot($0,payload:.init(text:snapshot.text),scope:personalEditingScope)}
    }
    @discardableResult func clearPersonalInput(_ snapshot:CrewdDrafts.Snapshot,personal:CrewPersonalDraftService.Snapshot?)->Bool {
        guard messageDrafts.clearIfUnchanged(snapshot) else{return false}
        if let personal{personalDrafts.clearIfUnchanged(personal)}
        return true
    }
    func restorePersonalInput(_ context:CrewdDrafts.Context)async {
        guard let key=personalDraftKey(context) else{return}
        let old=messageDrafts.snapshot(for:context),scope=personalDrafts.editingScope()
        guard old.text.isEmpty,let payload=await personalDrafts.restoreOwn(key),personalDrafts.isCurrent(scope),messageDrafts.text(for:context).isEmpty,personalDraftKey(context) == key,messageDrafts.snapshot(for:context) == old else{return}
        messageDrafts.set(payload.text,for:context)
    }
    private func qualifyPersonalDraftContext(_ crew:String)async {
        guard client.readyForWork,!crew.isEmpty,let expected=client.runtimeIdentity["dbId"].string else{return}
        let epoch=observationGeneration,connection=client.connectionEpoch
        do{
            let raw=try await client.call("data.workspace.get",.object(["crewId":.string(crew)]))
            let info=try CrewdKnowledgeStore.decode(raw,CrewdDataWorkspaceInfo.self)
            guard acceptsObservation(epoch),client.connectionEpoch == connection,info.context.dbId == expected,info.context.authorityId == expected,info.context.crewId == crew else{return}
            personalDraftContexts[crew]=info.context
        }catch{ /* Existing durable composer remains usable; no fabricated source key. */ }
    }
    static let resourceMutationMethods=CrewdPresentationJournal.resourceMutationMethods
    func lookupOriginalResource(_ record:CrewdPresentationJournal.Prepared)async throws->CrewdValue {
        guard record.scope == presentationScope,Self.resourceMutationMethods.contains(record.method),
              let id=record.params["requestId"].string.flatMap(UUID.init(uuidString:)) else{throw CrewdClientError(message:"原资源请求作用域或身份不符。",kind:.notSubmitted)}
        let epoch=observationGeneration
        let value=try await readValue("resource.requestLookup",.object(["requestId":.string(id.uuidString.lowercased())]))
        guard acceptsObservation(epoch),record.scope == presentationScope else{throw CancellationError()}
        try validateOriginalResource(value,record:record);return value
    }
    private func validateOriginalResource(_ value:CrewdValue,record:CrewdPresentationJournal.Prepared)throws {
        guard value["requestId"] == record.params["requestId"] else{throw CrewdClientError(message:"原资源结果请求身份不符。")}
        if value["status"] == .string("notFound") {
            guard value["operation"] == .null,value["result"] == .null else{throw CrewdClientError(message:"未找到结果却附带操作内容。")};return
        }
        guard value["status"] == .string("committed"),value["operation"] == .string(record.method),value["result"] != .null else{throw CrewdClientError(message:"原资源结果操作或状态不符。")}
        try CrewdPresentationJournal.validateResourceResult(value["result"],method:record.method,params:record.params)
    }
    func acknowledgeOriginalResource(_ record:CrewdPresentationJournal.Prepared,receipt:CrewdValue)throws {
        guard record.scope == presentationScope,Self.resourceMutationMethods.contains(record.method),receipt["status"] == .string("committed"),let journal=originalJournal else{throw CrewdClientError(message:"只能核对当前原作用域的已提交回执。",kind:.notSubmitted)}
        try validateOriginalResource(receipt,record:record)
        try journal.acknowledge(record,detail:"Verified original resource receipt; no replay")
        durableUnknown=try journal.unknownRecords()
        if let id=record.params["requestId"].string.flatMap(UUID.init(uuidString:)){todoMutationDrafts.acknowledge(id);knowledgeLinkDrafts.acknowledge(id)}
    }
    /// Personal SQL history reuses the original private command journal. Keeping
    /// observed attempts prevents reopening from silently minting a new UUID.
    func personalDraftHistory()throws->[CrewdPresentationJournal.Prepared]{
        guard let journal=originalJournal,originalJournalReady else{throw CrewdPublicWire.invalid()}
        return try journal.personalDraftHistory()
    }
    func acknowledgePersonalDraft(_ record:CrewdPresentationJournal.Prepared,receipt:CrewdPublicReceipt)throws{
        guard record.scope==presentationScope,record.params["command"]["operation"].string?.hasPrefix("personalDraft.")==true,
            record.params["requestId"].string.flatMap(UUID.init(uuidString:))==receipt.requestID,
            record.params["dbId"].string==receipt.databaseID,record.params["runtimeBindingId"].string==receipt.runtimeBindingID,
            record.params["command"]==receipt.value["command"],[.committed,.rejected,.suppressed].contains(receipt.phase),let journal=originalJournal else{throw CrewdPublicWire.invalid()}
        // Historical SQL outcome is the ORIGINAL result, never current presence
        // or permission to replay. The adapter rechecks the live scope separately.
        try journal.acknowledge(record,detail:"Verified original personal draft SQL receipt; no replay")
        durableUnknown=try journal.unknownRecords()
    }
    func fencePersonalDraft(_ record:CrewdPresentationJournal.Prepared)async throws->CrewdPublicReceipt{
        let binding=try publicCommandBinding(for:client)
        guard record.scope==binding.scope,record.params["dbId"].string==binding.databaseID,
            record.params["command"]["operation"].string?.hasPrefix("personalDraft.")==true,
            let request=record.params["requestId"].string.flatMap(UUID.init(uuidString:)) else{throw CrewdPublicWire.invalid()}
        let value=try await client.call("personalDraft.requestFence",.object(["original":record.params]))
        guard try publicCommandBinding(for:client)==binding else{throw CancellationError()}
        try CrewPersonalCoreWire.fields(value,["state","receipt"])
        let receipt=try CrewdPublicReceipt(value["receipt"],request:request,database:binding.databaseID,originalRuntimeBinding:try CrewdPublicWire.token(record.params["runtimeBindingId"]),originalCommand:record.params["command"])
        switch value["state"].string{
        case "missingFenced":guard receipt.phase == .rejected,receipt.value["result"]["reason"] == .string("notSubmitted") else{throw CrewdPublicWire.invalid()}
        case "existingTerminal":guard [.committed,.rejected,.suppressed].contains(receipt.phase) else{throw CrewdPublicWire.invalid()}
        case "existingPending":guard [.reserved,.admitted,.dispatched,.unknown].contains(receipt.phase) else{throw CrewdPublicWire.invalid()}
        default:throw CrewdPublicWire.invalid()
        }
        // The adapter must verify its captured writer before acknowledging a
        // terminal result. A valid envelope alone is not a branch confirmation.
        return receipt
    }
    func lookupOriginalCommand(_ record: CrewdPresentationJournal.Prepared) async throws -> CrewdPublicReceipt? {
        _ = try publicCommandBinding(for: client)
        guard record.scope == presentationScope, record.method == "command.submit", let id = record.params["requestId"].string.flatMap(UUID.init(uuidString:)) else {
            throw CrewdClientError(message: "原请求不属于当前可信作用域，未查找。", kind: .notSubmitted)
        }
        try publicCommands.restore([record])
        guard let observation = publicCommands.observations[id] else { throw CrewdPublicWire.invalid() }
        let receipt = try await publicCommands.lookup(observation)
        if let receipt, settledSessionStart(receipt), observation.envelope.operation == .sessionStart {
            // Local presentation acknowledgement only, after lookup of the
            // original UUID. This never dispatches a replacement operation.
            try acknowledgeOriginalCommand(record, receipt: receipt)
        }
        return receipt
    }
    /// Explicit read-only receipt recovery. Historical SQL acceptance alone
    /// cannot prove current presence; verify both current crew/channel before
    /// changing only presentation selection. Never submit the command again.
    /// Explicit search navigation rechecks current ACL/ownership before selecting.
    /// Re-read source ACL before selecting an actual accessible conversation.
    /// This does not claim that a historical message is in the current timeline page.
    func openManagedMessageSource(object:String, stillCurrent:()->Bool) async throws {
        let epoch=observationGeneration,route=selection,connection=client.connectionEpoch,origin=presentationScope
        func valid()->Bool { stillCurrent() && acceptsObservation(epoch) && selection==route &&
            client.connectionEpoch==connection && presentationScope==origin && !Task.isCancelled }
        guard valid(),client.capabilities.contains(CrewdManagedMessageWire.capability) else { throw CrewdPublicWire.invalid() }
        let source=try CrewdManagedMessageWire.Source(await readValue(CrewdManagedMessageWire.source,.object(["id":.string(object)])))
        guard valid(),case .available(let crew,_,let channel,_) = source else { throw CrewdPublicWire.invalid() }
        let crews=try await readValue("crew.list")
        guard valid(),try CrewSearchWire.rows(crews,id:{$0["id"].string}).contains(where:{$0["id"].string?.utf8.elementsEqual(crew.utf8)==true}) else { throw CrewdPublicWire.invalid() }
        let channels=try await readValue("channel.list",.object(["crewId":.string(crew)]))
        guard valid(),try CrewSearchWire.rows(channels,id:{$0["channel"]["id"].string}).contains(where:{
            $0["channel"]["id"].string?.utf8.elementsEqual(channel.utf8)==true && $0["channel"]["crewId"].string?.utf8.elementsEqual(crew.utf8)==true
        }) else { throw CrewdPublicWire.invalid() }
        // Permission can change during navigation reads. Recheck the exact source once more.
        let verified=try CrewdManagedMessageWire.Source(await readValue(CrewdManagedMessageWire.source,.object(["id":.string(object)])))
        guard valid(),verified==source else { throw CrewdPublicWire.invalid() }
        selectCrew(crew);selectChannel(channel);originalSection="消息"
    }
    func openSearchMessage(crew:String,channel:String,message:String,stillCurrent:()->Bool)async throws {
        let epoch=observationGeneration,route=selection,connection=client.connectionEpoch,origin=presentationScope
        func valid()->Bool{stillCurrent() && acceptsObservation(epoch) && selection==route && client.connectionEpoch==connection && presentationScope==origin && !Task.isCancelled}
        let crews=try await readValue("crew.list")
        guard valid(),try CrewSearchWire.rows(crews,id:{$0["id"].string}).contains(where:{$0["id"].string==crew}) else{throw CrewdPublicWire.invalid()}
        let channels=try await readValue("channel.list",.object(["crewId":.string(crew)]))
        guard valid(),try CrewSearchWire.rows(channels,id:{$0["channel"]["id"].string}).contains(where:{$0["channel"]["id"].string==channel && $0["channel"]["crewId"].string==crew}) else{throw CrewdPublicWire.invalid()}
        let messages=try await readValue("message.list",.object(["channelId":.string(channel),"limit":.integer(1000)]))
        guard valid(),try CrewSearchWire.rows(messages,id:{$0["id"].string}).contains(where:{$0["id"].string==message && $0["channelId"].string==channel}) else{throw CrewdClientError(message:"该搜索结果当前不可访问，请重新搜索。",kind:.notSubmitted)}
        selectCrew(crew);selectChannel(channel)
    }
    func openOriginalFirstMessage(_ record: CrewdPresentationJournal.Prepared, receipt: CrewdPublicReceipt) async throws -> String {
        let epoch = observationGeneration, selected = selection
        guard record.scope == presentationScope, receipt.phase == .committed,
              record.params["command"]["operation"].string == "crew.createFirstMessage",
              record.params["requestId"].string.flatMap(UUID.init(uuidString:)) == receipt.requestID,
              record.params["dbId"].string == receipt.databaseID,
              record.params["runtimeBindingId"].string == receipt.runtimeBindingID,
              record.params["command"] == receipt.value["command"],
              let crew = receipt.value["result"]["crew"]["id"].string,
              let channel = receipt.value["result"]["message"]["channelId"].string else { throw CrewdPublicWire.invalid() }
        let currentCrews = try await readValue("crew.list")
        guard acceptsObservation(epoch), selection == selected, currentCrews.array.contains(where: { $0["id"].string == crew }) else {
            throw CrewdClientError(message: "原回执仍保留，当前后台没有该机组；不会重建或重发。", kind: .notSubmitted)
        }
        let currentChannels = try await readValue("channel.list", .object(["crewId": .string(crew)]))
        guard acceptsObservation(epoch), selection == selected, currentChannels.array.contains(where: { $0["channel"]["id"].string == channel }) else {
            throw CrewdClientError(message: "原回执仍保留，当前后台没有该可读会话；不会重发。", kind: .notSubmitted)
        }
        selectCrew(crew); selectChannel(channel)
        return crew
    }
    private func settledSessionStart(_ receipt: CrewdPublicReceipt) -> Bool {
        guard !receipt.historical else { return false }
        if [.rejected, .suppressed].contains(receipt.phase) { return true }
        return [.committed, .launched].contains(receipt.phase) &&
            receipt.value["result"]["sessionRegistered"] == .bool(true) &&
            receipt.value["result"]["sessionId"].string.map { !$0.isEmpty } == true
    }
    func acknowledgeOriginalCommand(_ record: CrewdPresentationJournal.Prepared, receipt: CrewdPublicReceipt, detail: String = "人类明确核对原后台回执；没有重放。") throws {
        guard record.params["command"]["operation"].string?.hasPrefix("personalDraft.") != true else{
            throw CrewdClientError(message:"请在个人同步设置中核对这项原请求；需先核实原空间与设备 writer，不能从通用回执入口结束。",kind:.notSubmitted)
        }
        _ = try publicCommandBinding(for: client)
        // These terminal receipts prove the original write across restart.
        // Current patrol and review state have their own reads.
        let operation = record.params["command"]["operation"].string
        let historicalRecordedRequest = receipt.historical &&
            [CrewdPublicOperation.patrolStart.rawValue, CrewdPublicOperation.requirementReview.rawValue,
             CrewdPublicOperation.workDuplicateClose.rawValue,CrewdPublicOperation.askResolve.rawValue,
             CrewdPublicOperation.askInboxSchedule.rawValue,CrewdPublicOperation.requirementRuleClose.rawValue,
             CrewdPublicOperation.workCloseWithReceipt.rawValue].contains(operation ?? "") &&
            [.committed, .rejected, .suppressed].contains(receipt.phase)
        guard record.scope == presentationScope, !receipt.historical || historicalRecordedRequest,
              record.params["requestId"].string.flatMap(UUID.init(uuidString:)) == receipt.requestID,
              record.params["dbId"].string == receipt.databaseID,
              record.params["runtimeBindingId"].string == receipt.runtimeBindingID,
              record.params["command"] == receipt.value["command"],
              [.committed, .launched, .written, .resized, .confirmed, .suppressed, .rejected].contains(receipt.phase),
              let journal = originalJournal else { throw CrewdPublicWire.invalid() }
        if record.params["command"]["operation"].string == "session.start", !settledSessionStart(receipt) { throw CrewdPublicWire.invalid() }
        try journal.acknowledge(record, detail: historicalRecordedRequest ? "已核对原请求的历史回执。当前状态另行读取。" : detail)
        durableUnknown = try journal.unknownRecords()
        if record.params["command"]["operation"].string == "session.start" {
            uncertainSessionStartRecords.removeAll { $0.request == receipt.requestID }
        }
    }
    func isCurrentOriginalObservation(_ record: CrewdPresentationJournal.Prepared) -> Bool {
        record.scope == presentationScope
    }
    /// Coverage is used only for this explicit attempt; the original journal remains untouched.
    private func claudeRecoveryCoverage(session: String?, params: CrewdValue) async throws -> Set<String> {
        guard let session, hasOriginalUnknown(method: "session.resume", crew: crewID, session: session),
              client.capabilities.contains("session.claude.recovery") else { return [] }
        let epoch = generation, route = selection, connection = client.connectionEpoch
        let runtime = client.runtimeIdentity, binding = client.authorityBinding, scope = presentationScope
        func current() -> Bool {
            acceptsObservation(epoch) && selection == route && client.connectionEpoch == connection &&
            client.runtimeIdentity == runtime && client.authorityBinding == binding && presentationScope == scope &&
            !Task.isCancelled && !stopping && client.capabilities.contains("session.claude.recovery") &&
            profileForSession(session)?.backend == "existingClaudePty"
        }
        let inspection = try CrewdClaudeWire.recoveryInspect(
            await readValue("session.claude.recoveryInspect", .object(["sessionId": .string(session)])), session: session)
        guard current() else { throw CancellationError() }
        let target = try await refreshControlTarget(session)
        guard current(), target.value == inspection["target"] else { throw CrewdPublicWire.invalid() }
        // Typed callers must still hold exactly the inspected target; legacy callers
        // obtain a fresh target in explicitControlParameters before command submission.
        let supplied = params["target"] == .null ? params : params["target"]
        if supplied["generation"] != .null {
            guard try CrewdCommandTarget(supplied) == target else { throw CrewdPublicWire.invalid() }
        }
        guard inspection["eligibility"] == .string("noUnresolvedRequests"), inspection["unresolved"].array.isEmpty else { return [] }
        return CrewdClaudeWire.recoveryCovered(inspection)
    }
    func hasOriginalUnknown(method: String, crew: String, channel: String? = nil, session:String? = nil, excludingRequests: Set<String> = []) -> Bool {
        durableUnknown.contains { record in
            guard isCurrentOriginalObservation(record) else { return false }
            let operation = record.method == "command.submit" ? record.params["command"]["operation"].string : record.method
            let params = record.method == "command.submit" ? record.params["command"]["params"] : record.params
            guard operation == method || (method == "crew.create" && operation == "crew.createFirstMessage") else { return false }
            if method == "crew.create" { return true }
            if let channel { return params["channelId"].string == channel }
            guard params["crewId"].string == crew || record.crew == crew else{return false}
            if method == "session.resume", let session, let originalSession = params["target"]["sessionId"].string ?? params["sessionId"].string {
                guard originalSession == session else { return false }
                if record.method == "command.submit", let request = record.params["requestId"].string.flatMap(UUID.init(uuidString:)),
                   excludingRequests.contains(where: { UUID(uuidString: $0) == request }) { return false }
                return true
            }
            return true // Legacy without an exact binding retains its original safe scope.
        }
    }

    /// Read a projection without scheduling writes or implicitly changing selection.
    func readValue(_ method: String, _ params: CrewdValue = .null) async throws -> CrewdValue {
        guard !retired else { throw CrewdClientError(message: "原连接已关闭，不能读取当前团队。", kind: .notSubmitted) }
        let epoch = generation
        let result = try await client.call(method, params)
        guard !retired, generation == epoch else { throw CancellationError() }
        return result
    }

    /// Typed result keeps submission certainty and backend facts with the caller.
    /// The compatibility Bool entry remains for the original Mac controls.
    func performValue(_ method: String, _ params: CrewdValue) async throws -> CrewdValue {
        guard !retired, !busy else { throw CrewdClientError(message: "连接已关闭或正在提交，尚未发送本次操作。", kind: .notSubmitted) }
        let current = generation, selected = selection, requestCrew = crewID
        busy = true; defer { busy = false }; error = nil
        do {
            let result = try await submitRPC(method, params)
            guard !retired, generation == current else { throw CrewdClientError(message: "连接已切换，原操作结果需要在原团队核对。") }
            if method == "crew.create" || method == "session.start" {
                guard let id = result["id"].string, !id.isEmpty else { throw CrewdClientError(message: "创建回执缺少真实编号，结果需要核对。") }
                // A valid older write still returns its receipt to the original draft
                // owner, but must not replace the user's newer selection.
                if selected == selection {
                    if method == "crew.create" { selectCrew(id) }
                    else if crewID == requestCrew, params["crewId"].string == requestCrew { selectSession(id) }
                }
            }
            scheduleSync(); return result
        } catch {
            if !retired, generation == current, selected == selection { self.error = error.localizedDescription; scheduleSync() }
            throw error
        }
    }
    /// Original chat uses a typed single submission. Snapshot identity is UI
    /// bookkeeping only until the public request-status contract freezes.
    func submitOriginalText(_ params: CrewdValue, snapshot: CrewdDrafts.Snapshot, clearPersonal:Bool=true) async throws -> CrewdValue {
        guard !hasOriginalUnknown(method: "message.post", crew: snapshot.context.crew, channel: snapshot.context.channel), !uncertainTextSubmissions.contains(where: { $0.snapshot.context == snapshot.context }) else {
            throw CrewdClientError(message: "原消息结果尚未确认。请核对原连接记录，不会重新发送。", kind: .notSubmitted)
        }
        let personal=personalInputSnapshot(snapshot)
        do {
            let result = try await performValue("message.post", params)
            if clearPersonal{clearPersonalInput(snapshot,personal:personal)}else{messageDrafts.clearIfUnchanged(snapshot)}
            return result
        } catch {
            if CrewdClientError.isUncertainSubmission(error) {
                uncertainTextSubmissions.append(TextSubmission(id: UUID(), method: "message.post", params: params, snapshot: snapshot))
            }
            throw error
        }
    }

    @discardableResult func performText(_ method:String,_ params:CrewdValue,snapshot:CrewdDrafts.Snapshot) async -> Bool {
        let personal=personalInputSnapshot(snapshot)
        do {
            _ = try await performValue(method,params)
            clearPersonalInput(snapshot,personal:personal)
            return true
        } catch {
            if CrewdClientError.isUncertainSubmission(error) {
                uncertainTextSubmissions.append(TextSubmission(id:UUID(),method:method,params:params,snapshot:snapshot))
            }
            return false
        }
    }
    func perform(_ method: String, _ params: CrewdValue) async -> Bool {
        do { _ = try await performValue(method, params); return true }
        catch { return false }
    }
}

#if os(macOS)
/// A retained capability for one real App-owned child. It is separate from the
/// retired business workspace and never grants old crew/terminal write access.
@MainActor
final class CrewdOwnedLocalLifecycle {
    struct Connection {
        let binding: () throws -> CrewdPublicCommands.Binding
        let call: CrewdPublicCommands.Call
        let close: () -> Void
    }
    typealias Connect = () async throws -> Connection
    private let owned: Process
    private let birth: CrewdKernelProcessIdentity
    private let origin: CrewdPublicCommands.Binding
    private let journal: CrewdPresentationJournal
    private let verifyChild: (Int32) throws -> CrewdKernelProcessIdentity
    private let connect: Connect
    private let wait: () async throws -> Void
    private let maximumWaits: Int
    private var closing = false
    private var submitted = false
    private var originalPrepared: CrewdPresentationJournal.Prepared?
    private var originalObservationFinished = false
    private(set) var originalEnvelope: CrewdPublicCommandEnvelope?
    private(set) var lastReceipt: CrewdPublicReceipt?

    init(owned: Process, birth: CrewdKernelProcessIdentity, origin: CrewdPublicCommands.Binding,
         journal: CrewdPresentationJournal, connect: @escaping Connect,
         verifyChild: @escaping (Int32) throws -> CrewdKernelProcessIdentity = CrewdKernelProcessIdentity.ownChild,
         maximumWaits: Int = 300,
         wait: @escaping () async throws -> Void = { try await Task.sleep(for: .milliseconds(100)) }) throws {
        guard maximumWaits > 0, maximumWaits <= 300, owned.isRunning, owned.processIdentifier == birth.pid,
              try verifyChild(birth.pid) == birth, journal.scope == origin.scope,
              origin.scope.localDirectory != nil, origin.scope.databaseID == origin.databaseID,
              UUID(uuidString: origin.runtimeBindingID) != nil else {
            throw CrewdClientError(message: "原本机后台或提交历史未具备关闭资格。", kind: .notSubmitted)
        }
        self.owned = owned; self.birth = birth; self.origin = origin; self.journal = journal
        self.connect = connect; self.verifyChild = verifyChild; self.wait = wait; self.maximumWaits = maximumWaits
    }
    func matches(_ process: Process, birth: CrewdKernelProcessIdentity?) -> Bool {
        owned === process && birth == self.birth
    }
    private var ownsLiveChild: Bool {
        owned.isRunning && owned.processIdentifier == birth.pid && (try? verifyChild(birth.pid)) == birth
    }
    private func qualified(_ socket: Connection) throws -> CrewdPublicCommands.Binding {
        guard ownsLiveChild else { throw CrewdClientError(message: "原后台直接子进程身份未确认，关闭未提交。", kind: .notSubmitted) }
        let actual = try socket.binding()
        guard actual.scope == origin.scope, actual.databaseID == origin.databaseID,
              actual.runtimeBindingID == origin.runtimeBindingID else {
            throw CrewdClientError(message: "原关闭连接的数据库或实例已经改变，未提交。", kind: .notSubmitted)
        }
        return actual
    }
    /// New socket admission and same original child/DB/instance are reverified.
    /// An unknown earlier send permits lookup only, never another submit.
    func quit(crew: String) async throws {
        if !owned.isRunning { return } // Foundation owns and reaps this exact Process.
        guard !closing, ownsLiveChild else {
            throw CrewdClientError(message: "原后台关闭正在核对或拥有关系未确认。", kind: .notSubmitted)
        }
        closing = true; defer { closing = false }
        let socket: Connection
        do { socket = try await connect() }
        catch {
            if !owned.isRunning { return }
            throw CrewdClientError(message: "原后台关闭连接未能重新核实；保留后台：" + error.localizedDescription, kind: .notSubmitted)
        }
        defer { socket.close() }
        let captured: CrewdPublicCommands.Binding
        do { captured = try qualified(socket) }
        catch { if !owned.isRunning { return }; throw error }
        let envelope: CrewdPublicCommandEnvelope
        if let existing = originalEnvelope { envelope = existing }
        else {
            envelope = try CrewdPublicCommandEnvelope(request: UUID(),
                binding: .object(["dbId": .string(origin.databaseID), "runtimeBindingId": .string(origin.runtimeBindingID)]),
                operation: .hibernate, params: .object(["deadlineSecs": .number(0)]))
            originalEnvelope = envelope
        }
        let controller = CrewdPublicCommands(binding: { [self] in
            let value = try qualified(socket)
            guard value == captured else { throw CrewdClientError(message: "原关闭连接已更换，未提交。", kind: .notSubmitted) }
            return value
        }, ownsDaemon: { [self] in ownsLiveChild }, call: { [self] method, params in
            _ = try qualified(socket)
            if method == "command.submit", params == envelope.value, !submitted {
                submitted = true // Conservatively retained before the real call.
            } else if method != "command.lookup" || params != envelope.lookup {
                throw CrewdClientError(message: "原拥有关闭仅允许固定休眠或原请求只读核对。", kind: .notSubmitted)
            }
            return try await socket.call(method, params)
        }, prepare: { [self] proposed, selectedCrew in
            guard proposed == envelope else { throw CrewdPublicWire.invalid() }
            let record = try journal.prepare(method: "command.submit", params: proposed.value, crew: selectedCrew)
            originalPrepared = record
            return record
        }, finish: { [self] record, state, detail in
            // Observations are immutable. Keep the first durable fact, including
            // Unknown; a later physical wait or lookup must not overwrite it.
            guard !originalObservationFinished else { return }
            try journal.finish(record, state: state, detail: detail)
            originalObservationFinished = true
        })
        do {
            let receipt: CrewdPublicReceipt?
            if submitted {
                guard let original = originalPrepared, original.params == envelope.value else {
                    throw CrewdClientError(message: "原关闭已经发送；只读核对资料尚未就绪，不会再次发送。")
                }
                try controller.restore([original])
                guard let observation = controller.observations[envelope.requestID] else { throw CrewdPublicWire.invalid() }
                receipt = try await controller.lookup(observation)
            } else { receipt = try await controller.submit(envelope, crew: crew) }
            lastReceipt = receipt
            if let receipt, [.suppressed, .rejected].contains(receipt.phase) {
                throw CrewdClientError(message: "原关闭请求已经拒绝，后台保留。", kind: .notSubmitted)
            }
        } catch {
            if !owned.isRunning { return } // Unknown journal remains retained.
            if !CrewdClientError.isUncertainSubmission(error) { throw error }
        }
        for _ in 0..<maximumWaits {
            if !owned.isRunning { return }
            try await wait()
        }
        // Exit may occur during the last bounded wait, including lookup.
        if !owned.isRunning { return }
        if let original = originalPrepared, !originalObservationFinished {
            try journal.finish(original, state: "unknown", detail: "原后台实际退出仍未确认；只读核对原关闭，不重发。")
            originalObservationFinished = true
        }
        throw CrewdClientError(message: "尚未确认原后台实际退出；原请求保留，不强制终止或重发。")
    }

    static func live(owned: Process, birth: CrewdKernelProcessIdentity, origin: CrewdPublicCommands.Binding,
                     journal: CrewdPresentationJournal, directory: URL) throws -> CrewdOwnedLocalLifecycle {
        guard let resolved = realpath(directory.path, nil) else { throw CrewdPublicWire.invalid() }
        defer { free(resolved) }
        // POSIX canonical paths preserve /private/tmp; Foundation may rewrite
        // that system path to /tmp. Still reject every noncanonical input.
        guard String(cString: resolved) == directory.path,
              origin.scope.localDirectory == directory.path else { throw CrewdPublicWire.invalid() }
        return try CrewdOwnedLocalLifecycle(owned: owned, birth: birth, origin: origin, journal: journal, connect: {
            guard owned.isRunning, (try? CrewdKernelProcessIdentity.ownChild(birth.pid)) == birth else {
                throw CrewdClientError(message: "原 App 的真实子进程已经改变，未建立关闭连接。", kind: .notSubmitted)
            }
            let next = CrewdClient()
            do {
                try await next.connect(path: directory.appendingPathComponent("crewd.sock").path)
                let binding: () throws -> CrewdPublicCommands.Binding = {
                    guard owned.isRunning, next.connected,
                          (try? CrewdKernelProcessIdentity.ownChild(birth.pid)) == birth,
                          (try? next.verifiedLocalPeerIdentity()) == birth,
                          next.capabilities.contains("command.receipts"),
                          next.runtimeIdentity["dbId"].string == origin.databaseID,
                          next.runtimeIdentity["runtimeBindingId"].string == origin.runtimeBindingID else {
                        throw CrewdClientError(message: "原后台的新关闭连接未通过内核对端和实例核对。", kind: .notSubmitted)
                    }
                    return .init(scope: origin.scope, databaseID: origin.databaseID,
                                 runtimeBindingID: origin.runtimeBindingID, connectionEpoch: next.connectionEpoch)
                }
                _ = try binding()
                return Connection(binding: binding, call: { method, params in try await next.call(method, params) }, close: { next.retire() })
            } catch { next.retire(); throw error }
        })
    }
}
#endif


extension CrewdCodexSessionStore {
    convenience init(workspace:CrewdWorkspace,sessionID:String){
        let crew=workspace.crewID
        self.init(sessionID:sessionID,read:{[weak workspace] method,params in
            guard let workspace else{throw CancellationError()}
            return try await workspace.readValue(method,params)
        },submit:{[weak workspace] method,params in
            guard let workspace else{throw CancellationError()}
            return try await workspace.performValue(method,params)
        },scope:{[weak workspace] in
            guard let workspace,!workspace.isRetired,workspace.client.readyForWork,workspace.crewID == crew else{return nil}
            return workspace.observationGeneration.uuidString+"/"+workspace.client.connectionEpoch.uuidString+"/"+crew
        },available:{[weak workspace] in workspace?.client.capabilities.contains("session.codex") == true})
        observe(workspace.authorityEvents)
    }
}
