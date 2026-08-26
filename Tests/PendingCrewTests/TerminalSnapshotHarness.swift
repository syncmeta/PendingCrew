#if os(macOS)
import Foundation
import SwiftTerm

/// 快照往返测试的公共器材（前后端分离 P3 / 设计 §7.1–§7.5）。
///
/// 这里只做两件事，**故意都不带断言**：造一个无画面的 `Terminal`，以及把两个
/// `Terminal` 逐格摊平成可比较的值。断言留在各个测试里，这样「哪一格不一样」
/// 由 XCTest 自己报，不用我在这儿再造一套 diff。
///
/// ⚠️ **摊平函数是这一整组测试的地基**：它读什么，测试就能钉住什么；它读不到的，
/// 测试就是瞎的。所以它只走 SwiftTerm 的公开接口，且**逐格读属性**——不是
/// `translateToString` 那种只看文本的读法（那会把「颜色全丢了」判成绿）。

/// 无画面终端 + 一个会把 `send` 攒起来的 delegate。
///
/// `send` 攒起来这件事不是可有可无：`?1006`（鼠标 SGR 编码）与 `?25`（光标可见性）
/// 在 SwiftTerm 里没有公开读口，**唯一的公开问法是 DECRQM** —— 往终端喂
/// `\e[?N$p`，答案从 delegate 的 `send` 出来。快照编码器要靠这条路问模式。
final class HeadlessTerminalHarness {
    let terminal: Terminal
    /// `Terminal.tdel` 是 weak 且没有 public setter，所以 delegate 必须在构造那一刻
    /// 就位、并且由我们自己持有 —— 否则它当场被释放，`send` 再也回不来。
    private let sink = SendSink()

    /// delegate 收到的回程字节（DECRQM 的答复就在这里）。
    var sent: [UInt8] { sink.sent }

    init(cols: Int = 80, rows: Int = 25, scrollback: Int = 10_000) {
        var opts = TerminalOptions.default
        opts.cols = cols
        opts.rows = rows
        opts.scrollback = scrollback
        terminal = Terminal(delegate: sink, options: opts)
    }

    func feed(_ text: String) { terminal.feed(text: text) }
    func feed(_ bytes: [UInt8]) { terminal.feed(byteArray: bytes) }

    /// 编码器要的「问一句、收一句」。
    func probe(_ query: [UInt8]) -> [UInt8] {
        sink.sent = []
        terminal.feed(byteArray: query)
        let answer = sink.sent
        sink.sent = []
        return answer
    }
}

private final class SendSink: TerminalDelegate {
    var sent: [UInt8] = []
    func send(source: Terminal, data: ArraySlice<UInt8>) { sent.append(contentsOf: data) }
}

// MARK: - 逐格摊平

/// 一个单元格摊平后的样子。**属性整个带上**——`Attribute` 自己是 Equatable，
/// 所以颜色/样式/下划线样式/下划线颜色任何一项对不上都会让这一格不等。
struct FlatCell: Equatable, CustomStringConvertible {
    let char: Character
    let width: Int
    let attribute: Attribute

    var description: String {
        let shown = char == "\0" ? "·" : String(char)
        return "\(shown)|w\(width)|\(attribute)"
    }
}

struct FlatLine: Equatable, CustomStringConvertible {
    let cells: [FlatCell]
    let isWrapped: Bool

    var text: String { String(cells.map { $0.char == "\0" ? " " : $0.char }) }
    var description: String {
        "\(isWrapped ? "↩" : " ")\(text.replacingOccurrences(of: "\0", with: "·"))"
    }
}

/// 一个终端摊平后的样子。
struct FlatTerminal: Equatable {
    let cols: Int
    let rows: Int
    /// 活跃缓冲区的**全部**行（含回滚历史），自上而下。
    let activeLines: [FlatLine]
    /// 非活跃缓冲区的文本（`getBufferAsData` 只给得出文本，见 §5.3 的 A 口径）。
    let inactiveText: String
    let isAlternate: Bool
    let cursor: Position
    let scrollTop: Int
    let scrollBottom: Int
    let applicationCursor: Bool
    let bracketedPaste: Bool
    let mouseModeDescription: String
}

enum TerminalFlattener {

    /// 活跃缓冲区的所有行（回滚 + 视口）。
    ///
    /// `Buffer.lines` 是 internal，公开的只有 `getScrollInvariantLine(row:)` —— 它
    /// 越界返回 nil，所以行数只能问出来，不能读出来。从 `totalLinesTrimmed`
    /// （= `linesTop`）起往上数到 nil 为止。
    static func activeLines(_ t: Terminal) -> [BufferLine] {
        var result: [BufferLine] = []
        var row = t.buffer.totalLinesTrimmed
        while let line = t.getScrollInvariantLine(row: row) {
            result.append(line)
            row += 1
        }
        return result
    }

    static func flattenLine(_ line: BufferLine, terminal: Terminal, cols: Int) -> FlatLine {
        var cells: [FlatCell] = []
        cells.reserveCapacity(cols)
        for i in 0..<min(cols, line.count) {
            let cell = line[i]
            cells.append(FlatCell(char: terminal.getCharacter(for: cell),
                                  width: Int(cell.width),
                                  attribute: cell.attribute))
        }
        return FlatLine(cells: cells, isWrapped: line.isWrapped)
    }

    static func flatten(_ t: Terminal) -> FlatTerminal {
        let lines = activeLines(t).map { flattenLine($0, terminal: t, cols: t.cols) }
        let inactiveKind: Terminal.BufferKind = t.isCurrentBufferAlternate ? .normal : .alt
        let inactive = String(data: t.getBufferAsData(kind: inactiveKind), encoding: .utf8) ?? ""
        return FlatTerminal(
            cols: t.cols,
            rows: t.rows,
            activeLines: lines,
            inactiveText: inactive,
            isAlternate: t.isCurrentBufferAlternate,
            cursor: Position(col: t.buffer.x, row: t.buffer.y),
            scrollTop: t.buffer.scrollTop,
            scrollBottom: t.buffer.scrollBottom,
            applicationCursor: t.applicationCursor,
            bracketedPaste: t.bracketedPasteMode,
            mouseModeDescription: "\(t.mouseMode)")
    }
}
#endif
