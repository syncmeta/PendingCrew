#if os(macOS)
import XCTest
// Sources compiled directly into the test bundle (see project.yml) — no module import needed.

/// Todo #61「只看 @ 我的消息」的判定核心。核心事实：`@人类` 有**结构化 mentions**
/// 和**只写在正文里**两种存法，本机真实白板上并集 712 条、单认一边分别漏 134 / 355 条，
/// 所以判定必须取并集。中文没有词边界，还要防「人」误伤「人机交互组」。
final class CrewMentionFilterTests: XCTestCase {

    // MARK: - Fixtures

    private let human = "人"
    private let captain = "机长"
    private let peer = "加载更早保持位置"

    /// 常规花名册：一个人类 + 机长 + 一个 session。
    private var roster: CrewMentionFilter.Roster {
        .init(humanNames: [human], otherNames: [captain, peer])
    }

    private func entry(
        id: String = "e1", text: String, mentionKinds: [String] = []
    ) -> CrewWhiteboardEntry {
        CrewWhiteboardEntry(
            id: id, senderKind: "session", senderSessionId: "s1", senderUserId: nil,
            senderBotId: nil, messageKind: "instruction", summary: text,
            createdAt: "2026-01-01T00:00:00Z",
            payload: CrewWhiteboardEntry.Payload(
                text: text, kind: nil, question: nil, status: nil, permissionRequestId: nil,
                action: nil, taskBrief: nil, runnerKind: nil),
            attachments: nil, relay: nil, senderDisplayName: nil, senderMemberId: nil,
            inReplyTo: nil,
            mentions: mentionKinds.isEmpty
                ? nil : mentionKinds.map { CrewMention(kind: $0, targetId: nil) })
    }

    // MARK: - 1) 只有结构化 → 命中

    func testStructuredHumanMentionOnly() {
        let e = entry(text: "活干完了，结论在下面。", mentionKinds: ["human"])
        XCTAssertTrue(CrewMentionFilter.isHumanMention(e, roster: roster))
    }

    /// 结构化 human mention 在真实数据里**恒无 target_id**（578/578）。判定不许
    /// 依赖它，否则 agent 侧发的那 578 条会被整体滤掉。
    func testStructuredHumanMentionHitsWithoutTargetId() {
        let e = entry(text: "没写名字的正文", mentionKinds: ["human"])
        XCTAssertNil(e.mentions?.first?.targetId)
        XCTAssertTrue(CrewMentionFilter.isHumanMention(e, roster: roster))
    }

    /// 定向 @session / @captain 不算 @ 人类。
    func testNonHumanStructuredMentionMisses() {
        let e = entry(text: "交接给你了", mentionKinds: ["session", "captain"])
        XCTAssertFalse(CrewMentionFilter.isHumanMention(e, roster: roster))
    }

    // MARK: - 2) 只有正文 `@人` → 命中

    func testBodyMentionOnly() {
        let e = entry(text: "@人 这条要你拍板")
        XCTAssertNil(e.mentions)
        XCTAssertTrue(CrewMentionFilter.isHumanMention(e, roster: roster))
    }

    // MARK: - 3) 两者都有 → 命中一次，不重复计数

    func testBothHitsOnceInFilteredArray() {
        let e = entry(text: "@人 看一下", mentionKinds: ["human"])
        XCTAssertTrue(CrewMentionFilter.isHumanMention(e, roster: roster))
        XCTAssertEqual(CrewMentionFilter.onlyHumanMentions([e], roster: roster).count, 1)
    }

    // MARK: - 4) 正文 @ 的是别人的名字 → 不该命中

    func testBodyMentionOfOthersMisses() {
        for text in ["@\(captain) 帮我看下", "@\(peer) 你先别动 CrewChatView", "@PendingCrew·机长 汇报"] {
            XCTAssertFalse(
                CrewMentionFilter.isHumanMention(entry(text: text), roster: roster),
                "不该命中：\(text)")
        }
    }

    /// 花名册里查无此名的 `@xxx` 也不算 @ 人类。
    func testBodyMentionOfUnknownNameMisses() {
        XCTAssertFalse(CrewMentionFilter.isHumanMention(entry(text: "@某个不存在的人物 在吗"), roster: roster))
    }

    // MARK: - 5) `@人` 出现在句中而不是开头 → 该命中

    func testBodyMentionMidSentence() {
        let e = entry(text: "这条我改完了，细节 @人 你自己看，别的先不动。")
        XCTAssertTrue(CrewMentionFilter.isHumanMention(e, roster: roster))
    }

    func testBodyMentionAtVeryEnd() {
        XCTAssertTrue(CrewMentionFilter.isHumanMention(entry(text: "需要拍板 @人"), roster: roster))
    }

    // MARK: - 6) 前缀误伤：`@人机交互组` 不该被「人」吃掉

    func testLongerRosterNameWinsOverHumanPrefix() {
        let r = CrewMentionFilter.Roster(
            humanNames: [human], otherNames: [captain, peer, "人机交互组"])
        XCTAssertFalse(
            CrewMentionFilter.isHumanMention(entry(text: "@人机交互组 你们评估一下"), roster: r),
            "@人机交互组 是另一个成员，不该算 @ 人类")
        // 同一份花名册下，真正的 @人 仍然命中。
        XCTAssertTrue(CrewMentionFilter.isHumanMention(entry(text: "@人 这条要你拍板"), roster: r))
        // 两个都出现时，@人 那一处照样命中。
        XCTAssertTrue(
            CrewMentionFilter.isHumanMention(entry(text: "@人机交互组 抄送 @人"), roster: r))
    }

    /// 已知取舍：花名册里**没有**「人事」时，`@人事` 的最长匹配仍是「人」→ 命中。
    /// 花名册之外的词无从判定，这里选择宁可多收一条也不漏。
    func testUnknownLongerWordStillHitsHumanPrefix() {
        XCTAssertTrue(CrewMentionFilter.isHumanMention(entry(text: "@人事 那边的流程"), roster: roster))
    }

    // MARK: - 数组形状（下一轮 CrewChatWindow 直接喂它）

    func testOnlyHumanMentionsKeepsInputOrder() {
        let list = [
            entry(id: "a", text: "普通进展一"),
            entry(id: "b", text: "@人 拍板一下"),
            entry(id: "c", text: "@\(captain) 汇报"),
            entry(id: "d", text: "结构化那条", mentionKinds: ["human"]),
            entry(id: "e", text: "普通进展二"),
        ]
        XCTAssertEqual(
            CrewMentionFilter.onlyHumanMentions(list, roster: roster).map(\.id), ["b", "d"])
    }

    func testEmptyHumanNamesDisablesBodyMatchingOnly() {
        let r = CrewMentionFilter.Roster(humanNames: [], otherNames: [captain])
        XCTAssertFalse(CrewMentionFilter.isHumanMention(entry(text: "@人 在吗"), roster: r))
        XCTAssertTrue(
            CrewMentionFilter.isHumanMention(entry(text: "无名", mentionKinds: ["human"]), roster: r))
    }

    // MARK: - 自己发的消息一并留下（默认行为）

    func testKeepsOwnMessagesWhenLocalUserIdGiven() {
        let mine = CrewWhiteboardEntry(
            id: "mine", senderKind: "user", senderSessionId: nil, senderUserId: "u1",
            senderBotId: nil, messageKind: "instruction", summary: "我说的一句话",
            createdAt: "2026-01-01T00:00:00Z",
            payload: nil, attachments: nil, relay: nil, senderDisplayName: nil,
            senderMemberId: nil, inReplyTo: nil, mentions: nil)
        // 没 @ 任何人、也不是 @ 我 —— 但它是我自己发的。
        XCTAssertFalse(CrewMentionFilter.isHumanMention(mine, roster: roster))
        XCTAssertEqual(
            CrewMentionFilter.onlyHumanMentions(
                [mine, entry(text: "别人的闲聊")], roster: roster, includingFrom: "u1").map(\.id),
            ["mine"])
        // 不传 localUserId → 纯粹只看被 @ 的，自己发的也不留。
        XCTAssertTrue(
            CrewMentionFilter.onlyHumanMentions([mine], roster: roster).isEmpty)
        // 别人的 user 消息不算「我自己发的」。
        XCTAssertTrue(
            CrewMentionFilter.onlyHumanMentions([mine], roster: roster, includingFrom: "u2").isEmpty)
    }

    // MARK: - 渲染窗口按**筛选后**的列表算（下一轮的雷：显示还有 N 条、点开什么都没有）

    func testWindowCountsFollowFilteredList() {
        // 40 条里只有 3 条与人类有关。
        var all: [CrewWhiteboardEntry] = []
        for i in 0..<40 {
            all.append(entry(id: "n\(i)", text: i % 13 == 0 ? "@人 第 \(i) 条" : "第 \(i) 条闲聊"))
        }
        let filtered = CrewMentionFilter.onlyHumanMentions(all, roster: roster)
        XCTAssertEqual(filtered.count, 4)   // i = 0/13/26/39

        // 未筛选时：一页 12 条，上面还有 28 条。
        XCTAssertTrue(CrewChatWindow.hasMore(total: all.count, limit: CrewChatWindow.pageSize))
        XCTAssertEqual(
            CrewChatWindow.remaining(total: all.count, limit: CrewChatWindow.pageSize), 28)

        // 筛选后：4 条不足一页 —— 「加载更早」必须消失、剩余数必须是 0。
        // 若窗口仍按未筛选的 40 条算，这里会写着「上面还有 28 条」而点开空空如也。
        XCTAssertFalse(
            CrewChatWindow.hasMore(total: filtered.count, limit: CrewChatWindow.pageSize))
        XCTAssertEqual(
            CrewChatWindow.remaining(total: filtered.count, limit: CrewChatWindow.pageSize), 0)
        XCTAssertEqual(
            CrewChatWindow.window(filtered, limit: CrewChatWindow.pageSize).count, 4)
        XCTAssertEqual(
            CrewChatWindow.insertedAbove(total: filtered.count, limit: CrewChatWindow.pageSize), 0)
    }

    /// 筛选后仍然长过一页时，`anchorOnExpand` 钉的是**筛选后**窗口的首条。
    func testExpandAnchorFollowsFilteredList() {
        var all: [CrewWhiteboardEntry] = []
        for i in 0..<60 { all.append(entry(id: "n\(i)", text: i % 2 == 0 ? "@人 \(i)" : "闲聊 \(i)")) }
        let filtered = CrewMentionFilter.onlyHumanMentions(all, roster: roster)
        XCTAssertEqual(filtered.count, 30)
        let anchor = CrewChatWindow.anchorOnExpand(
            CrewChatWindow.window(filtered, limit: CrewChatWindow.pageSize),
            limit: CrewChatWindow.pageSize, isFollowing: false)
        // 筛后 30 条的最后 12 条，首条是 filtered[18] —— 即原列表的 n36。
        XCTAssertEqual(anchor?.id, "n36")
        XCTAssertEqual(
            CrewChatWindow.insertedAbove(total: filtered.count, limit: CrewChatWindow.pageSize), 12)
    }

    // MARK: - Roster 归一化 / 从成员列表构造

    func testRosterStripsLeadingAtAndDedupes() {
        let r = CrewMentionFilter.Roster(humanNames: ["@人", " 人 "], otherNames: ["", "机长"])
        XCTAssertEqual(r.humanNames, ["人"])
        XCTAssertEqual(r.allNames, ["人", "机长"])
    }

    func testRosterFromMembersUsesSharedDisplayNames() {
        let members = [
            member(id: "m1", kind: "human", userId: "u1", displayName: nil),
            member(id: "m2", kind: "captain", botId: "bot-1", displayName: nil),
            member(id: "m3", kind: "code_session", sessionId: "s1", displayName: peer),
        ]
        let r = CrewMentionFilter.Roster.from(members: members, captainBotId: "bot-1")
        // human / captain 的兜底名与 CrewSenderNaming 同一份（「人」/「机长」）。
        XCTAssertEqual(r.humanNames, ["人"])
        XCTAssertEqual(Set(r.allNames), Set(["人", "机长", peer]))
        XCTAssertTrue(CrewMentionFilter.isHumanMention(entry(text: "@人 看下"), roster: r))
        XCTAssertFalse(CrewMentionFilter.isHumanMention(entry(text: "@\(peer) 看下"), roster: r))
    }

    private func member(
        id: String, kind: String, userId: String? = nil, botId: String? = nil,
        sessionId: String? = nil, displayName: String?
    ) -> CrewMember {
        CrewMember(
            id: id, memberKind: kind, userId: userId, botId: botId, codeSessionId: sessionId,
            displayName: displayName, role: nil, status: "active", representsCrewId: nil,
            sessionStatus: nil)
    }
}
#endif
