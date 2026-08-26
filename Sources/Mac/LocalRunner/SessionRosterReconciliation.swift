#if os(macOS)
import Foundation

/// viewer 侧「daemon 的 roster ↔ 本地镜像」的**纯判定**（前后端分离 P4）。
///
/// 为什么要把这几行单独拎出来：`applyRemoteRoster` 每 2 秒就被全量喂一次，
/// 而它一旦把「已经有的那个」当成「新的」，右栏正开着的 session 就会被换掉一个
/// `runID` —— 表现是终端每两秒闪一下、选中态乱跳，而看代码时那一行长得非常无辜。
/// 判定在这儿、有测试钉着；`CrewSessionRunner` 那边只负责按判定去建/删对象。
enum SessionRosterReconciliation {
    struct Plan: Equatable {
        /// 本地还没有、要新建镜像的（保持 daemon 给的顺序）。
        var create: [SessionSummary] = []
        /// 本地已有、只更新状态的。
        var update: [SessionSummary] = []
        /// daemon 那边已经没有了、本地要删掉的 sessionId。
        var remove: [String] = []
    }

    /// - Parameters:
    ///   - incoming: daemon 的全量 roster。
    ///   - localSessionIds: 本地镜像现在有哪些。
    static func plan(incoming: [SessionSummary], localSessionIds: [String]) -> Plan {
        var plan = Plan()
        let local = Set(localSessionIds)
        var seen: Set<String> = []
        for summary in incoming {
            // **没有编排身份的条目一律跳过**，不是「用默认值建一个」：那种条目来自
            // 更旧的 daemon（§4.4 的向前兼容），拿默认值建出来的镜像会顶着空 crewId
            // 挂在右栏上，比不显示更难查。
            guard summary.run != nil,
                  LocalCodingAgentKind(rawValue: summary.state.kind) != nil else { continue }
            seen.insert(summary.sessionId)
            if local.contains(summary.sessionId) {
                plan.update.append(summary)
            } else {
                plan.create.append(summary)
            }
        }
        plan.remove = localSessionIds.filter { !seen.contains($0) }
        return plan
    }
}
#endif
