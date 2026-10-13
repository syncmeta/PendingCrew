#if os(macOS)
import Foundation
import Combine

@MainActor
final class CrewdWorkspace: ObservableObject {
    let client = CrewdClient()
    lazy var workStore = CrewdWorkStore { [weak self] method, params in
        guard let self else { throw CancellationError() }
        return try await self.client.call(method, params)
    }
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
    let dataDirectory: URL
    private var generation = UUID()
    private var selection = UUID()
    private var sync: Task<Void, Never>?
    private var reconnect: Task<Void, Never>?
    private var reconnectID: UUID?
    private var backend: Process?
    private var again = false
    private var recoverNeeded = false
    private var sleeping = false
    private var booting = false

    init(dataDirectory: URL) {
        self.dataDirectory = dataDirectory
        client.onEvent = { [weak self] name, data in
            guard let self else { return }
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
                self.workStore.invalidateConnection(); self.sleeping = true; self.status = "后台已休眠"; self.reconnect?.cancel(); self.reconnectID = nil
                self.again = false; self.recoverNeeded = false; self.sync?.cancel()
            } else { self.scheduleSync(recoverHistory: name == "events.lagged") }
        }
        client.onDisconnect = { [weak self] in
            guard let self, !self.booting, !self.sleeping, self.reconnectID == nil else { return }
            self.workStore.invalidateConnection(); self.workDirty = true
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

    /// Explicit first-edition launch uses an independent root; no legacy importer runs.
    func connect() async {
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

    private func connectOnly() async throws {
        workStore.invalidateConnection(); workDirty = true; activityDirty = true
        generation = UUID(); let current = generation; sync?.cancel(); sync = nil
        try await client.connect(path: dataDirectory.appendingPathComponent("crewd.sock").path)
        guard generation == current, !Task.isCancelled, !sleeping else { throw CancellationError() }
        error = nil
        status = "已连接 · crewd \(client.version)"
        scheduleSync(recoverHistory: true)
        await sync?.value
        guard generation == current, !Task.isCancelled, !sleeping else { throw CancellationError() }
        guard client.connected else { throw CrewdClientError(message: client.lastError ?? "后台连接已中断。") }
    }

    func selectCrew(_ id: String) {
        guard crewID != id else { return }
        workStore.selectCrew(id); workDirty = true
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
    func selectChannel(_ id: String) { selection = UUID(); channelID = id; messages = []; scheduleSync() }

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
        let current = generation, selected = selection
        let newCrews = try await client.call("crew.list")
        guard current == generation, !sleeping, !Task.isCancelled else { return }
        guard selected == selection else { again = true; return }
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
        workStore.selectCrew(crew)
        if workDirty {
            workDirty = false
            do { try await workStore.refresh() } catch { /* WorkStore retains typed failures; first-edition messaging stays usable. */ }
            guard current == generation, selected == selection, !sleeping, !Task.isCancelled else { return }
        }
        guard !crew.isEmpty else { return }
        let newSessions = try await client.call("session.list", .object(["crewId": .string(crew)]))
        guard !sleeping, !Task.isCancelled, current == generation, selected == selection, crewID == crew else { again = true; return }
        let newChannels = try await client.call("channel.list", .object(["crewId": .string(crew)]))
        guard !sleeping, !Task.isCancelled, current == generation, selected == selection, crewID == crew else { again = true; return }
        for row in newSessions.array {if let id=row["id"].string {knownSessions[id]=row}}
        sessions = newSessions.array; channelSummaries = newChannels.array; channels = newChannels.array.map { $0["channel"] }
        let dmPeer = selectedMembers.contains(where: {$0["kind"].string == "session" && $0["id"].string == sessionID}) && channels.first(where: {$0["id"].string == channelID})?["kind"].string == "dm"
        if !dmPeer && !sessions.contains(where: {$0["id"].string == sessionID}) { sessionID = sessions.first?["id"].string ?? ""; runnerItems = []; history = "" }
        if !channels.contains(where: {$0["id"].string == channelID}) { channelID = channels.first(where: {$0["kind"].string == "human"})?["id"].string ?? channels.first?["id"].string ?? "" }
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

    func perform(_ method: String, _ params: CrewdValue) async -> Bool {
        guard !busy else { return false }
        busy = true; defer { busy = false }; error = nil
        do {
            let result = try await client.call(method, params)
            if method == "crew.create", let id = result["id"].string { selectCrew(id) }
            if method == "session.start", let id = result["id"].string { selectSession(id) }
            scheduleSync(); return true
        }
        catch { self.error = error.localizedDescription; scheduleSync(); return false }
    }
}
#endif
