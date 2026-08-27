import Foundation

/// 一个 crew 在侧栏需要表达的人类 Todo 范围（Todo #73）。
///
/// 自身和后代分开存，不能只合成一个总数：黄色都压过绿色，但悬浮/辅助功能文案
/// 必须让人知道 Todo 在本 crew，还是要继续往下找。
struct CrewHumanTodoAttention: Equatable, Sendable {
    enum Scope: Equatable, Sendable {
        case none
        case own
        case descendant
        case ownAndDescendant
    }

    static let none = CrewHumanTodoAttention(ownUnanswered: 0, descendantUnanswered: 0)

    let ownUnanswered: Int
    let descendantUnanswered: Int

    init(ownUnanswered: Int, descendantUnanswered: Int) {
        self.ownUnanswered = max(0, ownUnanswered)
        self.descendantUnanswered = max(0, descendantUnanswered)
    }

    var scope: Scope {
        switch (ownUnanswered > 0, descendantUnanswered > 0) {
        case (false, false): return .none
        case (true, false): return .own
        case (false, true): return .descendant
        case (true, true): return .ownAndDescendant
        }
    }

    var hasUnanswered: Bool { scope != .none }

    /// 状态点的悬浮提示和辅助功能共用同一份可测试语义。
    var accessibilityLabel: String? {
        var parts: [String] = []
        if ownUnanswered > 0 {
            parts.append("本 crew 有 \(ownUnanswered) 条人类 Todo 等你拍板")
        }
        if descendantUnanswered > 0 {
            parts.append("下属 crew 有 \(descendantUnanswered) 条人类 Todo 等你拍板")
        }
        return parts.isEmpty ? nil : parts.joined(separator: "；")
    }
}

/// 「每个 crew 的**人类 Todo** 还有几条没回应」的指纹门控快照（Todo #62 ④）。
///
/// Todo #71 起侧栏黄点只表示「这本账里有未回应条目」；`attentionReason` 不再参与
/// 颜色。`dismissedAt` 一打，算出来就是灭（判据见 `LocalTodoItem.isUnanswered`）。
///
/// ## 为什么不在 body 里现算
/// `LocalTodoStore.list(crewId:)` 是 flock + 整份 JSON 解码。侧栏每个 crew 一行、
/// 每帧一次，就是 2026-08-17「开久了卡」的同一个形状（那次的病根是白板末条，
/// 这次会是 Todo 列表）。所以走和 `CrewLastMessageCache` **同一套** `FileFingerprintCache`：
/// 收到目录 tick 后先 stat 一遍，只有 `<crewId>.human-todos.json` 的 mtime+size
/// 真变了的那个 crew 才重新读+解码；`CrewStore` 在后台队列上刷，结果 hop 回主线程
/// 发布，body 里零磁盘 IO。
///
/// 非 `@MainActor`：**刻意**要在后台队列上跑（stat 与解码都是磁盘 IO）。
final class CrewHumanTodoAttentionCache: @unchecked Sendable {
    private let cache: FileFingerprintCache<String, Int>

    /// 生产用：挂在 `.human` 那本账上。
    convenience init(store: LocalTodoStore = .shared(.human)) {
        self.init(
            fingerprintOf: { store.fingerprint(crewId: $0) },
            loadUnansweredCount: { store.list(crewId: $0).filter(\.isUnanswered).count })
    }

    /// 单测用：两条 IO 都可注入，好数「到底真读了几次」。
    init(fingerprintOf: @escaping (String) -> FileChangeGate.Fingerprint?,
         loadUnansweredCount: @escaping (String) -> Int?) {
        cache = FileFingerprintCache(fingerprintOf: fingerprintOf, load: loadUnansweredCount)
    }

    /// 本 cache 迄今真正做过多少次「读文件 + 全量解码」。缓存命中不计 ——
    /// 这是「没把 2026-08-17 那个形状再造一遍」的验收口径。
    var decodeCount: Int { cache.loadCount }

    /// 刷新这批 crew 的未回应条数。
    ///
    /// - Returns: crewId → 未回应条数。**键缺失 = 这本账是空的 / 文件不存在**
    ///   （不是「没读」）—— 每次调用都覆盖全表，调用方拿到的恒是完整快照。
    @discardableResult
    func refresh(crewIds: [String]) -> [String: Int] {
        cache.refresh(keys: crewIds)
    }

    /// 在同一次后台刷新里，把指纹门控得到的**本 crew**条数沿 DAG 父边向上传播。
    /// View 只消费返回的内存快照，不接触 Todo 文件，也不自己遍历整棵树。
    func refresh(
        crewIds: [String],
        parentsByCrew: [String: [String]]
    ) -> [String: CrewHumanTodoAttention] {
        Self.aggregate(
            directCounts: cache.refresh(keys: crewIds),
            parentsByCrew: parentsByCrew)
    }

    /// 把每个来源 crew 的条数向所有已知祖先累计。
    ///
    /// - 每个来源各带一份 `visited`：多父 DAG 的菱形路径不会让同一后代重复计数；
    /// - `visited` 从来源自身起步：脏环不会无限走，也不会把来源算成自己的后代；
    /// - 父 id 不在 `parentsByCrew`：停在已知边界，不造幽灵 crew。
    static func aggregate(
        directCounts: [String: Int],
        parentsByCrew: [String: [String]]
    ) -> [String: CrewHumanTodoAttention] {
        let knownCrewIds = Set(parentsByCrew.keys)
        var own: [String: Int] = [:]
        var descendant = Dictionary(uniqueKeysWithValues: knownCrewIds.map { ($0, 0) })

        for crewId in knownCrewIds {
            own[crewId] = max(0, directCounts[crewId] ?? 0)
        }

        for (sourceCrewId, rawCount) in directCounts {
            let count = max(0, rawCount)
            guard count > 0, knownCrewIds.contains(sourceCrewId) else { continue }

            var visited: Set<String> = [sourceCrewId]
            var pending = parentsByCrew[sourceCrewId] ?? []
            while let parentId = pending.popLast() {
                guard knownCrewIds.contains(parentId), visited.insert(parentId).inserted else {
                    continue
                }
                descendant[parentId, default: 0] += count
                pending.append(contentsOf: parentsByCrew[parentId] ?? [])
            }
        }

        return Dictionary(uniqueKeysWithValues: knownCrewIds.map { crewId in
            (crewId, CrewHumanTodoAttention(
                ownUnanswered: own[crewId] ?? 0,
                descendantUnanswered: descendant[crewId] ?? 0))
        })
    }

    /// 丢掉全部缓存（`CrewStore.reset` 调）。
    func clear() { cache.clear() }
}
