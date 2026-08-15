import XCTest
// LocalWhiteboardStore / WhiteboardCursor / CrewTimestamp 直接编进 PendingCrewTests
// target（见 project.yml），无需 import。

/// 白板游标 **fail-closed** 的单测（#595）。
///
/// 2026-08-12 事故：本机多个 crew 的 session 同时被「几周前的旧 @」唤醒重放。
/// 六环因果链的最后两环在这里钉死 —— 前四环（fd 打满 → open 失败 → 误判损坏 →
/// 归档重建空板）归 P0「读失败不许销毁原件」那条：
///
///   … → 白板换了一批新 id → **全机游标集体悬空** → **每次唤醒全量重放**
///                              ↑ 第 5 环                 ↑ 第 6 环
///
/// 第 6 环的病根是 `entries(crewId:afterId:)` 的 fail-open：「afterId 找不到 →
/// 返回全部」。游标认不得的那一刻，整部历史被当成新增。本组测试要求的语义是
/// **游标不认得，绝不等于「全是新的」**。
final class WhiteboardCursorFailClosedTests: XCTestCase {

    // MARK: - Fixtures

    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("wb-cursor-test-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func msg(_ text: String, at createdAt: String,
                     id: String = UUID().uuidString.lowercased()) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: id, senderKind: "session", senderUserId: nil, senderSessionId: "s1",
            category: nil, text: text, createdAt: createdAt)
    }

    /// 直接把一批消息落成 `<dir>/<crewId>.json`（要精确控制 createdAt，不能走 append）。
    private func seed(_ rows: [LocalWhiteboardMessage], crewId: String, in dir: URL) {
        let data = try! JSONEncoder().encode(rows)
        try! data.write(to: dir.appendingPathComponent("\(crewId).json"), options: .atomic)
    }

    private func cursorFile(_ dir: URL, _ crewId: String, _ sessionId: String) -> URL {
        dir.appendingPathComponent("\(crewId).\(sessionId).cursor")
    }

    private func cursorContents(_ dir: URL, _ crewId: String, _ sessionId: String) -> String? {
        try? String(contentsOf: cursorFile(dir, crewId, sessionId), encoding: .utf8)
    }

    // MARK: - ① 游标 id 不在表里 → 不重放全部（纯函数层）

    func testNilPositionReturnsAll() {
        // 真「没有游标」仍是全部 —— 这是首次投递的合法语义，本次修复不动它。
        // 关键在于**谁**有资格传 nil：只有游标文件确实不存在时（见 unread 那组）。
        let rows = [msg("1", at: "2026-08-01T00:00:00Z"), msg("2", at: "2026-08-02T00:00:00Z")]
        XCTAssertEqual(
            LocalWhiteboardStore.entries(in: rows, after: nil).map(\.text), ["1", "2"])
    }

    func testKnownAnchorReturnsSuffixAfterIt() {
        let rows = [
            msg("1", at: "2026-08-01T00:00:00Z", id: "a"),
            msg("2", at: "2026-08-02T00:00:00Z", id: "b"),
            msg("3", at: "2026-08-03T00:00:00Z", id: "c"),
        ]
        let pos = WhiteboardCursorPosition(id: "b", createdAt: "2026-08-02T00:00:00Z")
        XCTAssertEqual(LocalWhiteboardStore.entries(in: rows, after: pos).map(\.text), ["3"])
    }

    func testDanglingAnchorWithTimestampCutsByTimeNotWholeHistory() {
        // 第 5→6 环：归档重建把 id 全换了一批，锚点 id 从此不在表里。
        // fail-open 时这里会返回全部三条（= 全机重放）；fail-closed 只给真正更新的。
        let rows = [
            msg("旧-1", at: "2026-07-25T00:00:00Z"),
            msg("旧-2", at: "2026-07-26T00:00:00Z"),
            msg("新", at: "2026-08-12T00:00:00Z"),
        ]
        let pos = WhiteboardCursorPosition(
            id: "id-that-no-longer-exists", createdAt: "2026-08-01T00:00:00Z")
        XCTAssertEqual(LocalWhiteboardStore.entries(in: rows, after: pos).map(\.text), ["新"])
    }

    func testDanglingAnchorWithTimestampNewerThanEverythingReturnsNothing() {
        let rows = [msg("旧", at: "2026-07-25T00:00:00Z"), msg("旧2", at: "2026-07-26T00:00:00Z")]
        let pos = WhiteboardCursorPosition(id: "gone", createdAt: "2026-08-12T00:00:00Z")
        XCTAssertTrue(LocalWhiteboardStore.entries(in: rows, after: pos).isEmpty)
    }

    func testDanglingLegacyIdOnlyAnchorReturnsNothing() {
        // 旧格式游标只有 id、没有时间戳：无从判断新旧 → fail-closed 一条都不给。
        // （不给之后由 `unread` resync 到当前尾，见下面的迁移那组，游标不会卡死。）
        let rows = [msg("旧", at: "2026-07-25T00:00:00Z"), msg("新", at: "2026-08-12T00:00:00Z")]
        let pos = WhiteboardCursorPosition(id: "gone", createdAt: nil)
        XCTAssertTrue(LocalWhiteboardStore.entries(in: rows, after: pos).isEmpty)
    }

    func testUnparseableMessageTimestampIsNotTreatedAsNew() {
        // 时间戳读不出来的行在「按时间戳切」这条路上算不出新旧 —— fail-closed 排除，
        // 绝不因为「解析不了」就当新消息重投。
        let rows = [msg("脏时间", at: "not-a-timestamp"), msg("新", at: "2026-08-12T00:00:00Z")]
        let pos = WhiteboardCursorPosition(id: "gone", createdAt: "2026-08-01T00:00:00Z")
        XCTAssertEqual(LocalWhiteboardStore.entries(in: rows, after: pos).map(\.text), ["新"])
    }

    func testFractionalSecondTimestampsCompareCorrectly() {
        // relay 从 edge 搬进来的 createdAt 带小数秒，本机写的不带 —— 两种混在一张表里
        // 必须比得对（口径走 CrewTimestamp.parse，与侧栏排序同一份）。
        let rows = [
            msg("旧", at: "2026-08-12T00:00:00.500Z"),
            msg("新", at: "2026-08-12T00:00:02Z"),
        ]
        let pos = WhiteboardCursorPosition(id: "gone", createdAt: "2026-08-12T00:00:01.250Z")
        XCTAssertEqual(LocalWhiteboardStore.entries(in: rows, after: pos).map(\.text), ["新"])
    }

    // MARK: - lenient 解码丢行 → 同样不许重放

    func testAnchorRowDroppedByLenientDecodeDoesNotReplayHistory() {
        // `MultiProcessJSONStore` 的逐条 lenient 解码坏一条丢一条 —— 被丢掉的那条
        // 恰好是游标锚点时，锚点同样悬空。走的是同一条 fail-closed 分支。
        let dir = tempDir()
        let crewId = "local-lenient"
        let good1 = msg("旧", at: "2026-07-25T00:00:00Z", id: "keep-1")
        let good2 = msg("新", at: "2026-08-12T00:00:00Z", id: "keep-2")
        // 中间那条 id 是数字 → 元素解码失败 → 被丢掉（不连坐整表）。
        let raw = """
        [\(String(data: try! JSONEncoder().encode(good1), encoding: .utf8)!),\
        {"id":12345,"senderKind":"session","senderUserId":null,"senderSessionId":"s1",\
        "category":null,"text":"锚点行","createdAt":"2026-08-01T00:00:00Z"},\
        \(String(data: try! JSONEncoder().encode(good2), encoding: .utf8)!)]
        """
        try! raw.write(to: dir.appendingPathComponent("\(crewId).json"),
                       atomically: true, encoding: .utf8)
        let store = LocalWhiteboardStore(directory: dir)
        XCTAssertEqual(store.list(crewId: crewId).count, 2, "坏行应被丢掉、好行留下")

        let pos = WhiteboardCursorPosition(id: "anchor-dropped", createdAt: "2026-08-01T00:00:00Z")
        XCTAssertEqual(store.entries(crewId: crewId, after: pos).map(\.text), ["新"])
    }

    // MARK: - ② unread：「文件不存在」与「锚点悬空」必须是两态

    func testMissingCursorFileIsGenuineFirstDelivery() {
        let dir = tempDir()
        let crewId = "local-first"
        seed([msg("1", at: "2026-08-12T00:00:00Z"), msg("2", at: "2026-08-12T00:00:01Z")],
             crewId: crewId, in: dir)
        let store = LocalWhiteboardStore(directory: dir)
        let cursor = WhiteboardCursor(directory: dir, crewId: crewId, sessionId: "sess-1")
        XCTAssertEqual(cursor.unread(in: store).map(\.text), ["1", "2"])
    }

    func testFirstDeliveryIsCappedToRecentEntries() {
        // 202KB 的历史白板一次性灌进 session 既撑爆上下文又毫无价值 —— 只给最近这批。
        let dir = tempDir()
        let crewId = "local-cap"
        let rows = (0..<(WhiteboardCursor.firstDeliveryLimit + 20)).map {
            msg("m\($0)", at: String(format: "2026-08-12T00:%02d:00Z", $0))
        }
        seed(rows, crewId: crewId, in: dir)
        let store = LocalWhiteboardStore(directory: dir)
        let unread = WhiteboardCursor(directory: dir, crewId: crewId, sessionId: "sess-1")
            .unread(in: store)
        XCTAssertEqual(unread.count, WhiteboardCursor.firstDeliveryLimit)
        XCTAssertEqual(unread.last?.text, rows.last?.text, "留的是最近这批，不是最早那批")
    }

    func testFreshClaudeSessionFirstPromptIncludesRecentWhiteboardHistory() {
        // Todo #56 ③ 的真实启动边界：仅证明 cursor.unread() 有数据还不够；Claude 的
        // 第一轮必须在任何 PostToolUse hook 发生前，就把这批历史与 brief 一起拿到。
        let dir = tempDir()
        let crewId = "local-first-prompt"
        var rows = (0..<35).map {
            msg("ordinary-\($0)", at: String(format: "2026-08-12T00:00:%02dZ", $0))
        }
        rows[4] = msg("excluded-old-boundary", at: "2026-08-12T00:00:04Z")
        rows[5] = msg("included-recent-boundary", at: "2026-08-12T00:00:05Z")
        rows[34] = msg("latest-history", at: "2026-08-12T00:00:34Z")
        seed(rows, crewId: crewId, in: dir)

        let prompt = LocalSessionLaunch.initialPromptWithWhiteboard(
            "do the assigned task", crewId: crewId, sessionId: "fresh-session",
            captain: false, directory: dir)

        XCTAssertTrue(prompt.contains("do the assigned task"), "原始 brief 不能丢")
        XCTAssertTrue(prompt.contains("included-recent-boundary"), "首次应注入最近 30 条")
        XCTAssertTrue(prompt.contains("latest-history"), "白板尾部必须在首轮可见")
        XCTAssertFalse(prompt.contains("excluded-old-boundary"), "首次上限不能被无脑放开")
        XCTAssertEqual(
            WhiteboardCursor(directory: dir, crewId: crewId, sessionId: "fresh-session").read(),
            .anchored(.init(id: rows.last!.id, createdAt: rows.last!.createdAt)),
            "首轮组装后推进同一游标，PostToolUse 不应重复注入")
    }

    func testClaudeFirstPromptDoesNotReplayHistoryForDanglingLegacyCursor() {
        // #595 不变式：补首轮接线不能把「悬空旧游标」重新解释成真首次。
        let dir = tempDir()
        let crewId = "local-first-prompt-dangling"
        seed([msg("stale-history-must-not-replay", at: "2026-07-25T00:00:00Z")],
             crewId: crewId, in: dir)
        try! "gone-anchor".write(
            to: cursorFile(dir, crewId, "returning-session"),
            atomically: true, encoding: .utf8)

        let prompt = LocalSessionLaunch.initialPromptWithWhiteboard(
            "resume assigned task", crewId: crewId, sessionId: "returning-session",
            captain: false, directory: dir)

        XCTAssertEqual(prompt, "resume assigned task")
        XCTAssertFalse(prompt.contains("stale-history-must-not-replay"))
    }

    func testPresentButEmptyCursorFileIsNotTreatedAsFirstDelivery() {
        // 「文件在但读不出来 / 是空的」≠「本来就没投过」。8/11 那次 P0 是同一个错误
        // 形状（把"读不出来"和"本来就空"混成一态），这里不许重蹈。
        let dir = tempDir()
        let crewId = "local-empty-cursor"
        seed([msg("历史", at: "2026-07-25T00:00:00Z")], crewId: crewId, in: dir)
        try! "".write(to: cursorFile(dir, crewId, "sess-1"), atomically: true, encoding: .utf8)
        let store = LocalWhiteboardStore(directory: dir)
        XCTAssertTrue(WhiteboardCursor(directory: dir, crewId: crewId, sessionId: "sess-1")
            .unread(in: store).isEmpty)
    }

    // MARK: - ③ 迁移：旧格式（纯 id）游标绝不能当首次

    func testLegacyIdOnlyCursorWithDanglingAnchorIsNotTreatedAsFirstDelivery() {
        // 上线那一刻的形状：磁盘上全是旧格式游标（只有 id），而白板已被归档重建、
        // id 换了一批。当成首次 = 修复上线即再触发一次全机重放，比 bug 本身还难看。
        let dir = tempDir()
        let crewId = "local-migrate"
        seed([msg("历史1", at: "2026-07-25T00:00:00Z"), msg("历史2", at: "2026-07-26T00:00:00Z")],
             crewId: crewId, in: dir)
        try! "id-from-the-archived-board".write(
            to: cursorFile(dir, crewId, "sess-1"), atomically: true, encoding: .utf8)

        let store = LocalWhiteboardStore(directory: dir)
        let cursor = WhiteboardCursor(directory: dir, crewId: crewId, sessionId: "sess-1")
        XCTAssertTrue(cursor.unread(in: store).isEmpty, "旧格式悬空游标一条都不许重放")
    }

    func testLegacyDanglingCursorResyncsToTailSoLaterMessagesStillArrive() {
        // fail-closed 不能变成「从此再也送不出去」：悬空且无从判新旧时，游标 resync
        // 到当前尾，此后写进来的照常是未读。
        let dir = tempDir()
        let crewId = "local-migrate-2"
        seed([msg("历史", at: "2026-07-25T00:00:00Z", id: "old-tail")], crewId: crewId, in: dir)
        try! "gone".write(to: cursorFile(dir, crewId, "sess-1"),
                          atomically: true, encoding: .utf8)

        let store = LocalWhiteboardStore(directory: dir)
        let cursor = WhiteboardCursor(directory: dir, crewId: crewId, sessionId: "sess-1")
        XCTAssertTrue(cursor.unread(in: store).isEmpty)

        store.appendSessionMessage(crewId: crewId, sessionId: "s2", text: "新消息")
        XCTAssertEqual(cursor.unread(in: store).map(\.text), ["新消息"])
    }

    func testLegacyCursorWithLiveAnchorKeepsWorkingAndUpgradesToComposite() {
        // 锚点还在表里的旧格式游标：语义完全不变（照常给它之后那批），顺手升格成
        // (id, 时间戳) —— 下一次万一悬空就有时间戳可切，不必再走 resync 丢消息。
        let dir = tempDir()
        let crewId = "local-migrate-3"
        seed([
            msg("1", at: "2026-08-12T00:00:00Z", id: "a"),
            msg("2", at: "2026-08-12T00:00:01Z", id: "b"),
        ], crewId: crewId, in: dir)
        try! "a".write(to: cursorFile(dir, crewId, "sess-1"), atomically: true, encoding: .utf8)

        let store = LocalWhiteboardStore(directory: dir)
        let cursor = WhiteboardCursor(directory: dir, crewId: crewId, sessionId: "sess-1")
        XCTAssertEqual(cursor.unread(in: store).map(\.text), ["2"])

        let raw = cursorContents(dir, crewId, "sess-1") ?? ""
        XCTAssertTrue(raw.contains("a"), "锚点 id 不变")
        XCTAssertTrue(raw.contains("2026-08-12T00:00:00Z"), "已升格成 (id, 时间戳)")
        XCTAssertEqual(WhiteboardCursor(directory: dir, crewId: crewId, sessionId: "sess-1")
            .read(), .anchored(.init(id: "a", createdAt: "2026-08-12T00:00:00Z")))
    }

    // MARK: - ④ advance：悬空不推进这个第二放大器

    func testAdvanceMovesForwardEvenWhenCurrentAnchorIsDangling() {
        // 病根第二个放大器：`guard let toIdx = ... else { return }` 让游标永远卡在
        // 悬空位，同一批消息每次唤醒都会再来一遍。悬空态必须能被推出去。
        let dir = tempDir()
        let crewId = "local-adv-1"
        let tail = msg("新", at: "2026-08-12T00:00:00Z", id: "fresh")
        seed([tail], crewId: crewId, in: dir)
        try! "vanished-anchor".write(to: cursorFile(dir, crewId, "sess-1"),
                                     atomically: true, encoding: .utf8)

        let store = LocalWhiteboardStore(directory: dir)
        let cursor = WhiteboardCursor(directory: dir, crewId: crewId, sessionId: "sess-1")
        cursor.advance(to: tail, in: store)
        XCTAssertEqual(cursor.read(), .anchored(.init(id: "fresh", createdAt: tail.createdAt)))
    }

    func testAdvanceStillRefusesToGoBackwards() {
        // forward-only 语义保住：多进程并发写不能把游标推回旧位（重复注入的老病）。
        let dir = tempDir()
        let crewId = "local-adv-2"
        let first = msg("1", at: "2026-08-12T00:00:00Z", id: "a")
        let second = msg("2", at: "2026-08-12T00:00:01Z", id: "b")
        seed([first, second], crewId: crewId, in: dir)

        let store = LocalWhiteboardStore(directory: dir)
        let cursor = WhiteboardCursor(directory: dir, crewId: crewId, sessionId: "sess-1")
        cursor.advance(to: second, in: store)
        cursor.advance(to: first, in: store)
        XCTAssertEqual(cursor.read(), .anchored(.init(id: "b", createdAt: second.createdAt)))
    }

    func testAdvanceRefusesToGoBackwardsByTimestampWhenAnchorDangling() {
        // 悬空态也不许倒退：锚点认不出，但时间戳更新的那个才是「更靠后」。
        let dir = tempDir()
        let crewId = "local-adv-3"
        let older = msg("旧", at: "2026-07-25T00:00:00Z", id: "old")
        seed([older], crewId: crewId, in: dir)
        try! "vanished\t2026-08-12T00:00:00Z".write(
            to: cursorFile(dir, crewId, "sess-1"), atomically: true, encoding: .utf8)

        let store = LocalWhiteboardStore(directory: dir)
        let cursor = WhiteboardCursor(directory: dir, crewId: crewId, sessionId: "sess-1")
        cursor.advance(to: older, in: store)
        XCTAssertEqual(
            cursor.read(), .anchored(.init(id: "vanished", createdAt: "2026-08-12T00:00:00Z")),
            "时间戳更旧的目标不许把游标拉回去")
    }

    func testAdvanceThenUnreadIsEmpty() {
        let dir = tempDir()
        let crewId = "local-adv-4"
        let rows = [msg("1", at: "2026-08-12T00:00:00Z"), msg("2", at: "2026-08-12T00:00:01Z")]
        seed(rows, crewId: crewId, in: dir)
        let store = LocalWhiteboardStore(directory: dir)
        let cursor = WhiteboardCursor(directory: dir, crewId: crewId, sessionId: "sess-1")
        cursor.advance(to: rows[1], in: store)
        XCTAssertTrue(cursor.unread(in: store).isEmpty)
    }

    // MARK: - 游标文件编解码

    func testCursorRoundTripsCompositePosition() {
        let dir = tempDir()
        let crewId = "local-codec"
        let only = msg("x", at: "2026-08-12T03:04:05Z", id: "only")
        seed([only], crewId: crewId, in: dir)
        let store = LocalWhiteboardStore(directory: dir)
        let cursor = WhiteboardCursor(directory: dir, crewId: crewId, sessionId: "sess-1")
        cursor.advance(to: only, in: store)
        XCTAssertEqual(cursor.read(),
                       .anchored(.init(id: "only", createdAt: "2026-08-12T03:04:05Z")))
    }

    func testMissingCursorFileReadsAsAbsent() {
        let dir = tempDir()
        XCTAssertEqual(
            WhiteboardCursor(directory: dir, crewId: "c", sessionId: "s").read(), .absent)
    }

    func testLegacyCursorFileReadsAsAnchoredWithoutTimestamp() {
        let dir = tempDir()
        try! "legacy-id".write(to: cursorFile(dir, "c", "s"), atomically: true, encoding: .utf8)
        XCTAssertEqual(WhiteboardCursor(directory: dir, crewId: "c", sessionId: "s").read(),
                       .anchored(.init(id: "legacy-id", createdAt: nil)))
    }
}
