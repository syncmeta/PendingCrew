#if os(macOS)
import XCTest
// CrewLocalMentionWakeLogic / LocalWhiteboardStore / WhiteboardCursor 直接编进
// PendingCrewTests target，无需 import。

/// 扫描游标该钉在哪（`CrewLocalMentionWakeLogic.pinPosition`）的单测（#595）。
///
/// 读失败时 `LocalWhiteboardStore.list()` 返回的是一条只存在于内存的
/// `whiteboard-read-failure` 警示行 —— 磁盘上根本没有这条。拿它当游标 = 当场悬空，
/// 下一次扫描把整部历史当新增。这次不钉、下次白板事件再钉，才是 fail-closed 的做法
///（退回 nil 同样不行：nil 在 `entries(in:after:)` 里等于全量）。
final class CrewLocalMentionWakePinTests: XCTestCase {

    private func row(id: String, createdAt: String = "2026-08-12T01:02:03Z",
                     sessionId: String = "s1") -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: id, senderKind: "session", senderUserId: nil, senderSessionId: sessionId,
            category: nil, text: "正文", createdAt: createdAt)
    }

    func testPinSkipsReadFailureSyntheticRow() {
        let synthetic = row(id: LocalWhiteboardStore.readFailureRowId, sessionId: "system")
        XCTAssertEqual(CrewLocalMentionWakeLogic.pinPosition(rows: [synthetic]), .retryLater)
    }

    func testPinSkipsWhenSyntheticRowIsTheTail() {
        // 白板重建后的形态：警示行在最后，前面还有历史。末条才是要钉的那条。
        let rows = [row(id: "history"),
                    row(id: LocalWhiteboardStore.readFailureRowId, sessionId: "system")]
        XCTAssertEqual(CrewLocalMentionWakeLogic.pinPosition(rows: rows), .retryLater)
    }

    func testPinOnEmptyWhiteboardHasNoAnchor() {
        // 空白板：无锚点 = 「钉之后的一切」，语义正确 —— 与「锚点悬空」是两回事。
        XCTAssertEqual(CrewLocalMentionWakeLogic.pinPosition(rows: []), .pin(nil))
    }

    func testPinUsesLastRowWithItsTimestamp() {
        let rows = [row(id: "a", createdAt: "2026-08-12T01:00:00Z"),
                    row(id: "tail-id", createdAt: "2026-08-12T01:02:03Z")]
        XCTAssertEqual(
            CrewLocalMentionWakeLogic.pinPosition(rows: rows),
            .pin(WhiteboardCursorPosition(id: "tail-id", createdAt: "2026-08-12T01:02:03Z")))
    }

    // MARK: - 没钉上的那个窗口要在盘上留痕（账本：这条丢法零痕迹）

    /// `.retryLater` 之后到补钉成功之间写进来的定向 @ 落在补钉位置之前，谁也扫不到，
    /// 而**这件事原本在磁盘上一点痕迹都没有** —— 没有告警、没有计数，白板上看不出
    /// 「这里本该有人被叫醒」。所以它既量不到，也没人会回头查。
    ///
    /// 补不回那些消息（读失败那一刻 `list()` 只返回一条内存里的警示行，连一个真实
    /// 锚点都没有，「钉在最后一条真行上」这条路不存在）。能做的是把**静默的丢**
    /// 变成**看得见的丢**：补钉成功时白板已经可读，就在那儿留一行写清窗口两端。
    func test_有缺口时留痕写清窗口两端() {
        let failed = Date(timeIntervalSince1970: 1_000)
        let text = CrewLocalMentionWakeLogic.missedPinWindowNotice(
            failedAt: failed, recoveredAt: failed.addingTimeInterval(90))
        let t = try? XCTUnwrap(text)
        XCTAssertNotNil(t)
        guard let t else { return }
        let f = ISO8601DateFormatter()
        XCTAssertTrue(t.contains(f.string(from: failed)), "没写窗口左端：\(t)")
        XCTAssertTrue(t.contains(f.string(from: failed.addingTimeInterval(90))),
                      "没写窗口右端 —— 人无从知道该翻到哪：\(t)")
        XCTAssertTrue(t.contains("90"), "没写窗口有多长：\(t)")
        XCTAssertTrue(t.contains("定向 @"), "没说丢的是什么：\(t)")
    }

    /// 反面两条，缺一不可：没缺口时**一个字都不许写**。
    /// 一个「永远留一行」的实现会让上面那条照样绿，而它会在每次钉游标时往白板上
    /// 灌一条没有内容的警示 —— 那比不留痕更坏（人会开始忽略这种行）。
    func test_没缺口时不留痕() {
        let now = Date(timeIntervalSince1970: 1_000)
        XCTAssertNil(CrewLocalMentionWakeLogic.missedPinWindowNotice(
            failedAt: nil, recoveredAt: now),
            "从来没失败过也留了一行")
        XCTAssertNil(CrewLocalMentionWakeLogic.missedPinWindowNotice(
            failedAt: now, recoveredAt: now),
            "窗口宽度为零也留了一行")
    }

    /// 这一行**不许唤醒任何人**：它是给人回头查的留痕，不是又一次派工。
    /// 形状由 `pending` 定 —— 无 mention 的 session 条目返回空。这里把那个前提钉住，
    /// 免得哪天有人给系统条目加上「默认 @机长」，让留痕变成每次都吵醒机长。
    func test_留痕那一行谁也叫不醒() {
        let notice = LocalWhiteboardMessage(
            id: "notice-1", senderKind: "session", senderUserId: nil,
            senderSessionId: "system", category: nil,
            text: "唤醒器的扫描游标上次没钉上……",
            createdAt: ISO8601DateFormatter().string(from: Date()))
        XCTAssertTrue(CrewLocalMentionWakeLogic.pending(entries: [notice]).isEmpty,
                      "留痕那一行把人叫醒了 —— 它只该留在板上给人查")
    }
}
#endif
