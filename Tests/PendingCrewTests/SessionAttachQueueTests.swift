#if os(macOS)
import XCTest
import SwiftTerm

/// 背压 / 重同步（前后端分离 P3 / 设计 §5.4 / §7.5）。
///
/// 这一组要钉的不是「性能」，是**一条比性能重要的硬规则**：绝不允许「丢掉中间
/// 一段字节、继续发后面的」。它的失败方式是静默的 —— 窗口那份 `Terminal` 从此
/// 一直是歪的，没有任何报错。所以这里用一个**故意慢的假 viewer** 把队列灌爆，
/// 然后把 viewer 真正收到的每一帧原样重放进一个干净终端，逐格比对。
///
/// **这组断言当场证过会红**（2026-08-26，按 CONTRIBUTING「先证明它会红」）：把队列
/// 改成溢出时「丢掉最老的一批、继续发后面的」——也就是 §5.4 明令禁止的那种做法——
/// 5 条测试里 3 条红、共 8 处断言炸，其中 viewer 那份终端从 601 行掉到 150 行、
/// 第 11 行起整片对不上。这正是真实世界里那种「画面从此一直是歪的、没有任何报错」
/// 的坏法，只不过在这儿它响了。
final class SessionAttachQueueTests: XCTestCase {

    /// 故意慢的假 viewer：只在被允许时取走有限的几帧，其余全压在队列里。
    ///
    /// 它同时是「重放器」：收到 `.resync` 就把手上这份终端整个扔掉重来 —— 那正是
    /// resync 在真实 viewer 里的含义。
    private final class SlowViewer {
        private(set) var terminal: HeadlessTerminalHarness
        private var snapshotBuffer: [UInt8] = []
        private(set) var sawResync = 0
        /// 每一代收到的实时字节数，用来核「本代一个字节都没漏」。
        private(set) var liveBytesInEpoch = 0

        init(cols: Int, rows: Int, scrollback: Int) {
            terminal = HeadlessTerminalHarness(cols: cols, rows: rows, scrollback: scrollback)
        }

        func consume(_ frame: SessionAttachQueue.Frame, scrollback: Int) {
            switch frame {
            case .resync:
                sawResync += 1
                snapshotBuffer = []
                liveBytesInEpoch = 0
            case .snapshot(_, let isLast, let cols, let rows, let bytes):
                snapshotBuffer.append(contentsOf: bytes)
                if isLast {
                    // 真 viewer 在这里也是「先按快照给的尺寸 resize，再喂」。
                    terminal = HeadlessTerminalHarness(cols: cols, rows: rows,
                                                       scrollback: scrollback)
                    terminal.feed(snapshotBuffer)
                    snapshotBuffer = []
                }
            case .live(let bytes):
                XCTAssertTrue(snapshotBuffer.isEmpty,
                              "实时字节绝不许出现在快照末片之前")
                liveBytesInEpoch += bytes.count
                terminal.feed(bytes)
            }
        }
    }

    /// 造一份 (源终端, 队列)。源终端每吃一批字节就把同一批排进队列 —— 顺序与
    /// daemon 里一致：**先喂权威 Terminal，再 append**。
    private func makeRig(capacityBytes: Int, chunkBytes: Int = 64 * 1024,
                         cols: Int = 80, rows: Int = 25, scrollback: Int = 10_000)
        -> (HeadlessTerminalHarness, SessionAttachQueue) {
        let source = HeadlessTerminalHarness(cols: cols, rows: rows, scrollback: scrollback)
        let queue = SessionAttachQueue(capacityBytes: capacityBytes, chunkBytes: chunkBytes) {
            TerminalSnapshotEncoder.encode(source.terminal, probe: source.probe)
        }
        return (source, queue)
    }

    private func push(_ source: HeadlessTerminalHarness, _ queue: SessionAttachQueue,
                      _ text: String) {
        let bytes = Array(text.utf8)
        source.feed(bytes)
        queue.append(bytes)
    }

    // MARK: - 分片

    func testSnapshotIsChunkedAndLastChunkIsMarked() {
        let (source, queue) = makeRig(capacityBytes: 8 * 1024 * 1024, chunkBytes: 1024)
        for i in 0..<400 { source.feed("chunky line \(i) " + String(repeating: "x", count: 60) + "\r\n") }
        queue.begin()

        var seqs: [Int] = []
        var lastCount = 0
        var assembled: [UInt8] = []
        while let frame = queue.next() {
            guard case .snapshot(let seq, let isLast, _, _, let bytes) = frame else {
                return XCTFail("begin() 之后应当只有快照片")
            }
            seqs.append(seq)
            assembled.append(contentsOf: bytes)
            if isLast { lastCount += 1 }
            if !isLast { XCTAssertEqual(bytes.count, 1024, "非末片必须是满片") }
        }
        XCTAssertEqual(seqs, Array(0..<seqs.count), "seq 必须从 0 连号")
        XCTAssertGreaterThan(seqs.count, 1, "前置条件：这段语料该切出不止一片")
        XCTAssertEqual(lastCount, 1, "有且只有一片是末片")

        let expected = TerminalSnapshotEncoder.encode(source.terminal, probe: source.probe).bytes
        XCTAssertEqual(assembled, expected, "拼回来必须与快照原样一致")
    }

    // MARK: - §7.5 背压

    func testSlowViewerGetsResyncAndEndsUpIdentical() {
        // 容量故意压得很小，好让「慢 viewer」这件事在几十行之内就发生。
        let (source, queue) = makeRig(capacityBytes: 4 * 1024, chunkBytes: 4096)
        let viewer = SlowViewer(cols: 80, rows: 25, scrollback: 10_000)
        queue.begin()

        for i in 0..<600 {
            push(source, queue, "\u{1b}[3\(i % 8)mflood row \(i) "
                 + String(repeating: "y", count: 40) + "\u{1b}[0m\r\n")
            // 故意慢：每 7 批才取一帧，队列必然涨破上限。
            if i % 7 == 0, let frame = queue.next() {
                viewer.consume(frame, scrollback: 10_000)
            }
        }
        while let frame = queue.next() { viewer.consume(frame, scrollback: 10_000) }

        XCTAssertGreaterThan(queue.diagnostics.resyncCount, 0,
                             "前置条件：这一趟必须真的把队列灌爆过")
        // viewer 收到的 resync 数**必然少于**溢出次数：连着溢出好几次只合并成一份
        // 最新的快照（见 SessionAttachQueue 规矩 2）。少的那些是被更新的一份取代掉
        // 的，不是丢的。
        XCTAssertGreaterThan(viewer.sawResync, 0, "灌爆过就该真收到 resync")
        XCTAssertEqual(viewer.sawResync, queue.diagnostics.snapshotsSent - 1,
                       "除了 attach 那一份，每份快照前面都该有一条 resync")
        XCTAssertLessThanOrEqual(queue.diagnostics.snapshotsSent,
                                 queue.diagnostics.resyncCount + 1,
                                 "快照份数不许超过「溢出次数 + attach 那一份」")

        assertCellEqual(source, viewer.terminal, "重同步之后")
    }

    /// **这一条就是那条硬规则本身**：本代内交付的实时字节数，必须与源头在本代快照
    /// 之后吐出的字节数**分毫不差**。少一个字节就意味着「丢了中间一段继续发」——
    /// 那正是会让窗口那份 Terminal 永久错乱、而且不报任何错的那种坏法。
    func testNoBytesAreEverDroppedInTheMiddleOfAnEpoch() {
        let (source, queue) = makeRig(capacityBytes: 8 * 1024, chunkBytes: 4096)
        queue.begin()
        var drained: [SessionAttachQueue.Frame] = []

        for i in 0..<400 {
            push(source, queue, "row \(i) " + String(repeating: "z", count: 50) + "\r\n")
            if i % 5 == 0, let f = queue.next() { drained.append(f) }
        }
        while let f = queue.next() { drained.append(f) }

        // 从最后一次 resync 之后开始数（之前那几代是被整代丢掉的，那是允许的）。
        let lastResync = drained.lastIndex(of: .resync) ?? 0
        var liveAfterLastEpoch = 0
        for frame in drained[lastResync...] {
            if case .live(let bytes) = frame { liveAfterLastEpoch += bytes.count }
        }
        let expected = queue.diagnostics.totalAppended - queue.diagnostics.epochBaseAppended
        XCTAssertEqual(liveAfterLastEpoch, expected,
                       "本代交付的实时字节必须与源头本代吐出的字节分毫不差 —— "
                       + "差一个字节就是「丢了中间一段继续发」，那是 §5.4 明令禁止的")
        XCTAssertEqual(queue.diagnostics.deliveredLiveInEpoch, expected,
                       "队列自己的账也要对得上")
    }

    /// 合并规矩本身（`SessionAttachQueue` 规矩 2）：viewer 一帧不取、连着溢出很多次，
    /// 也只该拍出**一份**新快照。
    ///
    /// 第一版没有这条规矩：600 批字节触发了 **526 次重同步**，每次都重拍一份几 MB
    /// 的快照，而其中 525 份在被人看到之前就被下一次重同步丢掉了 —— 纯烧 CPU，
    /// 一个字节也没多送出去。
    func testRepeatedOverflowWithoutAnyReadCoalescesIntoOneSnapshot() {
        let (source, queue) = makeRig(capacityBytes: 2 * 1024, chunkBytes: 4096)
        queue.begin()
        _ = queue.next()                       // 取走 attach 那一份的第一片，进入推流
        while let f = queue.next(), case .snapshot = f {}

        let snapshotsBefore = queue.diagnostics.snapshotsSent
        for i in 0..<200 {                     // 一帧都不取，一路灌
            push(source, queue, "burst \(i) " + String(repeating: "q", count: 60) + "\r\n")
        }
        XCTAssertEqual(queue.diagnostics.resyncCount, 1,
                       "200 批灌爆只该标脏一次 —— 已经脏了就没有「再脏一次」这回事")
        XCTAssertEqual(queue.diagnostics.snapshotsSent, snapshotsBefore,
                       "没人来取之前，一份快照都不该拍")
        XCTAssertGreaterThan(queue.diagnostics.totalAppended, 2 * 1024 * 5,
                             "前置条件：这一趟灌进去的字节要远超上限，才谈得上溢出")

        _ = queue.next()
        XCTAssertEqual(queue.diagnostics.snapshotsSent, snapshotsBefore + 1,
                       "200 批灌爆只该合并成 1 份快照")
    }

    func testNoOverflowMeansNoResyncAndPlainForwarding() {
        let (source, queue) = makeRig(capacityBytes: 8 * 1024 * 1024)
        let viewer = SlowViewer(cols: 80, rows: 25, scrollback: 10_000)
        queue.begin()
        for i in 0..<200 { push(source, queue, "calm row \(i)\r\n") }
        while let frame = queue.next() { viewer.consume(frame, scrollback: 10_000) }

        XCTAssertEqual(queue.diagnostics.resyncCount, 0, "没溢出就不许重同步")
        XCTAssertEqual(viewer.sawResync, 0)
        assertCellEqual(source, viewer.terminal, "未溢出的原样转发")
    }

    // MARK: -

    private func assertCellEqual(_ a: HeadlessTerminalHarness, _ b: HeadlessTerminalHarness,
                                 _ what: String,
                                 file: StaticString = #filePath, line: UInt = #line) {
        let x = TerminalFlattener.flatten(a.terminal)
        let y = TerminalFlattener.flatten(b.terminal)
        XCTAssertEqual(x.activeLines.count, y.activeLines.count, "\(what)：行数",
                       file: file, line: line)
        for (i, (l, r)) in zip(x.activeLines, y.activeLines).enumerated() where l != r {
            XCTFail("\(what)：第 \(i) 行对不上\n  源 \(l)\n  viewer \(r)", file: file, line: line)
            break
        }
        XCTAssertEqual(x.cursor, y.cursor, "\(what)：光标", file: file, line: line)
    }
}
#endif
