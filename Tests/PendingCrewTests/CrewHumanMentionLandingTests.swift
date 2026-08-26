import XCTest

/// 人类发的 mentions 从「收了就扔」到真落盘（Todo #62 ③，走机长拍的 C 案）。
///
/// 在这一笔之前，`LocalBackend.postCrewMessage` 收下 `mentions` 却一个字不写进消息 ——
/// @ 只喂给了旁边那条直投唤醒链。后果有两层：
///   1. 人类 @ 谁都是全组可见（`CrewWhiteboardVisibility` 看的是**消息上**的
///      mentions，而消息上压根没有），composer 的「全体」（`broadcast`）在这条路上
///      从来没落过盘；
///   2. 于是 A 线做好的可见性 + 注入面消歧对人类发的消息一概不生效。
///
/// C 案的两条语义，本族各钉一条：
///   * **「回复」按钮的自动 @ = 广播 + 叫醒**（人点回复时没选过定向，照直改会让
///     每条回复悄悄变私信）；
///   * **手打的 `@小王` 保持排他**（#543，人是主动选的）。
final class CrewHumanMentionLandingTests: XCTestCase {

    private func wbMsg(_ mentions: [CrewMention]) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: UUID().uuidString, senderKind: "user", senderUserId: "local-byok-user",
            senderSessionId: nil, category: nil, text: "接着说",
            createdAt: "2027-02-01T00:00:00Z", senderName: "人",
            mentions: mentions.map(LocalWhiteboardMention.init))
    }

    // ── 纯逻辑：两种来源，两种语义 ─────────────────────────────────────────

    /// 「回复」按钮 = 广播 + 叫醒。没有手打的 @ 时，自动 @ 被放宽成
    /// `[broadcast, 被回复者]` —— 全组看得见，只叫醒他。
    func testReplyAutoMentionBecomesBroadcastPlusTarget() {
        XCTAssertEqual(
            CrewComposerMentionParser.mentionsToSend(staged: [], replyTo: .session("w-1")),
            [.broadcast, .session("w-1")])
    }

    /// 回复机长同理 —— 放宽器不挑 kind。
    func testReplyToCaptainAlsoWidens() {
        XCTAssertEqual(
            CrewComposerMentionParser.mentionsToSend(staged: [], replyTo: .captain),
            [.broadcast, .captain])
    }

    /// 手打的 `@小王` **排他**，一个 broadcast 都不许自己长出来（#543 一个字不松）。
    func testHandTypedMentionStaysExclusive() {
        XCTAssertEqual(
            CrewComposerMentionParser.mentionsToSend(
                staged: [CrewStagedMention(token: "@小王", mention: .session("w-1"))],
                replyTo: nil),
            [.session("w-1")])
    }

    /// 手打 @ **和**回复同时在场 → 人已经明确选了排他，不许被一个他没点过的自动 @
    /// 悄悄放宽。想两者兼得就点一下「全体」（下一条）。
    func testHandTypedMentionSuppressesReplyWidening() {
        let sent = CrewComposerMentionParser.mentionsToSend(
            staged: [CrewStagedMention(token: "@小王", mention: .session("w-1"))],
            replyTo: .session("w-2"))
        XCTAssertEqual(sent, [.session("w-1"), .session("w-2")])
        XCTAssertFalse(sent.contains(.broadcast), "人自己选的排他说了算")
    }

    /// 点了「全体」再回复 → 已经有 broadcast，不重复塞第二个。
    func testExplicitBroadcastNotDuplicatedByReply() {
        XCTAssertEqual(
            CrewComposerMentionParser.mentionsToSend(
                staged: [CrewStagedMention(token: "@全体", mention: .broadcast)],
                replyTo: .session("w-1")),
            [.broadcast, .session("w-1")])
    }

    /// 不在回复 → 老行为一个字不动（含「什么都没选 = 广播」这一档）。
    func testNoReplyLeavesStagedUntouched() {
        XCTAssertEqual(CrewComposerMentionParser.mentionsToSend(staged: [], replyTo: nil), [])
    }

    // ── 串起来：可见性 + 唤醒面 ────────────────────────────────────────────

    /// 一条回复：**全组看得见**、**只叫醒被回复的那个**、旁观者**看得出不是自己的活**。
    /// 三条判定各自独立，一起钉住才叫「广播 + 叫醒」真的成立。
    func testReplyIsVisibleToAllButWakesOnlyTheRepliedTo() {
        let sent = CrewComposerMentionParser.mentionsToSend(staged: [], replyTo: .session("w-1"))
        let stored = wbMsg(sent)

        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(stored, to: "w-1"))
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(stored, to: "w-2"),
                      "回复不是私信 —— 旁观者也该看得见")
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(stored, to: "cap", isCaptain: true))

        XCTAssertEqual(
            CrewLocalMentionInjectLogic.plannedInjections(
                mentions: sent,
                runs: [.init(sessionId: "w-1", isBusy: false),
                       .init(sessionId: "w-2", isBusy: false)],
                messageText: "接着说", senderName: "人").map(\.sessionId),
            ["w-1"], "只叫醒被回复的那个")

        XCTAssertEqual(
            CrewWhiteboardVisibility.directedNote(
                stored, to: "w-2", displayName: { $0 == "w-1" ? "小王" : nil }),
            "（发给 小王 的）",
            "旁观者要看得出这条不是自己的活")
        XCTAssertNil(CrewWhiteboardVisibility.directedNote(
            stored, to: "w-1", displayName: { _ in "小王" }))
    }

    /// 反证：**去掉 broadcast**，同一条回复旁观者就看不见了 —— 说明「全组可见」
    /// 是放宽器挣来的，不是碰巧。
    func testWithoutBroadcastTheSameReplyGoesPrivate() {
        let stored = wbMsg([.session("w-1")])
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(stored, to: "w-1"))
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(stored, to: "w-2"))
    }

    // ── 落盘：这一族的病根 ────────────────────────────────────────────────

    /// `appendUserMessage` 真把 mentions 写进消息 —— 在这一笔之前**这个形参根本
    /// 不存在**，人类发的每一条 mentions 都是 nil。落盘保真是上面所有判定的前提：
    /// 存不进去，可见性/消歧/唤醒读的都是空。
    func testUserMessageMentionsArePersisted() {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("human-mentions-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let store = LocalWhiteboardStore(directory: dir)
        let sent = CrewComposerMentionParser.mentionsToSend(staged: [], replyTo: .session("w-1"))

        store.appendUserMessage(crewId: "c", text: "接着说", senderName: "人",
                                mentions: sent.map(LocalWhiteboardMention.init))

        let rows = store.list(crewId: "c")
        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows[0].mentions?.map(\.kind), ["broadcast", "session"])
        XCTAssertEqual(rows[0].mentions?[1].targetId, "w-1")
        // 存下来的那条，可见性判定才吃得到 —— 这是「真落盘」和「看着像落了」的分界。
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(rows[0], to: "w-2"))
    }

    /// 什么都没 @ → 落 nil，不用空数组占位（与 `mentions` 字段的「无 @」语义对齐）。
    func testNoMentionsStillLandsAsNil() {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("human-mentions-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let store = LocalWhiteboardStore(directory: dir)
        store.appendUserMessage(crewId: "c", text: "都看一下", senderName: "人", mentions: [])
        XCTAssertNil(store.list(crewId: "c")[0].mentions)
    }

    /// 手打 @ 的那条**确实**是私信 —— 排他不是说说而已。
    func testHandTypedMentionReallyHidesFromOthers() {
        let sent = CrewComposerMentionParser.mentionsToSend(
            staged: [CrewStagedMention(token: "@小王", mention: .session("w-1"))],
            replyTo: nil)
        let stored = wbMsg(sent)
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(stored, to: "w-1"))
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(stored, to: "w-2"),
                       "人主动选的定向就该是定向（#543）")
    }
}
