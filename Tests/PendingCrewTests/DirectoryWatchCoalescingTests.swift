import XCTest
// 不 import PendingCrew —— PendingCrewTests 是 standalone bundle（TEST_HOST=""），
// 待测源码（DirectoryWatchCoalescing.swift / LocalWhiteboardStore.swift）直接编进
// bundle，见 project.yml 的 test target sources。

/// #443（点进群聊转彩虹圈）病根 1 的两块判定逻辑。
///
/// 现场：`whiteboards/` 目录 648 个文件，别的 session 的 `.cursor`/`.turn`/
/// approvals 持续在写；每一次写都被原样转成一个 tick，群聊中栏就整表重拉重排，
/// 主线程 100% busy。这里钉住的是「合流」与「相关性」两条闸的语义。
final class DirectoryWatchCoalescingTests: XCTestCase {

    // MARK: - 合流

    func testWindowCollapsesBurstIntoOneFlush() {
        let c = DirectoryEventCoalescer()
        // 第一个事件负责排 flush，同窗口内其余全部被吸收。
        XCTAssertTrue(c.noteEvent())
        for _ in 0 ..< 50 {
            XCTAssertFalse(c.noteEvent(), "同一窗口内的后续事件不该再排一次 flush")
        }
        XCTAssertTrue(c.flush(), "窗口到期应确实发一个 tick")
    }

    func testNextWindowFiresAgain() {
        let c = DirectoryEventCoalescer()
        XCTAssertTrue(c.noteEvent())
        XCTAssertTrue(c.flush())
        // 窗口结束后是干净的下一轮 —— 合流不能把后续变更永久吞掉。
        XCTAssertTrue(c.noteEvent())
        XCTAssertTrue(c.flush())
    }

    func testFlushWithoutEventIsNoop() {
        let c = DirectoryEventCoalescer()
        XCTAssertFalse(c.flush(), "没有待发事件时 flush 不该凭空造一个 tick")
    }

    // MARK: - 相关性

    /// 无关文件写盘 → 白板 json 指纹不动 → 不出 tick。这是主因那一条。
    func testUnrelatedWriteDoesNotYield() throws {
        let dir = try makeTempDir()
        let store = LocalWhiteboardStore(directory: dir)
        let crewId = "local-crew-a"
        store.appendUserMessage(crewId: crewId, text: "hi")

        var gate = FileChangeGate(seed: store.fingerprint(crewId: crewId))

        // 别的 session 写自己的状态文件（同一个被监听目录）。
        try "{}".write(to: dir.appendingPathComponent("\(crewId).cursor.json"),
                       atomically: true, encoding: .utf8)
        try "{}".write(to: dir.appendingPathComponent("other-session.turn.json"),
                       atomically: true, encoding: .utf8)

        XCTAssertFalse(gate.shouldYield(store.fingerprint(crewId: crewId)),
                       "白板 json 没动，不该把整条群聊拉起来重排")
    }

    /// 别的 crew 的白板变了也不该惊动本 crew 的流。
    func testOtherCrewWhiteboardDoesNotYield() throws {
        let dir = try makeTempDir()
        let store = LocalWhiteboardStore(directory: dir)
        let mine = "local-crew-a"
        store.appendUserMessage(crewId: mine, text: "hi")
        var gate = FileChangeGate(seed: store.fingerprint(crewId: mine))

        store.appendUserMessage(crewId: "local-crew-b", text: "别人群里的消息")

        XCTAssertFalse(gate.shouldYield(store.fingerprint(crewId: mine)))
    }

    /// 白板 json 真变了必须出 tick —— 收敛不能把该刷的刷没了。
    func testWhiteboardAppendYields() throws {
        let dir = try makeTempDir()
        let store = LocalWhiteboardStore(directory: dir)
        let crewId = "local-crew-a"
        store.appendUserMessage(crewId: crewId, text: "第一条")
        var gate = FileChangeGate(seed: store.fingerprint(crewId: crewId))

        store.appendSessionMessage(crewId: crewId, sessionId: "s1", text: "agent 的进展")

        XCTAssertTrue(gate.shouldYield(store.fingerprint(crewId: crewId)))
        // 同一状态再判一次不该重复 yield。
        XCTAssertFalse(gate.shouldYield(store.fingerprint(crewId: crewId)))
    }

    /// 白板文件从无到有（新 crew 第一条消息）也算变化。
    func testFileAppearingYields() throws {
        let dir = try makeTempDir()
        let store = LocalWhiteboardStore(directory: dir)
        let crewId = "local-crew-new"
        XCTAssertNil(store.fingerprint(crewId: crewId), "还没有白板文件")
        var gate = FileChangeGate(seed: nil)

        store.appendUserMessage(crewId: crewId, text: "开张")

        XCTAssertTrue(gate.shouldYield(store.fingerprint(crewId: crewId)))
    }

    /// `sync` 记状态但不 yield —— 本进程 append 已经沿进程内那条推过了，同一次
    /// 写盘随后的目录事件不该再来一遍。
    func testSyncSwallowsTheFollowUpDirectoryTick() throws {
        let dir = try makeTempDir()
        let store = LocalWhiteboardStore(directory: dir)
        let crewId = "local-crew-a"
        store.appendUserMessage(crewId: crewId, text: "第一条")
        let gate = FileChangeGateBox(seed: store.fingerprint(crewId: crewId))

        // 本进程 append → 进程内那条 sink 无条件 yield，并 sync 指纹。
        store.appendUserMessage(crewId: crewId, text: "我发的")
        gate.sync(store.fingerprint(crewId: crewId))

        // 同一次写盘触发的目录事件到了。
        XCTAssertFalse(gate.shouldYield(store.fingerprint(crewId: crewId)),
                       "同一次写盘不该刷两遍")
    }

    // MARK: - helpers

    private func makeTempDir() throws -> URL {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("dirwatch-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: dir) }
        return dir
    }
}
