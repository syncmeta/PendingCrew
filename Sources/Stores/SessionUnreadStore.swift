import Foundation
import Combine

/// session 未读状态（chunk2 T6：切换条角标）。
///
/// 记每个 session「最后看过」的时间（UserDefaults 持久化，重启不丢）；未读数 =
/// 本 session 的 pending 待办（LocalApprovalStore）+ lastViewed 之后该 session
/// 发到白板的消息（LocalWhiteboardStore）。纯派生、不缓存 —— 调用方（切换条）
/// 用轮询 tick 驱动重算，与 approval 卡片同套路。
@MainActor
final class SessionUnreadStore: ObservableObject {
    static let shared = SessionUnreadStore()

    private static let defaultsKey = "PendingCrew.sessionLastViewed"

    /// sessionId → 最后查看时间。
    private var lastViewed: [String: Date]

    /// 白板 createdAt 是 ISO8601 字符串（LocalWhiteboardMessage.createdAt）。
    private let iso = ISO8601DateFormatter()

    init() {
        let raw = UserDefaults.standard.dictionary(forKey: Self.defaultsKey) as? [String: TimeInterval] ?? [:]
        lastViewed = raw.mapValues { Date(timeIntervalSince1970: $0) }
    }

    /// 标记某 session 已被查看（切到前台时调）。
    func markViewed(_ sessionId: String) {
        objectWillChange.send()
        lastViewed[sessionId] = Date()
        persist()
    }

    /// 某 session 最后查看时间；从未看过 → .distantPast（全部算未读）。
    func lastViewedAt(_ sessionId: String) -> Date {
        lastViewed[sessionId] ?? .distantPast
    }

    /// 未读数 = 本 session 的 pending 待办 + lastViewed 之后的白板消息。
    func unreadCount(crewId: String, sessionId: String,
                     approvals: LocalApprovalStore, whiteboard: LocalWhiteboardStore) -> Int {
        let pendingCount = approvals.pending(crewId: crewId)
            .filter { $0.sessionId == sessionId }.count
        let since = lastViewedAt(sessionId)
        let newMessages = whiteboard.list(crewId: crewId).filter { msg in
            guard msg.senderSessionId == sessionId,
                  let date = iso.date(from: msg.createdAt) else { return false }
            return date > since
        }.count
        return pendingCount + newMessages
    }

    private func persist() {
        let raw = lastViewed.mapValues { $0.timeIntervalSince1970 }
        UserDefaults.standard.set(raw, forKey: Self.defaultsKey)
    }
}
