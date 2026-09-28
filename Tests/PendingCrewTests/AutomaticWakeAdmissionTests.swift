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

    func testUnreadableLedgerFailsClosedButManualPasses() throws {
        let gate = store()
        try FileManager.default.createDirectory(at: gate.directory, withIntermediateDirectories: true)
        try Data("broken".utf8).write(to: gate.fileURL)
        XCTAssertFalse(gate.reserve(sessionId: "s", crewId: "c", sourceKey: "auto").isAllowed)
        XCTAssertTrue(gate.reserve(sessionId: "s", crewId: "c", sourceKey: "human",
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
        XCTAssertTrue(gate.reserve(sessionId: "new", crewId: "new", sourceKey: "human",
                                   priority: .human, now: t, uptime: 100).isAllowed)
        XCTAssertTrue(gate.reserve(sessionId: "new", crewId: "new", sourceKey: "urgent",
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

    func testExplicitHumanStartCanResetLatchWithoutErasingAcceptedLease() {
        let gate = store()
        let t = Date(timeIntervalSince1970: 1_000_000)
        for index in 0..<2 {
            guard case let .allowed(token) = gate.reserve(
                sessionId: "s", crewId: "c", sourceKey: "source-\(index)",
                now: t, uptime: 100) else { return XCTFail("setup") }
            XCTAssertTrue(gate.finish(token: token, accepted: true))
        }
        XCTAssertFalse(gate.reserve(sessionId: "s", crewId: "c", sourceKey: "trip",
                                    now: t, uptime: 100).isAllowed)
        XCTAssertTrue(gate.resetHardStopsAfterHumanStart())
        let reopened = AutomaticWakeAdmission(directory: gate.directory)
        XCTAssertTrue(reopened.reserve(sessionId: "s", crewId: "c", sourceKey: "fresh",
                                       now: t.addingTimeInterval(3600), uptime: 3700).isAllowed)
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
}
