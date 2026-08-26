import XCTest

@MainActor
final class LocalCrewStoreTests: XCTestCase {
    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory.appendingPathComponent("crewstore-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }
    private func req(
        title: String?, source: LocalCrewTitleSource? = nil
    ) -> CreateCrewRequest {
        .make(responsibleSubjectId: "local-byok", title: title, machineId: nil,
              workingDirectory: "/tmp/x", captainAgentKind: "codex",
              initialTitleSource: source,
              captain: .systemGenerated(templateName: nil))
    }

    func testCreateUsesProvidedTitle() {
        let s = LocalCrewStore(baseDirectory: tempDir())
        let resp = s.createCrew(req(title: "支付接入"))
        XCTAssertEqual(s.getCrew(resp.crewId)?.crew.title, "支付接入")
    }

    // 自动名（nil title）兜底成地名（测试 bundle 无 crewground_places.txt →
    // PlaceNames 回落 ["Atlantis"]）—— 关键是**不再**是「未命名 crew」。
    func testCreateNilTitleIsNotUnnamed() {
        let s = LocalCrewStore(baseDirectory: tempDir())
        let resp = s.createCrew(req(title: nil))
        let title = s.getCrew(resp.crewId)?.crew.title ?? ""
        XCTAssertFalse(title.isEmpty)
        XCTAssertNotEqual(title, "未命名 crew")
    }

    func testSetTitleUpdates() {
        let s = LocalCrewStore(baseDirectory: tempDir())
        let resp = s.createCrew(req(title: "Lisbon"))
        s.setTitle(resp.crewId, "鉴权重构", source: .captain)
        XCTAssertEqual(s.getCrew(resp.crewId)?.crew.title, "鉴权重构")
        XCTAssertEqual(s.listCrews().first(where: { $0.id == resp.crewId })?.title, "鉴权重构")
    }

    func testSetTitlePersistsAcrossInstances() {
        let dir = tempDir()
        let s = LocalCrewStore(baseDirectory: dir)
        let resp = s.createCrew(req(title: "Lisbon"))
        s.setTitle(resp.crewId, "深色模式", source: .captain)
        XCTAssertEqual(LocalCrewStore(baseDirectory: dir).getCrew(resp.crewId)?.crew.title, "深色模式")
    }

    func testSetTitleEmptyIgnored() {
        let s = LocalCrewStore(baseDirectory: tempDir())
        let resp = s.createCrew(req(title: "Lisbon"))
        s.setTitle(resp.crewId, "   ", source: .captain)
        XCTAssertEqual(s.getCrew(resp.crewId)?.crew.title, "Lisbon")
    }

    func testSetTitleUnknownCrewIgnored() {
        let s = LocalCrewStore(baseDirectory: tempDir())
        s.setTitle("local-does-not-exist", "x", source: .captain)  // 不崩即可
        XCTAssertNil(s.getCrew("local-does-not-exist"))
    }

    func testSetCaptainAgentKindPersistsAcrossInstances() {
        let dir = tempDir()
        let s = LocalCrewStore(baseDirectory: dir)
        let id = s.createCrew(req(title: "交接测试")).crewId
        s.setCaptainAgentKind(id, LocalCodingAgentKind.claudeCode.rawValue)
        XCTAssertEqual(
            LocalCrewStore(baseDirectory: dir).getCrew(id)?.crew.captainAgentKind,
            LocalCodingAgentKind.claudeCode.rawValue)
    }

    func testSetCaptainAgentKindRejectsTerminalAndUnknownValues() {
        let s = LocalCrewStore(baseDirectory: tempDir())
        let id = s.createCrew(req(title: "交接测试")).crewId
        s.setCaptainAgentKind(id, LocalCodingAgentKind.terminal.rawValue)
        s.setCaptainAgentKind(id, "not-an-agent")
        XCTAssertEqual(s.getCrew(id)?.crew.captainAgentKind, LocalCodingAgentKind.codex.rawValue)
    }

    func testCaptainReassignmentRequestReplacesPerCrewAndCompletesById() throws {
        let dir = tempDir()
        let store = LocalCaptainReassignmentStore(directory: dir)
        let first = try store.request(
            crewId: "crew-a", sourceSessionId: "s1", sourceDisplayName: "旧候选",
            agentKind: "claude_code", now: Date(timeIntervalSince1970: 1))
        let second = try store.request(
            crewId: "crew-a", sourceSessionId: "s2", sourceDisplayName: "新候选",
            agentKind: "codex", now: Date(timeIntervalSince1970: 2))
        let other = try store.request(
            crewId: "crew-b", sourceSessionId: "s3", sourceDisplayName: "别组候选",
            agentKind: "codex", now: Date(timeIntervalSince1970: 3))

        XCTAssertEqual(Set(store.pending().map(\.id)), Set([second.id, other.id]))
        store.complete(first.id)
        XCTAssertEqual(store.pending().count, 2, "旧 request id 不能误删覆盖它的新选择")
        store.complete(second.id)
        XCTAssertEqual(store.pending(), [other])
        XCTAssertEqual(LocalCaptainReassignmentStore(directory: dir).pending(), [other],
                       "交接意图必须跨 app/store 重建保留")
    }

    func testCaptainReassignmentRefusesToStopSessionWhenIntentCannotPersist() throws {
        let dir = tempDir()
        try FileManager.default.createDirectory(
            at: dir.appendingPathComponent("captain-reassignments.json"),
            withIntermediateDirectories: true)
        let store = LocalCaptainReassignmentStore(directory: dir)

        XCTAssertThrowsError(try store.request(
            crewId: "crew-a", sourceSessionId: "s1", sourceDisplayName: "候选",
            agentKind: "codex")) { error in
                XCTAssertEqual(
                    error.localizedDescription,
                    LocalCaptainReassignmentStoreError.persistenceFailed.localizedDescription)
            }
    }

    func testTitleSourceInferenceIsPureAndExact() {
        let pool: Set<String> = ["Lisbon", "Tokyo"]
        XCTAssertEqual(LocalCrewTitleSource.inferLegacy(title: "Lisbon", placeNames: pool), .placeholder)
        XCTAssertEqual(LocalCrewTitleSource.inferLegacy(title: "鉴权重构", placeNames: pool), .human)
        XCTAssertEqual(LocalCrewTitleSource.inferLegacy(title: "lisbon", placeNames: pool), .human,
                       "地名池保存的是实际标题，迁移不做模糊猜测")
    }

    func testCreatePersistsExplicitPlaceholderAndHumanSources() {
        let s = LocalCrewStore(baseDirectory: tempDir())
        let automatic = s.createCrew(req(title: "Lisbon", source: .placeholder)).crewId
        let manual = s.createCrew(req(title: "鉴权重构", source: .human)).crewId
        XCTAssertEqual(s.titleSource(of: automatic), .placeholder)
        XCTAssertEqual(s.titleSource(of: manual), .human)
    }

    func testCaptainRenameChangesSourceEvenWhenTitleIsUnchanged() {
        let s = LocalCrewStore(baseDirectory: tempDir())
        let id = s.createCrew(req(title: "Lisbon", source: .placeholder)).crewId
        s.setTitle(id, "Lisbon", source: .captain)
        XCTAssertEqual(s.titleSource(of: id), .captain)
    }

    func testHumanRenameMarksHuman() {
        let s = LocalCrewStore(baseDirectory: tempDir())
        let id = s.createCrew(req(title: nil)).crewId
        s.setTitle(id, "人类定名", source: .human)
        XCTAssertEqual(s.titleSource(of: id), .human)
    }

    func testLegacyTitleSourceBackfillsOnceAndPersists() throws {
        let dir = tempDir()
        let legacy = """
        {"version":1,"crews":[
          {"id":"place","title":"Atlantis","responsibleSubjectId":"s","runtimeLocation":"local_host","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"},
          {"id":"named","title":"鉴权重构","responsibleSubjectId":"s","runtimeLocation":"local_host","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}
        ]}
        """
        let file = dir.appendingPathComponent("local-crews.json")
        try legacy.data(using: .utf8)!.write(to: file)
        let s = LocalCrewStore(baseDirectory: dir)
        XCTAssertEqual(s.titleSource(of: "place"), .placeholder)
        XCTAssertEqual(s.titleSource(of: "named"), .human)
        let persisted = try String(contentsOf: file, encoding: .utf8)
        XCTAssertEqual(persisted.components(separatedBy: "\"titleSource\"").count - 1, 2,
                       "两条旧记录都应一次性写回来源字段")
        XCTAssertEqual(LocalCrewStore(baseDirectory: dir).titleSource(of: "place"), .placeholder)
    }

    func testLocalTitleSourceHintIsNotEncodedToEdgeRequest() throws {
        let request = req(title: "Lisbon", source: .placeholder)
        let object = try JSONSerialization.jsonObject(with: JSONEncoder().encode(request)) as! [String: Any]
        XCTAssertNil(object["initialTitleSource"])
        XCTAssertEqual(object["title"] as? String, "Lisbon")
    }

    // MARK: - Attention（机长黄点持久化）

    func testSetAttentionUpdatesSummary() {
        let s = LocalCrewStore(baseDirectory: tempDir())
        let resp = s.createCrew(req(title: "Lisbon"))
        XCTAssertNil(s.listCrews().first?.attentionReason)
        s.setAttention(resp.crewId, reason: "需要人类拍板")
        XCTAssertEqual(s.listCrews().first(where: { $0.id == resp.crewId })?.attentionReason, "需要人类拍板")
    }

    func testSetAttentionNilClears() {
        let s = LocalCrewStore(baseDirectory: tempDir())
        let resp = s.createCrew(req(title: "Lisbon"))
        s.setAttention(resp.crewId, reason: "有问题")
        s.setAttention(resp.crewId, reason: nil)
        XCTAssertNil(s.listCrews().first(where: { $0.id == resp.crewId })?.attentionReason)
    }

    func testSetAttentionBlankTreatedAsClear() {
        let s = LocalCrewStore(baseDirectory: tempDir())
        let resp = s.createCrew(req(title: "Lisbon"))
        s.setAttention(resp.crewId, reason: "有问题")
        s.setAttention(resp.crewId, reason: "   ")
        XCTAssertNil(s.listCrews().first(where: { $0.id == resp.crewId })?.attentionReason)
    }

    func testSetAttentionPersistsAcrossInstances() {
        let dir = tempDir()
        let s = LocalCrewStore(baseDirectory: dir)
        let resp = s.createCrew(req(title: "Lisbon"))
        s.setAttention(resp.crewId, reason: "app 重启不丢")
        XCTAssertEqual(
            LocalCrewStore(baseDirectory: dir).listCrews()
                .first(where: { $0.id == resp.crewId })?.attentionReason,
            "app 重启不丢")
    }

    func testSetAttentionUnknownCrewIgnored() {
        let s = LocalCrewStore(baseDirectory: tempDir())
        s.setAttention("local-does-not-exist", reason: "x")  // 不崩即可
        XCTAssertNil(s.getCrew("local-does-not-exist"))
    }

    // 旧 JSON（无 attentionReason 键）向后兼容：能解码，字段兜 nil。
    func testDecodesLegacyJSONWithoutAttentionReason() throws {
        let dir = tempDir()
        let legacy = """
        {"version":1,"crews":[{
            "id":"local-legacy",
            "title":"老 crew",
            "responsibleSubjectId":"local-byok",
            "runtimeLocation":"local_host",
            "createdAt":"2026-01-01T00:00:00Z",
            "updatedAt":"2026-01-01T00:00:00Z"
        }]}
        """
        try legacy.data(using: .utf8)!.write(to: dir.appendingPathComponent("local-crews.json"))
        let s = LocalCrewStore(baseDirectory: dir)
        let summary = s.listCrews().first(where: { $0.id == "local-legacy" })
        XCTAssertNotNil(summary)                    // 解码没炸（没走 corrupt 备份路径）
        XCTAssertEqual(summary?.title, "老 crew")
        XCTAssertNil(summary?.attentionReason)
        // 老记录也能点亮。
        s.setAttention("local-legacy", reason: "补个黄点")
        XCTAssertEqual(s.listCrews().first(where: { $0.id == "local-legacy" })?.attentionReason, "补个黄点")
    }
}
