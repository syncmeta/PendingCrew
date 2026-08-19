#if os(macOS)
import XCTest
import SwiftTerm

/// **两份缓冲区必须逐格一致**（前后端分离 P1）。
///
/// core 那份是状态权威（`inspect_session` 读它、扫描器喂它）；mirror 那份提供
/// 原生的选中复制 / 回滚 / reflow。两份喂同一批字节，就必须得出同一个画面 ——
/// 一旦分叉，人看到的和机长看到的就不是同一件事，而那种 bug 极难被发现。
@MainActor
final class TerminalMirrorParityTests: XCTestCase {

    /// 逐格比较两个终端的主屏内容 + 光标位置。
    private func assertGridsEqual(
        _ a: Terminal, _ b: Terminal, _ what: String,
        file: StaticString = #filePath, line: UInt = #line
    ) {
        XCTAssertEqual(a.cols, b.cols, "\(what)：列数", file: file, line: line)
        XCTAssertEqual(a.rows, b.rows, "\(what)：行数", file: file, line: line)
        for row in 0..<min(a.rows, b.rows) {
            let la = a.getLine(row: row)?.translateToString(trimRight: true) ?? ""
            let lb = b.getLine(row: row)?.translateToString(trimRight: true) ?? ""
            XCTAssertEqual(la, lb, "\(what)：第 \(row) 行", file: file, line: line)
        }
        XCTAssertEqual(a.buffer.x, b.buffer.x, "\(what)：光标列", file: file, line: line)
        XCTAssertEqual(a.buffer.y, b.buffer.y, "\(what)：光标行", file: file, line: line)
    }

    /// 覆盖面语料：SGR、CJK 宽字符、alt-screen 进出、滚动区域、清屏、
    /// 绝对/相对光标移动、超出 scrollback 的溢出。
    private var corpus: [UInt8] {
        var s = ""
        s += "\u{1b}[2J\u{1b}[H"
        s += "\u{1b}[1;31m红色粗体\u{1b}[0m 普通\r\n"
        s += "\u{1b}[38;5;208m256色\u{1b}[0m \u{1b}[38;2;10;200;30m真彩\u{1b}[0m\r\n"
        s += "中文宽字符测试 CJK 宽度对齐\r\n"
        s += "\u{1b}[4;10r"                       // DECSTBM 滚动区域
        for i in 0..<200 { s += "line \(i)\r\n" } // 越过屏幕、进回滚
        s += "\u{1b}[?1049h"                      // 进 alt-screen
        s += "ALT SCREEN CONTENT\r\n"
        s += "\u{1b}[?1049l"                      // 回主屏
        s += "\u{1b}[5;3H光标绝对定位"
        return Array(s.utf8)
    }

    func testMirrorGridMatchesCoreGridForTheSameBytes() {
        let cols = 100, rows = 30
        let core = Terminal(delegate: NullTerminalDelegate(),
                            options: Self.options(cols: cols, rows: rows))
        let mirror = Self.makeMirror(cols: cols, rows: rows)

        core.feed(buffer: corpus[...])
        mirror.feed(byteArray: corpus[...])

        assertGridsEqual(core, mirror.getTerminal(), "同一批字节喂两份缓冲区")
    }

    func testMirrorStaysEqualAfterWidthChanges() {
        let core = Terminal(delegate: NullTerminalDelegate(),
                            options: Self.options(cols: 100, rows: 30))
        let mirror = Self.makeMirror(cols: 100, rows: 30)
        core.feed(buffer: corpus[...])
        mirror.feed(byteArray: corpus[...])

        // 改窗口宽度 = reflow。两侧走的是同一段 Buffer.resize，结果必须一致。
        for width in [60, 160, 80] {
            core.resize(cols: width, rows: 30)
            mirror.getTerminal().resize(cols: width, rows: 30)
            assertGridsEqual(core, mirror.getTerminal(), "reflow 到 \(width) 列后")
        }
    }

    /// **agent 的 TUI 大部分时间活在 alt-screen 里**（claude/codex 的全屏界面），
    /// 所以「退回主屏之后两份一致」根本没验到我们真正要还原的那块画面。
    /// 这条在**仍处于 alt-screen 时**断言，并且在 alt-screen 里改宽度。
    ///
    /// 注意 alt-screen 没有回滚缓冲、resize 语义也与主屏不同（不 reflow，由应用
    /// 自己重画）—— 这里不假设哪种语义对，只要求**两份缓冲区得出同一个结果**。
    func testMirrorMatchesCoreWhileInsideAltScreen() {
        let core = Terminal(delegate: NullTerminalDelegate(),
                            options: Self.options(cols: 100, rows: 30))
        let mirror = Self.makeMirror(cols: 100, rows: 30)

        var s = ""
        s += "\u{1b}[2J\u{1b}[H主屏上先留一点历史\r\n"
        for i in 0..<50 { s += "history \(i)\r\n" }
        s += "\u{1b}[?1049h"                       // 进 alt-screen，**不再退出来**
        s += "\u{1b}[2J\u{1b}[H"
        s += "\u{1b}[1;36m╭─ AGENT TUI ─────────╮\u{1b}[0m\r\n"
        s += "\u{1b}[36m│\u{1b}[0m 中文宽字符 + emoji ✅ \u{1b}[36m│\u{1b}[0m\r\n"
        s += "\u{1b}[36m╰─────────────────────╯\u{1b}[0m\r\n"
        s += "\u{1b}[2;5H"                          // alt 屏里定位光标
        let bytes = Array(s.utf8)

        core.feed(buffer: bytes[...])
        mirror.feed(byteArray: bytes[...])
        assertGridsEqual(core, mirror.getTerminal(), "仍在 alt-screen 内")

        // alt-screen 里改宽度 —— agent TUI 运行期间拖窗口就是这条路径。
        for width in [60, 160, 100] {
            core.resize(cols: width, rows: 30)
            mirror.getTerminal().resize(cols: width, rows: 30)
            assertGridsEqual(core, mirror.getTerminal(), "alt-screen 内 resize 到 \(width) 列后")
        }

        // 退回主屏后，之前那 50 行历史两边都要还在、且一致。
        let leave = Array("\u{1b}[?1049l".utf8)
        core.feed(buffer: leave[...])
        mirror.feed(byteArray: leave[...])
        assertGridsEqual(core, mirror.getTerminal(), "退出 alt-screen 回到主屏后")

        // 「历史还在」不能只看当前视口 —— 把回滚缓冲也逐行比一遍。
        assertScrollbackEqual(core, mirror.getTerminal(), "退出 alt-screen 后的回滚历史")
    }

    /// 逐行比较回滚缓冲区（`getScrollInvariantLine` 用从没被裁掉过的绝对行号）。
    private func assertScrollbackEqual(
        _ a: Terminal, _ b: Terminal, _ what: String,
        file: StaticString = #filePath, line: UInt = #line
    ) {
        var rowsCompared = 0
        for row in 0..<20_000 {
            let la = a.getScrollInvariantLine(row: row)?.translateToString(trimRight: true)
            let lb = b.getScrollInvariantLine(row: row)?.translateToString(trimRight: true)
            if la == nil && lb == nil { continue }
            XCTAssertEqual(la, lb, "\(what)：绝对行 \(row)", file: file, line: line)
            if la != nil { rowsCompared += 1 }
        }
        XCTAssertGreaterThan(rowsCompared, 50,
                             "\(what)：比对的行数太少（\(rowsCompared)），夹具没造出历史",
                             file: file, line: line)
    }

    private static func makeMirror(cols: Int, rows: Int) -> TerminalMirrorView {
        let mirror = TerminalMirrorView(frame: NSRect(x: 0, y: 0, width: 800, height: 600))
        mirror.getTerminal().resize(cols: cols, rows: rows)
        return mirror
    }

    private static func options(cols: Int, rows: Int) -> TerminalOptions {
        var o = TerminalOptions.default
        o.cols = cols
        o.rows = rows
        o.scrollback = AgentSessionCore.scrollbackLines
        return o
    }
}

/// 什么都不做的 `TerminalDelegate`（测试里只关心缓冲区）。
private final class NullTerminalDelegate: TerminalDelegate {
    func send(source: Terminal, data: ArraySlice<UInt8>) {}
}
#endif
