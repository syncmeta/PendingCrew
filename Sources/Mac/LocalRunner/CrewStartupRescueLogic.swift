#if os(macOS)
import Foundation

/// app 启动时的**积压对账**（B3：重启把「留待重投」变成「作废」）。
///
/// ## 病根
///
/// `CrewLocalMentionWaker.start()` 把每个 crew 的扫描游标钉到白板**当前尾**，
/// 而缺席目标的拉起（`wakeAbsent`）**只能从扫描发起**。于是：
/// **任何在重启前没被成功处理掉的定向 @，重启后不会被扫到，也没有第二条路会去
/// 拉起它的目标** —— 除非有人往那个群里再发一条新消息。
/// hook 路兜不住：它只投给**正在跑**的 session。
///
/// ## 两个实测现场（2026-09-07）
///
/// - **crew 33**：人类的 Todo 答复写在重启前 **38 秒**，目标在本地成员登记里、
///   本该被 `restartMember` 拉起来 —— 那个板从来没送到任何人手上，白板此后 33 小时全空。
/// - **crew 45**：两条 `唤醒失败…消息留待重投` 的告警，随后被重启抹掉。
///   **这条把 A1 和 B3 连起来了**：`confirmWake` 判失败时我们**故意**不消费、留着重投；
///   而重启把「留待」变成了「作废」。**修 B3 之前，A1 的那个取舍是名义上的。**
///
/// ## 改法：启动时不再「钉到尾巴就算数」，而是**按每个成员自己的未读对账**
///
/// 未读是**盘上**那本唯一账（#105 ④）——`confirmWake` 判失败时它**没有**被推进，
/// 所以「还欠这个人一条」这件事本来就写在盘上，只是从来没人在启动时去读它。
///
/// ## 为什么这不会重演 2026-08-12 的全机重放
///
/// 复用 `CrewLocalMentionWakeLogic.pending` 那道**独立于游标**的陈旧闸
/// （`maxWakeAge`，6 小时）。它当年就是为那次事故加的，破了游标也还有它兜着。
/// **代价要说清**：超过 6 小时的积压**捞不回来** —— 上面 crew 45 那两条已经
/// 56 小时了，这次修复救不了它们。**它防的是往后，不是往回。**
enum CrewStartupRescueLogic {

    /// 一条启动时发现的欠账：这个 session 有一条该唤醒它、却从没送到的 @。
    struct Pending: Equatable {
        let sessionId: String
        let entryId: String
    }

    /// - Parameters:
    ///   - unreadBySession: 每个**本地登记成员**（含机长）按盘上游标算出来的未读。
    ///   - captainSessionId: 本 crew 当前机长的 sessionId（`@captain` 归它）。
    /// - Returns: 需要补投 / 拉起的欠账，按 sessionId 排序（可预期，便于测试与日志）。
    static func pending(
        unreadBySession: [String: [LocalWhiteboardMessage]],
        captainSessionId: String?,
        now: Date = Date()
    ) -> [Pending] {
        var out: [Pending] = []
        for sessionId in unreadBySession.keys.sorted() {
            let unread = unreadBySession[sessionId] ?? []
            for d in CrewLocalMentionWakeLogic.pending(entries: unread, now: now) {
                // 自己 @ 自己不算欠账（与活体路同语义）。
                guard d.senderSessionId != sessionId else { continue }
                guard addressed(d.mentions, to: sessionId, captainSessionId: captainSessionId)
                else { continue }
                out.append(Pending(sessionId: sessionId, entryId: d.entryId))
            }
        }
        return out
    }

    /// 这条 @ 点到这个 session 了吗。**只认收窄型的两种**（session / captain）——
    /// 与活体唤醒路一致：broadcast / human 不唤醒具体 run。
    private static func addressed(
        _ mentions: [CrewMention], to sessionId: String, captainSessionId: String?
    ) -> Bool {
        mentions.contains { m in
            if m.kind == "session" { return m.targetId == sessionId }
            if m.kind == "captain" { return captainSessionId == sessionId }
            return false
        }
    }
}
#endif
