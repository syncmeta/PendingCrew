#if os(macOS)
import AppKit
import Foundation
import Combine

/// Original session UI projection. This object owns no model process or wake queue.
@MainActor
final class CrewdOriginalSessionBackend: SessionBackend, ObservableObject {
    let workspace: CrewdWorkspace
    let sessionID: String
    private let observedCrew: String
    let kind: LocalCodingAgentKind
    let terminalView: TerminalMirrorView?
    let transcript: CodexTranscript?
    let publicTerminal: CrewdPublicTerminal?
    @Published private(set) var status: SessionStatus = .exited(nil)
    @Published private(set) var isWorking = false
    @Published private(set) var health: CrewSessionHealth?
    @Published private(set) var failure: String?
    var statusPublisher: Published<SessionStatus>.Publisher { $status }
    var isWorkingPublisher: Published<Bool>.Publisher { $isWorking }
    var healthPublisher: Published<CrewSessionHealth?>.Publisher { $health }
    var isBusy: Bool { isWorking }
    private(set) var hasObservedLaunchSignal = false
    private var lastScreen: String?
    private var renderingCoreOutput = false
    private var retired = false
    private(set) var hasObservedLive = false
    init(workspace: CrewdWorkspace, crewID: String, sessionID: String, kind: LocalCodingAgentKind) {
        self.workspace = workspace; self.sessionID = sessionID; self.kind = kind; observedCrew = crewID
        terminalView = kind == .codex ? nil : TerminalMirrorView(frame: .zero)
        transcript = kind == .codex ? CodexTranscript() : nil
        publicTerminal = kind != .codex && workspace.client.capabilities.contains("session.terminal")
            ? CrewdPublicTerminal(workspace: workspace, sessionID: sessionID) : nil
        terminalView?.onSendBytes = { [weak publicTerminal] bytes in publicTerminal?.enqueueInput(Data(bytes)) }
        terminalView?.onResize = { [weak self] cols, rows in
            guard let self, !self.renderingCoreOutput else { return }
            Task { await self.publicTerminal?.resize(columns: cols, rows: rows) }
        }
        publicTerminal?.onSnapshot = { [weak self] snapshot in
            guard let self else { return }
            self.renderingCoreOutput = true; defer { self.renderingCoreOutput = false }
            let bytes = [UInt8]([27, 91, 50, 74, 27, 91, 72]) + Array(snapshot.bytes ?? Data((snapshot.text ?? "").utf8))
            self.terminalView?.feedFromCore(bytes[...])
        }
        publicTerminal?.onBytes = { [weak self] bytes in
            guard let self else { return }
            self.renderingCoreOutput = true; defer { self.renderingCoreOutput = false }
            let bytes = Array(bytes); self.terminalView?.feedFromCore(bytes[...])
        }
    }
    func update(live: CrewdValue, inspection: CrewdValue, items: [CrewdValue]) {
        guard !retired, workspace.acceptsObservation(workspace.observationGeneration), live["sessionId"].string == sessionID else {
            transcript?.revokeKnowledgeCallDetails(); return
        }
        let currentCrew = workspace.crewID == observedCrew && workspace.sessionID == sessionID
        if !currentCrew { transcript?.revokeKnowledgeCallDetails() }
        hasObservedLive = true
        status = live["running"] == .bool(true) ? .running : .exited(nil)
        isWorking = live["working"] == .bool(true)
        let kinds: [String: CrewSessionHealth.Kind] = ["auth_required": .authRequired, "usage_limit": .usageLimit,
            "rate_limited": .rateLimited, "launch_failed": .launchFailed, "brief_undelivered": .briefUndelivered,
            "prompt_undelivered": .promptUndelivered, "turn_failed": .turnFailed, "version_incompatible": .cliVersionIncompatible]
        if let raw = live["health"]["kind"].string, let kind = kinds[raw], let detail = live["health"]["detail"].string {
            health = CrewSessionHealth(kind: kind, detail: detail)
        } else { health = nil }
        if publicTerminal?.connected != true, let screen = inspection["screen"].string, screen != lastScreen {
            lastScreen = screen; hasObservedLaunchSignal = !screen.isEmpty
            let bytes = Array(("\u{1b}[2J\u{1b}[H" + screen).utf8)
            renderingCoreOutput = true
            terminalView?.feedFromCore(bytes[...])
            renderingCoreOutput = false
        }
        for item in items {
            guard let raw = item.pretty.data(using: .utf8),
                  let decoded = try? JSONSerialization.jsonObject(with: raw) as? [String: Any] else { continue }
            transcript?.apply(method: "item/completed", params: ["item": decoded])
            if currentCrew { transcript?.observeKnowledgeCall(decoded) }
        }
    }
    func send(_ text: String) { failure = "请通过机组消息发送，或在此原会话页明确取得手工终端控制。" }
    func submitWake(_ text: String) async -> SessionWakeSubmission { .blocked }
    func interrupt() { command("session.interrupt", ["sessionId": .string(sessionID)]) }
    func stop() { command("session.stop", ["sessionId": .string(sessionID), "reason": .string("人类在原窗口明确停止")]) }
    func resume() { command("session.resume", ["sessionId": .string(sessionID)]) }
    func retireDisplay() {
        retired = true
        transcript?.revokeKnowledgeCallDetails()
        publicTerminal?.retire()
        terminalView?.onSendBytes = nil; terminalView?.onResize = nil
    }
    private func command(_ method: String, _ fields: [String: CrewdValue]) {
        let epoch = workspace.observationGeneration
        guard !retired, workspace.acceptsObservation(epoch) else { return }
        var fields = fields
        if workspace.client.capabilities.contains("command.receipts") {
            guard let target = workspace.controlTargets[sessionID] else { failure = "当前真实代际尚未读取，控制尚未提交。"; return }
            fields.removeValue(forKey: "sessionId")
            if method == "session.interrupt" || method == "session.resume" {
                guard case .object(let current) = target.value else { return }; fields = current
            } else { fields["target"] = target.value }
        }
        Task {
            // Authority may have switched while this main-actor task was queued.
            guard !retired, workspace.acceptsObservation(epoch) else { return }
            do {
                _ = try await workspace.performValue(method, .object(fields))
                guard !retired, workspace.acceptsObservation(epoch) else { return }
                await refresh()
            } catch {
                guard !retired, workspace.acceptsObservation(epoch) else { return }
                failure = error.localizedDescription
            }
        }
    }
    func refresh() async {
        let epoch = workspace.observationGeneration
        guard !retired, workspace.acceptsObservation(epoch) else { return }
        do {
            let live = try await workspace.readValue("session.live", .object(["sessionId": .string(sessionID)]))
            guard !retired, workspace.acceptsObservation(epoch) else { return }
            let inspection = try await workspace.readValue("session.inspect", .object(["sessionId": .string(sessionID)]))
            guard !retired, workspace.acceptsObservation(epoch) else { return }
            if workspace.client.capabilities.contains("command.receipts") {
                _ = try await workspace.refreshControlTarget(sessionID)
                guard !retired, workspace.acceptsObservation(epoch) else { return }
            }
            update(live: live, inspection: inspection, items: inspection["items"].array)
        } catch {
            guard !retired, workspace.acceptsObservation(epoch) else { return }
            failure = error.localizedDescription
        }
    }
}

@MainActor
final class CrewdOriginalSessionProjection {
    let workspace: CrewdWorkspace
    weak var runner: CrewSessionRunner?
    private var observation: AnyCancellable?
    private var projections: [String: CrewSessionRun] = [:]
    private var observedGeneration: UUID?
    init(workspace: CrewdWorkspace, runner: CrewSessionRunner) {
        self.workspace = workspace; self.runner = runner
        runner.installCrewdProjection([], workspace: workspace)
        observation = workspace.objectWillChange.sink { [weak self] _ in Task { @MainActor [weak self] in self?.publish() } }
        publish()
    }
    deinit {
        // Queued actor work cannot revive a retired projection; observation
        // cancellation leaves every real crewd session untouched.
        let old = Array(projections.values)
        Task { @MainActor in old.forEach { $0.retireCrewdDisplayObservation() } }
    }
    func publish() {
        guard let runner else { return }
        let epoch = workspace.observationGeneration
        if observedGeneration != epoch {
            for old in projections.values { old.retireCrewdDisplayObservation() }
            projections.removeAll(); observedGeneration = epoch
            runner.installCrewdProjection([], workspace: workspace)
        }
        for old in projections.values where !workspace.acceptsObservation(epoch) || old.crewId != workspace.crewID || old.sessionId != workspace.sessionID {
            (old.backend as? CrewdOriginalSessionBackend)?.transcript?.revokeKnowledgeCallDetails()
        }
        guard workspace.acceptsObservation(epoch) else { return }
        for row in workspace.sessions {
            guard let id = row["id"].string, let crew = row["crewId"].string, let executor = row["executor"].string else { continue }
            guard executor == "codex" || executor == "claude" || executor.hasPrefix("acp:") else { continue }
            if projections[id] == nil {
                let kind: LocalCodingAgentKind = executor == "codex" ? .codex : .claudeCode
                let backend = CrewdOriginalSessionBackend(workspace: workspace, crewID: crew, sessionID: id, kind: kind)
                let display = CrewSessionRun(crewId: crew, sessionId: id, kind: kind,
                    taskBrief: row["brief"].string ?? "", title: row["title"].string ?? "",
                    workingDirectory: URL(fileURLWithPath: row["workingDirectory"].string ?? "/"),
                    model: row["model"].string, effort: row["effort"].string, backend: backend,
                    role: row["role"] == .string("captain") ? .captain : .worker, isMirror: true)
                display.remoteOrchestration = { [weak backend] action in
                    switch action { case .stop: backend?.stop(); case .interrupt: backend?.interrupt() }
                }
                projections[id] = display
                Task { await backend.refresh() }
            }
            if id == workspace.sessionID, let backend = projections[id]?.backend as? CrewdOriginalSessionBackend {
                backend.update(live: workspace.inspection["live"], inspection: workspace.inspection, items: workspace.runnerItems)
            }
        }
        runner.installCrewdProjection(projections.values.sorted { $0.sessionId < $1.sessionId }, workspace: workspace)
    }
}
#endif
