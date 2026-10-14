import Foundation
import Combine

@MainActor
final class CrewdWorkspace: ObservableObject {
    let client = CrewdClient()
    lazy var workStore = CrewdWorkStore { [weak self] method, params in
        guard let self, !self.retired else { throw CrewdClientError(message: "连接已切换，旧作用域不能提交。", kind: .notSubmitted) }
        return try await self.client.call(method, params)
    }
    lazy var resources=CrewdResources { [weak self] method,params in
        guard let self, !self.retired else {throw CrewdClientError(message: "连接已切换，旧作用域不能提交。", kind: .notSubmitted)}
        return try await self.client.call(method,params)
    }
    lazy var channelWork=CrewdChannelWorkStore { [weak self] method,params in
        guard let self, !self.retired else{throw CrewdClientError(message: "连接已切换，旧作用域不能提交。", kind: .notSubmitted)};return try await self.client.call(method,params)
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
    @Published var uncertainSessionStarts: [CrewdDrafts.Snapshot] = []
    @Published var crewCreationDrafts = CrewdCrewCreationDrafts()
    @Published var messageDrafts = CrewdDrafts()
    @Published var requirementDrafts = CrewdRequirementDrafts()
    @Published var planDrafts = CrewdPlanDrafts()
    @Published var humanDrafts = CrewdHumanDrafts()
    @Published private(set) var knownSessions: [String:CrewdValue] = [:]
    @Published var pendingAsks: [CrewdValue] = []
    @Published var restartPrompt: CrewdValue = .null
    @Published var messages: [CrewdValue] = []
    @Published var crewID = ""
    @Published var sessionID = ""
    @Published var channelID = ""
    @Published var inspection: CrewdValue = .null
    @Published var runnerItems: [CrewdValue] = []
    @Published var history = ""
    @Published var status = "未连接"
    @Published var error: String?
    @Published var busy = false
    /// Immutable cache/draft namespace. The pairing transport must separately
    /// authenticate the peer; a namespace UUID is never authentication evidence.
    let authorityRecordID: UUID
    let dataDirectory: URL?
    typealias Connector = @MainActor (CrewdClient) async throws -> Void
    private let connector: Connector
    private var retired = false
    private var generation = UUID()
    private var selection = UUID()
    private var sessionDirectoryQuery = UUID()
    private var crewDirectoryQuery = UUID()
    private var sync: Task<Void, Never>?
    private var reconnect: Task<Void, Never>?
    private var reconnectID: UUID?
    #if os(macOS)
    private var backend: Process?
    #endif
    private var again = false
    private var recoverNeeded = false
    private var sleeping = false
    private var booting = false

    #if os(macOS)
    convenience init(dataDirectory: URL) {
        self.init(authorityRecordID: UUID(), dataDirectory: dataDirectory, connector: { client in
            try await client.connect(path: dataDirectory.appendingPathComponent("crewd.sock").path)
        })
    }
    #endif
    /// One instance per paired authority. Switching authorities retires this
    /// instance and creates another; public W/#/H numbers never share caches.
    convenience init(authorityRecordID: UUID, connect: @escaping Connector) {
        self.init(authorityRecordID: authorityRecordID, dataDirectory: nil, connector: connect)
    }
    private init(authorityRecordID: UUID, dataDirectory: URL?, connector: @escaping Connector) {
        self.authorityRecordID = authorityRecordID
        self.dataDirectory = dataDirectory
        self.connector = connector
        client.onEvent = { [weak self] name, data in
            guard let self, !self.retired else { return }
            self.resources.handleEvent(name,data)
            self.channelWork.handleEvent(name,data)
            if name == "document.changed" || name == "viewFolder.changed" || name == "channel.workChanged" { return }
            if name.hasPrefix("message.") {
                if let channel=data["channelId"].string,let crew=self.activityChannelCrews[channel] {
                    if !self.activityDirty || !self.activityAffected.isEmpty {self.activityAffected.insert(crew)}
                } else {self.activityAffected.removeAll()}
                self.activityDirty=true
            } else if name.hasPrefix("crew.") || name == "events.lagged" {self.activityDirty=true;self.activityAffected.removeAll()}
            if name.hasPrefix("work.") || name.hasPrefix("ask.") || name.hasPrefix("crew.") || name == "events.lagged" { self.workDirty = true }
            if name == "session.runnerItem", data["sessionId"].string == self.sessionID {
                let item = data["item"]
                if let id = item["id"].string, let index = self.runnerItems.firstIndex(where: {$0["id"].string == id}) { self.runnerItems[index] = item }
                else { self.runnerItems.append(item); if self.runnerItems.count > 200 { self.runnerItems.removeFirst() } }
            } else if name == "system.shutdown" {
                self.resources.invalidateConnection(); self.channelWork.invalidateConnection(); self.workStore.invalidateConnection(); self.sleeping = true; self.status = "后台已休眠"; self.reconnect?.cancel(); self.reconnectID = nil
                self.again = false; self.recoverNeeded = false; self.sync?.cancel()
            } else { self.scheduleSync(recoverHistory: name == "events.lagged") }
        }
        client.onDisconnect = { [weak self] in
            guard let self, !self.retired, !self.booting, !self.sleeping, self.reconnectID == nil else { return }
            self.resources.invalidateConnection(); self.channelWork.invalidateConnection(); self.workStore.invalidateConnection(); self.workDirty = true
            self.status = "连接中断，正在重新连接"; self.error = self.client.lastError
            self.reconnect?.cancel()
            let attempt = UUID(); self.reconnectID = attempt
            self.reconnect = Task { [weak self] in
                defer { if self?.reconnectID == attempt { self?.reconnectID = nil } }
                for delay in [1, 2, 4, 8, 8] {
                    try? await Task.sleep(for: .seconds(delay))
                    guard !Task.isCancelled, let self, self.reconnectID == attempt else { return }
                    do { try await self.connectOnly(); return } catch {
                        guard !Task.isCancelled, self.reconnectID == attempt, !self.sleeping else { return }
                        self.error = error.localizedDescription
                    }
                }
                if self?.reconnectID == attempt { self?.status = "连接已中断" }
            }
        }
    }

    /// Retain unknown inputs in this old scope for inspection; never replay them
    /// into the next authority or let a late reconnect reactivate a retired one.
    func retire() {
        retired = true; sleeping = true; generation = UUID(); selection = UUID()
        sync?.cancel(); sync = nil; reconnect?.cancel(); reconnect = nil; reconnectID = nil
        again = false; recoverNeeded = false
        resources.invalidateConnection(); channelWork.invalidateConnection(); workStore.invalidateConnection()
        client.retire(); status = "已切换连接"
    }
    func connect() async {
        guard !retired, !booting else { return }
        #if os(macOS)
        if let dataDirectory { await connectLocal(dataDirectory); return }
        #endif
        booting = true; defer { booting = false }
        reconnect?.cancel(); reconnectID = nil; sleeping = false; error = nil; status = "正在连接"
        do { try await connectOnly() }
        catch { if !retired, !Task.isCancelled { self.error = error.localizedDescription; status = "连接失败" } }
    }
    #if os(macOS)
    /// Explicit local launch uses an independent root; no legacy importer runs.
    private func connectLocal(_ dataDirectory: URL) async {
        guard !booting else { return }
        booting = true; defer { booting = false }
        reconnect?.cancel(); reconnectID = nil; sleeping = false; error = nil; status = "正在连接"
        let legacy = ProcessInfo.processInfo.environment["PENDINGCREW_DATA_DIR"].map { URL(fileURLWithPath: $0) } ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/PendingCrew")
        let candidatePath = dataDirectory.standardizedFileURL.resolvingSymlinksInPath().path
        let legacyPath = legacy.standardizedFileURL.resolvingSymlinksInPath().path
        guard candidatePath != legacyPath, !candidatePath.hasPrefix(legacyPath + "/") else {
            error = "首版试用须使用独立数据目录，不能与旧版共用。"; status = "连接未建立"; return
        }
        do { try await connectOnly(); return } catch {
            guard !sleeping, !Task.isCancelled else { return }
            if let failure = error as? CrewdClientError, failure.kind == .backendHandshake {
                self.error = failure.localizedDescription; status = "后台协议连接失败"; return
            }
            guard backend?.isRunning != true else { self.error = error.localizedDescription; status = "后台连接失败"; return }
        }
        do {
            let env = ProcessInfo.processInfo.environment
            let bundled = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/crewd")
            let program = env["PENDINGCREW_CREWD_PROGRAM"].map { URL(fileURLWithPath: $0) } ?? bundled
            guard FileManager.default.isExecutableFile(atPath: program.path) else { throw CrewdClientError(message: "未找到配套后台程序，请使用完整的 Codex 首版应用。") }
            let codexCandidates = [env["PENDINGCREW_CODEX_PROGRAM"], FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".local/bin/codex").path, "/opt/homebrew/bin/codex", "/usr/local/bin/codex"].compactMap {$0}
            guard let codex = codexCandidates.first(where: {FileManager.default.isExecutableFile(atPath: $0)}) else { throw CrewdClientError(message: "未找到 Codex CLI，请先安装并登录 Codex。") }
            let p = Process(); p.executableURL = program
            p.arguments = ["--data-dir", dataDirectory.path, "serve", "--codex", codex, "--claude", "/usr/bin/false"]
            p.environment = env
            // crewd owns a lock and its sessions. GUI reconnect never imports data or replays tasks.
            let logs = dataDirectory.appendingPathComponent("client-backend.log")
            if !FileManager.default.fileExists(atPath: dataDirectory.path) {
                try FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            }
            if !FileManager.default.fileExists(atPath: logs.path) { FileManager.default.createFile(atPath: logs.path, contents: nil, attributes: [.posixPermissions: 0o600]) }
            let log = try FileHandle(forWritingTo: logs); try log.seekToEnd()
            p.standardOutput = log; p.standardError = log
            try p.run(); backend = p; try log.close()
            for _ in 0..<30 {
                try await Task.sleep(for: .milliseconds(100))
                do { try await connectOnly(); return } catch { if sleeping || Task.isCancelled { return } }
                if !p.isRunning { throw CrewdClientError(message: "后台启动失败，请查看连接信息中的日志。") }
            }
            throw CrewdClientError(message: "后台启动后未能建立连接，请检查后台日志。")
        } catch {
            guard !sleeping, !Task.isCancelled else { return }
            self.error = error.localizedDescription; status = "连接失败"
        }
    }

    #endif
    private func connectOnly() async throws {
        guard !retired else { throw CrewdClientError(message: "连接已切换，旧作用域不能重新提交。", kind: .notSubmitted) }
        resources.invalidateConnection(); channelWork.invalidateConnection(); workStore.invalidateConnection(); workDirty = true; activityDirty = true
        generation = UUID(); let current = generation; sync?.cancel(); sync = nil
        try await connector(client)
        guard generation == current, !retired, !Task.isCancelled, !sleeping else {
            if retired { client.disconnect() }
            throw CancellationError()
        }
        workStore.configureCapabilities(client.capabilities)
        resources.configureCapabilities(client.capabilities)
        channelWork.configureCapabilities(client.capabilities)
        guard generation == current, !Task.isCancelled, !sleeping else { throw CancellationError() }
        error = nil
        status = "已连接 · crewd \(client.version)"
        scheduleSync(recoverHistory: true)
        await sync?.value
        guard generation == current, !Task.isCancelled, !sleeping else { throw CancellationError() }
        guard client.connected else { throw CrewdClientError(message: client.lastError ?? "后台连接已中断。") }
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
        workStore.selectCrew(id); resources.selectCrew(id); workDirty = true
        channelWork.selectChannel(crew:id,channel:"")
        selection = UUID(); crewID = id; sessionID = ""; channelID = ""; sessions = []; channels = []; messages = []; inspection = .null; runnerItems = []; history = ""
        scheduleSync(recoverHistory: true)
    }
    func selectConversation(_ id: String) {
        selectChannel(id)
        if let summary = channelSummaries.first(where: {$0["channel"]["id"].string == id}), summary["channel"]["kind"].string == "dm",
           let member = summary["members"].array.first(where: {$0["kind"].string == "session"})?["id"].string { selectSession(member) }
    }
    var selectedMembers: [CrewdValue] { channelSummaries.first(where: {$0["channel"]["id"].string == channelID})?["members"].array ?? [] }
    func selectSession(_ id: String) { selection = UUID(); sessionID = id; inspection = .null; runnerItems = []; history = ""; scheduleSync(recoverHistory: true) }
    func selectChannel(_ id: String) { selection = UUID(); channelID = id;channelWork.selectChannel(crew:crewID,channel:id); messages = []; scheduleSync() }

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
        workStore.selectCrew(crew); resources.selectCrew(crew)
        if workDirty {
            workDirty = false
            do { try await workStore.refresh() } catch { /* WorkStore retains typed failures; first-edition messaging stays usable. */ }
            guard current == generation, selected == selection, !sleeping, !Task.isCancelled else { return }
        }
        guard !crew.isEmpty else { return }
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
        if !channels.contains(where: {$0["id"].string == channelID}) { channelID = channels.first(where: {$0["kind"].string == "human"})?["id"].string ?? channels.first?["id"].string ?? "" }
        channelWork.selectChannel(crew:crew,channel:channelID)
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
        }
    }

    /// Typed result keeps submission certainty and backend facts with the caller.
    /// The compatibility Bool entry remains for the original Mac controls.
    func performValue(_ method: String, _ params: CrewdValue) async throws -> CrewdValue {
        guard !retired, !busy else { throw CrewdClientError(message: "连接已关闭或正在提交，尚未发送本次操作。", kind: .notSubmitted) }
        let current = generation, selected = selection, requestCrew = crewID
        busy = true; defer { busy = false }; error = nil
        do {
            let result = try await client.call(method, params)
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
    @discardableResult func performText(_ method:String,_ params:CrewdValue,snapshot:CrewdDrafts.Snapshot) async -> Bool {
        do {
            _ = try await performValue(method,params)
            messageDrafts.clearIfUnchanged(snapshot)
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
