import SwiftUI

/// T4.5 viewer side — watch a crew's sessions running on **any** host (incl.
/// another machine).
///
/// Two layers, by design:
///   * **Durable transcript** — polls `listCrewSessions` + `getSessionEvents`
///     every few seconds. This is the verified, always-available source: the
///     runner writes those rows via register→create→claim→events→finish, so
///     the log renders even if the runner is offline or the socket is down.
///   * **Live state** — opens a `SessionProxyClient` (role:.viewer) on the
///     selected session and renders the runner's real-time `session.state`
///     fan-out in a banner, so progress shows up the instant the runner
///     publishes it instead of waiting for the next 3s poll.
///
/// The live layer is best-effort decoration over the durable one; if the WS
/// can't connect, the poll still drives the transcript.
struct RemoteSessionsView: View {
    let crewId: String
    let crewTitle: String

    @EnvironmentObject private var appModel: AppModel
    @Environment(\.dismiss) private var dismiss

    @State private var sessions: [CrewSessionSummary] = []
    @State private var selectedId: String?
    @State private var events: [CrewSessionEvent] = []
    @State private var loadError: String?
    @State private var didInitialLoad = false

    // Live state layer — one viewer proxy socket for the selected session.
    @State private var liveClient: SessionProxyClient?
    @State private var liveTask: Task<Void, Never>?
    @State private var liveSnapshot: SessionStateSnapshot?
    // Steering composer draft (send_prompt to the running session).
    @State private var promptDraft = ""
    // Pending ask_human interactions for the selected session + per-id reply drafts.
    @State private var pendingInteractions: [CrewInteraction] = []
    @State private var interactionDrafts: [String: String] = [:]

    private let pollInterval: UInt64 = 3_000_000_000 // 3s

    var body: some View {
        NavigationSplitView {
            sessionList
                .navigationSplitViewColumnWidth(min: 240, ideal: 280)
        } detail: {
            VStack(spacing: 0) {
                if let liveSnapshot {
                    liveBanner(liveSnapshot)
                    Divider()
                }
                if !pendingInteractions.isEmpty {
                    interactionPane
                    Divider()
                }
                eventPane
                if let s = selectedSession, isStoppable(s.status) {
                    Divider()
                    steeringComposer
                }
            }
        }
        .frame(minWidth: 720, minHeight: 460)
        .toolbar {
            ToolbarItem(placement: .principal) {
                Text("服务端 session · \(crewTitle)").font(.headline)
            }
            if let s = selectedSession, isStoppable(s.status) {
                ToolbarItem(placement: .automatic) {
                    Button(role: .destructive) {
                        guard let client = liveClient else { return }
                        Task { await client.sendCommand(kind: "cancel") }
                    } label: {
                        Label("停止", systemImage: "stop.circle")
                    }
                    .help("向运行该 session 的机器发送停止命令")
                    .disabled(liveClient == nil)
                }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button("完成") { dismiss() }
            }
        }
        .task { await pollLoop() }
        .onChange(of: selectedId) { _, newValue in subscribeLive(to: newValue) }
        .onDisappear { teardownLive() }
    }

    /// Steering composer — send a `send_prompt` to the running session. The
    /// runner applies it as a `--resume` follow-up turn (T4.5). Only shown for a
    /// live (non-terminal) session; disabled until the proxy socket is up.
    private var steeringComposer: some View {
        HStack(spacing: 8) {
            TextField("给这个 session 发一条指令…", text: $promptDraft, axis: .vertical)
                .textFieldStyle(.roundedBorder)
                .lineLimit(1...4)
                .onSubmit(sendSteering)
            // Primary tap = send once; menu = set/clear the standing note that
            // injects into every turn (T4.5 ambient steering).
            Menu {
                Button("发送（一次性）", systemImage: "paperplane") { sendSteering() }
                Button("设为常驻便签", systemImage: "pin") { setStandingNote() }
                Button("清除常驻便签", systemImage: "pin.slash", role: .destructive) { clearStandingNote() }
            } label: {
                Image(systemName: "paperplane.fill")
            } primaryAction: {
                sendSteering()
            }
            .menuStyle(.button)
            .fixedSize()
            .disabled(liveClient == nil)
        }
        .padding(8)
    }

    private func sendSteering() {
        let text = promptDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, let client = liveClient else { return }
        promptDraft = ""
        Task { await client.sendCommand(kind: "send_prompt", payload: ["text": .string(text)]) }
    }

    /// Set the standing steering note to the composer text (injected into every
    /// turn until changed/cleared).
    private func setStandingNote() {
        let text = promptDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, let client = liveClient else { return }
        promptDraft = ""
        Task { await client.sendCommand(kind: "set_steering", payload: ["text": .string(text)]) }
    }

    private func clearStandingNote() {
        guard let client = liveClient else { return }
        Task { await client.sendCommand(kind: "set_steering", payload: ["text": .string("")]) }
    }

    /// The agent is blocked asking the human something (T4.5 ask_human). One
    /// card per pending interaction; answering unblocks the session.
    private var interactionPane: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(pendingInteractions) { intr in
                VStack(alignment: .leading, spacing: 6) {
                    HStack(spacing: 6) {
                        Image(systemName: "person.fill.questionmark").foregroundStyle(.orange)
                        Text("Agent 在问你").font(.caption.weight(.semibold)).foregroundStyle(.orange)
                    }
                    Text(intr.question).font(.callout).textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    HStack(spacing: 8) {
                        TextField("回答…", text: interactionBinding(intr.id), axis: .vertical)
                            .textFieldStyle(.roundedBorder).lineLimit(1...4)
                            .onSubmit { answer(intr.id) }
                        Button("回答") { answer(intr.id) }
                            .keyboardShortcut(.return, modifiers: .command)
                            .disabled((interactionDrafts[intr.id] ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }
                .padding(10)
                .background(.orange.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
            }
        }
        .padding(12)
    }

    private func interactionBinding(_ id: String) -> Binding<String> {
        Binding(get: { interactionDrafts[id] ?? "" }, set: { interactionDrafts[id] = $0 })
    }

    private func answer(_ reqId: String) {
        let reply = (interactionDrafts[reqId] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        guard !reply.isEmpty, let api = try? appModel.loggedAPIClient() else { return }
        interactionDrafts[reqId] = nil
        pendingInteractions.removeAll { $0.id == reqId } // optimistic; refetch confirms
        Task { try? await api.answerInteraction(reqId: reqId, reply: reply) }
    }

    /// The live-state banner — driven by the runner's real-time `session.state`
    /// fan-out over the proxy socket, distinct from the polled durable log below.
    private func liveBanner(_ snap: SessionStateSnapshot) -> some View {
        HStack(spacing: 8) {
            Circle().fill(tintForStatus(snap.status)).frame(width: 7, height: 7)
            Text("实时").font(.caption.weight(.semibold)).foregroundStyle(tintForStatus(snap.status))
            Text(snap.status).font(.caption2).foregroundStyle(.secondary)
            if let last = snap.lastEvent, !last.isEmpty {
                Text("·").foregroundStyle(.secondary)
                Text(last).font(.caption).lineLimit(1).truncationMode(.tail)
            }
            Spacer()
            Text("\(snap.eventCount) 事件").font(.caption2).foregroundStyle(.secondary)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .background(.quaternary.opacity(0.5))
    }

    // MARK: - panes

    private var sessionList: some View {
        List(selection: $selectedId) {
            if let loadError {
                Section { Text(loadError).foregroundStyle(.red).font(.caption) }
            }
            if sessions.isEmpty && didInitialLoad {
                Section { Text("这个 crew 还没有服务端 session").foregroundStyle(.secondary).font(.caption) }
            }
            ForEach(sessions) { s in
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        statusDot(s.status)
                        Text(s.taskBrief.isEmpty ? "(无简介)" : s.taskBrief)
                            .lineLimit(1)
                        Spacer()
                    }
                    Text("\(runnerLabel(s.runnerKind)) · \(s.status)")
                        .font(.caption2).foregroundStyle(.secondary)
                }
                .padding(.vertical, 2)
                .tag(s.id)
            }
        }
    }

    @ViewBuilder
    private var eventPane: some View {
        if selectedId == nil {
            Text("选一个 session 看转写").foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if events.isEmpty {
            VStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("拉取事件…").font(.caption).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 8) {
                        ForEach(events) { ev in
                            eventRow(ev).id(ev.id)
                        }
                        Color.clear.frame(height: 1).id("tail")
                    }
                    .padding(12)
                }
                .onChange(of: events.count) { _, _ in
                    withAnimation(.linear(duration: 0.05)) { proxy.scrollTo("tail", anchor: .bottom) }
                }
            }
        }
    }

    private func eventRow(_ ev: CrewSessionEvent) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: icon(ev.eventType)).foregroundStyle(tint(ev.eventType))
            VStack(alignment: .leading, spacing: 2) {
                Text(ev.eventType).font(.caption.weight(.semibold))
                if let s = ev.summary, !s.isEmpty {
                    Text(s).font(.callout).textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
    }

    // MARK: - polling

    private func pollLoop() async {
        while !Task.isCancelled {
            await refresh()
            didInitialLoad = true
            try? await Task.sleep(nanoseconds: pollInterval)
        }
    }

    private func refresh() async {
        guard let api = try? appModel.loggedAPIClient() else {
            loadError = "未登录"
            return
        }
        do {
            let list = try await api.listCrewSessions(crewId: crewId)
            sessions = list
            loadError = nil
            if selectedId == nil { selectedId = list.first?.id }
            if let sid = selectedId {
                events = try await api.getSessionEvents(sessionId: sid)
                // Pending ask_human interactions for this session (best-effort —
                // a fetch failure leaves the prior list rather than blanking it).
                if let intr = try? await api.listSessionInteractions(sessionId: sid) {
                    pendingInteractions = intr
                }
            }
        } catch {
            loadError = error.localizedDescription
        }
    }

    /// The currently selected session row, if any.
    private var selectedSession: CrewSessionSummary? {
        guard let selectedId else { return nil }
        return sessions.first { $0.id == selectedId }
    }

    /// A session a viewer can still stop — pre-terminal states only.
    private func isStoppable(_ status: String) -> Bool {
        switch status {
        case "completed", "failed", "cancelled": return false
        default: return true
        }
    }

    // MARK: - live state (viewer proxy socket)

    /// (Re)subscribe the live layer to `sessionId`. Tears down the previous
    /// session's socket first so only the visible session holds a connection.
    /// Best-effort — failure to build the client (not logged in) just leaves the
    /// poll-driven transcript as the sole source.
    private func subscribeLive(to sessionId: String?) {
        teardownLive()
        guard let sessionId, let api = try? appModel.loggedAPIClient() else { return }
        let client = SessionProxyClient(
            baseURL: api.baseURL, sessionId: sessionId, role: .viewer, token: api.bearerToken)
        liveClient = client
        liveTask = Task {
            await client.connect()
            for await snap in client.states {
                liveSnapshot = snap
            }
        }
    }

    /// Cancel the live consumer + close the socket, and clear the banner.
    private func teardownLive() {
        liveTask?.cancel()
        liveTask = nil
        liveSnapshot = nil
        let client = liveClient
        liveClient = nil
        Task { await client?.close() }
    }

    // MARK: - presentation helpers

    private func runnerLabel(_ kind: String) -> String {
        switch kind {
        case "local_claude_code": return "Claude Code"
        case "local_codex": return "Codex"
        case "cloud_sandbox": return "Cloud"
        default: return kind
        }
    }

    private func statusDot(_ status: String) -> some View {
        Circle().fill(tintForStatus(status)).frame(width: 7, height: 7)
    }

    private func tintForStatus(_ status: String) -> Color {
        switch status {
        case "running", "waiting_runner", "waiting_permission": return .blue
        case "completed": return .green
        case "failed": return .red
        case "cancelled": return .secondary
        default: return .orange
        }
    }

    private func icon(_ eventType: String) -> String {
        switch eventType {
        case "started", "queued": return "play.circle"
        case "tool_call": return "wrench.and.screwdriver"
        case "tool_result": return "arrow.turn.down.right"
        case "permission_requested", "permission_resolved": return "lock.shield"
        case "blocked", "failed": return "exclamationmark.triangle"
        case "completed": return "checkmark.seal"
        case "cancelled": return "xmark.circle"
        default: return "circle"
        }
    }

    private func tint(_ eventType: String) -> Color {
        switch eventType {
        case "tool_call", "tool_result": return .orange
        case "permission_requested", "permission_resolved": return .purple
        case "blocked", "failed": return .red
        case "completed": return .green
        case "cancelled": return .secondary
        default: return .blue
        }
    }
}
