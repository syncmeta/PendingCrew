import XCTest
// CrewWhiteboardVisibility.swift + LocalWhiteboardStore.swift 编进 test bundle（见 project.yml）。

/// 白板消息 → session 注入面可见性的单一判定单测（#543 定向 @ 扩散根因）。
final class CrewWhiteboardVisibilityTests: XCTestCase {

    private func msg(
        sessionId: String? = "s-sender",
        kind: String = "session",
        mentions: [LocalWhiteboardMention]? = nil
    ) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: UUID().uuidString, senderKind: kind, senderUserId: nil,
            senderSessionId: sessionId, category: nil, text: "去把 X 做了",
            createdAt: "2027-01-15T00:00:00Z", senderName: "机长", mentions: mentions)
    }

    // ── 广播：唤醒/可见面不变 ─────────────────────────────────────────────

    func testBroadcastVisibleToEveryone() {
        let m = msg()
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-1"))
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-2"))
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "cap", isCaptain: true))
    }

    func testEmptyMentionsTreatedAsBroadcast() {
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(msg(mentions: []), to: "w-1"))
    }

    // ── 定向 @：只进被点名者的注入面 ───────────────────────────────────────

    func testDirectedSessionMentionOnlyVisibleToTarget() {
        let m = msg(mentions: [LocalWhiteboardMention(kind: "session", targetId: "w-1")])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-1"))
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "w-2"),
                       "定向派给 w-1 的活不该出现在 w-2 的注入面（#543 事故）")
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "w-3"))
    }

    func testCaptainMentionOnlyVisibleToCaptain() {
        let m = msg(mentions: [LocalWhiteboardMention(kind: "captain", targetId: nil)])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "cap", isCaptain: true))
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "w-1"))
    }

    func testHumanMentionNotVisibleToAgents() {
        let m = msg(mentions: [LocalWhiteboardMention(kind: "human", targetId: nil)])
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "w-1"),
                       "@人类 是给人看的标记，不进 agent 注入面")
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "cap", isCaptain: true))
    }

    func testSenderAlwaysSeesOwnDirectedMessage() {
        let m = msg(sessionId: "s-sender",
                    mentions: [LocalWhiteboardMention(kind: "session", targetId: "w-1")])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "s-sender"),
                      "自己发的定向 @ 在自己上下文里不该凭空消失")
    }

    func testMultiMentionVisibleToEachTarget() {
        let m = msg(mentions: [
            LocalWhiteboardMention(kind: "session", targetId: "w-1"),
            LocalWhiteboardMention(kind: "captain", targetId: nil),
        ])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-1"))
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "cap", isCaptain: true))
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "w-2"))
    }

    func testSessionMentionWithoutTargetIdVisibleToNobody() {
        let m = msg(sessionId: nil,
                    mentions: [LocalWhiteboardMention(kind: "session", targetId: nil)])
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "w-1"),
                       "缺 target_id 的定向 @ 不该退化成广播")
    }

    // ── 批量过滤保序 ──────────────────────────────────────────────────────

    func testVisibleKeepsOrderAndDropsOthers() {
        let broadcast = msg()
        let toMe = msg(mentions: [LocalWhiteboardMention(kind: "session", targetId: "w-1")])
        let toOther = msg(mentions: [LocalWhiteboardMention(kind: "session", targetId: "w-9")])
        let out = CrewWhiteboardVisibility.visible([broadcast, toOther, toMe], to: "w-1")
        XCTAssertEqual(out.map(\.id), [broadcast.id, toMe.id])
    }
}
