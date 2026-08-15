#if os(macOS)
import XCTest
// CrewLocalMentionWakeLogic + CrewModels 直接编进 PendingCrewTests target，无需 import。

/// 后台 mention 唤醒器纯扫描核心（`CrewLocalMentionWakeLogic.pending`）的单测
/// （wake-resilience 根因修复）：「session/机长发的定向 @ → 待唤醒」「人类条目
/// 跳过（composer 路已直投）」「broadcast/human mention 不唤醒」「system 条目
/// 唤醒但免回执」。
final class CrewLocalMentionWakeLogicTests: XCTestCase {

    private func entry(
        _ senderKind: String, sessionId: String? = nil, text: String = "正文",
        name: String? = nil, mentions: [LocalWhiteboardMention]? = nil,
        relayRemoteId: String? = nil
    ) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: UUID().uuidString.lowercased(), senderKind: senderKind,
            senderUserId: nil, senderSessionId: sessionId, category: nil,
            // 时间戳必须是「刚写的」——#595 之后 `pending` 带一道陈旧 @ 不唤醒的闸，
            // 固定的历史日期会随时间推移变成陈旧条目而全组失灵。那道闸本身的边界
            // 单测在 `CrewLocalMentionWakeStalenessTests`。
            text: text, createdAt: ISO8601DateFormatter().string(from: Date()),
            relayRemoteId: relayRemoteId,
            senderName: name, mentions: mentions)
    }

    func testCaptainDirectedMentionBecomesPendingDelivery() {
        let e = entry("captain", sessionId: "cap-1", text: "开工吧", name: "机长",
                      mentions: [LocalWhiteboardMention(kind: "session", targetId: "sess-a")])
        let out = CrewLocalMentionWakeLogic.pending(entries: [e])
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out.first?.entryId, e.id)
        XCTAssertEqual(out.first?.mentions, [.session("sess-a")])
        XCTAssertEqual(out.first?.messageText, "开工吧")
        XCTAssertEqual(out.first?.senderName, "机长")
        XCTAssertEqual(out.first?.senderSessionId, "cap-1")
        XCTAssertTrue(out.first!.trackReceipt)
    }

    func testSessionAtCaptainKept() {
        let e = entry("session", sessionId: "sess-b",
                      mentions: [LocalWhiteboardMention(kind: "captain", targetId: nil)])
        let out = CrewLocalMentionWakeLogic.pending(entries: [e])
        XCTAssertEqual(out.first?.mentions, [.captain])
    }

    func testHumanEntrySkipped() {
        // 本地 composer 直发的 user 条目（relayRemoteId == nil）：CrewChatView
        // 已直投 —— 后台唤醒器不重复。
        let e = entry("user", mentions: [LocalWhiteboardMention(kind: "session", targetId: "sess-a")])
        XCTAssertTrue(CrewLocalMentionWakeLogic.pending(entries: [e]).isEmpty)
    }

    func testRelayHumanEntryWithDirectedMentionProducesDelivery() {
        // #554 断链修复：远端 iOS 用户 @session 落到 Mac 本地白板
        // （relayRemoteId != nil）—— 没有 composer 直投过，唤醒器要补上这条投递。
        let e = entry("user", name: "PendingBot 用户",
                      mentions: [LocalWhiteboardMention(kind: "session", targetId: "sess-a")],
                      relayRemoteId: "edge-msg-1")
        let out = CrewLocalMentionWakeLogic.pending(entries: [e])
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out.first?.entryId, e.id)
        XCTAssertEqual(out.first?.mentions, [.session("sess-a")])
        XCTAssertEqual(out.first?.senderName, "PendingBot 用户")
        XCTAssertTrue(out.first!.trackReceipt)
    }

    func testRelayHumanEntryWithoutMentionsSkipped() {
        // relay 落地的 user 条目但无定向 @（广播/纯文本）—— 不该唤醒具体 run。
        let e = entry("user", relayRemoteId: "edge-msg-2")
        XCTAssertTrue(CrewLocalMentionWakeLogic.pending(entries: [e]).isEmpty)
    }

    func testNoMentionsOrNonWakeKindsSkipped() {
        let plain = entry("session", sessionId: "s1")
        let broadcastish = entry("session", sessionId: "s1", mentions: [
            LocalWhiteboardMention(kind: "human", targetId: "u1"),
            LocalWhiteboardMention(kind: "broadcast", targetId: nil),
        ])
        let emptyTarget = entry("session", sessionId: "s1", mentions: [
            LocalWhiteboardMention(kind: "session", targetId: nil),
        ])
        XCTAssertTrue(CrewLocalMentionWakeLogic
            .pending(entries: [plain, broadcastish, emptyTarget]).isEmpty)
    }

    func testSystemEntryWakesWithoutReceipt() {
        // 审批通告 / 唤醒失败告警：senderKind "session" + sessionId "system"。
        // 要能唤醒机长，但不做回执判定（告警条目自身回执失败会再发告警 → 环）。
        let e = entry("session", sessionId: "system", name: "系统",
                      mentions: [LocalWhiteboardMention(kind: "captain", targetId: nil)])
        let out = CrewLocalMentionWakeLogic.pending(entries: [e])
        XCTAssertEqual(out.count, 1)
        XCTAssertFalse(out.first!.trackReceipt)
        XCTAssertNil(out.first!.senderSessionId)
    }

    func testSenderLabelFallbacks() {
        // 无显示名的 captain → 「机长」；无名 session → session:<前6>。
        let cap = entry("captain", sessionId: "cap-1",
                        mentions: [LocalWhiteboardMention(kind: "session", targetId: "x")])
        let sess = entry("session", sessionId: "abcdef123456",
                         mentions: [LocalWhiteboardMention(kind: "session", targetId: "x")])
        let out = CrewLocalMentionWakeLogic.pending(entries: [cap, sess])
        XCTAssertEqual(out[0].senderName, "机长")
        XCTAssertEqual(out[1].senderName, "session:abcdef")
    }

    // 通讯录 contact（2026-08-11）：外线按 crew 号打进来的**广播**（不带 mentions）
    // 必须唤醒目标机长 —— 与「人类在群里无 @ 发言默认当 @机长」同语义。
    func testExternalContactBroadcastWakesCaptain() {
        var e = entry("session", sessionId: "worker-abc", text: "你们那边的发布脚本能复用吗？",
                      name: "PendingCrew · 1-2")
        e.externalContactFrom = "1-2"
        let out = CrewLocalMentionWakeLogic.pending(entries: [e])
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out.first?.mentions, [.captain])
        XCTAssertEqual(out.first?.senderName, "PendingCrew · 1-2")
    }

    // 外线来电带定向 @ 时按定向走，不额外扯上机长。
    func testExternalContactWithDirectedMentionKeepsThatTarget() {
        var e = entry("session", sessionId: "worker-abc",
                      mentions: [LocalWhiteboardMention(kind: "session", targetId: "sess-a")])
        e.externalContactFrom = "1-2"
        XCTAssertEqual(CrewLocalMentionWakeLogic.pending(entries: [e]).first?.mentions,
                       [.session("sess-a")])
    }

    // 本群成员之间的普通广播语义不变 —— 仍然不唤醒任何人。
    func testPlainBroadcastStillDoesNotWake() {
        let e = entry("session", sessionId: "s1", text: "我开始了")
        XCTAssertTrue(CrewLocalMentionWakeLogic.pending(entries: [e]).isEmpty)
    }

    func testMixedMentionsKeepOnlyWakeKinds() {
        let e = entry("session", sessionId: "s1", mentions: [
            LocalWhiteboardMention(kind: "human", targetId: "u1"),
            LocalWhiteboardMention(kind: "captain", targetId: nil),
            LocalWhiteboardMention(kind: "session", targetId: "sess-a"),
        ])
        let out = CrewLocalMentionWakeLogic.pending(entries: [e])
        XCTAssertEqual(out.first?.mentions, [.captain, .session("sess-a")])
    }
}
#endif
