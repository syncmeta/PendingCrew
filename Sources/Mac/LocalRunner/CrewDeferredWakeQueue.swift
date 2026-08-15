#if os(macOS)
import Foundation

/// Codex busy -> idle 唤醒补投的纯状态机。
///
/// `sourceKey` 由原消息身份生成，再与目标 session id 拼成 delivery key。同一条
/// 消息在 pending / 最近已投递窗口内都只认一次；idle 时一次只取一条，避免第一条
/// `send` 已经起了新 turn 后又把后续消息塞进正在运行的 turn。
struct CrewDeferredWakeQueue {
    struct Delivery: Equatable {
        let key: String
        let targetSessionId: String
        let text: String
    }

    enum Submission: Equatable {
        case deliver(Delivery)
        case deferred
        case duplicate
    }

    private var pending: [String: [Delivery]] = [:]
    private var pendingKeys: Set<String> = []
    private var deliveredKeys: Set<String> = []
    private var deliveredOrder: [String] = []
    private let deliveredCapacity: Int

    init(deliveredCapacity: Int = 512) {
        self.deliveredCapacity = max(1, deliveredCapacity)
    }

    mutating func submit(_ delivery: Delivery, isBusy: Bool) -> Submission {
        guard !pendingKeys.contains(delivery.key), !deliveredKeys.contains(delivery.key)
        else { return .duplicate }
        guard isBusy else {
            rememberDelivered(delivery.key)
            return .deliver(delivery)
        }
        pending[delivery.targetSessionId, default: []].append(delivery)
        pendingKeys.insert(delivery.key)
        return .deferred
    }

    /// 目标每次变 idle 只取一条；`run.send` 会立即开始下一 turn，剩余消息等下一次
    /// idle 边沿。重复 idle 信号拿不到已取走的条目，因此不会重复投递。
    mutating func popWhenIdle(sessionId: String) -> Delivery? {
        guard var deliveries = pending[sessionId], !deliveries.isEmpty else { return nil }
        let delivery = deliveries.removeFirst()
        if deliveries.isEmpty {
            pending.removeValue(forKey: sessionId)
        } else {
            pending[sessionId] = deliveries
        }
        pendingKeys.remove(delivery.key)
        rememberDelivered(delivery.key)
        return delivery
    }

    @discardableResult
    mutating func remove(sessionId: String) -> [Delivery] {
        let deliveries = pending.removeValue(forKey: sessionId) ?? []
        for delivery in deliveries { pendingKeys.remove(delivery.key) }
        return deliveries
    }

    func pendingCount(sessionId: String) -> Int {
        pending[sessionId]?.count ?? 0
    }

    private mutating func rememberDelivered(_ key: String) {
        guard deliveredKeys.insert(key).inserted else { return }
        deliveredOrder.append(key)
        while deliveredOrder.count > deliveredCapacity {
            deliveredKeys.remove(deliveredOrder.removeFirst())
        }
    }
}
#endif
