import XCTest
// SessionTurnTrace 直接编进 PendingCrewTests target，无需 import。

/// 「一轮收尾停在没 @ 到人的问句上才代发」的判定单测（人类 Todo #25 层 1）。
/// 全是纯函数：白板消息、marker、正文都由测试喂，无 IO、无 Date()。
///
/// 这批用例的头号职责是**钉住收窄**：静默轮次不发。旧版判据是「这轮往群里说过话没有」，
/// 没说话就无条件代发，把「机长在等 worker 干活」这种正常静默播成「需要人回它一句」，
/// 凭空给人造待办。谁要放宽回去，`testStaysQuietWhenSilentAndEndedOnAStatement` 先红。
final class SessionTurnTraceTests: XCTestCase {

    private func msg(_ id: String, session: String?,
                     mentions: [LocalWhiteboardMention]? = nil) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: id, senderKind: session == nil ? "human" : "session",
            senderUserId: session == nil ? "u" : nil, senderSessionId: session,
            category: nil, text: "t", createdAt: "2026-08-08T00:00:00Z",
            mentions: mentions)
    }

    // MARK: - 有没有人被叫到

    func testCallOutOnlyCountsMessagesAfterMarker() {
        let captain = [LocalWhiteboardMention(kind: "captain", targetId: nil)]
        let all = [msg("a", session: "s1", mentions: captain),
                   msg("b", session: nil), msg("c", session: nil)]
        // 上一轮结束时末条是 a → 本轮范围是 b/c，里面没有 s1 叫人 → 没叫到人。
        XCTAssertFalse(SessionTurnTrace.hasCallOutTrace(in: all, sessionId: "s1", since: "a"))
        // 从头看则 a 就是它叫人的那条。
        XCTAssertTrue(SessionTurnTrace.hasCallOutTrace(in: all, sessionId: "s1", since: nil))
    }

    func testOtherSessionsCallOutIsNotMine() {
        let all = [msg("a", session: nil),
                   msg("b", session: "other",
                       mentions: [LocalWhiteboardMention(kind: "human", targetId: nil)])]
        XCTAssertFalse(SessionTurnTrace.hasCallOutTrace(in: all, sessionId: "s1", since: "a"))
    }

    // MARK: - 收尾话头

    func testTrailingQuestionTakesTheLastSentenceOnly() {
        // 中间反问、末句是陈述 —— 不是在等人。
        XCTAssertNil(SessionTurnTrace.trailingQuestion(
            from: "我把 A 改完了。B 那块要不要一起动？然后我先跑测试。"))
        XCTAssertEqual(
            SessionTurnTrace.trailingQuestion(from: "我把 A 改完了。B 那块要不要一起动？"),
            "B 那块要不要一起动？")
    }

    func testEmptyTextYieldsNothing() {
        XCTAssertNil(SessionTurnTrace.trailingQuestion(from: "   \n  "))
    }

    // MARK: - 收尾闭合符（真事故：代发稿的正文只有一个「）」）

    /// 现场原文：正文以「（顺带一提：……不该产生任何群消息。）」收尾 → `split` 把句号
    /// 后面的 `）` 切成独立一句 → 收尾话头抽出来是个孤零零的右括号。
    func testClosingParenAfterFullStopIsNotItsOwnSentence() {
        let text = "已经改完了。\n（顺带一提：这一轮不该产生任何群消息。）"
        XCTAssertNil(SessionTurnTrace.trailingQuestion(from: text), "陈述句收尾，不该判成在等")
        XCTAssertNil(SessionTurnTrace.decide(input(
            messages: [msg("a", session: nil)], since: "a", lastTurn: nil, turn: "t1", text: text)))
    }

    /// 同一个毛病更要命的一面：问句以 `）` 收尾时末段变成 `）`，**问句判不出来** ——
    /// 收窄后这是唯一触发条件，漏判等于功能静默失灵。
    func testQuestionWrappedInParensStillCounts() {
        let text = "我先按 A 做了。（这样行吗？）"
        let q = SessionTurnTrace.trailingQuestion(from: text)
        XCTAssertNotNil(q)
        XCTAssertTrue(q!.contains("吗？"), "带出来的必须是那句问句，实得：\(q ?? "nil")")
        let post = SessionTurnTrace.decide(input(
            messages: [msg("a", session: nil)], since: "a", lastTurn: nil, turn: "t1", text: text))
        XCTAssertNotNil(post)
        XCTAssertTrue(post!.text.contains("吗？"))
    }

    func testQuestionFollowedByClosingQuoteStillCounts() {
        let q = SessionTurnTrace.trailingQuestion(from: "他问：“这样行吗？”")
        XCTAssertNotNil(q)
        XCTAssertTrue(q!.contains("吗？"), "实得：\(q ?? "nil")")
    }

    /// 剥完只剩标点/空白 → 没有可发的收尾话头，一律不发。
    func testPunctuationOnlyBodyYieldsNothing() {
        for text in ["）", "。）", "？", " 」 ", "…"] {
            XCTAssertNil(SessionTurnTrace.trailingQuestion(from: text), "不该从「\(text)」抽出话头")
            XCTAssertNil(SessionTurnTrace.decide(input(
                messages: [msg("a", session: nil)], since: "a", lastTurn: nil, turn: "t1",
                text: text)), "不该为「\(text)」代发")
        }
    }

    func testTruncatesLongQuestion() {
        let long = String(repeating: "字", count: 400) + "？"
        let out = SessionTurnTrace.trailingQuestion(from: long, limit: 10)
        XCTAssertEqual(out, String(repeating: "字", count: 10) + "…")
    }

    // MARK: - 选靶（#541 成环坑：绝不 @ 自己）

    func testWorkerMentionsCaptainCaptainMentionsHuman() {
        XCTAssertEqual(
            SessionTurnTrace.post(sessionName: "w", sessionId: "s", isCaptain: false, closing: "在等？")
                .mentionKinds, ["captain"])
        XCTAssertEqual(
            SessionTurnTrace.post(sessionName: "机长", sessionId: "s", isCaptain: true, closing: "在等？")
                .mentionKinds, ["human"])
    }

    // MARK: - 整轮判定

    private func input(
        messages: [LocalWhiteboardMessage], since: String?, lastTurn: String?, turn: String?,
        text: String, isCaptain: Bool = false
    ) -> SessionTurnTrace.Input {
        .init(messages: messages, sessionId: "s1", sessionName: "干活的", isCaptain: isCaptain,
              sinceMessageId: since, lastHandledTurnId: lastTurn, turnId: turn,
              lastAssistantMessage: text)
    }

    /// **收窄的核心用例**：这一轮一个字都没往群里说，但收尾是陈述句 —— 静默是正常状态，
    /// 不发。旧版在这里会无条件代发一条「没往群里说过话…需要人回它一句」。
    func testStaysQuietWhenSilentAndEndedOnAStatement() {
        XCTAssertNil(SessionTurnTrace.decide(input(
            messages: [msg("a", session: nil)], since: "a", lastTurn: nil, turn: "t1",
            text: "worker 说收到了，我等它干完。")))
    }

    /// 静默 + 停在问句 —— 这才是 #25 当初唯一要治的病：既没调 ask、画面上也没有菜单，
    /// 群里一个字都没有，人和机长都不知道它在等。
    func testPostsWhenSilentAndEndedOnAQuestion() {
        let post = SessionTurnTrace.decide(input(
            messages: [msg("a", session: nil)], since: "a", lastTurn: nil, turn: "t1",
            text: "这块要走哪条路？"))
        XCTAssertNotNil(post)
        XCTAssertTrue(post!.text.contains("这块要走哪条路？"))
        XCTAssertTrue(post!.text.contains("（session: s1）"))
    }

    /// 说过话、但广播的那句谁都没 @ ——界面这时会把它标红等回复，群里却没人被叫到，
    /// 等于只红给自己看。这种仍要补一条并 @ 到人。
    func testPostsWhenItSpokeButCalledNobodyAndStoppedOnAQuestion() {
        let post = SessionTurnTrace.decide(input(
            messages: [msg("a", session: nil), msg("b", session: "s1")], since: "a",
            lastTurn: nil, turn: "t1", text: "A 做完了。要不要接着做 B？"))
        XCTAssertNotNil(post)
        XCTAssertEqual(post?.mentionKinds, ["captain"])
    }

    func testStaysQuietWhenItAlreadyCalledTheCaptainOut() {
        XCTAssertNil(SessionTurnTrace.decide(input(
            messages: [msg("a", session: nil),
                       msg("b", session: "s1",
                           mentions: [LocalWhiteboardMention(kind: "captain", targetId: nil)])],
            since: "a", lastTurn: nil, turn: "t1", text: "这块要走哪条路？")))
    }

    func testStaysQuietWhenItAlreadyCalledAHumanOut() {
        XCTAssertNil(SessionTurnTrace.decide(input(
            messages: [msg("a", session: nil),
                       msg("b", session: "s1",
                           mentions: [LocalWhiteboardMention(kind: "human", targetId: nil)])],
            since: "a", lastTurn: nil, turn: "t1", text: "这块要走哪条路？", isCaptain: true)))
    }

    /// 只 @ 了别的 session 不算「叫到了能答的人」—— 那个 session 拍不了板。
    func testMentioningOnlyAnotherSessionStillCountsAsCallingNobody() {
        XCTAssertNotNil(SessionTurnTrace.decide(input(
            messages: [msg("a", session: nil),
                       msg("b", session: "s1",
                           mentions: [LocalWhiteboardMention(kind: "session", targetId: "s2")])],
            since: "a", lastTurn: nil, turn: "t1", text: "要不要接着做 B？")))
    }

    /// 说过话且停在陈述句上 —— 正常收尾，别补。
    func testStaysQuietWhenItSpokeAndDidNotEndOnAQuestion() {
        XCTAssertNil(SessionTurnTrace.decide(input(
            messages: [msg("a", session: nil), msg("b", session: "s1")], since: "a",
            lastTurn: nil, turn: "t1", text: "都做完了，已经合进 main。")))
    }

    /// 中途反问然后自己接着干完的，不是在等人。
    func testStaysQuietWhenTheQuestionIsMidBody() {
        XCTAssertNil(SessionTurnTrace.decide(input(
            messages: [msg("a", session: nil)], since: "a", lastTurn: nil, turn: "t1",
            text: "这样对吗？我先按 A 做了。")))
    }

    func testSameTurnPostsOnlyOnce() {
        XCTAssertNil(SessionTurnTrace.decide(input(
            messages: [msg("a", session: nil)], since: "a", lastTurn: "t1", turn: "t1",
            text: "这块要走哪条路？")))
    }

    func testNothingToSayStaysQuiet() {
        XCTAssertNil(SessionTurnTrace.decide(input(
            messages: [msg("a", session: nil)], since: "a", lastTurn: nil, turn: "t1", text: "")))
    }
}
