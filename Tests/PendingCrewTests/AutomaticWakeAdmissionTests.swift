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
}
