import Foundation

/// 「每个 crew 白板的**末条消息**」的指纹门控缓存。
///
/// ## 病根（2026-08-17：开久了卡、重启就好）
///
/// 侧栏两条路（时间流列表的排序键 / 层级视图行里的预览）都直接调
/// `LocalWhiteboardStore.list(crewId:).last`：**flock + 读整个文件 + 全量 JSON
/// 解码**，而且是在 SwiftUI body 里、主线程上、**每个 crew 各来一遍**。触发源是
/// `LocalWhiteboardStore.directoryChanged` —— 白板目录里任何文件被写就 tick
/// （已 250ms 合流 = 最高 4 次/秒），而目录里每个活着的 session 都在持续写自己的
/// `.cursor` / `.turn` / approvals。现场量级：28 个 crew、白板 JSON 合计 2.7 MB
/// → 主线程每秒解析约 11 MB JSON + 112 次加锁读文件。派的活越多越卡；重启后没有
/// session 在跑、目录不动，所以「重启就好」。
///
/// ## 这里做的事
///
/// 收到 tick 后先 `fingerprint(crewId:)`（mtime+size，**只 stat 不读内容**），与
/// 上次记录相同就直接返回缓存的末条消息，**一个字节都不读**。只有指纹真变了的那
/// 个 crew 才重新解码。28 次 stat 取代 28 次全量解码。
///
/// 判定闸不是新发明的 —— 群聊中栏那条流（`PendingCrewBackend.whiteboardChanges`
/// 的 `FileChangeGateBox`）用的就是同一个 `FileChangeGate`，侧栏这条路只是漏了。
///
/// ## mtime+size 指纹漏判的可能性（为什么在这里可以接受）
///
/// 理论弱点是「同一时刻把文件改写成同样大小」。这个场景撞不上：
/// - 白板只有一条写路径（`LocalWhiteboardStore.appendReportingFailure` 的
///   重读-合并-整写），每次都**追加**一条非空 JSON 对象 → 尺寸必变；
/// - 唯一会缩短文件的是损坏重建（归档原文件 + 只留一条系统警示），与原文件同尺寸
///   的概率可忽略；
/// - `contentModificationDate` 在 APFS 上是亚秒精度（这里原样存成
///   `TimeInterval`），不是秒级取整，所以连「同一秒内两次写」都分得开。
///
/// 而且漏判的后果是**侧栏预览晚一拍**（下一次真变化即纠正），不是数据错乱 ——
/// 与「主线程每秒解析 11 MB」相比，这个风险敞口是划算的。
///
/// ## 为什么没做「只读文件尾部拿末条」
///
/// 想过，故意不做。白板落盘是**一整个 JSON 数组**（`MultiProcessJSONStore` 原子
/// 整写），要只取末条就得在那套逐条 lenient 解码之外再养一个「从尾部倒扫最后一个
/// `{…}`」的自定义解析器（还得处理字符串里的括号与转义、以及损坏归档重建后的形态），
/// 两套解析口径迟早分家。收益也已经很薄：指纹门控之后，一次目录 tick 至多解一份
/// 白板，而且不在主线程上（现场实测一次 tick 0.773 ms，大头还是那 64 次 stat）。
/// 不值得为这点尾巴引入第二套解析。
///
/// ## 线程
///
/// 非 `@MainActor`：**刻意**要在后台队列上跑（stat 与解码都是磁盘 IO，`FileChangeGate`
/// 自己的注释就写着别在主线程调）。`CrewStore` 在后台队列刷新它、把结果 hop 回主线程
/// 发布，所以 SwiftUI body 里零磁盘 IO。
///
/// ## 门控本身不在这里
///
/// 「指纹没变就别读」那套记账已经抽成 `FileFingerprintCache`（2026-08-18，点名快照
/// 那条路要用同一套）。这层只剩「白板 → 末条消息」这一个语义，不再自己管缓存表。
final class CrewLastMessageCache: @unchecked Sendable {
    /// 一次解码从一块白板里取到的**两样**东西。
    ///
    /// 为什么合成一个载荷而不是加第二个缓存：侧栏那一行现在要显示「各机长自己填的
    /// 那句状态」（Todo #136），而它的判据是**往回找最近一条带非空状态的消息** ——
    /// 只看末条不够。再开一个缓存去扫，就是每次文件变动**多解一整板 JSON**，
    /// 正是 2026-08-17「开久了卡」的形状。**一次解码，两样都拿出来。**
    struct Digest: Equatable {
        /// 末条消息（时间、预览、相对时间都来自它）。
        let last: LocalWhiteboardMessage
        /// 当前生效的那句状态 + 它所在那条消息的时间。
        /// `nil` = 这块白板上**一条都没填过**（不是「这次没填」）。
        let status: LocalWhiteboardMessage?
    }

    private let cache: FileFingerprintCache<String, Digest>

    /// 生产用：直接挂在一个白板 store 上。
    convenience init(store: LocalWhiteboardStore) {
        self.init(
            fingerprintOf: { store.fingerprint(crewId: $0) },
            loadDigest: { crewId in
                let messages = store.list(crewId: crewId)
                guard let last = messages.last else { return nil }
                // 往回找最近一条带非空状态的 —— 「这次没填就沿用上一次」由此成立。
                let carrier = messages.reversed().first {
                    ($0.crewStatus ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == false
                }
                return Digest(last: last, status: carrier)
            })
    }

    /// 单测 / 基准用：两条 IO 都可注入，好数「到底真读了几次」。
    init(fingerprintOf: @escaping (String) -> FileChangeGate.Fingerprint?,
         loadDigest: @escaping (String) -> Digest?) {
        cache = FileFingerprintCache(fingerprintOf: fingerprintOf, load: loadDigest)
    }

    /// 本 cache 迄今真正做过多少次「读文件 + 全量解码」。缓存命中不计。
    ///
    /// ## 载荷从「末条消息」改成「末条 + 那句状态」之后，这个数没有变（Todo #136）
    /// 改之前 / 改之后，`CrewLastMessageCacheTests` 里那几条断言**一个数都没动**：
    /// 冷启 5 个 crew = 5 次、无关 tick 追加 = 0 次、只改一个 crew = warm + 1。
    /// **这就是这次改动的验收口径**：状态是在**同一次已经付过的解码**里顺手取出来的，
    /// 不是多解一遍。**如果哪天这些数变大了，说明有人把它做成了「解码两次、看起来
    /// 像一次」** —— 那正是这条基准该拦住的东西，别去调大它。
    /// 这是 fix 的验收口径：一次 tick 里它涨多少 = 主线程本来要付几份整板解码。
    var decodeCount: Int { cache.loadCount }

    /// 刷新这批 crew 的末条消息。
    ///
    /// - Returns: crewId → 末条消息。**键缺失 = 该 crew 白板是空的**（不是「没读」）
    ///   —— 每次调用都覆盖全表，所以调用方拿到的恒是完整快照。
    @discardableResult
    func refresh(crewIds: [String]) -> [String: Digest] {
        cache.refresh(keys: crewIds)
    }

    /// 丢掉全部缓存（退出登录 / 换后端时 `CrewStore.reset` 调）。
    func clear() { cache.clear() }
}
