import Foundation

/// 白板目录监听的两块收敛逻辑（#443：点进群聊转彩虹圈）。
///
/// 病根现场：`whiteboards/` 目录里除了 `<crewId>.json` 白板本身，还堆着每个
/// session 的 `.cursor` / `.turn` / approvals / todos / wakeups / quota /
/// agent-sessions / crew-sessions —— 有 session 活着时它们**持续在写**（实测
/// 目录 648 个文件）。`DispatchSource` 一个事件直发一个 tick，订阅方（尤其群聊
/// 中栏）就按别人的写盘频率被反复拉起整表重排：`sample` 抓到主线程 100% busy，
/// 整条栈在 `LazyHVStack.lengthAndSpacing → sizeThatFits`。
///
/// 这里放两道闸，**都不改 `directoryChanged` 的语义**（todo / approvals / 改名
/// 通道 / listen / mention 唤醒都订阅同一个 subject，各自关心的文件不同，不能
/// 一刀切只放行白板 json）：
///
/// 1. `DirectoryEventCoalescer` —— 目录事件按固定窗口合流，全体订阅方受益。
/// 2. `FileChangeGate` —— 订阅方**自己**判断「我关心的那个文件真变了吗」。
///    目前只有群聊白板流用它；别的订阅方要不要用，各自决定。

// MARK: - 事件合流

/// 固定窗口合流器。窗口内第一个事件安排一次 flush，其余事件被吸收；flush 时
/// 出一个 tick。
///
/// 刻意**不是** trailing debounce（每来一个事件就把定时器往后推）—— 那种写法在
/// 「有 session 一直在写状态文件」这种持续事件流下会被无限推迟，UI 反而永远等不
/// 到刷新。固定窗口保证：至多 `window` 出一次 tick，且永不饿死。
///
/// 只做判定，不持有队列/定时器 —— 定时由调用方（`LocalWhiteboardStore`）在自己
/// 的后台队列上排，逻辑这层就能脱离时序单测。
final class DirectoryEventCoalescer: @unchecked Sendable {
    private let lock = NSLock()
    private var flushPending = false

    /// 收到一个目录事件。
    /// - Returns: true = 调用方应在窗口后调一次 `flush()`；
    ///   false = 已经有 flush 在路上，本次被吸收。
    func noteEvent() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard !flushPending else { return false }
        flushPending = true
        return true
    }

    /// 窗口到期。
    /// - Returns: true = 确有待发事件，调用方发 tick。
    func flush() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard flushPending else { return false }
        flushPending = false
        return true
    }
}

// MARK: - 相关性判定

/// 「我关心的那个文件真的变了吗」。
///
/// 目录事件不带文件名，收到 tick 只知道「目录里有东西动过」。订阅方拿着自己那个
/// 文件的 mtime+size 指纹比一下：没变就别 yield，别人写自己的状态文件不再把整条
/// 群聊拉起来重排。
///
/// `nil` 指纹 = 文件此刻不存在，也是一种状态：出现 / 消失都算变化。
struct FileChangeGate {
    struct Fingerprint: Equatable {
        /// `contentModificationDate`，秒（APFS 亚秒精度保留在 Double 里）。
        let modified: TimeInterval
        let size: Int
    }

    private var last: Fingerprint?

    /// `seed` = 建流那一刻的指纹。订阅方建流前一般已经先全量拉过一次，所以从
    /// 当下状态起步，不会因为第一个无关事件白刷一遍。
    init(seed: Fingerprint?) {
        last = seed
    }

    /// - Returns: true = 与上次记录不同（调用方应 yield）。
    mutating func shouldYield(_ current: Fingerprint?) -> Bool {
        guard current != last else { return false }
        last = current
        return true
    }

    /// 记下当前指纹但不判定 —— 用在「本进程自己刚写完、已经沿别的路 yield 过」的
    /// 场合：同一次写盘随后还会触发一个目录事件，不吞掉就会双份刷新。
    mutating func sync(_ current: Fingerprint?) {
        last = current
    }

    /// 读一个文件的指纹。文件不存在 / 读不到属性 → nil。
    /// **别在主线程调**：一次 stat 比重拉整板便宜几个数量级，但它仍是磁盘 IO。
    static func fingerprint(of url: URL) -> Fingerprint? {
        // 每次新建 URL 值来读，避开 NSURL 的 resource-value 缓存拿到陈旧结果。
        let fresh = URL(fileURLWithPath: url.path)
        guard let values = try? fresh.resourceValues(
                forKeys: [.contentModificationDateKey, .fileSizeKey]),
              let modified = values.contentModificationDate,
              let size = values.fileSize else { return nil }
        return Fingerprint(modified: modified.timeIntervalSinceReferenceDate, size: size)
    }
}

/// `FileChangeGate` 的加锁盒子。群聊白板流的两个上游（本进程 `changes` 在
/// MainActor 路径、跨进程目录事件在后台队列）会同时碰同一个 gate。
final class FileChangeGateBox: @unchecked Sendable {
    private let lock = NSLock()
    private var gate: FileChangeGate

    init(seed: FileChangeGate.Fingerprint?) {
        gate = FileChangeGate(seed: seed)
    }

    func shouldYield(_ current: FileChangeGate.Fingerprint?) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return gate.shouldYield(current)
    }

    func sync(_ current: FileChangeGate.Fingerprint?) {
        lock.lock()
        defer { lock.unlock() }
        gate.sync(current)
    }
}
