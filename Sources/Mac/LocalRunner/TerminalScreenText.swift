#if os(macOS)
import Foundation
import SwiftTerm

/// **把终端格子变成文本**的唯一口径（2026-09-04 真机抓到的那条）。
///
/// 现场：`--daemon-attach` 打出来的真 claude 首屏是 `ClaudeCodev2.1.260`、
/// `automodeon`、`我在，attach探针测试中。` —— 词与词之间的空格没了。逐字节看权威
/// 那份（`AgentSessionCore.screenText`，也就是 `inspect_session` 看到的那份）：
/// ```
/// ' ▐▛███▛█\0\0\0Claude\0Code\0v2.1.260'
/// ```
/// **本该是空格的位置是 NUL**，而且不只在全角字后面。病根：TUI 用绝对定位画屏，
/// 跳过去没被写的格在 SwiftTerm 里就是 `\0`，`translateToString` 原样交出来。
/// 终端上 NUL 不显示，所以肉眼一直看着是对的 —— **只有把这份东西当文本用的时候
/// 才露馅**（`inspect_session` 的输出、白板里那句「它最后一句话」，都吃这条）。
///
/// ## 为什么必须逐格读、不能在字符串上替换
///
/// `\0` 有两种，长得一模一样，处理方式相反：
/// - **没被写过的格**（`width == 1`）→ 该是**空格**
/// - **全角字的后半格**（`width == 0`）→ 该**丢掉**（当成空格会把「我在」变成「我 在」）
///
/// `translateToString` 的结果里这两种已经分不开了，所以口径只能落在格子这一层。
///
/// ## 谁在用
///
/// 权威那份（`AgentSessionCore.screenText` → `inspect_session` / 白板）与无画面探针
/// （`--daemon-attach` 把快照喂进一台没有画面的终端再取文本）**共用这一个函数** ——
/// 两个消费者、一份文本。哪天再多一个消费者，也接这里，别另写一套。
///
/// ⚠️ 这条路**不是** AppKit 那条选中/剪贴板（`TerminalMirrorView` 的跨行复制，
/// 已单独记账），也不是 `PendingDecisionTracker` 那条去 ANSI 的字节尾窗。
/// 「屏幕 → 文本」现在有三条各走各的路；这里只统一其中一条。
enum TerminalScreenText {

    /// 一行格子 → 一行文本（尾部留白去掉，行首留白保留）。
    static func line(_ line: BufferLine, terminal: Terminal, cols: Int) -> String {
        var out = ""
        out.reserveCapacity(cols)
        for index in 0..<min(cols, line.count) {
            let cell = line[index]
            // 全角字的后半格：它是上一个字的一部分，不是一个字符。
            if cell.width == 0 { continue }
            let character = terminal.getCharacter(for: cell)
            out.append(character == "\0" ? " " : character)
        }
        while out.hasSuffix(" ") { out.removeLast() }
        return out
    }

    /// 当前**视口**逐行的文本（与窗口滚到哪无关）。尾部空行去掉。
    static func rows(of terminal: Terminal) -> [String] {
        var lines: [String] = []
        for row in 0..<terminal.rows {
            guard let buffered = terminal.getLine(row: row) else { continue }
            lines.append(line(buffered, terminal: terminal, cols: terminal.cols))
        }
        while let last = lines.last, last.trimmingCharacters(in: .whitespaces).isEmpty {
            lines.removeLast()
        }
        return lines
    }

    /// 视口文本，最多 `maxLines` 行（取尾部）。
    static func screen(of terminal: Terminal, maxLines: Int) -> String {
        rows(of: terminal).suffix(max(0, maxLines)).joined(separator: "\n")
    }
}
#endif
