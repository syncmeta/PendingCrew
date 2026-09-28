import Foundation
import XCTest

final class AutomaticWakeAdmissionTests: XCTestCase {
    private func store() -> AutomaticWakeAdmission {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("wake-admission-tests-\(UUID().uuidString)")
        return AutomaticWakeAdmission(directory: dir)
    }

    func testSameAndCrossSourceShareSessionBudget() {
        let gate = store()
        let t = Date(timeIntervalSince1970: 1_000_000)
        for source in ["sweep", "timer"] {
            guard case let .allowed(token) = gate.reserve(
                sessionId: "s", crewId: "c", sourceKey: source, now: t,
                uptime: 100) else { return XCTFail("first two must pass") }
            XCTAssertTrue(gate.finish(token: token, accepted: true))
        }
        XCTAssertFalse(gate.reserve(sessionId: "s", crewId: "c", sourceKey: "listen",
                                    now: t, uptime: 100).isAllowed)
        XCTAssertFalse(gate.reserve(sessionId: "s", crewId: "c", sourceKey: "sweep",
                                    now: t, uptime: 100).isAllowed)
    }

    func testRestartAndClockJumpsCannotResetBudget() {
        let first = store()
        let t = Date(timeIntervalSince1970: 1_000_000)
        for index in 0..<2 {
            guard case let .allowed(token) = first.reserve(
                sessionId: "s", crewId: "c", sourceKey: "source-\(index)",
                now: t, uptime: 100) else { return XCTFail("setup") }
            XCTAssertTrue(first.finish(token: token, accepted: true))
        }
        let reopened = AutomaticWakeAdmission(directory: first.directory)
        XCTAssertFalse(reopened.reserve(sessionId: "s", crewId: "c", sourceKey: "back",
                                       now: t.addingTimeInterval(-3600), uptime: 101).isAllowed)
        XCTAssertFalse(reopened.reserve(sessionId: "s", crewId: "c", sourceKey: "forward",
                                       now: t.addingTimeInterval(86400), uptime: 102).isAllowed)
        XCTAssertFalse(reopened.reserve(sessionId: "s", crewId: "c", sourceKey: "reboot",
                                       now: t.addingTimeInterval(172800), uptime: 1).isAllowed)
    }

    func testUnreadableLedgerFailsClosedForAutomaticAndManual() throws {
        let gate = store()
        try FileManager.default.createDirectory(at: gate.directory, withIntermediateDirectories: true)
        try Data("broken".utf8).write(to: gate.fileURL)
        XCTAssertFalse(gate.reserve(sessionId: "s", crewId: "c", sourceKey: "auto").isAllowed)
        XCTAssertFalse(gate.reserve(sessionId: "s", crewId: "c", sourceKey: "human",
                                    priority: .human).isAllowed)
        XCTAssertEqual(try Data(contentsOf: gate.fileURL), Data("broken".utf8))
    }

    func testConcurrentReservationsAreAtomicAcrossInstances() {
        let gate = store()
        let lock = NSLock()
        var tokens: [String] = []
        DispatchQueue.concurrentPerform(iterations: 20) { index in
            let other = AutomaticWakeAdmission(directory: gate.directory)
            if case let .allowed(token) = other.reserve(
                sessionId: "s", crewId: "c", sourceKey: "source-\(index)",
                now: Date(timeIntervalSince1970: 1_000_000), uptime: 100) {
                lock.lock(); tokens.append(token); lock.unlock()
            }
        }
        XCTAssertLessThanOrEqual(tokens.count, 2)
    }

    func testRejectedBackendDoesNotCountAsDelivered() {
        let gate = store()
        guard case let .allowed(token) = gate.reserve(sessionId: "s", crewId: "c",
                                                      sourceKey: "one") else { return XCTFail("setup") }
        XCTAssertTrue(gate.finish(token: token, accepted: false))
        XCTAssertTrue(gate.reserve(sessionId: "s", crewId: "c", sourceKey: "one").isAllowed)
    }

    func testRejectedBackendStillConsumesAttemptBudgetAndTripsStickyCircuit() {
        let gate = store()
        let t = Date(timeIntervalSince1970: 1_000_000)
        for index in 0..<2 {
            guard case let .allowed(token) = gate.reserve(
                sessionId: "s", crewId: "c", sourceKey: "failed-\(index)",
                now: t, uptime: 100) else { return XCTFail("two reservations should pass") }
            XCTAssertTrue(gate.finish(token: token, accepted: false))
        }
        XCTAssertFalse(gate.reserve(sessionId: "s", crewId: "c", sourceKey: "third",
                                    now: t, uptime: 100).isAllowed)
        XCTAssertFalse(AutomaticWakeAdmission(directory: gate.directory).reserve(
            sessionId: "s", crewId: "c", sourceKey: "after-restart",
            now: t.addingTimeInterval(12 * 3600), uptime: 12 * 3600 + 100).isAllowed)
    }

    func testHumanAndEmergencyStillObserveMachineHardLimitAndUnreadableLedger() throws {
        let gate = store()
        let t = Date(timeIntervalSince1970: 1_000_000)
        for index in 0..<12 {
            let priority: AutomaticWakeAdmission.Priority = index.isMultiple(of: 2)
                ? .human : .emergency
            guard case let .allowed(token) = gate.reserve(
                sessionId: "s\(index)", crewId: "c\(index)", sourceKey: "explicit-\(index)",
                priority: priority, now: t, uptime: 100) else { return XCTFail("setup") }
            XCTAssertTrue(gate.finish(token: token, accepted: false))
        }
        XCTAssertFalse(gate.reserve(sessionId: "human", crewId: "new", sourceKey: "more",
                                    priority: .human, now: t, uptime: 100).isAllowed)
        XCTAssertFalse(gate.reserve(sessionId: "urgent", crewId: "new", sourceKey: "more",
                                    priority: .emergency, now: t, uptime: 100).isAllowed)
        let corrupt = store()
        try FileManager.default.createDirectory(at: corrupt.directory, withIntermediateDirectories: true)
        try Data("broken".utf8).write(to: corrupt.fileURL)
        XCTAssertFalse(corrupt.reserve(sessionId: "s", crewId: "c", sourceKey: "human",
                                       priority: .human).isAllowed)
    }

    func testCrewAndMachineWindowsCannotBeBypassedWithNewSessions() {
        let gate = store()
        let t = Date(timeIntervalSince1970: 1_000_000)
        for index in 0..<5 {
            guard case let .allowed(token) = gate.reserve(
                sessionId: "crew-one-session-\(index)", crewId: "one",
                sourceKey: "message-\(index)", now: t, uptime: 100)
            else { return XCTFail("crew setup") }
            XCTAssertTrue(gate.finish(token: token, accepted: true))
        }
        XCTAssertFalse(gate.reserve(sessionId: "fresh", crewId: "one", sourceKey: "new",
                                    now: t, uptime: 100).isAllowed)
        for index in 0..<7 {
            guard case let .allowed(token) = gate.reserve(
                sessionId: "other-session-\(index)", crewId: "other-\(index)",
                sourceKey: "message-\(index)", now: t, uptime: 100)
            else { return XCTFail("machine setup") }
            XCTAssertTrue(gate.finish(token: token, accepted: true))
        }
        XCTAssertFalse(gate.reserve(sessionId: "new", crewId: "new", sourceKey: "new",
                                    now: t, uptime: 100).isAllowed)
        XCTAssertFalse(gate.reserve(sessionId: "new", crewId: "new", sourceKey: "human",
                                    priority: .human, now: t, uptime: 100).isAllowed)
        XCTAssertFalse(gate.reserve(sessionId: "new", crewId: "new", sourceKey: "urgent",
                                    priority: .emergency, now: t, uptime: 100).isAllowed)
    }

    func testAcceptedTimedLeaseSurvivesRestartUntilSourceAcknowledgesIt() {
        let gate = store()
        guard case let .allowed(token) = gate.reserve(
            sessionId: "s", crewId: "c", sourceKey: "scheduled:w1|target:s")
        else { return XCTFail("setup") }
        XCTAssertTrue(gate.finish(token: token, accepted: true))
        let reopened = AutomaticWakeAdmission(directory: gate.directory)
        XCTAssertEqual(reopened.acceptedLease(sourceKey: "scheduled:w1"), true)
        XCTAssertTrue(reopened.acknowledgeLease(sourceKey: "scheduled:w1"))
        XCTAssertEqual(AutomaticWakeAdmission(directory: gate.directory)
            .acceptedLease(sourceKey: "scheduled:w1"), false)
    }

    func testCrossSourceSessionStormHardStopsAcrossHoursAndRestart() {
        let gate = store()
        let t = Date(timeIntervalSince1970: 1_000_000)
        for index in 0..<2 {
            guard case let .allowed(token) = gate.reserve(
                sessionId: "s", crewId: "c", sourceKey: "source-\(index)",
                now: t, uptime: 100) else { return XCTFail("setup") }
            XCTAssertTrue(gate.finish(token: token, accepted: true))
        }
        XCTAssertFalse(gate.reserve(sessionId: "s", crewId: "c", sourceKey: "third",
                                    now: t, uptime: 100).isAllowed)
        let reopened = AutomaticWakeAdmission(directory: gate.directory)
        XCTAssertFalse(reopened.reserve(sessionId: "s", crewId: "c", sourceKey: "new-after-hours",
                                       now: t.addingTimeInterval(12 * 3600), uptime: 12 * 3600 + 100).isAllowed)
        XCTAssertTrue(reopened.reserve(sessionId: "s", crewId: "c", sourceKey: "manual",
                                      priority: .human).isAllowed)
    }

    func testCrewAndMachineHardStopsSurviveClockAndRestart() {
        let gate = store()
        let t = Date(timeIntervalSince1970: 1_000_000)
        for index in 0..<5 {
            guard case let .allowed(token) = gate.reserve(
                sessionId: "s\(index)", crewId: "crew", sourceKey: "one",
                now: t, uptime: 100) else { return XCTFail("crew setup") }
            XCTAssertTrue(gate.finish(token: token, accepted: true))
        }
        XCTAssertFalse(gate.reserve(sessionId: "s5", crewId: "crew", sourceKey: "trip",
                                    now: t, uptime: 100).isAllowed)
        let reopened = AutomaticWakeAdmission(directory: gate.directory)
        XCTAssertFalse(reopened.reserve(sessionId: "fresh", crewId: "crew", sourceKey: "later",
                                       now: t.addingTimeInterval(86400), uptime: 100_000).isAllowed)
        let machineGate = store()
        for index in 0..<12 {
            guard case let .allowed(token) = machineGate.reserve(
                sessionId: "other\(index)", crewId: "other\(index)", sourceKey: "one",
                now: t, uptime: 100) else { return XCTFail("machine setup") }
            XCTAssertTrue(machineGate.finish(token: token, accepted: true))
        }
        XCTAssertFalse(machineGate.reserve(sessionId: "last", crewId: "last", sourceKey: "trip",
                                           now: t, uptime: 100).isAllowed)
        XCTAssertFalse(AutomaticWakeAdmission(directory: machineGate.directory).reserve(
            sessionId: "never-seen", crewId: "never-seen", sourceKey: "after-reboot",
            now: t.addingTimeInterval(7 * 86400), uptime: 1).isAllowed)
    }

    func testScopedHumanRecoveryIsAuditedSingleUseAndPreservesOtherLatches() {
        let gate = store()
        let t = Date(timeIntervalSince1970: 1_000_000)
        for session in ["s1", "s2"] {
            for index in 0..<2 {
                guard case let .allowed(token) = gate.reserve(
                    sessionId: session, crewId: session,
                    sourceKey: "source-\(index)", now: t, uptime: 100)
                else { return XCTFail("setup") }
                XCTAssertTrue(gate.finish(token: token, accepted: true))
            }
            XCTAssertFalse(gate.reserve(sessionId: session, crewId: session,
                                        sourceKey: "trip", now: t, uptime: 100).isAllowed)
        }
        let request = AutomaticWakeAdmission.HumanRecoveryRequest.directUI(
            scope: .session("s1"))
        XCTAssertTrue(gate.recover(request))
        XCTAssertFalse(gate.recover(request), "one action must never be reusable")
        let reopened = AutomaticWakeAdmission(directory: gate.directory)
        XCTAssertTrue(reopened.reserve(sessionId: "s1", crewId: "s1", sourceKey: "fresh",
                                       now: t.addingTimeInterval(3600), uptime: 3700).isAllowed)
        XCTAssertFalse(reopened.reserve(sessionId: "s2", crewId: "s2", sourceKey: "still-stopped",
                                        now: t.addingTimeInterval(3600), uptime: 3700).isAllowed)
        XCTAssertEqual(reopened.recoveryHistory()?.count, 1)
    }

    func testStoppedSessionLookupKeepsExactCrewAndScopeAcrossRestart() {
        let gate = store()
        let t = Date(timeIntervalSince1970: 1_000_000)
        for (session, crew) in [("captain-old", "one"), ("worker-old", "one"),
                                ("captain-other", "two")] {
            for index in 0..<2 {
                guard case let .allowed(token) = gate.reserve(
                    sessionId: session, crewId: crew, sourceKey: "source-\(index)",
                    now: t, uptime: 100) else { return XCTFail("setup") }
                XCTAssertTrue(gate.finish(token: token, accepted: false))
            }
            XCTAssertFalse(gate.reserve(sessionId: session, crewId: crew,
                                        sourceKey: "trip", now: t, uptime: 100).isAllowed)
        }
        let reopened = AutomaticWakeAdmission(directory: gate.directory)
        XCTAssertEqual(reopened.knownCrew(sessionId: "captain-old"), "one")
        XCTAssertEqual(Set(reopened.stoppedSessions(crewId: "one") ?? []),
                       ["captain-old", "worker-old"])
        XCTAssertEqual(reopened.stoppedSessions(crewId: "two"), ["captain-other"])
        XCTAssertTrue(reopened.recover(.directUI(scope: .session("captain-old")),
                                       now: t.addingTimeInterval(61), uptime: 161))
        XCTAssertEqual(Set(reopened.stoppedSessions(crewId: "one") ?? []), ["worker-old"])
        XCTAssertEqual(reopened.stoppedSessions(crewId: "two"), ["captain-other"])
    }

    func testDeniedHumanTextSurvivesRestartAndClearsOnlyAfterBackendAcceptance() async {
        let gate = store()
        for index in 0..<12 {
            guard case let .allowed(token) = gate.reserve(
                sessionId: "seed-\(index)", crewId: "crew-\(index)",
                sourceKey: "seed-\(index)", priority: .human) else { return XCTFail("setup") }
            XCTAssertTrue(gate.finish(token: token, accepted: false))
        }
        let item = AutomaticWakeAdmission.PendingExplicitText(
            id: "human-id", sessionId: "target", crewId: "human-crew", text: "原始人工消息")
        var calls = 0
        let denied = await gate.performExplicitText(item) {
            calls += 1
            return SessionWakeSubmission.accepted
        }
        guard case .denied = denied else { return XCTFail("machine cap must deny") }
        XCTAssertEqual(calls, 0)
        let reopened = AutomaticWakeAdmission(directory: gate.directory)
        XCTAssertEqual(reopened.pendingExplicitText(crewId: "human-crew"), [item])
        XCTAssertTrue(reopened.recover(.directUI(scope: .machine),
                                       now: Date().addingTimeInterval(61),
                                       uptime: ProcessInfo.processInfo.systemUptime + 61))
        let rejected = await reopened.performExplicitText(item) {
            calls += 1
            return SessionWakeSubmission.retry
        }
        guard case .attempted(.retry, recorded: true) = rejected else {
            return XCTFail("backend retry must remain pending")
        }
        XCTAssertEqual(reopened.pendingExplicitText(crewId: "human-crew"), [item])
        let accepted = await reopened.performExplicitText(item) {
            calls += 1
            return SessionWakeSubmission.accepted
        }
        guard case .attempted(.accepted, recorded: true) = accepted else {
            return XCTFail("backend accepted receipt must settle")
        }
        XCTAssertEqual(calls, 2)
        XCTAssertEqual(reopened.pendingExplicitText(crewId: "human-crew"), [])
        let duplicate = await reopened.performExplicitText(item) {
            calls += 1
            return SessionWakeSubmission.accepted
        }
        guard case .attempted(.accepted, recorded: true) = duplicate else {
            return XCTFail("same delivery ID should read accepted receipt")
        }
        XCTAssertEqual(calls, 2)
        let changed = AutomaticWakeAdmission.PendingExplicitText(
            id: item.id, sessionId: item.sessionId, crewId: item.crewId, text: "不同原文")
        let collision = await reopened.performExplicitText(changed) {
            calls += 1
            return SessionWakeSubmission.accepted
        }
        guard case .denied = collision else {
            return XCTFail("accepted ID cannot acknowledge a different message")
        }
        XCTAssertEqual(calls, 2)
    }

    func testLegacyPersistedReturnIsScrubbedAndNeverResubmitted() async throws {
        let gate = store()
        let old = AutomaticWakeAdmission.PendingExplicitText(
            id: "old-return", sessionId: "session", crewId: "crew", text: "")
        XCTAssertTrue(gate.rememberExplicitText(old))
        var root = try XCTUnwrap(JSONSerialization.jsonObject(
            with: Data(contentsOf: gate.fileURL)) as? [String: Any])
        var pending = try XCTUnwrap(root["pendingExplicitText"] as? [String: [String: Any]])
        pending[old.id]?["rawBytes"] = [13]
        root["pendingExplicitText"] = pending
        try JSONSerialization.data(withJSONObject: root).write(to: gate.fileURL)

        let restarted = AutomaticWakeAdmission(directory: gate.directory)
        let migrated = try XCTUnwrap(restarted.pendingExplicitText(crewId: "crew")?.first)
        XCTAssertTrue(migrated.requiresManualReview)
        let scrubbed = try String(contentsOf: gate.fileURL, encoding: .utf8)
        XCTAssertFalse(scrubbed.contains("\"rawBytes\""),
                       "the first read must scrub legacy control bytes on disk")
        var sends = 0
        let result = await restarted.performExplicitText(migrated) {
            sends += 1
            return SessionWakeSubmission.accepted
        }
        guard case .denied = result else { return XCTFail("legacy Enter must not replay") }
        XCTAssertEqual(sends, 0)
        XCTAssertTrue(AutomaticWakeAdmission(directory: gate.directory)
            .pendingExplicitText(crewId: "crew")?.first?.requiresManualReview == true)
    }

    func testSuppressedWhiteboardDebtSurvivesRestartUntilAcknowledged() {
        let gate = store()
        let pending = AutomaticWakeAdmission.PendingWhiteboard(
            crewId: "c", entryId: "old-entry", targetId: "s")
        XCTAssertTrue(gate.rememberWhiteboard(pending))
        let reopened = AutomaticWakeAdmission(directory: gate.directory)
        XCTAssertEqual(reopened.pendingWhiteboard(crewId: "c"), [pending])
        XCTAssertTrue(reopened.acknowledgeWhiteboard(pending))
        XCTAssertEqual(AutomaticWakeAdmission(directory: gate.directory)
            .pendingWhiteboard(crewId: "c"), [])
    }

    func testUnreadableAdmissionCannotPretendPendingWhiteboardIsEmpty() throws {
        let gate = store()
        try FileManager.default.createDirectory(at: gate.directory, withIntermediateDirectories: true)
        try Data("broken".utf8).write(to: gate.fileURL)
        XCTAssertNil(gate.pendingWhiteboard(crewId: "c"))
        XCTAssertFalse(gate.rememberWhiteboard(.init(crewId: "c", entryId: "e", targetId: "s")))
    }

    func testRealSendOperationIsNotInvokedAfterRejectedAttemptsTripSessionCircuit() async {
        let gate = store()
        var submitCount = 0
        for source in ["todo", "timer", "supervision"] {
            let outcome = await gate.performSend(
                sessionId: "s", crewId: "c", sourceKey: source,
                isAccepted: { (result: Bool) in result },
                operation: { submitCount += 1; return false })
            if source == "supervision" {
                guard case .denied = outcome else { return XCTFail("third source must be denied") }
            }
        }
        XCTAssertEqual(submitCount, 2, "the backend submit closure must never run on denial")
    }

    func testRealLaunchOperationIsNotInvokedAfterFailedAttemptsAndRestart() async throws {
        enum FakeLaunchError: Error { case backend, denied }
        let gate = store()
        var launchCount = 0
        for source in ["first", "second"] {
            do {
                try await gate.performLaunch(
                    sessionId: "s", crewId: "c", sourceKey: source,
                    deniedError: { _ in FakeLaunchError.denied },
                    operation: { launchCount += 1; throw FakeLaunchError.backend })
                XCTFail("fake launch must reject")
            } catch FakeLaunchError.backend { }
        }
        let reopened = AutomaticWakeAdmission(directory: gate.directory)
        do {
            try await reopened.performLaunch(
                sessionId: "s", crewId: "c", sourceKey: "third",
                deniedError: { _ in FakeLaunchError.denied },
                operation: { launchCount += 1 })
            XCTFail("third launch must be denied")
        } catch FakeLaunchError.denied { }
        XCTAssertEqual(launchCount, 2, "denial must precede the actual start closure")
    }

    func testMachineLimitedHumanSendKeepsOriginalWhiteboardDebtForRecovery() async {
        let gate = store()
        let debt = AutomaticWakeAdmission.PendingWhiteboard(
            crewId: "human-crew", entryId: "human-entry", targetId: "captain")
        XCTAssertTrue(gate.rememberWhiteboard(debt))
        for index in 0..<12 {
            guard case let .allowed(token) = gate.reserve(
                sessionId: "s\(index)", crewId: "c\(index)", sourceKey: "human-\(index)",
                priority: .human) else { return XCTFail("setup") }
            XCTAssertTrue(gate.finish(token: token, accepted: false))
        }
        var submissions = 0
        let outcome = await gate.performSend(
            sessionId: "captain", crewId: "human-crew", sourceKey: "whiteboard:human-entry",
            priority: .human, isAccepted: { (result: Bool) in result },
            operation: { submissions += 1; return true })
        guard case .denied = outcome else { return XCTFail("human turn must obey machine cap") }
        XCTAssertEqual(submissions, 0)
        XCTAssertEqual(AutomaticWakeAdmission(directory: gate.directory)
            .pendingWhiteboard(crewId: "human-crew"), [debt])
        let recovery = AutomaticWakeAdmission.HumanRecoveryRequest.directUI(scope: .machine)
        XCTAssertFalse(gate.recover(recovery), "hard window has not cooled yet")
        XCTAssertTrue(gate.recover(recovery, now: Date().addingTimeInterval(61),
                                   uptime: ProcessInfo.processInfo.systemUptime + 61))
        let recovered = await gate.performSend(
            sessionId: "captain", crewId: "human-crew", sourceKey: "whiteboard:human-entry",
            priority: .human, isAccepted: { (result: Bool) in result },
            operation: { submissions += 1; return true })
        guard case .attempted(true, recorded: true) = recovered else {
            return XCTFail("explicit scoped recovery should reoffer the original human message")
        }
        XCTAssertEqual(submissions, 1)
        XCTAssertEqual(gate.pendingWhiteboard(crewId: "human-crew"), [debt],
                       "backend acceptance alone must not fake a whiteboard consumption receipt")
        XCTAssertTrue(gate.acknowledgeWhiteboard(debt))
        XCTAssertEqual(gate.pendingWhiteboard(crewId: "human-crew"), [])
    }

    func testRebootRecoveryRequiresMeasuredUptimeAndPersistsItsNewBaseline() {
        let gate = store()
        let t = Date(timeIntervalSince1970: 1_000_000)
        for index in 0..<12 {
            guard case let .allowed(token) = gate.reserve(
                sessionId: "s\(index)", crewId: "c\(index)", sourceKey: "e\(index)",
                priority: .human, now: t, uptime: 1000) else { return XCTFail("setup") }
            XCTAssertTrue(gate.finish(token: token, accepted: false))
        }
        XCTAssertFalse(gate.reserve(sessionId: "extra", crewId: "extra", sourceKey: "trip",
                                    priority: .human, now: t, uptime: 1000).isAllowed)
        let request = AutomaticWakeAdmission.HumanRecoveryRequest.directUI(scope: .machine)
        XCTAssertFalse(gate.recover(request, now: t.addingTimeInterval(86400), uptime: 1),
                       "wall-clock jump and reboot cannot clear the window immediately")
        let restarted = AutomaticWakeAdmission(directory: gate.directory)
        XCTAssertTrue(restarted.recover(request, now: t.addingTimeInterval(86461), uptime: 62),
                      "61 measured seconds on the new boot should safely cool the window")
    }
}
