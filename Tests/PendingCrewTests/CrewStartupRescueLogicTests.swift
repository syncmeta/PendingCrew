#if os(macOS)
import XCTest

/// **B3：重启把「留待重投」变成「作废」** —— 启动时的积压对账。
///
/// 实测（47 个白板）：三次有记录的重启，各有 **14 / 12 / 13** 个 crew 的白板
/// 最后一条是没人回过的定向 @。⚠️ **那是暴露面，不是受害者名单** —— 板死可能有
/// 别的原因。只有 **crew 33** 是硬的（人类的 Todo 答复写在重启前 **38 秒**、
/// 目标在本地成员登记里、此后白板 33 小时全空），**crew 45** 次硬（两条
/// 「消息留待重投」被随后的重启抹掉）。
final class CrewStartupRescueLogicTests: XCTestCase {

    @MainActor
    func testRunnerScheduledAndSupervisionSleepChecksNeverReadOrConsumeLedger() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("runner-sleep-\(UUID().uuidString)")
        let store = LocalWakeupStore(directory: directory)
        let instant = Date(timeIntervalSince1970: 1_788_000_000)
        let scheduled = LocalWakeupStore.PendingWakeup(
            id: "scheduled", crewId: "sleep-test", sessionId: "absent",
            fireAt: ISO8601DateFormatter().string(from: instant), note: "sleep check")
        let supervised = LocalWakeupStore.PendingWakeup(
            id: "supervised", crewId: "sleep-test", sessionId: "absent",
            fireAt: scheduled.fireAt, note: "supervision", planNumber: 1)
        XCTAssertTrue(store.register(scheduled))
        XCTAssertTrue(store.register(supervised))
        let before = try Data(contentsOf: directory.appendingPathComponent("wakeups.json"))
        var reads = 0
        var notices = 0
        let gate = AutomaticWakeAdmission(directory: directory, onDiagnostic: { _ in },
                                          readData: { _ in
            reads += 1
            throw NSError(domain: NSPOSIXErrorDomain, code: Int(EMFILE))
        })
        let admissionBefore = try JSONEncoder().encode(AutomaticWakeAdmission.State())
        try admissionBefore.write(to: gate.fileURL)
        let runner = CrewSessionRunner(
            sessionPublisher: InProcessSessionProtocolBridge(wakeAdmission: gate),
            wakeAdmission: gate, wakeupStore: store, observePowerNotifications: false,
            wakeAdmissionReadFailureReporter: { _ in notices += 1 })
        runner.receiveSystemSleepTransition(.willSleep, at: instant)
        runner.fire(scheduled, at: instant)
        runner.fire(supervised, at: instant)
        XCTAssertEqual(reads, 0)
        XCTAssertEqual(notices, 0)
        runner.receiveSystemSleepTransition(.didWake, at: instant)
        runner.fire(scheduled, at: instant.addingTimeInterval(119))
        runner.fire(supervised, at: instant.addingTimeInterval(119))
        XCTAssertEqual(reads, 0)
        XCTAssertEqual(notices, 0)
        XCTAssertEqual(try Data(contentsOf: directory.appendingPathComponent("wakeups.json")), before)
        runner.fire(scheduled, at: instant.addingTimeInterval(120))
        runner.fire(supervised, at: instant.addingTimeInterval(120))
        XCTAssertEqual(reads, 2, "after grace, a persistent fault must be checked again")
        XCTAssertEqual(notices, 2, "the awake fault must remain visible")
        XCTAssertEqual(try Data(contentsOf: directory.appendingPathComponent("wakeups.json")), before)
        XCTAssertEqual(try Data(contentsOf: gate.fileURL), admissionBefore,
                       "neither sleep nor unreadable admission can fabricate a receipt")
    }

    @MainActor
    func testRunnerStartRejectsAbsentCaptainAndMemberWithoutConstructingBackend() async throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("runner-instance-denial-\(UUID().uuidString)")
        let gate = AutomaticWakeAdmission(directory: directory)
        for index in 0..<12 {
            guard case let .allowed(token) = gate.reserve(
                sessionId: "seed-\(index)", crewId: "crew-\(index)",
                sourceKey: "seed-\(index)") else { return XCTFail("setup") }
            XCTAssertTrue(gate.finish(token: token, accepted: false))
        }
        var constructed = 0
        var notices = 0
        for role in [CrewSessionRun.Role.captain, .worker] {
            let reopened = AutomaticWakeAdmission(directory: directory)
            let runner = CrewSessionRunner(
                sessionPublisher: InProcessSessionProtocolBridge(wakeAdmission: reopened),
                wakeAdmission: reopened,
                launchOperationOverride: { constructed += 1 },
                launchDenialNoticeOverride: { _ in notices += 1 })
            do {
                try await runner.start(
                    crewId: "launch-crew",
                    sessionId: role == .captain ? "captain-1" : "member-1",
                    config: SessionConfig(kind: .codex), workingDirectory: directory,
                    taskBrief: "headless denial", role: role)
                XCTFail("machine stop must reject the Runner.start entry")
            } catch { }
            XCTAssertTrue(runner.runs.isEmpty)
        }
        XCTAssertEqual(constructed, 0, "neither start may enter the backend body")
        XCTAssertEqual(notices, 2, "both denials must be surfaced")
    }

    func testProductionLaunchGateRejectsAbsentCaptainAndMemberBeforeProcessAcrossRestart()
        async throws {
        enum LaunchError: Error { case denied }
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("runner-launch-denial-\(UUID().uuidString)")
        let gate = AutomaticWakeAdmission(directory: directory)
        for index in 0..<12 {
            guard case let .allowed(token) = gate.reserve(
                sessionId: "seed-\(index)", crewId: "crew-\(index)",
                sourceKey: "seed-\(index)") else { return XCTFail("setup") }
            XCTAssertTrue(gate.finish(token: token, accepted: false))
        }
        var constructed = 0
        for isCaptain in [true, false] {
            let reopened = AutomaticWakeAdmission(directory: directory)
            do {
                _ = try await CrewSessionLaunchAdmission.perform(
                    admission: reopened, sessionId: isCaptain ? "captain-1" : "member-1",
                    crewId: "launch-crew", isCaptain: isCaptain, isAgent: true,
                    userInitiated: false, requestedPriority: .automatic,
                    deniedError: { _ in LaunchError.denied },
                    operation: { constructed += 1 })
                XCTFail("machine stop must reject before backend construction")
            } catch LaunchError.denied { }
        }
        XCTAssertEqual(constructed, 0,
                       "neither absent-target launch may enter its process body")
    }

    func testProductionLaunchGatePreservesExplicitHumanPriorityAndRealAcceptance()
        async throws {
        enum LaunchError: Error { case failed, denied }
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("runner-launch-priority-\(UUID().uuidString)")
        let gate = AutomaticWakeAdmission(directory: directory)
        var constructed = 0
        for _ in 0..<2 {
            do {
                _ = try await CrewSessionLaunchAdmission.perform(
                    admission: gate, sessionId: "member-1", crewId: "launch-crew",
                    isCaptain: false, isAgent: true, userInitiated: false,
                    requestedPriority: .automatic,
                    deniedError: { _ in LaunchError.denied },
                    operation: { constructed += 1; throw LaunchError.failed })
                XCTFail("fake backend must fail")
            } catch LaunchError.failed { }
        }
        do {
            _ = try await CrewSessionLaunchAdmission.perform(
                admission: gate, sessionId: "member-1", crewId: "launch-crew",
                isCaptain: false, isAgent: true, userInitiated: false,
                requestedPriority: .automatic,
                deniedError: { _ in LaunchError.denied },
                operation: { constructed += 1 })
            XCTFail("third automatic launch must be refused")
        } catch LaunchError.denied { }
        XCTAssertEqual(constructed, 2)
        let recorded = try await CrewSessionLaunchAdmission.perform(
            admission: gate, sessionId: "member-1", crewId: "launch-crew",
            isCaptain: false, isAgent: true, userInitiated: true,
            requestedPriority: .automatic,
            deniedError: { _ in LaunchError.denied },
            operation: { constructed += 1 })
        XCTAssertTrue(recorded)
        XCTAssertEqual(constructed, 3)
        let state = try JSONDecoder().decode(AutomaticWakeAdmission.State.self,
                                              from: Data(contentsOf: gate.fileURL))
        XCTAssertEqual(state.events.filter { $0.sessionId == "member-1" }
            .map(\.sourceKey), ["launch:member:member-1"])
        XCTAssertEqual(state.attempts?.filter { $0.sessionId == "member-1" }.count, 3,
                       "two failed starts and one accepted human start each use one budget slot")
    }

    private let now = Date(timeIntervalSince1970: 1_788_000_000)
    private func iso(_ d: Date) -> String { ISO8601DateFormatter().string(from: d) }

    private func msg(_ id: String, mentions: [CrewMention],
                     ageSeconds: TimeInterval = 60,
                     sender: String? = "someone-else") -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: id, senderKind: "session", senderUserId: nil, senderSessionId: sender,
            category: nil, text: "去做 X", createdAt: iso(now.addingTimeInterval(-ageSeconds)),
            senderName: "别人",
            mentions: mentions.map { LocalWhiteboardMention(kind: $0.kind, targetId: $0.targetId) })
    }

    // MARK: - 正身：两个现场的形状

    /// crew 33 的形状：@ 写在重启前几十秒，重启后扫描游标钉到尾巴 —— 它落在游标
    /// 后面，再也没人扫得到。启动对账必须把它捞回来。
    func test_重启窗口内写进来的定向at_启动时必须被捞回来() {
        let out = CrewStartupRescueLogic.pending(
            unreadBySession: ["worker-a": [msg("e1", mentions: [.session("worker-a")])]],
            captainSessionId: nil, now: now)
        XCTAssertEqual(out, [.init(sessionId: "worker-a", entryId: "e1")])
    }

    /// crew 45 的形状：**重启前已经判失败、正「留待重投」的那一条**。
    /// 它之所以还在未读里，正是因为 `confirmWake` 判失败时故意没推进游标 ——
    /// 「还欠这个人一条」本来就写在盘上，只是从来没人在启动时去读它。
    ///
    /// **这一支才是把 A1 吃掉的那个**：不捞它，「留待重投」这句承诺跨重启就是空的。
    func test_重启前已判失败正留待重投的那条_也必须被捞回来() {
        let out = CrewStartupRescueLogic.pending(
            unreadBySession: ["cap-1": [msg("e-retry", mentions: [.captain])]],
            captainSessionId: "cap-1", now: now)
        XCTAssertEqual(out, [.init(sessionId: "cap-1", entryId: "e-retry")])
    }

    func testAdmissionRegistrationFailureRetainsScanCursorForSameProcessAndRestart() throws {
        let anchor = msg("before", mentions: [.broadcast], ageSeconds: 120)
        let owed = msg("wake-id", mentions: [.session("worker-a")])
        let initial = WhiteboardCursorPosition(id: anchor.id, createdAt: anchor.createdAt)
        var progress = CrewWakeScanProgress(cursor: initial)
        var attempted: [String] = []
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("scan-debt-failure-\(UUID().uuidString)")
        let gate = AutomaticWakeAdmission(directory: directory)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try Data("unreadable".utf8).write(to: gate.fileURL)
        let debt = AutomaticWakeAdmission.PendingWhiteboard(
            crewId: "crew-scan", entryId: owed.id, targetId: "worker-a")
        XCTAssertFalse(progress.process(rows: [anchor, owed], now: now) { delivery in
            attempted.append(delivery.entryId)
            return gate.rememberWhiteboard(debt)
        })
        XCTAssertEqual(progress.cursor, initial, "failed registration must not consume scan cursor")
        XCTAssertEqual(attempted, [owed.id])

        // Restart before repair: the durable delivery cursor still says unread.
        let restartedOwed = CrewStartupRescueLogic.pending(
            unreadBySession: ["worker-a": [owed]], captainSessionId: nil, now: now)
        XCTAssertEqual(restartedOwed, [.init(sessionId: "worker-a", entryId: owed.id)])

        // Same process, same whiteboard fingerprint: the caller keeps its old
        // file gate too, so this original ID is presented again without new mail.
        try JSONEncoder().encode(AutomaticWakeAdmission.State()).write(to: gate.fileURL)
        XCTAssertTrue(progress.process(rows: [anchor, owed], now: now) { delivery in
            attempted.append(delivery.entryId)
            return gate.rememberWhiteboard(debt)
        })
        XCTAssertEqual(attempted, [owed.id, owed.id])
        XCTAssertEqual(progress.cursor?.id, owed.id)
        XCTAssertEqual(AutomaticWakeAdmission(directory: directory)
            .pendingWhiteboard(crewId: "crew-scan"), [debt])
    }

    func testUnreadableWhiteboardSentinelNeverBecomesScanCursor() {
        let anchor = msg("before", mentions: [.broadcast], ageSeconds: 120)
        let original = WhiteboardCursorPosition(id: anchor.id, createdAt: anchor.createdAt)
        var progress = CrewWakeScanProgress(cursor: original)
        let warning = msg(LocalWhiteboardStore.readFailureRowId, mentions: [])
        XCTAssertFalse(progress.process(rows: [warning], now: now) { _ in
            XCTFail("synthetic read failure is not a real message")
            return true
        })
        XCTAssertEqual(progress.cursor, original)
    }

    // MARK: - 反面：不许变成一条「只要有 @ 就捞」的规则

    /// **2026-08-12 全机重放的那道独立闸必须仍然管用。** 启动时把几周前的 @ 全捞
    /// 起来，就是再演一次那场事故 —— 那次的代价是两位数的无效轮次。
    ///
    /// **代价要说清**：超过 `maxWakeAge` 的积压捞不回来。crew 45 那两条已经 56 小时，
    /// **这次修复救不了它们。它防的是往后，不是往回。**
    func test_陈旧的at不捞() {
        let stale = CrewLocalMentionWakeLogic.maxWakeAge + 3600
        let out = CrewStartupRescueLogic.pending(
            unreadBySession: ["worker-a": [
                msg("old", mentions: [.session("worker-a")], ageSeconds: stale)]],
            captainSessionId: nil, now: now)
        XCTAssertTrue(out.isEmpty, "启动对账把几周前的 @ 捞起来 = 再演一次 2026-08-12")
    }

    func test_点名别人的不捞() {
        let out = CrewStartupRescueLogic.pending(
            unreadBySession: ["worker-a": [msg("e1", mentions: [.session("worker-b")])]],
            captainSessionId: nil, now: now)
        XCTAssertTrue(out.isEmpty, "@ 别人的被当成自己的活 —— 那是 #543 那场扩散")
    }

    func test_广播不唤醒具体run() {
        let out = CrewStartupRescueLogic.pending(
            unreadBySession: ["worker-a": [msg("e1", mentions: [.broadcast])]],
            captainSessionId: nil, now: now)
        XCTAssertTrue(out.isEmpty, "与活体唤醒路同语义：broadcast 看得见，但不叫醒谁")
    }

    func test_at机长的只算给机长() {
        let unread = [msg("e1", mentions: [.captain])]
        let out = CrewStartupRescueLogic.pending(
            unreadBySession: ["cap-1": unread, "worker-a": unread],
            captainSessionId: "cap-1", now: now)
        XCTAssertEqual(out, [.init(sessionId: "cap-1", entryId: "e1")])
    }

    /// 没有未读 = 没有欠账。启动对账不许凭空造出唤醒。
    func test_没有未读就没有欠账() {
        XCTAssertTrue(CrewStartupRescueLogic.pending(
            unreadBySession: ["worker-a": []], captainSessionId: nil, now: now).isEmpty)
    }

    /// 自己 @ 自己不算（与活体路同语义）。
    func test_自己at自己不算欠账() {
        let out = CrewStartupRescueLogic.pending(
            unreadBySession: ["worker-a": [
                msg("e1", mentions: [.session("worker-a")], sender: "worker-a")]],
            captainSessionId: nil, now: now)
        XCTAssertTrue(out.isEmpty)
    }
}
#endif
