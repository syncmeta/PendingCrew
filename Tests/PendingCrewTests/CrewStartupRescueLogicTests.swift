#if os(macOS)
import XCTest

/// **B3：重启把「留待重投」变成「作废」** —— 启动时的积压对账。
///
/// 实测（47 个白板）：三次有记录的重启，各有 **14 / 12 / 13** 个 crew 的白板
/// 最后一条是没人回过的定向 @。⚠️ **那是暴露面，不是受害者名单** —— 板死可能有
/// 别的原因。只有 **crew 33** 是硬的（人类的 Todo 答复写在重启前 **38 秒**、
/// 目标在本地成员登记里、此后白板 33 小时全空），**crew 45** 次硬（两条
/// 「消息留待重投」被随后的重启抹掉）。
final class CrewStartupRescueLogicTests: XCTestCase {

    private let now = Date(timeIntervalSince1970: 1_788_000_000)
    private func iso(_ d: Date) -> String { ISO8601DateFormatter().string(from: d) }

    private func msg(_ id: String, mentions: [CrewMention],
                     ageSeconds: TimeInterval = 60,
                     sender: String? = "someone-else") -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: id, senderKind: "session", senderUserId: nil, senderSessionId: sender,
            category: nil, text: "去做 X", createdAt: iso(now.addingTimeInterval(-ageSeconds)),
            senderName: "别人",
            mentions: mentions.map { LocalWhiteboardMention(kind: $0.kind, targetId: $0.targetId) })
    }

    // MARK: - 正身：两个现场的形状

    /// crew 33 的形状：@ 写在重启前几十秒，重启后扫描游标钉到尾巴 —— 它落在游标
    /// 后面，再也没人扫得到。启动对账必须把它捞回来。
    func test_重启窗口内写进来的定向at_启动时必须被捞回来() {
        let out = CrewStartupRescueLogic.pending(
            unreadBySession: ["worker-a": [msg("e1", mentions: [.session("worker-a")])]],
            captainSessionId: nil, now: now)
        XCTAssertEqual(out, [.init(sessionId: "worker-a", entryId: "e1")])
    }

    /// crew 45 的形状：**重启前已经判失败、正「留待重投」的那一条**。
    /// 它之所以还在未读里，正是因为 `confirmWake` 判失败时故意没推进游标 ——
    /// 「还欠这个人一条」本来就写在盘上，只是从来没人在启动时去读它。
    ///
    /// **这一支才是把 A1 吃掉的那个**：不捞它，「留待重投」这句承诺跨重启就是空的。
    func test_重启前已判失败正留待重投的那条_也必须被捞回来() {
        let out = CrewStartupRescueLogic.pending(
            unreadBySession: ["cap-1": [msg("e-retry", mentions: [.captain])]],
            captainSessionId: "cap-1", now: now)
        XCTAssertEqual(out, [.init(sessionId: "cap-1", entryId: "e-retry")])
    }

    // MARK: - 反面：不许变成一条「只要有 @ 就捞」的规则

    /// **2026-08-12 全机重放的那道独立闸必须仍然管用。** 启动时把几周前的 @ 全捞
    /// 起来，就是再演一次那场事故 —— 那次的代价是两位数的无效轮次。
    ///
    /// **代价要说清**：超过 `maxWakeAge` 的积压捞不回来。crew 45 那两条已经 56 小时，
    /// **这次修复救不了它们。它防的是往后，不是往回。**
    func test_陈旧的at不捞() {
        let stale = CrewLocalMentionWakeLogic.maxWakeAge + 3600
        let out = CrewStartupRescueLogic.pending(
            unreadBySession: ["worker-a": [
                msg("old", mentions: [.session("worker-a")], ageSeconds: stale)]],
            captainSessionId: nil, now: now)
        XCTAssertTrue(out.isEmpty, "启动对账把几周前的 @ 捞起来 = 再演一次 2026-08-12")
    }

    func test_点名别人的不捞() {
        let out = CrewStartupRescueLogic.pending(
            unreadBySession: ["worker-a": [msg("e1", mentions: [.session("worker-b")])]],
            captainSessionId: nil, now: now)
        XCTAssertTrue(out.isEmpty, "@ 别人的被当成自己的活 —— 那是 #543 那场扩散")
    }

    func test_广播不唤醒具体run() {
        let out = CrewStartupRescueLogic.pending(
            unreadBySession: ["worker-a": [msg("e1", mentions: [.broadcast])]],
            captainSessionId: nil, now: now)
        XCTAssertTrue(out.isEmpty, "与活体唤醒路同语义：broadcast 看得见，但不叫醒谁")
    }

    func test_at机长的只算给机长() {
        let unread = [msg("e1", mentions: [.captain])]
        let out = CrewStartupRescueLogic.pending(
            unreadBySession: ["cap-1": unread, "worker-a": unread],
            captainSessionId: "cap-1", now: now)
        XCTAssertEqual(out, [.init(sessionId: "cap-1", entryId: "e1")])
    }

    /// 没有未读 = 没有欠账。启动对账不许凭空造出唤醒。
    func test_没有未读就没有欠账() {
        XCTAssertTrue(CrewStartupRescueLogic.pending(
            unreadBySession: ["worker-a": []], captainSessionId: nil, now: now).isEmpty)
    }

    /// 自己 @ 自己不算（与活体路同语义）。
    func test_自己at自己不算欠账() {
        let out = CrewStartupRescueLogic.pending(
            unreadBySession: ["worker-a": [
                msg("e1", mentions: [.session("worker-a")], sender: "worker-a")]],
            captainSessionId: nil, now: now)
        XCTAssertTrue(out.isEmpty)
    }
}
#endif
