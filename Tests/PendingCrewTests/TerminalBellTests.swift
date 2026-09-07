#if os(macOS)
import XCTest
import SwiftTerm

/// **BEL(0x07) 不许再放系统提示音，但也不许被静静吞掉**（人类 Todo #110）。
///
/// 病根：`TerminalViewDelegate.bell(source:)` 在 SwiftTerm 里是**协议要求 + 默认
/// 实现**（`Apple/TerminalViewDelegate.swift` 声明，`Mac/MacTerminalView.swift` 的
/// `extension TerminalViewDelegate` 给默认体，函数体就一行 `NSSound.beep()`）。
/// `TerminalMirrorView` 自己当自己的 `terminalDelegate`（三处装配都是），却唯独
/// 没实现这一条 —— 于是 agent 每吐一个 BEL 就走进那行 `NSSound.beep()`。
///
/// **这套测试怎么证明「没走到发声那条路」**：协议要求的方法只会被派发到**一个**
/// 实现上 —— 类里自己写了就用类的，没写才用扩展里那份会响的。所以「回调进了我们
/// 自己的实现」与「走进了 NSSound.beep()」是互斥的两件事，断言前者成立即后者不成立。
/// 变异自证：把 `TerminalMirrorView.bell(source:)` 删掉，本文件第一条立刻红
/// （而且跑测试时你会**听见**那一声 —— 那正是它要挡住的东西）。
@MainActor
final class TerminalBellTests: XCTestCase {

    /// 与生产装配一致：mirror 自己当自己的 delegate（`AgentTerminalSession` /
    /// `PlainTerminalSession` / `RemoteSessionBackend` 三处都是这么接的）。
    private func makeMirror() -> TerminalMirrorView {
        let mirror = TerminalMirrorView(frame: NSRect(x: 0, y: 0, width: 800, height: 600))
        mirror.terminalDelegate = mirror
        return mirror
    }

    /// 字节流里的 BEL → 落进我们自己的实现（不发声）+ 留下痕迹。
    func testBelInByteStreamLeavesATraceInsteadOfRingingTheSystemBeep() {
        let mirror = makeMirror()
        var rings = 0
        mirror.onBell = { rings += 1 }

        mirror.feed(byteArray: Array("done\u{07}".utf8)[...])

        XCTAssertEqual(rings, 1,
                       "BEL 必须落进 TerminalMirrorView 自己的 bell(source:)；回调没进来 = 走的是 SwiftTerm 那份 NSSound.beep() 默认实现")
        XCTAssertEqual(mirror.bellTrace.count, 1, "响过一次就要记一次")
        XCTAssertNotNil(mirror.bellTrace.lastAt, "痕迹要带时刻，否则事后答不出「什么时候响的」")
        XCTAssertTrue(mirror.bellTrace.showsHint, "刚响过、人还没看过 → session 行上要有提示")
    }

    /// 多次响铃累计；文本形态的痕迹在响过之前不存在（不长期占位）。
    func testTraceAccumulatesAndOnlyExistsAfterItHasRung() {
        let mirror = makeMirror()
        XCTAssertEqual(mirror.bellTrace.count, 0)
        XCTAssertFalse(mirror.bellTrace.showsHint, "没响过就不该有提示")
        XCTAssertNil(TerminalBellTrace.summary(count: 0, timeText: "15:04:05"),
                     "从没响过时不显示任何一句 —— 长亮的「一切正常」只会训练人忽略它")

        mirror.feed(byteArray: Array("a\u{07}b\u{07}".utf8)[...])

        XCTAssertEqual(mirror.bellTrace.count, 2)
        XCTAssertEqual(TerminalBellTrace.summary(count: 2, timeText: "15:04:05"),
                       "响铃 2 次 · 最近 15:04:05")
    }

    /// 看过之后提示消掉，但痕迹留着 —— 两层是故意分开的。
    func testAcknowledgeClearsTheHintButKeepsTheTrace() {
        var trace = TerminalBellTrace()
        trace.record(at: Date(timeIntervalSince1970: 1))
        trace.record(at: Date(timeIntervalSince1970: 2))

        trace.acknowledge()

        XCTAssertFalse(trace.showsHint, "看过之后不再亮")
        XCTAssertEqual(trace.count, 2, "痕迹不因看过而消失")
        XCTAssertEqual(trace.lastAt, Date(timeIntervalSince1970: 2))

        trace.record(at: Date(timeIntervalSince1970: 3))
        XCTAssertTrue(trace.showsHint, "看过之后又响 → 重新亮")
        XCTAssertEqual(trace.unseen, 1)
    }

    /// **拿真录制的 PTY 字节验，别只用手编的字节流**（这个仓库编尺子踩过的坑：
    /// 凭印象编的语料复现不了真病）。语料是 `AgentTuiFixtureRecorder` 在真 PTY 里
    /// 录的原始字节（入库）。
    ///
    /// 这条同时钉住两件**量出来的**事实（写这条测试时才发现，之前是猜的）：
    /// - `tui-shell.bin`：13 个 ground 态 BEL（录制里 `printf` 敲出来的）→ **会响**。
    ///   修之前，回放这段就会放出那么多声系统提示音。
    /// - `tui-claude*.bin`：这三段录制里的 BEL **全部**是 OSC 终止符
    ///   （`ESC]0;标题 BEL` 设窗口标题、`ESC]11;? BEL` 查背景色）→ **一次都不响**。
    ///   也就是说「agent 吐 BEL 就响」这句话对**这几段 claude 录制**并不成立；
    ///   会响的那种 BEL 在这套语料里只出现在 shell 那段。
    ///
    /// 断的是「>0 / ==0」而不是具体数字 —— 数字归录制那一版，会随重录变。
    func testRecordedShellOutputRingsWhileRecordedClaudeTuiOnlyUsesOscTerminators() throws {
        let shell = try loadFixture("tui-shell.bin")
        let shellMirror = makeMirror()
        shellMirror.feed(byteArray: shell[...])
        XCTAssertGreaterThan(
            shellMirror.bellTrace.count, 0,
            "shell 录制里一个响铃都没解析出来（原始 0x07 有 \(shell.filter { $0 == 0x07 }.count) 个）—— 要么录制变了，要么解析路径断了")

        for name in ["tui-claude.bin", "tui-claude-ready.bin", "tui-claude-trust.bin"] {
            let bytes = try loadFixture(name)
            let raw = bytes.filter { $0 == 0x07 }.count
            let mirror = makeMirror()
            mirror.feed(byteArray: bytes[...])
            XCTAssertEqual(
                mirror.bellTrace.count, 0,
                "\(name)：这段录制里的 \(raw) 个 BEL 本该全是 OSC 终止符（不响）—— 变了就说明 claude 的输出或 SwiftTerm 的状态机变了，值得回头看一眼")
        }
    }

    private func loadFixture(_ name: String) throws -> [UInt8] {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Fixtures", isDirectory: true)
            .appendingPathComponent(name)
        guard let data = try? Data(contentsOf: url) else {
            XCTFail("缺 fixture：\(url.path)（它是入库的，不该缺 —— 这里故意 fail 而不是静默 skip）")
            throw XCTSkip("missing fixture")
        }
        return [UInt8](data)
    }

    /// **边界，只钉现状、不改行为**：OSC 串里的 0x07 是**字符串终止符**，不是响铃
    /// （SwiftTerm 的状态机在 `.oscString` 态把 0x07 当 oscEnd，压根走不到那条
    /// `case 7: tdel?.bell(...)`）。所以 `ESC]0;title BEL` 这种设标题的序列**不留痕迹**。
    ///
    /// 这条在这里是为了钉住「痕迹只在真响铃时留下」这个前提 —— 如果哪天 SwiftTerm
    /// 换了状态机，它会红，那正是我们要知道的事。#110 不修这一族，见交付说明。
    func testOscTerminatingBelIsNotCountedAsABell() {
        let mirror = makeMirror()
        var rings = 0
        mirror.onBell = { rings += 1 }

        mirror.feed(byteArray: Array("\u{1b}]0;标题\u{07}".utf8)[...])

        XCTAssertEqual(rings, 0, "OSC 的收尾 BEL 不是响铃")
        XCTAssertEqual(mirror.bellTrace.count, 0)
    }
}
#endif
