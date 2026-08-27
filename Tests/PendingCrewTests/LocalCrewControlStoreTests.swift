import XCTest

final class LocalCrewControlStoreTests: XCTestCase {
    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory.appendingPathComponent("crewctl-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    func testRequestThenPeek() {
        let s = LocalCrewControlStore(directory: tempDir())
        s.requestRename(crewId: "local-abc", name: "鉴权重构")
        XCTAssertEqual(s.pendingRename(crewId: "local-abc"), "鉴权重构")
    }

    func testRequestTrimsAndRejectsEmpty() {
        let s = LocalCrewControlStore(directory: tempDir())
        s.requestRename(crewId: "c", name: "  深色模式  ")
        XCTAssertEqual(s.pendingRename(crewId: "c"), "深色模式")
        s.requestRename(crewId: "blank", name: "   \n  ")
        XCTAssertNil(s.pendingRename(crewId: "blank"))
    }

    func testDrainReturnsAndDeletes() {
        let s = LocalCrewControlStore(directory: tempDir())
        s.requestRename(crewId: "local-abc", name: "语音重连")
        let drained = s.drainRenames()
        XCTAssertEqual(drained.count, 1)
        XCTAssertEqual(drained[0].crewId, "local-abc")
        XCTAssertEqual(drained[0].title, "语音重连")
        // 排空后文件已删 —— 再 drain / peek 为空。
        XCTAssertNil(s.pendingRename(crewId: "local-abc"))
        XCTAssertTrue(s.drainRenames().isEmpty)
    }

    func testLastWriteWins() {
        let s = LocalCrewControlStore(directory: tempDir())
        s.requestRename(crewId: "c", name: "旧名")
        s.requestRename(crewId: "c", name: "新名")
        let drained = s.drainRenames()
        XCTAssertEqual(drained.count, 1)
        XCTAssertEqual(drained[0].title, "新名")
    }

    func testCrewsIsolated() {
        let s = LocalCrewControlStore(directory: tempDir())
        s.requestRename(crewId: "local-a", name: "甲")
        s.requestRename(crewId: "local-b", name: "乙")
        let map = Dictionary(uniqueKeysWithValues: s.drainRenames().map { ($0.crewId, $0.title) })
        XCTAssertEqual(map["local-a"], "甲")
        XCTAssertEqual(map["local-b"], "乙")
    }

    func testPersistsAcrossInstances() {
        let dir = tempDir()
        LocalCrewControlStore(directory: dir).requestRename(crewId: "c", name: "持久")
        XCTAssertEqual(LocalCrewControlStore(directory: dir).pendingRename(crewId: "c"), "持久")
    }

    func testEnqueueStartSessionThenDrain() {
        let s = LocalCrewControlStore(directory: tempDir())
        s.enqueueStartSession(crewId: "local-a", brief: "修登录", runner: "claude", isolation: true)
        let cmds = s.drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].kind, "start_session")
        XCTAssertEqual(cmds[0].crewId, "local-a")
        XCTAssertEqual(cmds[0].brief, "修登录")
        XCTAssertEqual(cmds[0].runner, "claude")
        XCTAssertEqual(cmds[0].isolation, true)
        // 排空后再 drain 为空(文件已删)
        XCTAssertTrue(s.drainCommands().isEmpty)
    }

    func testEnqueueMultipleCommandsAllDrained() {
        let s = LocalCrewControlStore(directory: tempDir())
        s.enqueueStartSession(crewId: "c", brief: "活一", runner: nil, isolation: nil)
        s.enqueueStartSession(crewId: "c", brief: "活二", runner: "codex", isolation: false)
        s.enqueueCreateChildCrew(
            crewId: "c", sessionId: "captain-parent", brief: "拆一块", title: "支付")
        let cmds = s.drainCommands()
        XCTAssertEqual(cmds.count, 3)
        XCTAssertEqual(cmds.filter { $0.kind == "start_session" }.count, 2)
        XCTAssertEqual(cmds.filter { $0.kind == "create_child_crew" }.count, 1)
        XCTAssertEqual(cmds.first { $0.kind == "create_child_crew" }?.title, "支付")
        XCTAssertEqual(
            cmds.first { $0.kind == "create_child_crew" }?.sessionId, "captain-parent")
    }

    func testEnqueueRejectsEmptyBrief() {
        let s = LocalCrewControlStore(directory: tempDir())
        s.enqueueStartSession(crewId: "c", brief: "   \n ", runner: nil, isolation: nil)
        s.enqueueCreateChildCrew(
            crewId: "c", sessionId: "captain-parent", brief: "", title: nil)
        XCTAssertTrue(s.drainCommands().isEmpty)
    }

    // MARK: - Attention（旧会话兼容文案）

    func testRequestAttentionThenPeek() {
        let s = LocalCrewControlStore(directory: tempDir())
        s.requestAttention(crewId: "local-abc", reason: "需要人类拍板部署时机")
        XCTAssertEqual(s.pendingAttention(crewId: "local-abc")?.reason, "需要人类拍板部署时机")
    }

    func testRequestAttentionTrimsAndRejectsEmpty() {
        let s = LocalCrewControlStore(directory: tempDir())
        s.requestAttention(crewId: "c", reason: "  额度到顶了  ")
        XCTAssertEqual(s.pendingAttention(crewId: "c")?.reason, "额度到顶了")
        s.requestAttention(crewId: "blank", reason: "   \n  ")
        XCTAssertNil(s.pendingAttention(crewId: "blank"))
    }

    func testClearAttentionWritesNilReason() {
        let s = LocalCrewControlStore(directory: tempDir())
        s.requestClearAttention(crewId: "c")
        let pending = s.pendingAttention(crewId: "c")
        XCTAssertNotNil(pending)       // clear 也是一条待应用变更
        XCTAssertNil(pending?.reason)  // reason nil = 熄灭
    }

    func testDrainAttentionsReturnsAndDeletes() {
        let s = LocalCrewControlStore(directory: tempDir())
        s.requestAttention(crewId: "local-abc", reason: "卡在权限上")
        let drained = s.drainAttentions()
        XCTAssertEqual(drained.count, 1)
        XCTAssertEqual(drained[0].crewId, "local-abc")
        XCTAssertEqual(drained[0].reason, "卡在权限上")
        // 排空后文件已删 —— 再 drain / peek 为空。
        XCTAssertNil(s.pendingAttention(crewId: "local-abc"))
        XCTAssertTrue(s.drainAttentions().isEmpty)
    }

    // raise 后 clear：last-write-wins 只留最后的熄灭态。
    func testAttentionLastWriteWinsRaiseThenClear() {
        let s = LocalCrewControlStore(directory: tempDir())
        s.requestAttention(crewId: "c", reason: "有问题")
        s.requestClearAttention(crewId: "c")
        let drained = s.drainAttentions()
        XCTAssertEqual(drained.count, 1)
        XCTAssertNil(drained[0].reason)
    }

    func testAttentionAndRenameDoNotInterfere() {
        let s = LocalCrewControlStore(directory: tempDir())
        s.requestRename(crewId: "c", name: "起个名")
        s.requestAttention(crewId: "c", reason: "要人看")
        // drainAttentions 只拿 attention,不碰 rename;反之亦然。
        XCTAssertEqual(s.drainAttentions().count, 1)
        XCTAssertEqual(s.pendingRename(crewId: "c"), "起个名")
        XCTAssertEqual(s.drainRenames().count, 1)
        XCTAssertTrue(s.drainAttentions().isEmpty)
    }

    func testAttentionPersistsAcrossInstances() {
        let dir = tempDir()
        LocalCrewControlStore(directory: dir).requestAttention(crewId: "c", reason: "持久")
        XCTAssertEqual(LocalCrewControlStore(directory: dir).pendingAttention(crewId: "c")?.reason, "持久")
    }

    func testCommandsAndRenamesDoNotInterfere() {
        let s = LocalCrewControlStore(directory: tempDir())
        s.requestRename(crewId: "c", name: "起个名")
        s.enqueueStartSession(crewId: "c", brief: "干活", runner: nil, isolation: nil)
        // drainCommands 只拿命令,不碰 rename
        XCTAssertEqual(s.drainCommands().count, 1)
        XCTAssertEqual(s.pendingRename(crewId: "c"), "起个名")
        // drainRenames 只拿 rename,不碰命令(命令已被上面 drain 掉,这里重造一条验证隔离)
        s.enqueueStartSession(crewId: "c", brief: "再干", runner: nil, isolation: nil)
        XCTAssertEqual(s.drainRenames().count, 1)
        XCTAssertEqual(s.drainCommands().count, 1)
    }

    // MARK: - #528 drain 坏文件：归档 + 白板回执，不滞留重复解码、不静默丢

    func testDrainCommandsQuarantinesBadFileAndPostsReceipt() throws {
        let dir = tempDir()
        let s = LocalCrewControlStore(directory: dir)
        s.enqueueStartSession(crewId: "local-c1", brief: "好命令", runner: nil, isolation: nil)
        let garbage = Data("not json {{{".utf8)
        let badURL = dir.appendingPathComponent("local-c1.deadbeef.crewcmd.json")
        try garbage.write(to: badURL)

        // 第一次 drain：好命令照常返回,坏文件不再滞留(归档改名,后缀不再匹配)。
        let cmds = s.drainCommands()
        XCTAssertEqual(cmds.map(\.brief), ["好命令"])
        XCTAssertFalse(FileManager.default.fileExists(atPath: badURL.path))
        let archived = try FileManager.default.contentsOfDirectory(atPath: dir.path)
            .filter { $0.hasPrefix("local-c1.deadbeef.crewcmd.json.corrupt-") }
        XCTAssertEqual(archived.count, 1)
        XCTAssertEqual(try Data(contentsOf: dir.appendingPathComponent(archived[0])), garbage)
        // 第二次 drain：空 —— 不再每 tick 重复解码同一个坏文件。
        XCTAssertTrue(s.drainCommands().isEmpty)
        // 白板回执：发起方看得见命令被丢了。
        let notices = LocalWhiteboardStore(directory: dir).list(crewId: "local-c1")
        XCTAssertEqual(notices.count, 1)
        XCTAssertEqual(notices[0].senderSessionId, "system")
        XCTAssertTrue(notices[0].text.contains("损坏"))
        XCTAssertTrue(notices[0].text.contains("重发"))
    }

    func testDrainRenamesQuarantinesBadFile() throws {
        let dir = tempDir()
        let s = LocalCrewControlStore(directory: dir)
        try Data("{broken".utf8).write(to: dir.appendingPathComponent("local-c1.crewmeta.json"))
        s.requestRename(crewId: "local-c2", name: "好名字")
        XCTAssertEqual(s.drainRenames().map(\.title), ["好名字"])
        // 坏文件已归档,再 drain 不重现。
        XCTAssertTrue(s.drainRenames().isEmpty)
        XCTAssertEqual(LocalWhiteboardStore(directory: dir).list(crewId: "local-c1").count, 1)
    }
}
