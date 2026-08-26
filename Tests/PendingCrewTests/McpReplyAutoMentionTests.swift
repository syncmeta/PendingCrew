import XCTest

/// `post_to_crew(reply_to:)` 的自动 @（Todo #14 ①）。
///
/// 这一笔之前，agent 路的 `reply_to` **只进了 `inReplyTo:`** —— 工具描述和世界观
/// 模板却都写着「给了会自动 @ 那条的原发送者」。危害是**静默的**：agent 以为自己
/// 点名了，实际一个 mention 都没长出来，**回执照样回「已发到」**，然后它安心等一个
/// 永远不会醒的人。
///
/// 接上的形状是 **`[broadcast, 被回复者]`**，不是裸 `session(X)` ——
/// 理由见 `CrewComposerMentions.mentionsToSend(staged:replyTo:)` 上那段注释：
/// 「人点『回复』时压根没选过『定向』，他只是在接一句话；照直挂 `session(X)` 会让
/// 每一条回复悄悄变私信。」裸 session 会**收窄可见范围**，比原来那个 bug 更糟。
///
/// **判定复用人类 composer 那个纯函数，McpServer 里没有第二份** —— 所以那两道守卫
/// （手打定向 @ 在场时不放宽 / 已显式给 broadcast 时不再叠）在 agent 路上也成立，
/// 本族各钉一条**会红**的用例。
final class McpReplyAutoMentionTests: XCTestCase {

    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("mcp-reply-at-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func server(_ dir: URL, sessionId: String = "me") -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: dir),
                  approvals: LocalApprovalStore(directory: dir),
                  control: LocalCrewControlStore(directory: dir),
                  crewId: "c", sessionId: sessionId)
    }

    /// 发一条 `post_to_crew`，返回落盘后的**最后一条**消息。
    @discardableResult
    private func post(_ s: McpServer, _ arguments: [String: Any]) -> LocalWhiteboardMessage {
        var args = arguments
        args["message"] = args["message"] ?? "接着说"
        let params: [String: Any] = ["name": "post_to_crew", "arguments": args]
        let req: [String: Any] = ["jsonrpc": "2.0", "id": 1, "method": "tools/call",
                                  "params": params]
        let line = String(data: try! JSONSerialization.data(withJSONObject: req), encoding: .utf8)!
        _ = s.handleLine(line)
        return s.store.list(crewId: "c").last!
    }

    /// 种一条别人发的消息进白板，返回它的 id（= 将来 `reply_to` 的那个值）。
    private func seedSessionMessage(_ s: McpServer, from sessionId: String) -> String {
        s.store.appendSessionMessage(crewId: "c", sessionId: sessionId,
                                     text: "谁来接一下这个", senderName: "小王")
        return s.store.list(crewId: "c").last!.id
    }

    // ── ① 接上：形状是 [broadcast, 被回复者] ───────────────────────────────

    /// 病根本身：给了 `reply_to`，就该长出 `[broadcast, session(原发送者)]`。
    /// **接上之前这条红在 `kinds` 那句** —— 落盘的 mentions 是 nil。
    func testReplyAutoMentionsBroadcastPlusRepliedTo() {
        let s = server(tempDir())
        let target = seedSessionMessage(s, from: "w-1")

        let sent = post(s, ["message": "我来", "reply_to": target])

        XCTAssertEqual(sent.mentions?.map(\.kind), ["broadcast", "session"])
        XCTAssertEqual(sent.mentions?[1].targetId, "w-1")
        XCTAssertEqual(sent.inReplyTo, target, "自动 @ 不能把 reply_to 本身弄丢")
    }

    /// 回复机长 → `[broadcast, captain]`。放宽器不挑 kind。
    func testReplyToCaptainMentionsCaptain() {
        let s = server(tempDir())
        s.store.appendSessionMessage(crewId: "c", sessionId: "cap-run", text: "看一下",
                                     senderName: "机长", senderKind: "captain")
        let target = s.store.list(crewId: "c").last!.id

        let sent = post(s, ["reply_to": target])

        XCTAssertEqual(sent.mentions?.map(\.kind), ["broadcast", "captain"])
        XCTAssertNil(sent.mentions?[1].targetId)
    }

    /// 回复人类 → `[broadcast, human(userId)]`。`human` 本来就不收窄，但**放宽器
    /// 照给**：判定只有一份，不按 kind 分叉。
    func testReplyToHumanMentionsHuman() {
        let s = server(tempDir())
        s.store.appendUserMessage(crewId: "c", text: "谁跟一下", senderName: "人")
        let target = s.store.list(crewId: "c").last!.id

        let sent = post(s, ["reply_to": target])

        XCTAssertEqual(sent.mentions?.map(\.kind), ["broadcast", "human"])
    }

    /// `reply_to` 指向一条不在白板里的 id（写错 / 已归档）→ **不编一个 @ 出来**，
    /// mentions 保持原样，`inReplyTo` 照旧记下。宁可不 @，也不 @ 错人。
    func testUnknownReplyTargetAddsNoMention() {
        let s = server(tempDir())
        let sent = post(s, ["reply_to": "no-such-id"])
        XCTAssertNil(sent.mentions)
        XCTAssertEqual(sent.inReplyTo, "no-such-id")
    }

    /// 没给 `reply_to` → 老行为一个字不动。
    func testNoReplyLeavesMentionsUntouched() {
        let s = server(tempDir())
        XCTAssertNil(post(s, [:]).mentions)
        XCTAssertEqual(
            post(s, ["mentions": [["kind": "session", "target_id": "w-9"]]]).mentions?.map(\.kind),
            ["session"])
    }

    // ── ② 两道守卫，各一条会红的用例 ──────────────────────────────────────

    /// **守卫一：手打定向 @ 在场时不放宽。** 调用方已经显式点名了 `w-9`，就别替他
    /// 把范围拉开 —— 他选的排他说了算（#543）。
    ///
    /// **不加这道守卫时，本条红在最后那句 `XCTAssertFalse(...contains("broadcast"))`
    /// 上**：`mentions` 会变成 `[broadcast, session(w-9), session(w-1)]`。
    /// 红在别处（比如 count / targetId）说明坏的不是这道守卫。
    func testExplicitDirectedMentionSuppressesReplyWidening() {
        let s = server(tempDir())
        let target = seedSessionMessage(s, from: "w-1")

        let sent = post(s, ["mentions": [["kind": "session", "target_id": "w-9"]],
                            "reply_to": target])

        XCTAssertEqual(sent.mentions?.map(\.kind), ["session", "session"])
        XCTAssertEqual(sent.mentions?.map(\.targetId), ["w-9", "w-1"])
        XCTAssertFalse(sent.mentions?.contains(where: { $0.kind == "broadcast" }) ?? true,
                       "调用方已显式点名，不许替他放宽可见范围")
    }

    /// **守卫二：调用方已显式给 `broadcast` 时不再叠。**
    ///
    /// **不加这道守卫时，本条红在第一句 `kinds` 上**：`[.broadcast] + collapsed`
    /// 会得到 `["broadcast", "broadcast", "session"]` —— 多一个重复的 broadcast。
    /// 红在别处说明坏的不是这道守卫。
    func testExplicitBroadcastIsNotDoubled() {
        let s = server(tempDir())
        let target = seedSessionMessage(s, from: "w-1")

        let sent = post(s, ["mentions": [["kind": "broadcast"]], "reply_to": target])

        XCTAssertEqual(sent.mentions?.map(\.kind), ["broadcast", "session"])
        XCTAssertEqual(sent.mentions?.count, 2)
    }

    /// 回复的对象**已经**在手打的 mentions 里 → 不重复落两条（`mentionsToSend`
    /// 的去重）。这条同时说明守卫一在场：手打了定向 @，就不放宽。
    func testRepliedToAlreadyMentionedIsNotDuplicated() {
        let s = server(tempDir())
        let target = seedSessionMessage(s, from: "w-1")

        let sent = post(s, ["mentions": [["kind": "session", "target_id": "w-1"]],
                            "reply_to": target])

        XCTAssertEqual(sent.mentions?.map(\.kind), ["session"])
        XCTAssertEqual(sent.mentions?[0].targetId, "w-1")
    }

    // ── ③ 落盘之后那一层：真的对第三方可见吗 ──────────────────────────────

    /// **函数返回了正确的数组 ≠ 那条消息真的对第三方可见。** 中间隔着
    /// `CrewWhiteboardVisibility` 一整层：它看的是**消息上**的 mentions，
    /// 而消息上有没有、是什么形状，取决于 `post_to_crew` 那一步写进去了什么。
    /// 所以这条从**落盘后的那条消息**出发，把三件事一起钉住：
    ///   1. 被回复的那个看得见（本来就该）；
    ///   2. **没被点名的第三方也看得见** —— 回复不是私信；
    ///   3. 第三方的注入面上带「（发给 …的）」，看得见也看得出不是自己的活。
    /// 只有 2 和 3 同时成立，「广播 + 叫醒」才算真的落到了 agent 路上。
    func testAgentReplyIsVisibleToBystandersAndLabelled() {
        let s = server(tempDir())
        let target = seedSessionMessage(s, from: "w-1")
        let stored = post(s, ["message": "我来", "reply_to": target])

        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(stored, to: "w-1"))
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(stored, to: "w-2"),
                      "回复不是私信 —— 没被点名的旁观者也该看得见全文")
        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(stored, to: "cap", isCaptain: true))

        XCTAssertEqual(
            CrewWhiteboardVisibility.directedNote(
                stored, to: "w-2", displayName: { $0 == "w-1" ? "小王" : nil }),
            "（发给 小王 的）",
            "旁观者要看得出这条不是自己的活")
        XCTAssertNil(CrewWhiteboardVisibility.directedNote(
            stored, to: "w-1", displayName: { _ in "小王" }))
    }

    /// **只叫醒被回复的那个** —— 广播不等于把全组都吵醒。
    func testAgentReplyWakesOnlyTheRepliedTo() {
        let s = server(tempDir())
        let target = seedSessionMessage(s, from: "w-1")
        let stored = post(s, ["message": "我来", "reply_to": target])
        let sent = (stored.mentions ?? []).map { CrewMention(kind: $0.kind, targetId: $0.targetId) }

        XCTAssertEqual(
            CrewLocalMentionInjectLogic.plannedInjections(
                mentions: sent,
                runs: [.init(sessionId: "w-1", isBusy: false),
                       .init(sessionId: "w-2", isBusy: false)],
                messageText: "我来", senderName: "我").map(\.sessionId),
            ["w-1"])
    }

    /// 反证：**同一条回复，去掉 broadcast 就成了私信。** 说明上面那条「全组可见」
    /// 是 `[broadcast, …]` 这个形状挣来的，不是碰巧 —— 也说明为什么绝不能挂裸
    /// `session(X)`。
    func testSameReplyGoesPrivateWithoutBroadcast() {
        let s = server(tempDir())
        s.store.appendSessionMessage(crewId: "c", sessionId: "me", text: "我来",
                                     mentions: [LocalWhiteboardMention(kind: "session",
                                                                       targetId: "w-1")])
        let bare = s.store.list(crewId: "c").last!

        XCTAssertTrue(CrewWhiteboardVisibility.isVisible(bare, to: "w-1"))
        XCTAssertFalse(CrewWhiteboardVisibility.isVisible(bare, to: "w-2"),
                       "裸 session 会把这条从全组眼前拿走 —— 比原来的 bug 更糟")
    }
}
