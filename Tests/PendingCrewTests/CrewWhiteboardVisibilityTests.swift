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

    /// 2026-08-23 修的正主：`@人类` 过去落在 `default: return false` 上，于是一条
    /// 只 @ 了人类的消息对**每一个** agent 隐身 —— 队友「@人 我做完了」发出去，全
    /// crew 没人看得见。human 是附加标记，不收窄可见范围。
    func testHumanOnlyMentionStillVisibleToEveryAgent() {
        let m = msg(mentions: [LocalWhiteboardMention(kind: "human", targetId: nil)])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-1"),
                      "只 @ 人类的消息不该从第三方 session 眼前消失")
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-2"))
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "cap", isCaptain: true),
                      "机长也不该看不见「@人 汇报…」")
    }

    /// 显式 broadcast mention 同样不收窄（mentions 非空 ≠ 定向）。
    func testBroadcastMentionStillVisibleToEveryone() {
        let m = msg(mentions: [LocalWhiteboardMention(kind: "broadcast", targetId: nil)])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-1"))
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "cap", isCaptain: true))
    }

    /// #543 不许回退：混了 human 也不放宽 session 的排他性。
    func testSessionPlusHumanStaysSessionExclusive() {
        let m = msg(mentions: [
            LocalWhiteboardMention(kind: "session", targetId: "w-1"),
            LocalWhiteboardMention(kind: "human", targetId: nil),
        ])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "w-1"))
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "w-2"),
                       "@session 配 @human 仍按 session 排他（#543 一个字不松）")
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "cap", isCaptain: true))
    }

    /// 同理 `@captain + @human`。
    func testCaptainPlusHumanStaysCaptainExclusive() {
        let m = msg(mentions: [
            LocalWhiteboardMention(kind: "captain", targetId: nil),
            LocalWhiteboardMention(kind: "human", targetId: nil),
        ])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(m, to: "cap", isCaptain: true))
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(m, to: "w-1"))
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

#if os(macOS)
/// 「可见」与「该叫醒」是两件事 —— 2026-08-23 放宽 human 的**可见性**时，这两条钉住
/// 唤醒面 / 收听面**一个字没变**。它们打在别的判据上（`CrewWhiteboardVisibility` 不参与），
/// 放这里是因为它们守的是同一条语义线：看得见 ≠ 该被它叫醒。
final class CrewHumanMentionWakeBoundaryTests: XCTestCase {

    private func whiteboardMsg(
        mentions: [LocalWhiteboardMention]?, senderKind: String = "session",
        sessionId: String? = "s-sender"
    ) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: UUID().uuidString, senderKind: senderKind, senderUserId: nil,
            senderSessionId: sessionId, category: nil, text: "@人 我做完了",
            createdAt: "2027-01-15T00:00:00Z", senderName: "worker", mentions: mentions)
    }

    /// 只 @ 人类 → 不唤醒任何 run（本地直投路）。
    func testHumanOnlyMentionWakesNobody() {
        let runs = [
            CrewLocalMentionInjectLogic.RunState(sessionId: "w-1", isBusy: false),
            CrewLocalMentionInjectLogic.RunState(sessionId: "cap", isBusy: false),
        ]
        let out = CrewLocalMentionInjectLogic.plannedInjections(
            mentions: [CrewMention(kind: "human", targetId: nil)],
            runs: runs, messageText: "@人 我做完了", senderName: "worker",
            captainSessionId: "cap")
        XCTAssertTrue(out.isEmpty, "@人类 只是标记，不该叫醒任何 run（可见 ≠ 该叫醒）")
    }

    /// 只 @ 人类 → 也不该把缺席的 session / 机长拉起来。
    func testHumanOnlyMentionSpawnsNobody() {
        let t = CrewLocalMentionInjectLogic.wakeTargets(
            mentions: [CrewMention(kind: "human", targetId: nil)],
            runningSessionIds: [], captainRunning: false)
        XCTAssertFalse(t.needCaptain)
        XCTAssertTrue(t.sessionIds.isEmpty)
    }

    /// 只 @ 人类 → 不投给收听者。这是**刻意的降噪**（人类没有直投链，去重的理由对它
    /// 不成立）：`@人 汇报…` 不值得把全 crew 正在 listen 的 session 全叫醒一遍。
    /// 谁若本着「human 不再是定向」的精神把 `deliverable` 那一支顺手删了，这条会红。
    func testHumanOnlyMentionNotDeliveredToListener() {
        let l = CrewListenLogic.Listener(
            sessionId: "s-listen", until: Date(timeIntervalSince1970: 1_800_000_600),
            senders: nil)
        let m = whiteboardMsg(mentions: [LocalWhiteboardMention(kind: "human", targetId: nil)])
        XCTAssertFalse(CrewListenLogic.deliverable(m, to: l),
                       "human-only 不投给收听者是刻意降噪，不许被顺手改掉")
        // 对照：纯广播照送，证明上面那条 false 来自 human 那一支而不是别的门。
        XCTAssertTrue(CrewListenLogic.deliverable(whiteboardMsg(mentions: nil), to: l))
    }
}
#endif
