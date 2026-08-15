import XCTest

/// 通讯录（2026-08-11）的纯逻辑单测：号码解析、发号/回填/不复用、directory 过滤、
/// contact 寻址。全部脱离 app 跑 —— 数据源就是两份共享 JSON，测试自己造。
@MainActor
final class CrewDirectoryTests: XCTestCase {

    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("crewdir-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func req(_ title: String) -> CreateCrewRequest {
        .make(responsibleSubjectId: "local-byok", title: title, machineId: nil,
              workingDirectory: "/tmp/x", captainAgentKind: "claude_code",
              initialTitleSource: .human,
              captain: .systemGenerated(templateName: nil))
    }

    /// `local-crews.json` 落在白板目录的**父**目录 —— 复刻线上布局给 CrewDirectory 读。
    private func directory(from base: URL, sessions: CrewSessionsSnapshot? = nil) -> CrewDirectory {
        let whiteboards = base.appendingPathComponent("whiteboards", isDirectory: true)
        try? FileManager.default.createDirectory(at: whiteboards, withIntermediateDirectories: true)
        if let sessions, let data = try? JSONEncoder().encode(sessions) {
            try? data.write(to: whiteboards.appendingPathComponent(CrewSessionsSnapshot.fileName))
        }
        return CrewDirectory.load(whiteboardDirectory: whiteboards)
    }

    // MARK: - 号码解析

    func testParseCrewAndExtension() {
        XCTAssertEqual(CrewPhoneNumber.parse("7"), CrewPhoneNumber(crew: 7, ext: nil))
        XCTAssertEqual(CrewPhoneNumber.parse("7-3"), CrewPhoneNumber(crew: 7, ext: 3))
        XCTAssertEqual(CrewPhoneNumber.parse(" 12-1 "), CrewPhoneNumber(crew: 12, ext: 1))
        // 中文输入法常见的全角连字符 / 破折号也收（只宽容到这里）。
        XCTAssertEqual(CrewPhoneNumber.parse("7－3"), CrewPhoneNumber(crew: 7, ext: 3))
    }

    func testParseRejectsGarbage() {
        for bad in ["", "  ", "a", "7-", "-3", "7-0", "0", "7-3-2", "7.3", "٧", "-", "7 3"] {
            XCTAssertNil(CrewPhoneNumber.parse(bad), "「\(bad)」不该被当成有效号码")
        }
    }

    func testNumberText() {
        XCTAssertEqual(CrewPhoneNumber(crew: 7, ext: nil).text, "7")
        XCTAssertEqual(CrewPhoneNumber(crew: 7, ext: 1).text, "7-1")
        XCTAssertTrue(CrewPhoneNumber(crew: 7, ext: 1).isCaptain)
        XCTAssertFalse(CrewPhoneNumber(crew: 7, ext: 2).isCaptain)
    }

    // MARK: - 发号

    func testCrewNumbersStartAtOneAndIncrement() {
        let base = tempDir()
        let store = LocalCrewStore(baseDirectory: base)
        let a = store.createCrew(req("甲")).crewId
        let b = store.createCrew(req("乙")).crewId
        XCTAssertEqual(store.crewNumber(of: a), 1)
        XCTAssertEqual(store.crewNumber(of: b), 2)
    }

    func testExtensionsStartAtTwoBecauseOneIsTheCaptain() {
        let base = tempDir()
        let store = LocalCrewStore(baseDirectory: base)
        let crew = store.createCrew(req("甲")).crewId
        store.recordSessionMember(crewId: crew, sessionId: "w1", displayName: "Worker 1")
        store.recordSessionMember(crewId: crew, sessionId: "w2", displayName: "Worker 2")
        XCTAssertEqual(store.extensionNumber(crewId: crew, sessionId: "w1"), 2)
        XCTAssertEqual(store.extensionNumber(crewId: crew, sessionId: "w2"), 3)
        // 机长不在成员表里 —— 分机恒 1。
        XCTAssertEqual(store.phoneNumber(crewId: crew, sessionId: "cap", isCaptain: true)?.text, "1-1")
        XCTAssertEqual(store.phoneNumber(crewId: crew, sessionId: "w2", isCaptain: false)?.text, "1-3")
    }

    func testRecordingSameSessionAgainKeepsItsExtension() {
        // restartMember 复用原 sessionId 重启 → 会再走一遍登记，绝不能重新发号。
        let store = LocalCrewStore(baseDirectory: tempDir())
        let crew = store.createCrew(req("甲")).crewId
        store.recordSessionMember(crewId: crew, sessionId: "w1", displayName: "Worker 1")
        store.recordSessionMember(crewId: crew, sessionId: "w1", displayName: "Worker 1 改名")
        XCTAssertEqual(store.sessionMembers(crewId: crew).count, 1)
        XCTAssertEqual(store.extensionNumber(crewId: crew, sessionId: "w1"), 2)
    }

    func testNumbersSurviveReopenAndAreNeverReused() {
        // 删掉 1 号 crew 后重开：下一个仍是 3，1 号永不回收。
        let base = tempDir()
        do {
            let store = LocalCrewStore(baseDirectory: base)
            let a = store.createCrew(req("甲")).crewId
            _ = store.createCrew(req("乙"))
            store.deleteCrew(a)
        }
        let reopened = LocalCrewStore(baseDirectory: base)
        let c = reopened.createCrew(req("丙")).crewId
        XCTAssertEqual(reopened.crewNumber(of: c), 3)
    }

    func testExtensionsNeverReusedAcrossReopen() {
        let base = tempDir()
        do {
            let store = LocalCrewStore(baseDirectory: base)
            let crew = store.createCrew(req("甲")).crewId
            store.recordSessionMember(crewId: crew, sessionId: "w1", displayName: "W1")
            store.recordSessionMember(crewId: crew, sessionId: "w2", displayName: "W2")
        }
        let reopened = LocalCrewStore(baseDirectory: base)
        let crew = reopened.listCrews().first!.id
        reopened.recordSessionMember(crewId: crew, sessionId: "w3", displayName: "W3")
        XCTAssertEqual(reopened.extensionNumber(crewId: crew, sessionId: "w3"), 4)
    }

    func testNumberIsStableAcrossReparenting() {
        // 号码终身不变：换爹不重发（层级完全不参与编号）。
        let store = LocalCrewStore(baseDirectory: tempDir())
        let parent = store.createCrew(req("父")).crewId
        let child = store.createCrew(req("子")).crewId
        let before = store.crewNumber(of: child)
        try? store.adopt(crewId: child, underParent: parent)
        store.detachParent(crewId: child, parentCrewId: parent)
        XCTAssertEqual(store.crewNumber(of: child), before)
        XCTAssertEqual(before, 2)
    }

    // MARK: - 存量回填

    func testBackfillsLegacyFileOnceAndPersists() throws {
        // 通讯录之前的旧 JSON：没有 nextCrewNumber、crew 没号、成员没分机。
        let base = tempDir()
        let legacy = """
        {"version":1,"crews":[
          {"id":"local-b","title":"乙","responsibleSubjectId":"s","runtimeLocation":"local_host",
           "createdAt":"2026-02-02T00:00:00Z","updatedAt":"2026-02-02T00:00:00Z",
           "sessionMembers":[{"sessionId":"w1","displayName":"W1","createdAt":"2026-02-03T00:00:00Z"},
                             {"sessionId":"w2","displayName":"W2","createdAt":"2026-02-04T00:00:00Z"}]},
          {"id":"local-a","title":"甲","responsibleSubjectId":"s","runtimeLocation":"local_host",
           "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}
        ]}
        """
        try legacy.write(to: base.appendingPathComponent("local-crews.json"),
                         atomically: true, encoding: .utf8)
        let store = LocalCrewStore(baseDirectory: base)
        // 旧文件照样加载（不失败、不清空）。
        XCTAssertEqual(store.listCrews().count, 2)
        // 按 createdAt 升序补号：甲(2026-01) = 1，乙(2026-02) = 2。
        XCTAssertEqual(store.crewNumber(of: "local-a"), 1)
        XCTAssertEqual(store.crewNumber(of: "local-b"), 2)
        // 成员按现有顺序补分机，机长的 1 不占用。
        XCTAssertEqual(store.extensionNumber(crewId: "local-b", sessionId: "w1"), 2)
        XCTAssertEqual(store.extensionNumber(crewId: "local-b", sessionId: "w2"), 3)
        // 回填已落盘 —— 重开不再变号，新 crew 从 3 起。
        let reopened = LocalCrewStore(baseDirectory: base)
        XCTAssertEqual(reopened.crewNumber(of: "local-a"), 1)
        let fresh = reopened.createCrew(req("丙")).crewId
        XCTAssertEqual(reopened.crewNumber(of: fresh), 3)
    }

    // MARK: - directory

    func testDirectoryListsCrewsWithOrgPathAndMembers() {
        let base = tempDir()
        let store = LocalCrewStore(baseDirectory: base)
        let parent = store.createCrew(req("PendingCrew")).crewId
        let child = store.createCrew(req("应用自动更新")).crewId
        try? store.adopt(crewId: child, underParent: parent)
        store.recordSessionMember(crewId: child, sessionId: "w1", displayName: "Sparkle 接入")

        var snap = CrewSessionsSnapshot()
        snap.crews[child] = [
            .init(sessionId: "cap-child", name: "机长", role: "captain", brief: "", state: "idle"),
            .init(sessionId: "w1", name: "Sparkle 接入", role: "worker",
                  brief: "接 Sparkle 更新框架", state: "working"),
        ]
        let dir = directory(from: base, sessions: snap)
        let rows = dir.entries()
        XCTAssertEqual(rows.map(\.number.text), ["1", "1-1", "2", "2-1", "2-2"])
        let childCrewRow = rows.first { $0.number.text == "2" }
        XCTAssertEqual(childCrewRow?.name, "应用自动更新")
        XCTAssertEqual(childCrewRow?.kind, .crew)
        XCTAssertEqual(childCrewRow?.orgPath, "PendingCrew")
        let worker = rows.first { $0.number.text == "2-2" }
        XCTAssertEqual(worker?.orgPath, "PendingCrew / 应用自动更新")
        XCTAssertEqual(worker?.activity, "接 Sparkle 更新框架")
        XCTAssertEqual(worker?.status, "🟢 干活中")
        // 机长行即使快照里没有它也在（号码恒存在）；这里快照有 → 用实时状态。
        XCTAssertEqual(rows.first { $0.number.text == "2-1" }?.status, "🟡 空闲")
        XCTAssertEqual(rows.first { $0.number.text == "1-1" }?.status, "不在线")
    }

    func testDirectoryQueryFiltersByNumberPrefixNameAndKeyword() {
        let base = tempDir()
        let store = LocalCrewStore(baseDirectory: base)
        _ = store.createCrew(req("PendingCrew"))
        let b = store.createCrew(req("应用自动更新")).crewId
        store.recordSessionMember(crewId: b, sessionId: "w1", displayName: "Sparkle 接入")
        let dir = directory(from: base)

        // 号码前缀：2 命中 2 / 2-1 / 2-2，不带出 1 号。
        XCTAssertEqual(dir.entries(query: "2").map(\.number.text), ["2", "2-1", "2-2"])
        // 名字：命中 session 时它所在 crew 的表头一并带上。
        XCTAssertEqual(dir.entries(query: "sparkle").map(\.number.text), ["2", "2-2"])
        // 关键词命中 crew 名 → 整组。
        XCTAssertEqual(dir.entries(query: "自动更新").map(\.number.text), ["2", "2-1", "2-2"])
        // 查不到 → 空（渲染层会说「没有匹配」，不假装有数据）。
        XCTAssertTrue(dir.entries(query: "查无此物").isEmpty)
        XCTAssertTrue(dir.render(query: "查无此物").contains("没有匹配"))
    }

    // MARK: - contact 寻址

    func testResolveBroadcastCaptainAndSession() {
        let base = tempDir()
        let store = LocalCrewStore(baseDirectory: base)
        _ = store.createCrew(req("PendingCrew"))
        let b = store.createCrew(req("应用自动更新")).crewId
        store.recordSessionMember(crewId: b, sessionId: "worker-abc", displayName: "Sparkle 接入")
        let dir = directory(from: base)

        XCTAssertEqual(dir.resolve("2"), .broadcast(crewId: b, crewTitle: "应用自动更新"))
        XCTAssertEqual(dir.resolve("2-1"),
                       .captain(crewId: b, crewTitle: "应用自动更新", name: "机长"))
        XCTAssertEqual(dir.resolve("2-2"),
                       .session(crewId: b, crewTitle: "应用自动更新",
                                sessionId: "worker-abc", name: "Sparkle 接入"))
    }

    func testResolveUnknownNumbersReturnNil() {
        let base = tempDir()
        let store = LocalCrewStore(baseDirectory: base)
        _ = store.createCrew(req("甲"))
        let dir = directory(from: base)
        XCTAssertNil(dir.resolve("9"))       // 没有 9 号 crew
        XCTAssertNil(dir.resolve("1-2"))     // 该 crew 还没发过 2 号分机
        XCTAssertNil(dir.resolve("abc"))     // 压根不是号码
        XCTAssertNil(dir.resolve("1-0"))
    }

    func testSelfPhoneNumber() {
        let base = tempDir()
        let store = LocalCrewStore(baseDirectory: base)
        let a = store.createCrew(req("甲")).crewId
        store.recordSessionMember(crewId: a, sessionId: "w1", displayName: "W1")
        let dir = directory(from: base)
        XCTAssertEqual(dir.phoneNumber(crewId: a, sessionId: "cap", isCaptain: true)?.text, "1-1")
        XCTAssertEqual(dir.phoneNumber(crewId: a, sessionId: "w1", isCaptain: false)?.text, "1-2")
        XCTAssertNil(dir.phoneNumber(crewId: a, sessionId: "不认识的", isCaptain: false))
    }

    func testEmptyDirectoryRendersHonestly() {
        XCTAssertTrue(CrewDirectory(crews: []).render().contains("通讯录是空的"))
    }
}
