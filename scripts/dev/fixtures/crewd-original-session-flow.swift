// Calls the actual built App module: no replacement workspace or runner.
@testable import PendingCrew
import Foundation
@main struct OriginalSessionFlow {
    @MainActor static func main() async throws {
        let path = CommandLine.arguments[1], scenario = CommandLine.arguments[2]
        let root = URL(fileURLWithPath: path).deletingLastPathComponent().appendingPathComponent(scenario + "-data")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let workspace = scenario.hasPrefix("remote-")
            ? CrewdWorkspace(authorityRecordID: UUID(), connect: { client in try await client.connect(path: path) })
            : CrewdWorkspace(dataDirectory: root)
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
        if scenario == "initialization-once" {
            let previous = ProcessInfo.processInfo.environment["PENDINGCREW_CREWD_DATA_DIR"]
            setenv("PENDINGCREW_CREWD_DATA_DIR", "relative-path-must-be-refused", 1)
            defer {
                if let previous { setenv("PENDINGCREW_CREWD_DATA_DIR", previous, 1) }
                else { unsetenv("PENDINGCREW_CREWD_DATA_DIR") }
            }
            let model = AppModel()
            precondition(model.crewdDirectory == nil)
            await model.initializeCrewdAuthority()
            precondition(model.crewdWindow.workspace == nil && model.crewdWindow.error != nil)
            model.crewdWindow.error = "preserve first attempt"
            await model.initializeCrewdAuthority()
            precondition(model.crewdWindow.error == "preserve first attempt", "remount retried initialization")
            await model.connectCrewdAuthority(computerID: UUID())
            precondition(model.crewdWindow.error != "preserve first attempt", "explicit selection was blocked")
            let explicit = AppModel()
            await explicit.connectCrewdAuthority(computerID: UUID())
            explicit.crewdWindow.error = "preserve explicit choice"
            await explicit.initializeCrewdAuthority()
            precondition(explicit.crewdWindow.error == "preserve explicit choice", "startup replaced explicit choice")
            print("PASS actual App initialization: remount does not retry or override explicit selection; explicit retry remains")
            return
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
        if scenario == "authority-error-scope" {
            runner.lastStartError = "old authority resume remains unknown"
            runner.installCrewdProjection([run], workspace: workspace)
            precondition(runner.lastStartError != nil, "same authority refresh must retain its error")
            let other = CrewdWorkspace(dataDirectory: root.appendingPathComponent("other"))
            defer { other.retire() }
            runner.installCrewdProjection([], workspace: other)
            precondition(runner.lastStartError == nil, "old authority error leaked into another host")
            print("PASS actual App authority error isolation"); return
        }
        if scenario.hasPrefix("remote-") {
            // Exercise the real backend seam without claiming this Unix receiver is TLS.
            if scenario == "remote-quiet" {
                await backend.refresh()
                precondition(backend.hasObservedLive && backend.failure == nil)
            } else {
                backend.resumeFromExplicitOpen()
                if scenario == "remote-switch" {
                    try await Task.sleep(for: .milliseconds(50))
                    workspace.selectSession("session-b")
                }
                try await Task.sleep(for: .milliseconds(450))
                if scenario == "remote-malformed" { precondition(backend.failure != nil) }
                else if scenario != "remote-switch" { precondition(backend.failure == nil && backend.status == .running) }
            }
            print("PASS actual backend remote query route: " + scenario); return
        }
        func viewOriginal() async {
            runner.openSession(run.runID)
            precondition(runner.selectedRunId == run.runID && workspace.sessionID == "session-a")
            try? await Task.sleep(for: .milliseconds(450))
        }
        if scenario.hasPrefix("receipt-") {
            workspace.originalJournalRequired = true
            let journalRoot = root.appendingPathComponent("original-journal")
            await workspace.prepareOriginalJournal(root: journalRoot)
            precondition(workspace.originalJournalReady)
            do {
                _ = try await workspace.performValue("session.resume", .object(["sessionId": .string("session-a")]))
                fatalError("unresolved original resume was reported successful")
            } catch {}
            precondition(workspace.durableUnknown.count == 1)
            let original = workspace.durableUnknown[0]
            // A fresh explicit click must not mint another command while this one is unresolved.
            do {
                _ = try await workspace.performValue("session.resume", .object(["sessionId": .string("session-a")]))
                fatalError("repeat click bypassed original Unknown")
            } catch {}
            precondition(workspace.durableUnknown.count == 1 && workspace.durableUnknown[0].id == original.id)
            // Release the synthetic delayed receipt only after the real observation window ended.
            if scenario == "receipt-timeout" { try Data().write(to: root.appendingPathComponent("receipt-ready")) }
            let receipt = try await workspace.lookupOriginalCommand(original)
            precondition(receipt?.phase == .launched)
            // Resume recovery requires explicit acknowledgement, even when lookup finds launch.
            precondition(workspace.durableUnknown.count == 1)
            if scenario == "receipt-historical" {
                do {
                    try workspace.acknowledgeOriginalCommand(original, receipt: receipt!)
                    fatalError("historical launch cleared current uncertainty")
                } catch {}
            } else {
                try workspace.acknowledgeOriginalCommand(original, receipt: receipt!)
            }
            let reopened = try CrewdPresentationJournal(root: journalRoot, scope: workspace.presentationScope!)
            let remaining = try reopened.unknownRecords()
            precondition(remaining.contains { $0.id == original.id } == (scenario == "receipt-historical"))
            print("PASS original resume receipt recovery: " + scenario + "; one submission, original UUID lookup, durable acknowledgement")
            return
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
