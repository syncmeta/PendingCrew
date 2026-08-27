import Foundation

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

    /// 丢掉全部缓存（`CrewStore.reset` 调）。
    func clear() { cache.clear() }
}
