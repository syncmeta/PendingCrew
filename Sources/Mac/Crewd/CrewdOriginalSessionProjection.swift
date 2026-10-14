#if os(macOS)
import AppKit
import Foundation
import Combine

/// Original session UI projection. This object owns no model process or wake queue.
@MainActor
final class CrewdOriginalSessionBackend: SessionBackend {
    let workspace: CrewdWorkspace
    let sessionID: String
    let kind: LocalCodingAgentKind
    let terminalView: TerminalMirrorView?
    let transcript: CodexTranscript?
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
    init(workspace: CrewdWorkspace, sessionID: String, kind: LocalCodingAgentKind) {
        self.workspace = workspace; self.sessionID = sessionID; self.kind = kind
        terminalView = kind == .codex ? nil : TerminalMirrorView(frame: .zero)
        transcript = kind == .codex ? CodexTranscript() : nil
        // Public raw-input/resize custody has not frozen yet. Actual inspect
        // snapshots are read-only; this is not full terminal delivery.
        terminalView?.onSendBytes = nil; terminalView?.onResize = nil
    }
    func update(live: CrewdValue, inspection: CrewdValue, items: [CrewdValue]) {
        guard live["sessionId"].string == sessionID else { return }
        status = live["running"] == .bool(true) ? .running : .exited(nil)
        isWorking = live["working"] == .bool(true)
        if let screen = inspection["screen"].string, screen != lastScreen {
            lastScreen = screen; hasObservedLaunchSignal = !screen.isEmpty
            let bytes = Array(("\u{1b}[2J\u{1b}[H" + screen).utf8)
            terminalView?.feedFromCore(bytes[...])
        }
        for item in items {
            guard let raw = item.pretty.data(using: .utf8),
                  let decoded = try? JSONSerialization.jsonObject(with: raw) as? [String: Any] else { continue }
            transcript?.apply(method: "item/completed", params: ["item": decoded])
        }
    }
    func send(_ text: String) { failure = "请通过机组消息发送；终端输入须等待正式控制权协议。" }
    func submitWake(_ text: String) async -> SessionWakeSubmission { .blocked }
    func interrupt() { command("session.interrupt", ["sessionId": .string(sessionID)]) }
    func stop() { command("session.stop", ["sessionId": .string(sessionID), "reason": .string("人类在原窗口明确停止")]) }
    private func command(_ method: String, _ fields: [String: CrewdValue]) {
        Task {
            do { _ = try await workspace.performValue(method, .object(fields)); await refresh() }
            catch { failure = error.localizedDescription }
        }
    }
    func refresh() async {
        do {
            let live = try await workspace.readValue("session.live", .object(["sessionId": .string(sessionID)]))
            let inspection = try await workspace.readValue("session.inspect", .object(["sessionId": .string(sessionID)]))
            update(live: live, inspection: inspection, items: inspection["items"].array)
        } catch { failure = error.localizedDescription }
    }
}

@MainActor
final class CrewdOriginalSessionProjection {
    let workspace: CrewdWorkspace
    weak var runner: CrewSessionRunner?
    private var observation: AnyCancellable?
    private var projections: [String: CrewSessionRun] = [:]
    init(workspace: CrewdWorkspace, runner: CrewSessionRunner) {
        self.workspace = workspace; self.runner = runner
        observation = workspace.objectWillChange.sink { [weak self] _ in Task { @MainActor [weak self] in self?.publish() } }
        publish()
    }
    func publish() {
        guard let runner else { return }
        for row in workspace.sessions {
            guard let id = row["id"].string, let crew = row["crewId"].string, let executor = row["executor"].string else { continue }
            guard executor == "codex" || executor == "claude" || executor.hasPrefix("acp:") else { continue }
            if projections[id] == nil {
                let kind: LocalCodingAgentKind = executor == "codex" ? .codex : .claudeCode
                let backend = CrewdOriginalSessionBackend(workspace: workspace, sessionID: id, kind: kind)
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
