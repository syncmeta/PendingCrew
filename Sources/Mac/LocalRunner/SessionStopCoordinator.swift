import Foundation

/// `stop_session` 在碰真实 runner 前的门禁与副作用顺序。
///
/// 保持为纯 Foundation：生产路径把 `writeReceipt` 接白板、`terminate` 接
/// `CrewSessionRunner.stop`（最终复用 `run.stop()`）；测试则记录两者调用顺序，
/// 钉死「白板回执先于终止」以及跨 crew / 不存在 / 已退出均不触碰进程。
struct SessionStopTarget: Equatable {
    let sessionId: String
    let crewId: String
    let displayName: String
    let isRunning: Bool
}

enum SessionStopCoordinator {
    static func execute(
        requestCrewId: String,
        requesterSessionId: String,
        targetSessionId: String,
        reason: String,
        targets: [SessionStopTarget],
        writeReceipt: (String) -> Void,
        terminate: (SessionStopTarget) -> Void
    ) -> String {
        guard let target = targets.first(where: { $0.sessionId == targetSessionId }) else {
            return "ERROR: 找不到 session \(targetSessionId)：目标不存在、已被移除或 id 写错。请用 list_sessions 核对。"
        }
        guard target.crewId == requestCrewId else {
            return "ERROR: 拒绝终止 session \(targetSessionId)：它属于 crew \(target.crewId)，不是本 crew \(requestCrewId)。"
        }
        guard target.isRunning else {
            return "ERROR: session \(targetSessionId) 已退出，不能重复终止。请用 list_sessions 核对状态。"
        }

        let receipt = "机长（\(requesterSessionId)）终止了「\(target.displayName)」"
            + "（\(target.sessionId)）。原因：\(reason)"
        writeReceipt(receipt)
        terminate(target)
        return "已终止「\(target.displayName)」（\(target.sessionId)）。原因已先写入本 crew 白板。"
    }
}
