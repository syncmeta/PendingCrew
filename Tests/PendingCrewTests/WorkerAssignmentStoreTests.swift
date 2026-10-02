#if os(macOS)
import XCTest

final class WorkerAssignmentStoreTests: XCTestCase {
    private func directory() -> URL {
        FileManager.default.temporaryDirectory.appendingPathComponent("worker-assignment-\(UUID())")
    }

    private func source(_ generation: String = "epoch-1",
                        workingDirectory: String = "/tmp/work-a") -> WorkerAssignmentStore.Assignment {
        .init(crewId: "crew", sessionId: "worker-old", generation: generation,
              runner: "codex", brief: "original full assignment", workingDirectory: workingDirectory,
              isolation: true, model: "gpt-6.1-sol", effort: "high", title: "worker title",
              agentSessionId: nil, terminalReason: nil, successorSessionId: nil)
    }

    func testCompleteAssignmentAndTerminalReasonSurviveStoreReopen() throws {
        let dir = directory()
        let store = WorkerAssignmentStore(directory: dir)
        try store.recordLaunch(source())
        try store.recordAgentIdentity(crewId: "crew", sessionId: "worker-old",
                                      generation: "epoch-1", agentSessionId: "thread-original")
        try store.recordTerminal(crewId: "crew", sessionId: "worker-old",
                                 generation: "epoch-1", reason: .failed)
        let row = try XCTUnwrap(WorkerAssignmentStore(directory: dir).records().first)
        XCTAssertEqual(row.brief, "original full assignment")
        XCTAssertEqual(row.workingDirectory, "/tmp/work-a")
        XCTAssertEqual(row.model, "gpt-6.1-sol")
        XCTAssertEqual(row.effort, "high")
        XCTAssertEqual(row.agentSessionId, "thread-original")
        XCTAssertEqual(row.terminalReason, .failed)
    }

    func testOnlyTrustedFailedSourceCanBeClaimedOnceAcrossReopen() throws {
        let dir = directory()
        let store = WorkerAssignmentStore(directory: dir)
        try store.recordLaunch(source())
        try store.recordAgentIdentity(crewId: "crew", sessionId: "worker-old",
                                      generation: "epoch-1", agentSessionId: "thread-original")
        XCTAssertNil(try store.claimManualTakeover(crewId: "crew", sessionId: "worker-old",
                                                    generation: "epoch-1", agentSessionId: "thread-original",
                                                    successorSessionId: "worker-nil"),
                     "terminal=nil without typed old-epoch registry and missing-probe evidence must refuse")
        try store.recordTerminal(crewId: "crew", sessionId: "worker-old",
                                 generation: "epoch-1", reason: .failed)
        XCTAssertNil(try store.claimManualTakeover(crewId: "crew", sessionId: "worker-old",
                                                    generation: "stale", agentSessionId: "thread-original",
                                                    successorSessionId: "worker-new"))
        XCTAssertNil(try store.claimManualTakeover(crewId: "crew", sessionId: "worker-old",
                                                    generation: "epoch-1", agentSessionId: "wrong",
                                                    successorSessionId: "worker-new"))
        let claimed = try store.claimManualTakeover(crewId: "crew", sessionId: "worker-old",
                                                    generation: "epoch-1", agentSessionId: "thread-original",
                                                    successorSessionId: "worker-new")
        XCTAssertEqual(claimed?.successorSessionId, "worker-new")
        XCTAssertNil(try WorkerAssignmentStore(directory: dir).claimManualTakeover(
            crewId: "crew", sessionId: "worker-old", generation: "epoch-1",
            agentSessionId: "thread-original", successorSessionId: "worker-another"))
    }

    func testLaterGenerationAndConflictingEvidenceFailClosed() throws {
        let store = WorkerAssignmentStore(directory: directory())
        try store.recordLaunch(source())
        try store.recordAgentIdentity(crewId: "crew", sessionId: "worker-old",
                                      generation: "epoch-1", agentSessionId: "thread-original")
        XCTAssertThrowsError(try store.recordAgentIdentity(
            crewId: "crew", sessionId: "worker-old", generation: "epoch-1",
            agentSessionId: "different-thread"))
        XCTAssertNil(try store.claimManualTakeover(
            crewId: "crew", sessionId: "worker-old", generation: "epoch-1",
            agentSessionId: "thread-original", successorSessionId: "worker-new"))

        let fresh = WorkerAssignmentStore(directory: directory())
        try fresh.recordLaunch(source())
        try fresh.recordAgentIdentity(crewId: "crew", sessionId: "worker-old",
                                      generation: "epoch-1", agentSessionId: "thread-original")
        try fresh.recordLaunch(source("epoch-2"))
        XCTAssertNil(try fresh.claimManualTakeover(
            crewId: "crew", sessionId: "worker-old", generation: "epoch-1",
            agentSessionId: "thread-original", successorSessionId: "worker-new"))
    }

    func testCorruptAuthorityBytesAndUnavailableLockRefuseClaim() throws {
        let dir = directory()
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let store = WorkerAssignmentStore(directory: dir)
        let bad = Data("[{\"crewId\":\"crew\"}]".utf8)
        try bad.write(to: store.fileURL)
        XCTAssertThrowsError(try store.claimManualTakeover(
            crewId: "crew", sessionId: "worker-old", generation: "epoch-1",
            agentSessionId: "thread-original", successorSessionId: "worker-new"))
        XCTAssertEqual(try Data(contentsOf: store.fileURL), bad)

        let lockedDir = directory()
        try FileManager.default.createDirectory(at: lockedDir, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(
            at: lockedDir.appendingPathComponent("worker-assignments.lock"),
            withIntermediateDirectories: true)
        XCTAssertThrowsError(try WorkerAssignmentStore(directory: lockedDir).recordLaunch(source()))
    }

    func testTwoIndependentProcessesConsumeOnlyOneManualTakeover() throws {
        let dir = directory()
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let store = WorkerAssignmentStore(directory: dir)
        try store.recordLaunch(source())
        try store.recordAgentIdentity(crewId: "crew", sessionId: "worker-old",
                                      generation: "epoch-1", agentSessionId: "thread-original")
        try store.recordTerminal(crewId: "crew", sessionId: "worker-old",
                                 generation: "epoch-1", reason: .failed)
        let bundle = Bundle(for: Self.self).bundleURL
        func launch(_ suffix: String) throws -> Process {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/xcrun")
            process.arguments = ["xctest", "-XCTest",
                                 "WorkerAssignmentStoreTests/testManualTakeoverClaimHarness",
                                 bundle.path]
            var environment = ProcessInfo.processInfo.environment
            for key in environment.keys where key.hasPrefix("XCTest") {
                environment.removeValue(forKey: key)
            }
            environment["PENDINGCREW_WORKER_CLAIM_DIR"] = dir.path
            environment["PENDINGCREW_WORKER_CLAIM_SUFFIX"] = suffix
            process.environment = environment
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            try process.run()
            return process
        }
        let first = try launch("first")
        let second = try launch("second")
        defer {
            for process in [first, second] where process.isRunning {
                process.terminate()
                process.waitUntilExit()
            }
        }
        for _ in 0..<100 {
            let ready = ["first", "second"].allSatisfy {
                FileManager.default.fileExists(atPath: dir.appendingPathComponent("ready-\($0)").path)
            }
            if ready { break }
            Thread.sleep(forTimeInterval: 0.02)
        }
        XCTAssertTrue(["first", "second"].allSatisfy {
            FileManager.default.fileExists(atPath: dir.appendingPathComponent("ready-\($0)").path)
        })
        try Data().write(to: dir.appendingPathComponent("start"))
        first.waitUntilExit()
        second.waitUntilExit()
        XCTAssertEqual([first.terminationStatus, second.terminationStatus].filter { $0 == 0 }.count, 1)
        let outcomes = try ["first", "second"].map {
            try String(contentsOf: dir.appendingPathComponent("outcome-\($0)"))
        }
        XCTAssertEqual(outcomes.filter { $0 == "claimed" }.count, 1)
        XCTAssertEqual(outcomes.filter { $0 == "refused" }.count, 1)
    }

    func testManualTakeoverClaimHarness() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let directory = environment["PENDINGCREW_WORKER_CLAIM_DIR"],
              let suffix = environment["PENDINGCREW_WORKER_CLAIM_SUFFIX"]
        else { return }
        let dir = URL(fileURLWithPath: directory)
        try Data().write(to: dir.appendingPathComponent("ready-\(suffix)"))
        for _ in 0..<500 where !FileManager.default.fileExists(
            atPath: dir.appendingPathComponent("start").path) {
            Thread.sleep(forTimeInterval: 0.01)
        }
        let claimed = try WorkerAssignmentStore(directory: dir).claimManualTakeover(
            crewId: "crew", sessionId: "worker-old", generation: "epoch-1",
            agentSessionId: "thread-original", successorSessionId: "worker-\(suffix)") != nil
        try Data((claimed ? "claimed" : "refused").utf8).write(
            to: dir.appendingPathComponent("outcome-\(suffix)"))
        XCTAssertTrue(claimed)
    }

    func testCompletedStoppedAndMissingIdentityNeverClaim() throws {
        for reason in [WorkerAssignmentStore.Assignment.TerminalReason.completed, .userStopped] {
            let store = WorkerAssignmentStore(directory: directory())
            try store.recordLaunch(source())
            try store.recordAgentIdentity(crewId: "crew", sessionId: "worker-old",
                                          generation: "epoch-1", agentSessionId: "thread-original")
            try store.recordTerminal(crewId: "crew", sessionId: "worker-old",
                                     generation: "epoch-1", reason: reason)
            XCTAssertNil(try store.claimManualTakeover(crewId: "crew", sessionId: "worker-old",
                                                        generation: "epoch-1", agentSessionId: "thread-original",
                                                        successorSessionId: "worker-new"))
        }
        XCTAssertNil(try WorkerAssignmentStore(directory: directory()).claimManualTakeover(
            crewId: "crew", sessionId: "worker-old", generation: "epoch-1",
            agentSessionId: "thread-original", successorSessionId: "worker-new"))
    }

    func testTypedManualOperationRequiresExplicitConfirmationAndAgentRunner() throws {
        let request = try WorkerManualTakeoverRequest(
            crewId: "crew", sourceSessionId: "worker-old",
            runner: .claudeCode, confirmed: true)
        XCTAssertEqual(WorkerManualTakeoverRequest(wireArguments: request.wireArguments), request)
        var unconfirmed = request.wireArguments
        unconfirmed["confirmed"] = .bool(false)
        XCTAssertNil(WorkerManualTakeoverRequest(wireArguments: unconfirmed))
        unconfirmed.removeValue(forKey: "confirmed")
        XCTAssertNil(WorkerManualTakeoverRequest(wireArguments: unconfirmed))
        var terminal = request.wireArguments
        terminal["runner"] = .string("terminal")
        XCTAssertNil(WorkerManualTakeoverRequest(wireArguments: terminal))
        XCTAssertThrowsError(try WorkerManualTakeoverRequest(
            crewId: "crew", sourceSessionId: "worker-old",
            runner: .codex, confirmed: false))
    }

    @MainActor
    func testNilTerminalRefusesBeforeClaimAndZeroLaunch() async throws {
        let dir = directory()
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let store = WorkerAssignmentStore(directory: dir)
        try store.recordLaunch(source(workingDirectory: dir.path))
        try store.recordAgentIdentity(crewId: "crew", sessionId: "worker-old",
                                      generation: "epoch-1", agentSessionId: "thread-original")
        let detail = CrewDetail(
            crew: .init(id: "crew", title: "Crew", responsibleSubjectId: "human",
                        runtimeLocation: "local_host", workingDirectory: dir.path,
                        captainBotId: nil, status: nil, createdAt: "2026-10-01T00:00:00Z",
                        updatedAt: "2026-10-01T00:00:00Z", captainAgentKind: "codex"),
            parents: [], children: [], shares: [], captain: nil)
        var launches = 0
        let admission = AutomaticWakeAdmission(directory: dir)
        let runner = CrewSessionRunner(
            sessionPublisher: InProcessSessionProtocolBridge(wakeAdmission: admission),
            wakeAdmission: admission, launchOperationOverride: { launches += 1 },
            workerAssignmentStore: store, observePowerNotifications: false)
        do {
            _ = try await runner.takeOverWorkerManually(
                detail: detail, backend: nil, sourceSessionId: "worker-old",
                newRunner: .codex)
            XCTFail("terminal=nil has no typed absence proof")
        } catch WorkerManualTakeoverError.ineligibleSource { }
        XCTAssertEqual(launches, 0)
        XCTAssertNil(try store.latest(crewId: "crew", sessionId: "worker-old")?.successorSessionId)
    }

    @MainActor
    func testMissingOriginalAuthorityRefusesBeforeAnyLaunch() async throws {
        let dir = directory()
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let store = WorkerAssignmentStore(directory: dir)
        let detail = CrewDetail(
            crew: .init(id: "crew", title: "Crew", responsibleSubjectId: "human",
                        runtimeLocation: "local_host", workingDirectory: dir.path,
                        captainBotId: nil, status: nil, createdAt: "2026-10-01T00:00:00Z",
                        updatedAt: "2026-10-01T00:00:00Z", captainAgentKind: "codex"),
            parents: [], children: [], shares: [], captain: nil)
        var launches = 0
        let runner = CrewSessionRunner(
            launchOperationOverride: { launches += 1 }, workerAssignmentStore: store,
            observePowerNotifications: false)
        do {
            _ = try await runner.takeOverWorkerManually(
                detail: detail, backend: nil, sourceSessionId: "worker-old",
                newRunner: .claudeCode)
            XCTFail("missing original authority must refuse")
        } catch WorkerManualTakeoverError.ineligibleSource { }
        XCTAssertEqual(launches, 0)
        XCTAssertEqual(try store.records().count, 0)
    }

    @MainActor
    func testNoProcessHumanTakeoverUsesOriginalFullBriefAndCreatesNewRunner() async throws {
        let dir = directory()
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let store = WorkerAssignmentStore(directory: dir)
        var old = source()
        old = .init(crewId: old.crewId, sessionId: old.sessionId,
                    generation: old.generation, runner: old.runner, brief: old.brief,
                    workingDirectory: dir.path, isolation: false, model: old.model,
                    effort: old.effort, title: old.title, agentSessionId: nil,
                    terminalReason: nil, successorSessionId: nil)
        try store.recordLaunch(old)
        try store.recordAgentIdentity(crewId: "crew", sessionId: "worker-old",
                                      generation: "epoch-1", agentSessionId: "thread-original")
        try store.recordTerminal(crewId: "crew", sessionId: "worker-old",
                                 generation: "epoch-1", reason: .failed)
        let detail = CrewDetail(
            crew: .init(id: "crew", title: "Crew", responsibleSubjectId: "human",
                        runtimeLocation: "local_host", workingDirectory: dir.path,
                        captainBotId: nil, status: nil, createdAt: "2026-10-01T00:00:00Z",
                        updatedAt: "2026-10-01T00:00:00Z", captainAgentKind: "codex"),
            parents: [], children: [], shares: [], captain: nil)
        var launches = 0
        let admission = AutomaticWakeAdmission(directory: dir)
        let runner = CrewSessionRunner(
            sessionPublisher: InProcessSessionProtocolBridge(wakeAdmission: admission),
            wakeAdmission: admission, launchOperationOverride: { launches += 1 },
            workerAssignmentStore: store, observePowerNotifications: false)
        let successor = try await runner.takeOverWorkerManually(
            detail: detail, backend: nil, sourceSessionId: "worker-old",
            sourceGeneration: "epoch-1", sourceAgentSessionId: "thread-original",
            newRunner: .claudeCode)
        XCTAssertEqual(launches, 1)
        let rows = try store.records()
        XCTAssertEqual(rows.count, 2)
        XCTAssertEqual(rows[0].successorSessionId, successor)
        XCTAssertEqual(rows[1].sessionId, successor)
        XCTAssertEqual(rows[1].brief, "original full assignment")
        XCTAssertEqual(rows[1].runner, "claude_code")
        XCTAssertEqual(rows[1].workingDirectory, dir.path,
                       "new runner must use the original workspace, including uncommitted work")
        XCTAssertNil(try WorkerAssignmentStore(directory: dir).claimManualTakeover(
            crewId: "crew", sessionId: "worker-old", generation: "epoch-1",
            agentSessionId: "thread-original", successorSessionId: "worker-third"))
    }

    @MainActor
    func testManualTakeoverDoesNotBypassMachineAdmissionStop() async throws {
        let dir = directory()
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let store = WorkerAssignmentStore(directory: dir)
        try store.recordLaunch(source(workingDirectory: dir.path))
        try store.recordAgentIdentity(crewId: "crew", sessionId: "worker-old",
                                      generation: "epoch-1", agentSessionId: "thread-original")
        try store.recordTerminal(crewId: "crew", sessionId: "worker-old",
                                 generation: "epoch-1", reason: .failed)
        let admission = AutomaticWakeAdmission(directory: dir)
        for index in 0..<12 {
            guard case let .allowed(token) = admission.reserve(
                sessionId: "seed-\(index)", crewId: "crew-\(index)", sourceKey: "seed-\(index)")
            else { return XCTFail("setup") }
            XCTAssertTrue(admission.finish(token: token, accepted: false))
        }
        let detail = CrewDetail(
            crew: .init(id: "crew", title: "Crew", responsibleSubjectId: "human",
                        runtimeLocation: "local_host", workingDirectory: dir.path,
                        captainBotId: nil, status: nil, createdAt: "2026-10-01T00:00:00Z",
                        updatedAt: "2026-10-01T00:00:00Z", captainAgentKind: "codex"),
            parents: [], children: [], shares: [], captain: nil)
        var launches = 0
        let runner = CrewSessionRunner(
            sessionPublisher: InProcessSessionProtocolBridge(wakeAdmission: admission),
            wakeAdmission: admission, launchOperationOverride: { launches += 1 },
            launchDenialNoticeOverride: { _ in }, workerAssignmentStore: store,
            observePowerNotifications: false)
        do {
            _ = try await runner.takeOverWorkerManually(
                detail: detail, backend: nil, sourceSessionId: "worker-old",
                sourceGeneration: "epoch-1", sourceAgentSessionId: "thread-original",
                newRunner: .codex)
            XCTFail("machine admission must refuse the launch")
        } catch { }
        XCTAssertEqual(launches, 0)
        XCTAssertNotNil(try store.latest(crewId: "crew", sessionId: "worker-old")?.successorSessionId,
                        "failed handoff remains consumed for human review, never auto retried")
    }

    @MainActor
    func testClaimedOldSourceCannotRestartThroughSharedMemberEntryAcrossReopen() async throws {
        let dir = directory()
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let store = WorkerAssignmentStore(directory: dir)
        try store.recordLaunch(source(workingDirectory: dir.path))
        try store.recordAgentIdentity(crewId: "crew", sessionId: "worker-old",
                                      generation: "epoch-1", agentSessionId: "thread-original")
        try store.recordTerminal(crewId: "crew", sessionId: "worker-old",
                                 generation: "epoch-1", reason: .failed)
        XCTAssertNotNil(try store.claimManualTakeover(
            crewId: "crew", sessionId: "worker-old", generation: "epoch-1",
            agentSessionId: "thread-original", successorSessionId: "worker-new"))
        let detail = CrewDetail(
            crew: .init(id: "crew", title: "Crew", responsibleSubjectId: "human",
                        runtimeLocation: "local_host", workingDirectory: dir.path,
                        captainBotId: nil, status: nil, createdAt: "2026-10-01T00:00:00Z",
                        updatedAt: "2026-10-01T00:00:00Z", captainAgentKind: "codex"),
            parents: [], children: [], shares: [], captain: nil)
        let member = LocalSessionMember(sessionId: "worker-old", displayName: "old worker",
                                        createdAt: "2026-10-01T00:00:00Z")
        var launches = 0
        let admission = AutomaticWakeAdmission(directory: dir)
        let runner = CrewSessionRunner(
            sessionPublisher: InProcessSessionProtocolBridge(wakeAdmission: admission),
            wakeAdmission: admission, launchOperationOverride: { launches += 1 },
            workerAssignmentStore: WorkerAssignmentStore(directory: dir),
            observePowerNotifications: false)
        do {
            try await runner.restartMember(detail: detail, backend: nil, member: member,
                                           wakeText: "@ from old member", userInitiated: false)
            XCTFail("claimed old source must not restart")
        } catch {
            XCTAssertTrue(error.localizedDescription.contains("已被人工接手"),
                          "shared restart entry must reject the claim itself, not some later launch failure: \(error)")
        }
        XCTAssertEqual(launches, 0)
        XCTAssertEqual(try store.records().count, 1,
                       "restart must not append a new generation after successor claim")
        XCTAssertThrowsError(try WorkerAssignmentStore(directory: dir).recordLaunch(
            source("epoch-2", workingDirectory: dir.path)),
            "lock-time admission must also refuse a stale restart racing the claim")
    }

    @MainActor
    func testMissingOriginalWorkspaceRefusesBeforeConsumingClaim() async throws {
        let dir = directory()
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let store = WorkerAssignmentStore(directory: dir)
        try store.recordLaunch(source())
        try store.recordAgentIdentity(crewId: "crew", sessionId: "worker-old",
                                      generation: "epoch-1", agentSessionId: "thread-original")
        try store.recordTerminal(crewId: "crew", sessionId: "worker-old",
                                 generation: "epoch-1", reason: .failed)
        let detail = CrewDetail(
            crew: .init(id: "crew", title: "Crew", responsibleSubjectId: "human",
                        runtimeLocation: "local_host", workingDirectory: dir.path,
                        captainBotId: nil, status: nil, createdAt: "2026-10-01T00:00:00Z",
                        updatedAt: "2026-10-01T00:00:00Z", captainAgentKind: "codex"),
            parents: [], children: [], shares: [], captain: nil)
        var launches = 0
        let admission = AutomaticWakeAdmission(directory: dir)
        let runner = CrewSessionRunner(
            sessionPublisher: InProcessSessionProtocolBridge(wakeAdmission: admission),
            wakeAdmission: admission, launchOperationOverride: { launches += 1 },
            workerAssignmentStore: store, observePowerNotifications: false)
        do {
            _ = try await runner.takeOverWorkerManually(
                detail: detail, backend: nil, sourceSessionId: "worker-old",
                sourceGeneration: "epoch-1", sourceAgentSessionId: "thread-original",
                newRunner: .codex)
            XCTFail("missing original worktree must refuse")
        } catch WorkerManualTakeoverError.sourceWorkspaceUnavailable { }
        XCTAssertEqual(launches, 0)
        XCTAssertNil(try store.latest(crewId: "crew", sessionId: "worker-old")?.successorSessionId)
    }
}
#endif
