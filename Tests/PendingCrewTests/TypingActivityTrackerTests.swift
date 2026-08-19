#if os(macOS)
import XCTest

/// 「正在输入」显示态判定单测（Todo #24：气泡亮一下灭一下循环往复）。
/// 全部喂时间序列 —— 判定里没有 `Date()`，`now` 由外部注入。
final class TypingActivityTrackerTests: XCTestCase {

    private let t0 = Date(timeIntervalSince1970: 1_800_000_000)
    private func at(_ s: TimeInterval) -> Date { t0.addingTimeInterval(s) }

    // MARK: - 迟滞

    func testRisesImmediatelyOnFirstVisibleOutput() {
        var tracker = TypingActivityTracker()
        XCTAssertFalse(tracker.isTyping(isRunning: true, now: at(0)))
        tracker.feed(plainText: "Thinking… (1s)", at: at(1))
        XCTAssertTrue(tracker.isTyping(isRunning: true, now: at(1)))
    }

    /// 老实现 1s 就灭 —— 一次工具调用的思考停顿就能把气泡掐灭。
    func testStaysOnThroughShortGapUnderQuietFall() {
        var tracker = TypingActivityTracker()
        tracker.feed(plainText: "写文件…", at: at(1))
        XCTAssertTrue(tracker.isTyping(isRunning: true, now: at(2.4)))
    }

    func testFallsAfterQuietWindow() {
        var tracker = TypingActivityTracker()
        tracker.feed(plainText: "写文件…", at: at(1))
        XCTAssertFalse(tracker.isTyping(isRunning: true, now: at(3.6)))
    }

    func testKeepsOnWhileOutputChangesEverySecond() {
        var tracker = TypingActivityTracker()
        for i in 0..<20 {
            tracker.feed(plainText: "✳ Thinking… (\(i)s · ↑ \(i * 40) tokens)", at: at(Double(i)))
            XCTAssertTrue(tracker.isTyping(isRunning: true, now: at(Double(i) + 0.5)),
                          "第 \(i) 秒不该灭")
        }
        XCTAssertFalse(tracker.isTyping(isRunning: true, now: at(22)))
    }

    func testNeverTypingWhenNotRunning() {
        var tracker = TypingActivityTracker()
        tracker.feed(plainText: "输出", at: at(1))
        XCTAssertFalse(tracker.isTyping(isRunning: false, now: at(1)))
    }

    // MARK: - 指纹（治本的那条）

    /// 2026-08-08 PTY 实测：空闲 claude 的心跳块只有清行 + 光标归位，明文为空。
    func testControlOnlyOutputIsNotWork() {
        var tracker = TypingActivityTracker()
        tracker.feed(plainText: "\r\n   \n", at: at(1))
        XCTAssertFalse(tracker.isTyping(isRunning: true, now: at(1)))
    }

    /// 空闲期反复重画同一行提示 —— 老实现每次都点亮 1s 窗口，这才是「循环往复」的源头。
    func testRepeatedIdenticalRepaintDoesNotKeepIndicatorAlive() {
        var tracker = TypingActivityTracker()
        // 第一帧算数（画面确实变了），之后每 3 秒重画同一帧。
        tracker.feed(plainText: "› 试试 \"修一下这个 bug\"", at: at(0))
        XCTAssertTrue(tracker.isTyping(isRunning: true, now: at(0.1)))
        for i in 1...10 {
            tracker.feed(plainText: "› 试试 \"修一下这个 bug\"", at: at(Double(i) * 3))
            XCTAssertFalse(tracker.isTyping(isRunning: true, now: at(Double(i) * 3)),
                           "第 \(i) 次重绘不该把气泡点亮")
        }
    }

    /// 心跳常常是几帧轮流重画 —— 只记一帧会被 A,B,A,B 骗过去。
    func testAlternatingRepaintFramesSettleQuiet() {
        var tracker = TypingActivityTracker()
        let frames = ["› 提示 A", "› 提示 B"]
        for i in 0..<12 {
            tracker.feed(plainText: frames[i % 2], at: at(Double(i) * 3))
        }
        XCTAssertFalse(tracker.isTyping(isRunning: true, now: at(33)),
                       "轮流重绘不该让气泡长亮")
    }

    /// 重绘不算数，但真的新文本立刻算数（别把气泡整个压死）。
    func testNewTextAfterRepaintsRisesAgain() {
        var tracker = TypingActivityTracker()
        for i in 0..<6 { tracker.feed(plainText: "› 提示", at: at(Double(i) * 3)) }
        XCTAssertFalse(tracker.isTyping(isRunning: true, now: at(15.1)))
        tracker.feed(plainText: "好的，我来看一下这个文件", at: at(20))
        XCTAssertTrue(tracker.isTyping(isRunning: true, now: at(20.2)))
    }

    /// 同一句话被挪个位置重画（终端重绘常见）也认作同一帧。
    func testWhitespaceInsensitiveSignature() {
        var tracker = TypingActivityTracker()
        tracker.feed(plainText: "5 MCP servers need authentication", at: at(0))
        tracker.feed(plainText: "5   MCP servers\n need authentication\r\n", at: at(5))
        XCTAssertFalse(tracker.isTyping(isRunning: true, now: at(5)))
    }

    func testResetClearsFingerprintsAndTimer() {
        var tracker = TypingActivityTracker()
        tracker.feed(plainText: "首屏", at: at(0))
        tracker.reset()
        XCTAssertFalse(tracker.isTyping(isRunning: true, now: at(0.1)))
        tracker.feed(plainText: "首屏", at: at(1))
        XCTAssertTrue(tracker.isTyping(isRunning: true, now: at(1.1)), "reset 后同一帧应重新算数")
    }

    // MARK: - 视口重绘（Todo #32：切 crew 时气泡闪一下）

    /// 2026-08-08 PTY 实测 claude 2.1.226 收到 SIGWINCH 后吐的整屏重绘（明文骨架）。
    /// 列宽变了，那两条横线的长度就变 —— 所以两次重绘**指纹必然不同**，
    /// 「见过的指纹」那条判据一条都拦不住，只能靠视口宽限窗。
    private func fullRepaint(cols: Int) -> String {
        let rule = String(repeating: "─", count: cols)
        return """
         ▐▛███▜▌   Claude Code v2.1.226
         ▝▜█████▛▘  Opus 5 with high effort · Claude Max
          ▘▘ ▝▝    ~/Untitled/Pendingname/PendingBot/dev
        \(rule)
         ❯
        \(rule)
          ⏵⏵ auto mode on
        """
    }

    /// 病根钉死：切 crew 让终端视图重挂 → frame .zero → 真实尺寸 → 两次 SIGWINCH →
    /// 两屏重绘，全程不该点亮气泡。
    func testViewportRepaintDoesNotLightBubble() {
        var tracker = TypingActivityTracker()
        // 第一波：真实尺寸 → 0 列。
        tracker.noteViewportChange(at: at(0))
        tracker.feed(plainText: fullRepaint(cols: 1), at: at(0.01))
        XCTAssertFalse(tracker.isTyping(isRunning: true, now: at(0.02)),
                       "卸载引发的重绘不该点亮")
        // 第二波：0 列 → 真实尺寸（实测 4–15ms 后到）。
        tracker.noteViewportChange(at: at(0.05))
        tracker.feed(plainText: fullRepaint(cols: 80), at: at(0.06))
        XCTAssertFalse(tracker.isTyping(isRunning: true, now: at(0.07)),
                       "重新挂载引发的整屏重绘不该点亮 —— 这就是 #32 的病根")
        // 而且此后一直安静（不是「亮了 2.5 秒才灭」）。
        XCTAssertFalse(tracker.isTyping(isRunning: true, now: at(1.0)))
        XCTAssertFalse(tracker.isTyping(isRunning: true, now: at(2.4)))
    }

    /// 宽限窗只盖重绘那一瞬 —— 之后 agent 真开口，照旧**即时**点亮。
    func testRealTurnAfterViewportChangeStillRisesImmediately() {
        var tracker = TypingActivityTracker()
        tracker.noteViewportChange(at: at(0))
        tracker.feed(plainText: fullRepaint(cols: 80), at: at(0.01))
        XCTAssertFalse(tracker.isTyping(isRunning: true, now: at(0.02)))
        tracker.feed(plainText: "好的，我来看一下这个文件", at: at(0.5))
        XCTAssertTrue(tracker.isTyping(isRunning: true, now: at(0.5)),
                      "宽限窗过后的真输出必须即时点亮")
    }

    /// 回合**中途**撞上 resize（人拖窗宽 / 切走再切回来）不该把气泡掐灭 ——
    /// 已亮的 `lastMeaningfulAt` 不受影响，宽限窗一过真输出立刻续上。
    func testViewportChangeMidTurnDoesNotExtinguish() {
        var tracker = TypingActivityTracker()
        tracker.feed(plainText: "✳ Thinking… (1s)", at: at(0))
        XCTAssertTrue(tracker.isTyping(isRunning: true, now: at(0.1)))
        tracker.noteViewportChange(at: at(0.5))
        tracker.feed(plainText: fullRepaint(cols: 80), at: at(0.51))  // 被宽限窗吞掉
        XCTAssertTrue(tracker.isTyping(isRunning: true, now: at(2.4)), "回合中途不该误灭")
        tracker.feed(plainText: "✳ Thinking… (2s · ↑ 120 tokens)", at: at(1.0))
        XCTAssertTrue(tracker.isTyping(isRunning: true, now: at(3.0)), "真输出应把气泡续上")
    }

    /// 宽限窗里被吞掉的那帧仍进指纹记忆 —— 窗一过又原样重画一遍，判据 1 接着挡。
    func testRepaintSwallowedInGraceIsStillRemembered() {
        var tracker = TypingActivityTracker()
        tracker.noteViewportChange(at: at(0))
        tracker.feed(plainText: fullRepaint(cols: 80), at: at(0.01))
        tracker.feed(plainText: fullRepaint(cols: 80), at: at(1.0))   // 窗外，但同一帧
        XCTAssertFalse(tracker.isTyping(isRunning: true, now: at(1.0)),
                       "同一帧重绘无论在不在宽限窗内都不算干活")
    }

    /// 没发生过视口变化时，行为与 #24 完全一致（宽限窗不该无条件生效）。
    func testNoViewportChangeKeepsPreviousBehaviour() {
        var tracker = TypingActivityTracker()
        tracker.feed(plainText: fullRepaint(cols: 80), at: at(0))
        XCTAssertTrue(tracker.isTyping(isRunning: true, now: at(0)),
                      "没 resize 过的首屏输出仍算干活")
    }

    // MARK: - 指纹：等价性 + 耗时预算（Todo #59）

    /// **老实现的原样拷贝**，只当参照系。它把整段折叠完再截断，正因为这样才被
    /// 换掉 —— 但它定义了「对」的输出，所以留在这里逐字符比对。别改这个函数。
    private func signatureReferenceWholeText(_ text: String, limit: Int) -> String {
        let collapsed = text.split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
        return collapsed.count > limit ? String(collapsed.prefix(limit)) : collapsed
    }

    func testSignatureMatchesLegacyImplementation() {
        var long = ""
        while long.count < 4000 { long += "⎿  · Thinking… 中文一行  tokens 12345 \n\n  " }
        let cases: [String] = [
            "", "   ", "\n\t\n", "abc", "  abc  ", "a   b\n\nc  ",
            "e\u{301}\u{327} x", "🇨🇳 👩‍👩‍👧 ✻", String(repeating: "e\u{301}", count: 900),
            long,
        ]
        for limit in [1, 8, 512] {
            for c in cases {
                XCTAssertEqual(TypingActivityTracker.signature(c, limit: limit),
                               signatureReferenceWholeText(c, limit: limit),
                               "limit=\(limit) 上与老实现不等价：\(c.prefix(24).debugDescription)")
            }
        }
    }

    /// 指纹的代价必须与**输入长度脱钩** —— 它只用前 512 个字符，却挂在主线程的
    /// PTY 回调上。老实现整段都要折一遍：agent 打印一份大文件时那一笔就很贵。
    /// 这条量的是「喂 100 倍长的输入，耗时不该跟着涨 100 倍」。
    func testSignatureCostIsDecoupledFromInputLength() {
        var short = ""
        while short.count < 2_000 { short += "⎿  · Thinking… 中文一行 tokens 12345 " }
        let long = String(repeating: short, count: 100)   // ~200K 字符
        func cost(_ s: String) -> Double {
            let t0 = DispatchTime.now().uptimeNanoseconds
            for _ in 0..<20 { _ = TypingActivityTracker.signature(s, limit: 512) }
            return Double(DispatchTime.now().uptimeNanoseconds - t0) / 1_000_000
        }
        _ = cost(short)                                   // 预热
        let cShort = cost(short), cLong = cost(long)
        XCTAssertLessThan(cLong, max(cShort * 5, 20),
                          "短 \(String(format: "%.1f", cShort)) ms vs 长 \(String(format: "%.1f", cLong)) ms"
                          + " —— 指纹代价又跟输入长度挂钩了")
    }
}
#endif
