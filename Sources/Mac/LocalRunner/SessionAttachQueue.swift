#if os(macOS)
import Foundation

/// 一个 attach 的待发队列 —— 背压与重同步（设计
/// `docs/internal/2026-08-19-backend-split-design.md` §5.4）。
///
/// ## 这里只有一条硬规则
///
/// **绝不允许「丢掉中间一段字节、继续发后面的」。** 转义序列被拦腰截断会让窗口
/// 那份 `Terminal` 永久错乱，而且错法是静默的 —— 画面从此一直是歪的，没有任何
/// 报错，谁也查不到是这儿。所以溢出时的动作不是「丢一点」，而是**整个待发队列
/// 全丢、发一条 resync、重发一份完整快照**。后台那份 `Terminal` 从不丢字节，
/// 所以快照永远是对的：要么完整的字节流，要么一份干净的快照重来，没有第三种。
///
/// ## 两条不显然、但少一条就活锁的规矩（都是被 §7.5 那组测试逼出来的）
///
/// 1. **上限只管实时积压，不管快照。** 10000 行回滚的快照本身就有好几 MB，比
///    2 MB 的上限还大。如果把快照也算进上限，那么「排进快照 → 立刻超限 → 丢掉
///    重排快照」会无限循环，一个字节都发不出去。第一版就是这么写的，测试当场
///    抓到 600 批字节触发了 526 次重同步。快照是**必须发的载荷**，丢掉它不解决
///    任何问题。
/// 2. **脏了之后不立刻拍快照，等 viewer 真来取的时候才拍。** 否则每来一批字节就
///    重拍一次几 MB 的快照，而其中 525 份在被人看到之前就已经被下一次重同步丢掉了。
///    延后到取的那一刻拍，既省掉那 525 次，又保证 viewer 拿到的是**当时最新**的
///    那一份 —— 更省也更对。
///
/// ## 调用顺序是有要求的，别写反
///
/// 每一批 PTY 字节必须**先喂给权威 `Terminal`、再 `append` 进这里**：
/// ```
/// terminal.feed(bytes)      // 权威缓冲区先吃
/// queue.append(bytes)       // 再排队发给 viewer
/// ```
/// 写反了会在重同步那一拍悄悄丢一批 —— 那时重拍的快照还没吃到这批字节，而这批
/// 字节又跟着队列一起被丢了。这是本文件唯一一处顺序依赖，所以写在最显眼的地方。
///
/// ## 与 P2 的分工
///
/// 这里产出的是**逻辑帧**。上线成什么样的字节（`kind=2` 帧头、`seq` 怎么编、
/// 64KB 怎么切上线）由 P2 的协议定义，这里只保证「切片有序、末片之后才是实时字节」。
final class SessionAttachQueue {

    /// 实时积压的上限。超过就重同步 —— 见类型注释里那条硬规则。
    static let defaultCapacityBytes = 2 * 1024 * 1024
    /// 快照切片大小。
    static let defaultChunkBytes = 64 * 1024

    /// viewer 能收到的东西。
    enum Frame: Equatable {
        /// 「把你手上的都扔了，下面是从头开始的一份快照」。
        case resync
        /// 快照的一片。`isLast` 之后才会开始出现 `.live`。
        case snapshot(seq: Int, isLast: Bool, cols: Int, rows: Int, bytes: [UInt8])
        /// 实时 PTY 字节。
        case live([UInt8])
    }

    /// 给日志和测试看的账（§8.5 的滚动日志要的就是这几个数）。
    struct Diagnostics: Equatable {
        /// 溢出过几次（= 队列被整个丢掉过几次）。
        var resyncCount = 0
        /// 真正拍出去过几份快照。**会小于 `resyncCount`** —— 连着溢出好几次只会
        /// 合并成一份最新的快照，那是规矩 2 的直接结果。
        var snapshotsSent = 0
        /// 现在第几代 —— 每拍一份快照 +1。
        var epoch = 0
        /// 本代的快照拍下时，源头总共已经吐过多少字节。
        var epochBaseAppended = 0
        /// 源头总共吐过多少字节。
        var totalAppended = 0
        /// 本代已经交给 viewer 多少实时字节。
        var deliveredLiveInEpoch = 0
        /// 实时积压现在压着多少字节（不含未发完的快照片）。
        var pendingLiveBytes = 0
    }

    /// 队列当下在干什么。
    private enum Mode {
        /// 该拍一份快照了，但还没拍（等 viewer 来取的那一刻才拍，见规矩 2）。
        /// `withResyncMarker` 只有第一份快照是 false —— 那是 attach，不是重同步。
        case needsSnapshot(withResyncMarker: Bool)
        /// 快照已拍好、正在一片片往外发。这期间来的实时字节照常排队。
        case deliveringSnapshot
        /// 快照发完了，纯推流。
        case streaming
    }

    private let capacityBytes: Int
    private let chunkBytes: Int
    private let makeSnapshot: () -> TerminalSnapshotEncoder.Snapshot

    private var mode: Mode = .streaming
    private var snapshotFrames: [Frame] = []
    private var liveFrames: [[UInt8]] = []
    private(set) var diagnostics = Diagnostics()

    init(capacityBytes: Int = SessionAttachQueue.defaultCapacityBytes,
         chunkBytes: Int = SessionAttachQueue.defaultChunkBytes,
         makeSnapshot: @escaping () -> TerminalSnapshotEncoder.Snapshot) {
        self.capacityBytes = capacityBytes
        self.chunkBytes = chunkBytes
        self.makeSnapshot = makeSnapshot
    }

    /// attach 的第一件事：安排一份快照。真正拍是在 viewer 第一次来取的时候。
    func begin() {
        mode = .needsSnapshot(withResyncMarker: false)
        snapshotFrames = []
        liveFrames = []
        diagnostics.pendingLiveBytes = 0
    }

    /// 源头吐出一批字节。**调用前它必须已经喂过权威 `Terminal`**（见类型注释）。
    func append(_ bytes: [UInt8]) {
        guard !bytes.isEmpty else { return }
        diagnostics.totalAppended += bytes.count

        if case .needsSnapshot = mode {
            // 快照还没拍，拍的时候自然会把这批字节一起包进去 —— 不用排队，
            // 也**不是**「丢字节」：它们会以快照的形态送到。
            return
        }
        if diagnostics.pendingLiveBytes + bytes.count > capacityBytes {
            markDirty()
            return
        }
        liveFrames.append(bytes)
        diagnostics.pendingLiveBytes += bytes.count
    }

    /// viewer 取走一帧。没有就是 nil。
    func next() -> Frame? {
        if case .needsSnapshot(let marker) = mode {
            materializeSnapshot(withResyncMarker: marker)
        }
        if !snapshotFrames.isEmpty {
            let frame = snapshotFrames.removeFirst()
            if snapshotFrames.isEmpty { mode = .streaming }
            return frame
        }
        guard !liveFrames.isEmpty else { return nil }
        let bytes = liveFrames.removeFirst()
        diagnostics.pendingLiveBytes -= bytes.count
        diagnostics.deliveredLiveInEpoch += bytes.count
        return .live(bytes)
    }

    var isEmpty: Bool {
        if case .needsSnapshot = mode { return false }
        return snapshotFrames.isEmpty && liveFrames.isEmpty
    }

    // MARK: -

    /// 溢出：整个待发队列全丢，标记「该重来一份快照了」。
    private func markDirty() {
        diagnostics.resyncCount += 1
        snapshotFrames = []
        liveFrames = []
        diagnostics.pendingLiveBytes = 0
        mode = .needsSnapshot(withResyncMarker: true)
    }

    /// 「序列化 + 切到推流」在同一拍里做完，保证快照与后续字节之间不丢不重。
    private func materializeSnapshot(withResyncMarker: Bool) {
        let snapshot = makeSnapshot()
        diagnostics.snapshotsSent += 1
        diagnostics.epoch += 1
        // 快照是**在此刻**从权威 Terminal 拍的，所以它已经包含了到目前为止的每一个
        // 字节 —— 本代的实时字节从这个数往后算。
        diagnostics.epochBaseAppended = diagnostics.totalAppended
        diagnostics.deliveredLiveInEpoch = 0

        var frames: [Frame] = withResyncMarker ? [.resync] : []
        let chunks = chunk(snapshot.bytes)
        for (i, piece) in chunks.enumerated() {
            frames.append(.snapshot(seq: i,
                                    isLast: i == chunks.count - 1,
                                    cols: snapshot.cols,
                                    rows: snapshot.rows,
                                    bytes: piece))
        }
        snapshotFrames = frames
        mode = .deliveringSnapshot
    }

    private func chunk(_ bytes: [UInt8]) -> [[UInt8]] {
        guard !bytes.isEmpty else { return [[]] }
        var result: [[UInt8]] = []
        var i = 0
        while i < bytes.count {
            let end = min(i + chunkBytes, bytes.count)
            result.append(Array(bytes[i..<end]))
            i = end
        }
        return result
    }
}
#endif
