// Calls the actual built App module: no replacement workspace or runner.
@testable import PendingCrew
import Foundation
@main struct OriginalSessionFlow {
    @MainActor static func main() async throws {
        let path = CommandLine.arguments[1], scenario = CommandLine.arguments[2]
        let root = URL(fileURLWithPath: path).deletingLastPathComponent().appendingPathComponent(scenario + "-data")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let workspace = CrewdWorkspace(dataDirectory: root)
        try await workspace.client.connect(path: path)
        workspace.crewID = "crew-a"; workspace.sessionID = "session-a"
        defer { workspace.retire() }
        if scenario == "profiles-late" || scenario == "profiles-switch" {
            let task = Task { await workspace.refreshExecutorProfiles() }
            try await Task.sleep(for: .milliseconds(50))
            if scenario == "profiles-late" { workspace.retire() } else { workspace.selectCrew("crew-b") }
            await task.value
            do { try workspace.requireExecutor("codex", crew: "crew-a"); fatalError("stale profile authorized another scope") } catch {}
            print("PASS actual App profile scope: " + scenario); return
        }
        if scenario == "captain-switch-return" {
            let task = Task { await workspace.startCaptainForCreatedCrew("crew-a") }
            try await Task.sleep(for: .milliseconds(50))
            workspace.selectCrew("crew-b"); workspace.selectCrew("crew-a")
            let result = await task.value
            precondition(result == nil, "old captain click survived crew A-B-A selection")
            print("PASS actual App captain A-B-A: no start"); return
        }
        await workspace.refreshExecutorProfiles()
        try await workspace.refreshSessionDirectory("crew-a")
        if scenario == "profiles" {
            let claude = workspace.executorProfile("claude")!
            precondition(claude.canStart && !claude.initialContext && claude.manualTerminal && !claude.structuredDecisions && !claude.fullHistoryLoad)
            try workspace.requireExecutor("claude", crew: "crew-a")
            precondition(workspace.executorProfile("acp:claude-managed")?.canStart == false)
            do { try workspace.requireExecutor("acp:claude-managed", crew: "crew-a"); fatalError("unqualified native executor enabled") } catch {}
            print("PASS actual App profiles: text without extras, accurate features, native blocked"); return
        }
        if scenario == "profiles-missing" || scenario == "profiles-unavailable" {
            do { try workspace.requireExecutor("codex", crew: "crew-a"); fatalError("missing/unavailable profile accepted") } catch {}
            print("PASS actual App profile refusal: " + scenario); return
        }
        // Use the same presentation runner/member-open path as the original UI.
        let runner = CrewSessionRunner(presentationOnly: true, observePowerNotifications: false)
        let backend = CrewdOriginalSessionBackend(workspace: workspace, crewID: "crew-a", sessionID: "session-a", kind: scenario == "claude-unavailable" ? .claudeCode : .codex)
        let run = CrewSessionRun(crewId: "crew-a", sessionId: "session-a", kind: .codex,
                                 taskBrief: "", workingDirectory: root, backend: backend)
        runner.installCrewdProjection([run], workspace: workspace)
        func viewOriginal() async {
            runner.openSession(run.runID)
            precondition(runner.selectedRunId == run.runID && workspace.sessionID == "session-a")
            try? await Task.sleep(for: .milliseconds(450))
        }
        if scenario == "legacy-text" || scenario == "legacy-resume" {
            workspace.originalJournalRequired = true
            await workspace.prepareOriginalJournal(root: root.appendingPathComponent("original-journal"))
            precondition(workspace.originalJournalReady)
            do {
                if scenario == "legacy-text" {
                    _ = try await workspace.performValue("session.start", .object(["crewId": .string("crew-a"), "role": .string("worker"), "executor": .string("claude"), "brief": .string("One explicit text without extra context"), "isolation": .bool(false)]))
                } else {
                    _ = try await workspace.performValue("session.resume", .object(["sessionId": .string("session-a")]))
                }
                fatalError("synthetic unknown receipt treated as success")
            } catch {}
            precondition(workspace.durableUnknown.count == 1)
            await workspace.refreshExecutorProfiles()
            if scenario == "legacy-resume" { await viewOriginal() }
            print("PASS actual App public legacy command: " + scenario + "; unknown retained, no automatic retry"); return
        }
        switch scenario {
        case "quiet":
            runner.select(run.runID)
            await backend.refresh()
            runner.select(run.runID)
            precondition(!runner.viewingTerminal, "programmatic select must stay observational")
        case "click-unavailable", "claude-unavailable", "resume-refused":
            await viewOriginal()
            precondition(runner.lastStartError != nil && !runner.viewingTerminal, "qualification failure must be visible without starting")
        case "captain":
            let first = await workspace.startCaptainForCreatedCrew("crew-a")
            precondition(first == "session-a")
            let second = await workspace.startCaptainForCreatedCrew("crew-a")
            precondition(second == first)
        case "captain-refused":
            let result = await workspace.startCaptainForCreatedCrew("crew-a")
            precondition(result == nil && workspace.error?.contains("机组及已有消息仍保留") == true)
        case "switch", "switch-return":
            let task = Task { await viewOriginal() }
            try await Task.sleep(for: .milliseconds(50))
            if scenario == "switch-return" { workspace.selectSession("session-b"); workspace.selectSession("session-a") }
            else { workspace.sessionID = "session-b" }
            await task.value
        case "double-click":
            let first = Task { await viewOriginal() }
            let second = Task { await viewOriginal() }
            await first.value; await second.value
        default:
            await viewOriginal()
            if scenario == "malformed" { precondition(workspace.error != nil && !runner.viewingTerminal, "malformed liveness must fail visibly without resuming") }
            if scenario == "stopped" { await viewOriginal(); precondition(backend.failure == nil && runner.viewingTerminal) }
        }
        print("PASS actual App session flow: " + scenario)
    }
}
