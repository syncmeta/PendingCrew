#if os(macOS)
import XCTest
// CrewRecentContextRender + LocalWhiteboardStore(含 LocalWhiteboardMessage) 直接编进
// PendingCrewTests target（见 project.yml），无需 import。

/// 「近期群聊」上下文块渲染核心（`CrewRecentContextRender.block`，项8）的单测：
/// 空 → nil、发送者标注回退、按原序渲染、超长正文软截断。
final class CrewRecentContextRenderTests: XCTestCase {

    private func msg(
        kind: String,
        text: String,
        sessionId: String? = nil,
        senderName: String? = nil,
        displayName: String? = nil
    ) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: UUID().uuidString, senderKind: kind, senderUserId: nil,
            senderSessionId: sessionId, category: nil, text: text,
            createdAt: "2026-07-13T00:00:00Z",
            senderDisplayName: displayName, senderName: senderName)
    }

    func testEmptyReturnsNil() {
        XCTAssertNil(CrewRecentContextRender.block([]))
    }

    func testAllBlankBodiesReturnsNil() {
        // 只剩标题头无意义 → nil。
        XCTAssertNil(CrewRecentContextRender.block([msg(kind: "user", text: "   ")]))
    }

    func testHeaderAndOrder() {
        let out = CrewRecentContextRender.block([
            msg(kind: "user", text: "第一句", senderName: "小明"),
            msg(kind: "captain", text: "第二句"),
        ])
        guard let out else { return XCTFail("expected block") }
        XCTAssertTrue(out.hasPrefix("近期群聊："), out)
        let idx1 = out.range(of: "小明: 第一句")
        let idx2 = out.range(of: "机长: 第二句")
        XCTAssertNotNil(idx1); XCTAssertNotNil(idx2)
        XCTAssertTrue(idx1!.lowerBound < idx2!.lowerBound, "按原序渲染")
    }

    func testSenderLabelFallbacks() {
        let out = CrewRecentContextRender.block([
            msg(kind: "user", text: "a"),                                   // 人类
            msg(kind: "session", text: "b", sessionId: "sess-abc123"),      // session:sess-a
            msg(kind: "session", text: "c", displayName: "远端名"),          // relay 显示名优先
            msg(kind: "session", text: "d", senderName: "本地名"),           // 本地名最优先
            msg(kind: "bot", text: "e"),                                    // 未知 kind 原样
        ])!
        XCTAssertTrue(out.contains("人类: a"), out)
        XCTAssertTrue(out.contains("session:sess-a: b"), out)
        XCTAssertTrue(out.contains("远端名: c"), out)
        XCTAssertTrue(out.contains("本地名: d"), out)
        XCTAssertTrue(out.contains("bot: e"), out)
    }

    func testLongBodyTruncated() {
        let long = String(repeating: "长", count: 500)
        let out = CrewRecentContextRender.block([msg(kind: "user", text: long)])!
        XCTAssertTrue(out.contains("…"), "超长正文应软截断")
        XCTAssertLessThan(out.count, 500, "截断后应短于原文")
    }
}
#endif
