import XCTest

/// 「手动藏起来一个 crew」的**存储层**单测。
///
/// 钉三件事：
/// 1. **旧 JSON（没有 `manuallyHiddenAt` 键）照常解得开** —— 这台机器上有几十个
///    存量 crew，解不开就是开机全丢。
/// 2. setter 的幂等与时间戳语义：重复藏一个已经藏着的**不刷新时间戳**（那个时刻
///    是未读的参照点，刷新它等于把已有的未读抹掉）。
/// 3. 字段一路透到 `CrewSummary`（侧栏消费的是 summary，不是 `LocalCrew`）。
@MainActor
final class CrewManuallyHiddenStorageTests: XCTestCase {
    private func freshDir() -> URL {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("crewhide-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    private func req(title: String) -> CreateCrewRequest {
        .make(responsibleSubjectId: "local-byok", title: title, machineId: nil,
              workingDirectory: "/tmp/x", captainAgentKind: "codex",
              captain: .systemGenerated(templateName: nil))
    }

    // MARK: - 旧 JSON 向后兼容

    /// 存量 `local-crews.json` 里没有这个键 —— 必须照常解得开、字段兜 nil。
    func testLegacyJSONWithoutFieldStillDecodes() throws {
        let dir = freshDir()
        let now = ISO8601DateFormatter().string(from: Date())
        let json = """
        {
          "version": 1,
          "crews": [
            {
              "id": "local-a",
              "title": "a",
              "responsibleSubjectId": "local-byok",
              "runtimeLocation": "local_host",
              "createdAt": "\(now)",
              "updatedAt": "\(now)",
              "parentCrewIds": []
            }
          ]
        }
        """
        try json.write(to: dir.appendingPathComponent("local-crews.json"),
                       atomically: true, encoding: .utf8)

        let store = LocalCrewStore(baseDirectory: dir)
        let summaries = store.listCrews()
        XCTAssertEqual(summaries.count, 1)
        XCTAssertNil(summaries.first?.manuallyHiddenAt, "旧 JSON 缺键必须兜 nil，不是解码失败")
    }

    /// edge 的 `GET /v1/crews` 不下发这个字段（纯本地概念）—— `CrewSummary` 缺键兜 nil。
    func testCrewSummaryDecodesWithoutField() throws {
        let json = """
        {
          "id": "c1", "title": "t", "responsibleSubjectId": "s",
          "runtimeLocation": "local_host", "createdAt": "2026-08-26T00:00:00Z",
          "updatedAt": "2026-08-26T00:00:00Z"
        }
        """.data(using: .utf8)!
        let summary = try JSONDecoder().decode(CrewSummary.self, from: json)
        XCTAssertNil(summary.manuallyHiddenAt)
    }

    // MARK: - setter 语义

    func testHideStampsTimestampAndUnhideClearsIt() {
        let store = LocalCrewStore(baseDirectory: freshDir())
        let id = store.createCrew(req(title: "a")).crewId
        XCTAssertNil(store.listCrews().first(where: { $0.id == id })?.manuallyHiddenAt)

        store.setManuallyHidden(id, hidden: true)
        let stamped = store.listCrews().first(where: { $0.id == id })?.manuallyHiddenAt
        XCTAssertNotNil(stamped)
        XCTAssertNotNil(CrewTimestamp.parse(stamped ?? ""), "存的必须是解得开的 ISO8601")

        store.setManuallyHidden(id, hidden: false)
        XCTAssertNil(store.listCrews().first(where: { $0.id == id })?.manuallyHiddenAt)
    }

    /// 重复藏一个已经藏着的 crew **不刷新时间戳** —— 它是未读的参照点之一，
    /// 刷新它会把已经攒下的未读静默抹掉。
    func testHidingAnAlreadyHiddenCrewKeepsTheOriginalTimestamp() {
        let store = LocalCrewStore(baseDirectory: freshDir())
        let id = store.createCrew(req(title: "a")).crewId
        store.setManuallyHidden(id, hidden: true)
        let first = store.listCrews().first(where: { $0.id == id })?.manuallyHiddenAt
        store.setManuallyHidden(id, hidden: true)
        XCTAssertEqual(store.listCrews().first(where: { $0.id == id })?.manuallyHiddenAt, first)
    }

    /// crew 不存在 → 忽略（不崩、不写盘）。
    func testUnknownCrewIsIgnored() {
        let store = LocalCrewStore(baseDirectory: freshDir())
        store.setManuallyHidden("local-nope", hidden: true)
        XCTAssertTrue(store.listCrews().isEmpty)
    }

    /// 落盘后重开还在（`persistToDisk` 真把新键写出去了）。
    func testHiddenStateSurvivesReload() {
        let dir = freshDir()
        let store = LocalCrewStore(baseDirectory: dir)
        let id = store.createCrew(req(title: "a")).crewId
        store.setManuallyHidden(id, hidden: true)

        let reopened = LocalCrewStore(baseDirectory: dir)
        XCTAssertNotNil(reopened.listCrews().first(where: { $0.id == id })?.manuallyHiddenAt)
    }

    // MARK: - crew 级 lastViewed（UserDefaults，不进 crew 的磁盘 JSON）

    func testViewedStoreRoundTrips() {
        let suite = "CrewViewedStoreTests-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }

        let store = CrewViewedStore(defaults: defaults)
        XCTAssertNil(store.lastViewedAt("c1"), "从没看过 → nil")

        let at = Date(timeIntervalSince1970: 1_800_000_000)
        store.markViewed("c1", at: at)
        XCTAssertEqual(store.lastViewedAt("c1")?.timeIntervalSince1970, at.timeIntervalSince1970)

        // 重开（同一个 suite）仍在 —— 阅读状态重启不丢。
        let reopened = CrewViewedStore(defaults: defaults)
        XCTAssertEqual(reopened.lastViewedAt("c1")?.timeIntervalSince1970, at.timeIntervalSince1970)
        XCTAssertNil(reopened.lastViewedAt("c2"))
    }
}
