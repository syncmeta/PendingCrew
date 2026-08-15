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
}
#endif
