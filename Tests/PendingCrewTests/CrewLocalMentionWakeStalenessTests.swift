#if os(macOS)
import XCTest
// CrewLocalMentionWakeLogic / LocalWhiteboardStore / WhiteboardCursor 直接编进
// PendingCrewTests target，无需 import。

/// **陈旧 @ 不唤醒**（#595）—— 唤醒路的最后一道防线。
///
/// 就算游标层（`WhiteboardCursorFailClosedTests`）哪天再被别的路径破掉，几周前的
/// 历史 @ 也不该把 session 拉起来照着过期指令返工。2026-08-12 的代价是两位数的无效
/// 轮次（13 号 crew 一家六轮、本群四轮，全是已合 main 的活被重派）；没人误改代码
/// 只是因为 worker 都自己核了 main —— 那不是护栏，是运气。
final class CrewLocalMentionWakeStalenessTests: XCTestCase {

    private let now = Date(timeIntervalSince1970: 1_786_500_000)   // 2026-08-12

    private func iso(_ d: Date) -> String { ISO8601DateFormatter().string(from: d) }

    private func mention(at createdAt: String,
                         id: String = UUID().uuidString.lowercased()) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: id, senderKind: "captain", senderUserId: nil, senderSessionId: "cap-1",
            category: nil, text: "去做 Todo #17", createdAt: createdAt,
            senderName: "机长",
            mentions: [LocalWhiteboardMention(kind: "session", targetId: "sess-a")])
    }

    func testStaleMentionDoesNotWake() {
        // 7/25 的派工文案在 8/12 被重放 —— 正是事故当天的形状。
        let e = mention(at: "2026-07-25T02:00:00Z")
        XCTAssertTrue(CrewLocalMentionWakeLogic.pending(entries: [e], now: now).isEmpty)
    }

    func testFreshMentionStillWakes() {
        let e = mention(at: iso(now.addingTimeInterval(-60)))
        XCTAssertEqual(CrewLocalMentionWakeLogic.pending(entries: [e], now: now).count, 1)
    }

    func testMentionJustInsideTheWindowStillWakes() {
        let e = mention(at: iso(now.addingTimeInterval(-CrewLocalMentionWakeLogic.maxWakeAge + 60)))
        XCTAssertEqual(CrewLocalMentionWakeLogic.pending(entries: [e], now: now).count, 1)
    }

    func testMentionJustOutsideTheWindowDoesNotWake() {
        let e = mention(at: iso(now.addingTimeInterval(-CrewLocalMentionWakeLogic.maxWakeAge - 60)))
        XCTAssertTrue(CrewLocalMentionWakeLogic.pending(entries: [e], now: now).isEmpty)
    }

    func testFutureTimestampStillWakes() {
        // 时钟漂移 / relay 带来的未来时间戳不该被当成陈旧。
        let e = mention(at: iso(now.addingTimeInterval(600)))
        XCTAssertEqual(CrewLocalMentionWakeLogic.pending(entries: [e], now: now).count, 1)
    }

    func testUnparseableTimestampStillWakes() {
        // 这道闸是兜底，不是主防线：时间戳读不出来时宁可投递也不静默吃掉一条真 @
        // （主防线是游标 fail-closed，那边解析不了才按「不是新的」处理）。
        let e = mention(at: "垃圾时间戳")
        XCTAssertEqual(CrewLocalMentionWakeLogic.pending(entries: [e], now: now).count, 1)
    }

    func testStaleAndFreshMixedKeepsOnlyFresh() {
        let stale = mention(at: "2026-07-25T02:00:00Z")
        let fresh = mention(at: iso(now.addingTimeInterval(-30)))
        let out = CrewLocalMentionWakeLogic.pending(entries: [stale, fresh], now: now)
        XCTAssertEqual(out.map(\.entryId), [fresh.id])
    }
}
#endif
