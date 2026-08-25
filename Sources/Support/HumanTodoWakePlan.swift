import Foundation

/// 人类回应了一条「人类 Todo」之后，**该叫醒谁**（Todo #62 ②）。
///
/// 这本账（`TodoLedger.human`）是 agent 提问、人类拍板。人类拍完板，回应得回到
/// **提问的那个 session** 手里 —— 所以条目建的时候就记了 `createdBySessionId`
/// （`LocalTodoItem.createdBySessionId`）。缺了它整个功能落不了地：人类回应时
/// 根本不知道该叫醒谁。
///
/// ## 三条规则（两条是回落，写死不许静默丢）
///   1. 提问的 session **还在跑** → 叫醒它。
///   2. 提问的 session **已经退出** → **回落叫醒本 crew 机长**，由机长转达。
///      不是「等它下次起来自己看」—— 那等于静默丢，人类拍的板没人接。
///   3. **机长自己提的** → 叫醒机长自己（等价于规则 1，单列出来是因为机长的
///      sessionId 会随重启换，按「是不是机长」认比按 id 认稳）。
///   另加一条兜底：条目根本没记 `createdBySessionId`（老数据 / 写漏了）→ 同样
///   回落机长，绝不静默丢。
///
/// ## mentions 为什么是 `[.broadcast, .session(X)]`
/// 人类原话要的是「回应之后，直接在群里发消息」—— 那条回应**全组都该看得见**
/// （群聊是白板，不是私信），但**只该叫醒提问的那个**。`.broadcast` 是显式放宽器
/// （A 线 #62 落地的语义），`.session(X)` 负责定向唤醒。两个一起给，就是
/// 「全组可见 + 只叫醒提问者」。
///
/// 纯 Foundation、无 IO —— 「该叫醒谁」这条判据能脱离 app 直接单测。
enum HumanTodoWakePlan {

    /// 叫醒谁。
    enum Target: Equatable {
        /// 提问的那个 session（还在跑）。
        case session(String)
        /// 本 crew 机长 —— 提问者退出了 / 没记下 / 机长自己提的。
        case captain
    }

    /// 为什么落到机长身上。回落时要在注入文本里说清楚，否则机长收到一条
    /// 「人类回应了 #3」不知道该自己办还是该转达。
    enum CaptainReason: Equatable {
        /// 机长自己提的 —— 这就是给他本人的答复，不用转达。
        case askedByCaptain
        /// 提问的 session 已经退出 —— 请机长转达。
        case askerGone(String)
        /// 条目没记提问者（老数据 / 写漏）—— 请机长认领并转达。
        case askerUnknown
    }

    /// 一次完整的投递计划：叫醒谁 + 群消息该带哪些 mention + 回落时的说明。
    struct Plan: Equatable {
        let target: Target
        /// 群消息的 mentions：全组可见 + 只叫醒该醒的那个。
        let mentions: [CrewMention]
        /// 回落到机长时的原因（`target == .session` 时为 nil）。
        let captainReason: CaptainReason?

        /// 追加在注入文本尾巴上的一句话 —— 只有回落到机长时才有。
        var fallbackNote: String? {
            switch captainReason {
            case .none, .some(.askedByCaptain):
                return nil
            case .some(.askerGone(let sid)):
                return "（提这条的 session `\(sid)` 已经不在跑了，这条人类的答复转给你 —— 请你接住并转达/落地，别让它停在这儿。）"
            case .some(.askerUnknown):
                return "（这条 Todo 没记下是谁提的，人类的答复转给你 —— 请你认领并转达/落地。）"
            }
        }
    }

    /// 该叫醒谁。`runningSessionIds` = 本 crew 此刻**在跑**的 session（含 busy）；
    /// `captainSessionId` = 机长本地 run 的 sessionId（没在跑 → nil，仍返回
    /// `.captain`，由调用方按 `@captain` 把机长拉起来）。
    static func resolve(
        createdBySessionId: String?,
        runningSessionIds: Set<String>,
        captainSessionId: String?
    ) -> Target {
        guard let asker = createdBySessionId, !asker.isEmpty else { return .captain }
        // 机长自己提的 → 机长自己（按「是不是机长」认，不按 id 认 —— 机长重启后
        // sessionId 会变，认 id 会把他误判成「已退出」而绕一圈回落到他自己）。
        if let cap = captainSessionId, cap == asker { return .captain }
        if runningSessionIds.contains(asker) { return .session(asker) }
        return .captain   // 已退出 → 回落机长转达，别静默丢
    }

    /// 完整计划（`resolve` + mentions 组装 + 回落原因）。
    static func plan(
        createdBySessionId: String?,
        runningSessionIds: Set<String>,
        captainSessionId: String?
    ) -> Plan {
        let target = resolve(createdBySessionId: createdBySessionId,
                             runningSessionIds: runningSessionIds,
                             captainSessionId: captainSessionId)
        switch target {
        case .session(let sid):
            return Plan(target: target,
                        mentions: [.broadcast, .session(sid)],
                        captainReason: nil)
        case .captain:
            let reason: CaptainReason
            if let asker = createdBySessionId, !asker.isEmpty {
                reason = (captainSessionId == asker) ? .askedByCaptain : .askerGone(asker)
            } else {
                reason = .askerUnknown
            }
            return Plan(target: target,
                        mentions: [.broadcast, .captain],
                        captainReason: reason)
        }
    }
}
