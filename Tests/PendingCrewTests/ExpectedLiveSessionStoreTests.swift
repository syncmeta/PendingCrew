#if os(macOS)
import XCTest

/// P1：live 账本必须独立于 resume 账本，而且只能由真实 daemon 生命周期写入。
final class ExpectedLiveSessionStoreTests: XCTestCase {

    private func epoch() -> ExpectedLiveSessionStore.DaemonEpoch {
        .init(pid: 101, startedAt: Date(timeIntervalSince1970: 1_700_000_000))
    }

    func test_active只由写入方创建且终止时清理() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("expected-live-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = ExpectedLiveSessionStore(directory: dir)

        XCTAssertTrue((try? store.records())?.isEmpty == true)
        try store.activate(crewId: "crew", sessionId: "session", epoch: epoch())
        XCTAssertEqual(try store.records().count, 1)

        try store.clear(crewId: "crew", sessionId: "session", epoch: epoch())
        XCTAssertTrue(try store.records().isEmpty)
    }

    func test_新epoch不能清掉尚待核对的旧epoch() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("expected-live-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: dir) }
        let old = epoch()
        let newer = ExpectedLiveSessionStore.DaemonEpoch(
            pid: 102, startedAt: old.startedAt.addingTimeInterval(1))
        let store = ExpectedLiveSessionStore(directory: dir)
        try store.activate(crewId: "crew", sessionId: "session", epoch: old)

        try store.clear(crewId: "crew", sessionId: "session", epoch: newer)
        XCTAssertEqual(try store.activeRecord(crewId: "crew", sessionId: "session", epoch: old)?.epoch, old)
    }

    func test_旧epoch的同一故障只取得一次可见告警() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("expected-live-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = ExpectedLiveSessionStore(directory: dir)
        try store.activate(crewId: "crew", sessionId: "session", epoch: epoch())

        let record = try XCTUnwrap(store.nextUndeliveredNotice(
            crewId: "crew", sessionId: "session", epoch: epoch()))
        XCTAssertTrue(try store.markNoticeDelivered(record))
        XCTAssertNil(try ExpectedLiveSessionStore(directory: dir).nextUndeliveredNotice(
            crewId: "crew", sessionId: "session", epoch: epoch()))
    }

    func test_读失败必须上抛且恢复后同一告警仍可重试() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("expected-live-read-failure-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = ExpectedLiveSessionStore(directory: dir)
        try store.activate(crewId: "crew", sessionId: "session", epoch: epoch())
        let ledger = dir.appendingPathComponent("expected-live-sessions.json")
        try FileManager.default.setAttributes([.posixPermissions: 0], ofItemAtPath: ledger.path)
        defer { try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: ledger.path) }

        XCTAssertThrowsError(try store.nextUndeliveredNotice(
            crewId: "crew", sessionId: "session", epoch: epoch()))

        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: ledger.path)
        XCTAssertNotNil(try store.nextUndeliveredNotice(
            crewId: "crew", sessionId: "session", epoch: epoch()))
    }

    func test_写失败不能冒充已占用告警权且恢复后可重试() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("expected-live-write-failure-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = ExpectedLiveSessionStore(directory: dir)
        try store.activate(crewId: "crew", sessionId: "session", epoch: epoch())
        try FileManager.default.setAttributes([.posixPermissions: 0o500], ofItemAtPath: dir.path)
        defer { try? FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: dir.path) }

        let record = try XCTUnwrap(store.nextUndeliveredNotice(
            crewId: "crew", sessionId: "session", epoch: epoch()))
        XCTAssertThrowsError(try store.markNoticeDelivered(record))

        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: dir.path)
        XCTAssertNotNil(try store.nextUndeliveredNotice(
            crewId: "crew", sessionId: "session", epoch: epoch()))
    }

    /// expected-live 是整写账本；只要同一份 JSON 有一行不能完整解码，就不能把
    /// `decodeRows` 的幸存行当成完整账本写回。否则未通知的旧 epoch 会被静默抹掉。
    func test_混合有效坏行时保留原字节且恢复后仍可取得告警() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("expected-live-mixed-rows-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = ExpectedLiveSessionStore(directory: dir)
        let old = epoch()
        try store.activate(crewId: "crew", sessionId: "old", epoch: old)

        let ledger = dir.appendingPathComponent("expected-live-sessions.json")
        let validBytes = try Data(contentsOf: ledger)
        var mixedRows = try XCTUnwrap(
            JSONSerialization.jsonObject(with: validBytes) as? [Any])
        mixedRows.append(["crewId": "partial-row-without-required-fields"])
        let mixedBytes = try JSONSerialization.data(withJSONObject: mixedRows)
        try mixedBytes.write(to: ledger)

        XCTAssertThrowsError(try store.activate(
            crewId: "crew", sessionId: "new", epoch: .init(
                pid: 102, startedAt: old.startedAt.addingTimeInterval(1))))
        XCTAssertEqual(try Data(contentsOf: ledger), mixedBytes,
                       "混合行未完整解码时不得整写或修改原字节")
        XCTAssertThrowsError(try store.nextUndeliveredNotice(
            crewId: "crew", sessionId: "old", epoch: old))
        XCTAssertEqual(try Data(contentsOf: ledger), mixedBytes,
                       "拒绝提示也不得借机重写账本")

        try validBytes.write(to: ledger)
        XCTAssertNotNil(try ExpectedLiveSessionStore(directory: dir).nextUndeliveredNotice(
            crewId: "crew", sessionId: "old", epoch: old),
                        "外部恢复完整账本后，旧 epoch 必须仍可产生人工可见告警")
    }

    func test_production只把真实daemon生命周期接到live账本且不自动handoff() throws {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let runner = try String(contentsOf: root.appendingPathComponent(
            "Sources/Mac/Services/CrewSessionRunner.swift"))
        let daemon = try String(contentsOf: root.appendingPathComponent(
            "Sources/Mac/Services/SessionDaemonMain.swift"))
        let host = try String(contentsOf: root.appendingPathComponent(
            "Sources/Mac/LocalRunner/SessionDaemonHost.swift"))

        XCTAssertTrue(daemon.contains("ExpectedLiveSessionStore"),
                      "daemon 没有把自己的 epoch / store 注入编排器")
        XCTAssertTrue(runner.contains("expectedLiveStore.activate"),
                      "真实 run 入 roster 后没有写 expected-live")
        XCTAssertTrue(runner.contains("expectedLiveStore.clear"),
                      "真实 terminal callback 没有清 expected-live")
        XCTAssertTrue(host.contains("nextUndeliveredNotice"),
                      "上一 daemon 遗留记录不会产生一次 durable 人可见告警")
        XCTAssertTrue(host.contains("markNoticeDelivered"),
                      "白板成功前不应消费 expected-live 告警权")
        XCTAssertTrue(runner.contains("expectedLiveFailureReporter"),
                      "expected-live 读写失败不能被 runner 静默吞掉")
        XCTAssertTrue(host.contains("probeResult"),
                      "不能把进程读取错误冒充为已退出")
        XCTAssertFalse(runner.contains("recoverExpectedLive"),
                       "P1 禁止从 expected-live 自动 handoff")
    }
}
#endif
