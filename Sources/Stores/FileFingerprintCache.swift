import Foundation

/// 「一个文件 → 一个派生值」的指纹门控缓存底座。
///
/// ## 为什么要有这一层
///
/// 「开久了卡」那一族问题长得都一样：某个每秒跑好几次的循环，为了拿一个**很小的
/// 派生值**（末条消息 / 有没有待审批 / 上一轮的收尾问句），去把整份 JSON 加锁读出来
/// 全量解码一遍，而且是在主线程上、按 crew 或按 session **逐个**来一遍。文件绝大多数
/// 时候根本没动 —— 是别的 session 在写自己的状态文件把整条链拉起来的。
///
/// 治法只有一句：**先 stat 一下指纹（mtime+size），没变就用上次的结果，一个字节不读。**
/// 这个判断 `FileChangeGate` 里本来就有（群聊中栏那条流一直在用），这层只是把
/// 「一批文件各自门控 + 记账省了多少」抽成可复用的一份，免得每个调用点各写各的。
///
/// 现有两个使用方（**加新的请挂到这里，别再写第三套**）：
/// - `CrewLastMessageCache` —— 侧栏每个 crew 的末条消息（2026-08-17 头号病根）。
/// - `SessionAwaitingReplyInputsCache` —— 点名快照那 2 秒定时器要的审批账本 +
///   回合 marker（2026-08-18）。
///
/// ## 固定策略：指纹为 nil（文件不存在）就不 load
///
/// 两个使用方要的语义一致 —— 文件不在 = 派生值必然为空，没必要为了拿一个已知的 nil
/// 去开一次 flock。所以这条写死在这里，不做成开关。
///
/// ## mtime+size 指纹会不会漏判
///
/// 理论弱点是「同一时刻把文件改写成同样大小」。**能不能接受要由每个使用方各自论证**
/// （写路径不同，结论不能互相套用），论证写在各自的类型注释里。这一层只保证：
/// 指纹相同 → 返回上次的值；指纹不同 → 重新 load。
///
/// ## 线程
///
/// 非 `@MainActor`：**刻意**要在后台队列上跑（stat 与 load 都是磁盘 IO，
/// `FileChangeGate.fingerprint` 自己的注释就写着别在主线程调）。内部可变状态由
/// `lock` 保护；`load` / `fingerprintOf` 在锁**外**调用 —— 它们是磁盘 IO，
/// 拿着锁做 IO 会把并发刷新串成一条队。
final class FileFingerprintCache<Key: Hashable, Value>: @unchecked Sendable {
    private struct Entry {
        /// 上次求值时那个文件的指纹（nil = 那时文件不存在，也是一种状态）。
        var fingerprint: FileChangeGate.Fingerprint?
        /// 那次求值得到的派生值（nil = 确实没有值）。
        var value: Value?
    }

    private let lock = NSLock()
    private var entries: [Key: Entry] = [:]
    private var loads = 0

    private let fingerprintOf: (Key) -> FileChangeGate.Fingerprint?
    private let load: (Key) -> Value?

    /// - Parameters:
    ///   - fingerprintOf: 只 stat，不读内容。
    ///   - load: 真正的读 + 解码 + 求派生值。只在指纹变了且文件存在时被调。
    init(fingerprintOf: @escaping (Key) -> FileChangeGate.Fingerprint?,
         load: @escaping (Key) -> Value?) {
        self.fingerprintOf = fingerprintOf
        self.load = load
    }

    /// 本 cache 迄今真正做过多少次「读文件 + 解码」。缓存命中不计。
    /// 这是这一族 fix 的**验收口径**：一次 tick 里它涨多少 = 本来要付几份整份解码。
    var loadCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return loads
    }

    /// 刷新这批 key 的派生值。
    ///
    /// - Returns: key → 派生值。**键缺失 = 那个 key 确实没有值**（不是「没读」）——
    ///   每次调用都覆盖全表，所以调用方拿到的恒是完整快照（「相等就不赋值」那类
    ///   下游门控依赖这一点）。
    ///
    /// 不在本次列表里的 key 顺带淘汰，缓存不会随开机时长无限长胖。
    @discardableResult
    func refresh(keys: [Key]) -> [Key: Value] {
        lock.lock()
        let previous = entries
        lock.unlock()

        var next: [Key: Entry] = [:]
        next.reserveCapacity(keys.count)
        var result: [Key: Value] = [:]
        var loadedNow = 0

        for key in keys {
            let fingerprint = fingerprintOf(key)
            if let cached = previous[key], cached.fingerprint == fingerprint {
                next[key] = cached
                if let value = cached.value { result[key] = value }
                continue
            }
            let value: Value?
            if fingerprint == nil {
                value = nil          // 文件不存在 = 必然没有值，别为一个已知的 nil 开锁
            } else {
                loadedNow += 1
                value = load(key)
            }
            next[key] = Entry(fingerprint: fingerprint, value: value)
            if let value { result[key] = value }
        }

        lock.lock()
        entries = next
        loads += loadedNow
        lock.unlock()
        return result
    }

    /// 丢掉全部缓存（退出登录 / 换后端时调）。
    func clear() {
        lock.lock()
        entries = [:]
        lock.unlock()
    }
}
