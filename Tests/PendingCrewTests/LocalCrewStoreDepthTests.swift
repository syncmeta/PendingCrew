import XCTest

@MainActor
final class LocalCrewStoreDepthTests: XCTestCase {
    /// 用注入目录起干净 store，直接建链：a ← b ← c(b 的父是 a…)。
    private func freshStore() -> LocalCrewStore {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("crewdepth-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return LocalCrewStore(baseDirectory: dir)
    }

    private func req(title: String) -> CreateCrewRequest {
        .make(responsibleSubjectId: "local-byok", title: title, machineId: nil,
              workingDirectory: "/tmp/x", captainAgentKind: "codex",
              captain: .systemGenerated(templateName: nil))
    }

    func testRootDepthZero() {
        let s = freshStore()
        let resp = s.createCrew(req(title: "root"))
        XCTAssertEqual(s.depth(of: resp.crewId), 0)
    }

    func testChainDepthCounts() throws {
        let s = freshStore()
        let a = s.createCrew(req(title: "a")).crewId
        let b = s.createCrew(req(title: "b")).crewId
        let c = s.createCrew(req(title: "c")).crewId
        try s.attachParent(crewId: b, parentCrewId: a)
        try s.attachParent(crewId: c, parentCrewId: b)
        XCTAssertEqual(s.depth(of: a), 0)
        XCTAssertEqual(s.depth(of: b), 1)
        XCTAssertEqual(s.depth(of: c), 2)
    }

    func testUnknownCrewDepthZero() {
        XCTAssertEqual(freshStore().depth(of: "local-nope"), 0)
    }

    /// `attachParent` 是禁环的（见 LocalCrewStore.attachParent 文档），所以没法
    /// 经公开 API 挂出一个环。这里改从持久化文件这条真实缝隙注入脏数据：
    /// 直接写一份 a/b 互为父边的 `local-crews.json` 到 baseDirectory,再用公开
    /// 的 `LocalCrewStore(baseDirectory:)` 加载它 —— 复现"脏数据成环"这个
    /// `depth(of:)` 的 visited 守卫本来要防的场景。
    /// 断言只关心「会返回」（不挂/不崩），深度数值本身在环上没有语义,不强行
    /// 断言具体值。
    func testCyclicDataTerminates() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("crewdepth-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)

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
              "parentCrewIds": ["local-b"]
            },
            {
              "id": "local-b",
              "title": "b",
              "responsibleSubjectId": "local-byok",
              "runtimeLocation": "local_host",
              "createdAt": "\(now)",
              "updatedAt": "\(now)",
              "parentCrewIds": ["local-a"]
            }
          ]
        }
        """
        try json.write(to: dir.appendingPathComponent("local-crews.json"), atomically: true, encoding: .utf8)

        let s = LocalCrewStore(baseDirectory: dir)
        // 主张：在 a↔b 互为父边的脏数据上,depth(of:) 会返回而不是无限递归/挂死。
        // 环上深度本身没有稳定语义,只做非负性弱断言。
        XCTAssertGreaterThanOrEqual(s.depth(of: "local-a"), 0)
        XCTAssertGreaterThanOrEqual(s.depth(of: "local-b"), 0)
    }
}
