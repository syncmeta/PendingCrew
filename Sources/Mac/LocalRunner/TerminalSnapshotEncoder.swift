#if os(macOS)
import Foundation
import SwiftTerm

/// 把后台那份权威 `Terminal` 的缓冲区**还原成一段字节流**（设计
/// `docs/internal/2026-08-19-backend-split-design.md` §5.3）。
///
/// **为什么不能「留原始字节、重放」**：agent 的 TUI 靠整屏重绘刷新，几分钟就是几 MB，
/// 而那几 MB 里绝大部分是覆盖同一块屏幕的重绘 —— 换不来 10000 行历史；而且环形
/// 缓冲区的头部会切在转义序列中间，重放直接乱码。所以必须序列化缓冲区本身。
///
/// **SGR 那 40 行为什么自己写**：SwiftTerm 的 `Attribute.toSgr()` 不只是 internal，
/// 它本身也是残的 —— 低位色多吐一个分号、italic / dim / crossedOut / 下划线样式 /
/// 下划线颜色全不管。所以这不是「被迫重写」，是本来就得写。
///
/// ## 已知的一处不完整（机长 2026-08-26 拍板走 A，别当 bug 修）
///
/// SwiftTerm 只公开**活跃**那一份缓冲区（`Terminal.buffer` 是 `public private(set)`，
/// 按 kind 取 `Buffer` 的 `bufferFromKind` 没有 public）。所以当终端**正处在 alt 屏**
/// 时，主屏那份只能通过 `getBufferAsData(kind:.normal)` 拿到**纯文本**，属性拿不到。
///
/// 影响面只有一句话：**掉的只是「正处在 alt 屏时、主屏那段历史的颜色」，文本一个字
/// 不丢；不在 alt 屏时全程逐格精确。** 有一条测试把这条限制钉成显式断言 ——
/// 哪天 SwiftTerm 把 `bufferFromKind` 开成 public，那条测试就是回来收掉它的线索。
enum TerminalSnapshotEncoder {

    /// 快照 = 尺寸 + 一段能把干净终端喂成同一副样子的字节。
    ///
    /// 尺寸单独带出来而**不**编进字节里：改尺寸的正经escape（DECCOLM / XTWINOPS）
    /// 要么带副作用要么要 host 配合，而 attach 帧本来就带 cols/rows —— 由消费侧
    /// 先 `resize` 再 `feed` 是唯一没有歧义的顺序。
    struct Snapshot: Equatable {
        let cols: Int
        let rows: Int
        let bytes: [UInt8]
    }

    /// 「问一句、收一句」。
    ///
    /// `?1006`（鼠标 SGR 编码）和 `?25`（光标可见性）在 SwiftTerm 里没有公开读口，
    /// **唯一的公开问法是 DECRQM** —— 往终端喂 `\e[?N$p`，答案从 delegate 的 `send`
    /// 出来。所以得由**持有 delegate 的那一方**把这个动作交给我们。
    ///
    /// 给 nil 就退化：光标当可见、鼠标编码当默认。退化不会错到乱码，只会少一两个模式。
    typealias ModeProbe = (_ query: [UInt8]) -> [UInt8]

    // MARK: - 入口

    static func encode(_ terminal: Terminal, probe: ModeProbe? = nil) -> Snapshot {
        var out: [UInt8] = []
        let cols = terminal.cols
        let rows = terminal.rows

        // 1. 硬复位。之后 T2 的滚动区域、原点模式、字符集、SGR 全是已知初值，
        //    下面每一步都不必猜「它之前是什么」。
        emit(&out, "\u{1b}c")

        // `nil` = 「当前属性未知 / 刚被复位」。SwiftTerm 没有公开构造 `Attribute` 的
        // 口子（`CharData.defaultAttr` 是 internal，`Attribute.empty` 又不是 SGR 0 的
        // 那一个），所以这里不去表示「默认属性」，只表示「还没定过」—— 于是第一格
        // 必然会发一次 SGR，语义上等价且不用碰内部类型。
        var attr: Attribute? = nil
        let isAlt = terminal.isCurrentBufferAlternate

        if isAlt {
            // 主屏那份只拿得到文本（见类型注释）。先把它写进主屏，再切 alt。
            emitNormalBufferTextOnly(terminal, rows: rows, into: &out)
            emit(&out, "\u{1b}[?1049h")
            emit(&out, "\u{1b}[H")
            attr = nil
        }

        // 2/3/4. 活跃缓冲区逐行（不在 alt 屏时它就是「回滚 + 主屏」）。
        emitLines(activeLines(terminal), terminal: terminal, cols: cols,
                  into: &out, attr: &attr)

        // 5. 滚动区域。DECSTBM 会把光标拉回原点，所以必须排在内容之后、定位之前。
        let top = terminal.buffer.scrollTop
        let bottom = terminal.buffer.scrollBottom
        if top != 0 || bottom != rows - 1 {
            emit(&out, "\u{1b}[\(top + 1);\(bottom + 1)r")
        }

        // 6. 需要保留的 DEC 模式。排在定位之前 —— 它们都不动光标（1049 已经切完了）。
        emitModes(terminal, probe: probe, into: &out)

        // 7. 光标位置与可见性。位置是视口相对的，与上面「逐行铺满视口」的结果一致。
        let x = min(max(terminal.buffer.x, 0), cols - 1)
        let y = min(max(terminal.buffer.y, 0), rows - 1)
        emit(&out, "\u{1b}[\(y + 1);\(x + 1)H")
        if cursorHidden(terminal, probe: probe) {
            emit(&out, "\u{1b}[?25l")
        }

        return Snapshot(cols: cols, rows: rows, bytes: out)
    }

    // MARK: - 行

    /// 活跃缓冲区的全部行（回滚 + 视口）。
    ///
    /// `Buffer.lines` 是 internal，公开的只有 `getScrollInvariantLine(row:)` —— 它越界
    /// 返回 nil，所以行数只能问出来、不能读出来。
    static func activeLines(_ t: Terminal) -> [BufferLine] {
        var result: [BufferLine] = []
        var row = t.buffer.totalLinesTrimmed
        while let line = t.getScrollInvariantLine(row: row) {
            result.append(line)
            row += 1
        }
        return result
    }

    /// 逐行输出。
    ///
    /// **难点全在折行上，而且是真 TUI 语料逼出来的**（合成语料一次都没测到）。
    ///
    /// `BufferLine.isWrapped` 记的是「这一行是上一行自动折过来的」。它不是可有可无
    /// 的装饰：改窗口宽度时 `Buffer.resize` 就靠它把一条逻辑长行重新拆合。硬给每行
    /// 塞一个 `\r\n`，折行就变成了两行独立的行，再也不会合并回去。
    ///
    /// 而想造出这个标记，**只有一条路：真的让终端在右边界上自动折一次**。没有任何
    /// 转义序列能直接说「把这一行标成折行」。于是：
    ///
    /// 1. 上一行要**顶到右边界**才折得下去。它尾部若是没写过的格子，得拿空格垫满。
    /// 2. 光顶到边界还不够 —— SwiftTerm 是「延迟折行」：写满最后一格只是挂起，
    ///    **下一个字符写下去才真的换行**。所以续行即使整行空白，也得先写一个字符
    ///    把这一折触发掉。**第一版漏了这条，于是整行空白的折行续行凭空消失，
    ///    后面所有内容整体上移一行** —— 真 claude 语料里这种行成串出现（它的清屏
    ///    是一串 `\e[2K\e[1A`），26 行的缓冲区还原成了 25 行。
    /// 3. 垫的空格和触发用的那个字符都是**假的**（原本是 code 0），必须擦掉。
    ///    擦得掉，是因为此刻那两行都还在视口里、寻址得到，而且 `\e[K` 填的正是
    ///    code 0 + 默认属性、**且不会动 `isWrapped` 标记**（SwiftTerm 的
    ///    `cmdEraseInLine` 走 `eraseInBufferLine(clearWrap:)` 的默认值 false）。
    ///
    /// 所以一组折行的输出形状是：垫满 → 写一个触发字符（折下去了）→ 上去把垫的擦掉
    /// → 回来把触发字符擦掉 → 正常写这一行。多出来的字节只发生在真有折行的地方。
    private static func emitLines(_ lines: [BufferLine], terminal: Terminal, cols: Int,
                                  into out: inout [UInt8], attr: inout Attribute?) {
        var i = 0
        while i < lines.count {
            // 这一组 = 一条逻辑行：lines[i] 加上紧跟着的所有折行续行。
            var last = i
            while last + 1 < lines.count && lines[last + 1].isWrapped { last += 1 }

            if i > 0 { emit(&out, "\r\n") }
            var paddedFrom: Int? = nil
            for k in i...last {
                if k > i {
                    // 触发那一折（见上面第 2 条）。
                    emit(&out, " ")
                    if let from = paddedFrom {
                        // 上去把垫的空格擦回 code 0（见第 3 条）。
                        emit(&out, "\u{1b}[A\u{1b}[\(from + 1)G\u{1b}[m\u{1b}[K\u{1b}[B")
                    }
                    // 回到本行行首，把触发用的那个字符也擦掉。
                    emit(&out, "\r\u{1b}[m\u{1b}[K")
                    attr = nil
                }
                paddedFrom = emitLine(lines[k], terminal: terminal, cols: cols,
                                      fillToRightMargin: k < last, into: &out, attr: &attr)
            }
            i = last + 1
        }
    }

    /// 返回「从第几列起是垫出来的空格」，没垫就是 nil —— 调用方要靠它把垫的擦回去。
    @discardableResult
    private static func emitLine(_ line: BufferLine, terminal: Terminal, cols: Int,
                                 fillToRightMargin: Bool,
                                 into out: inout [UInt8], attr: inout Attribute?) -> Int? {
        let limit = min(cols, line.count)
        let contentEnd = fillToRightMargin ? limit : meaningfulEnd(line, terminal: terminal, limit: limit)
        var skipped = 0
        var i = 0

        func flushSkip() {
            if skipped > 0 {
                emit(&out, "\u{1b}[\(skipped)C")
                skipped = 0
            }
        }

        while i < contentEnd {
            let cell = line[i]
            let ch = terminal.getCharacter(for: cell)

            if ch == "\0" {
                // 宽字符的第二格：光标已经被前一格带过去了，不能再跳一次。
                if i > 0 && line[i - 1].width == 2 {
                    i += 1
                    continue
                }
                if isUntouched(cell.attribute) {
                    skipped += 1
                    i += 1
                    continue
                }
                // **擦过、而且擦的时候带着背景色**的格子。
                //
                // 这一格是这个文件里最容易被「无害地简化」掉的地方，所以把三种写法
                // 各错在哪写全（都是往返测试逐格比对才看得出来的 —— 三种写法的
                // **文本完全一样**，只看文本的比较会一律判绿）：
                //
                // | 写法 | 错在哪 |
                // |---|---|
                // | 填空格 | `\e[K` 填的是 **code 0**，空格是 code 32。填空格等于把
                //   「这块屏没画过字」改写成「这块屏画了一片空白」——`getTrimmedLength`
                //   从此裁不掉它，改窗口宽度时这一行的有效长度就变了 |
                // | 用 CUF 跳过去 | 光标是过去了，但那片**背景色**一点没还原。TUI 的
                //   状态栏、选中高亮、进度条底色全是这么来的，跳过去就等于整条状态栏
                //   变透明 |
                // | 什么都不做（靠 `getTrimmedLength` 裁掉） | 整行都被带色地擦过时
                //   它返回 0，于是**整片背景色一格都不还原** |
                //
                // 只有 ECH（`\e[nX`）填的东西与 `\e[K` 完全同源 —— 都走 `eraseAttr()`
                // （前景默认、背景取当时的 curAttr.bg、无样式）—— 所以只有它能原样还原。
                var run = 1
                while i + run < contentEnd {
                    let next = line[i + run]
                    guard terminal.getCharacter(for: next) == "\0",
                          !isUntouched(next.attribute),
                          next.attribute == cell.attribute else { break }
                    run += 1
                }
                flushSkip()
                if cell.attribute != attr {
                    emit(&out, sgr(for: cell.attribute))
                    attr = cell.attribute
                }
                emit(&out, "\u{1b}[\(run)X")      // ECH：擦 run 格，光标不动
                emit(&out, "\u{1b}[\(run)C")      // 再把光标挪过去
                i += run
                continue
            }

            flushSkip()
            if cell.attribute != attr {
                emit(&out, sgr(for: cell.attribute))
                attr = cell.attribute
            }
            out.append(contentsOf: Array(String(ch).utf8))
            i += max(1, Int(cell.width))
        }

        // 折行的前一行必须真的把光标顶到右边界（CUF 到边界不触发自动折行），
        // 尾部是没写过的格子时只能拿空格垫 —— 垫完由调用方擦回去。
        if fillToRightMargin && skipped > 0 {
            emit(&out, "\u{1b}[m")
            attr = nil
            emit(&out, String(repeating: " ", count: skipped))
            return cols - skipped
        }
        return nil
    }

    /// 这一行最后一个「值得还原」的格子之后的位置。
    ///
    /// 不能直接用 `BufferLine.getTrimmedLength()`：它只裁 code 0，于是把「整行都被
    /// 带背景色地擦过」算成 0 —— 那一整片背景色会一格都不还原。这里的判据是
    /// 「code 非 0 **或** 属性不是从没碰过的那个默认值」。
    private static func meaningfulEnd(_ line: BufferLine, terminal: Terminal, limit: Int) -> Int {
        for i in (0..<limit).reversed() {
            let cell = line[i]
            let isBlankCode = terminal.getCharacter(for: cell) == "\0"
            if !isBlankCode || !isUntouched(cell.attribute) {
                return min(limit, i + max(1, Int(cell.width)))
            }
        }
        return 0
    }

    /// 「从没被碰过」的格子属性 —— 前景背景都是默认、没有任何样式。
    ///
    /// 直接写死判据而不去够 `CharData.defaultAttr`（那是 internal），也不用
    /// `Attribute.empty`（它的 bg 是 `.defaultInvertedColor`，**不是** SGR 0 的那一个）。
    private static func isUntouched(_ a: Attribute) -> Bool {
        a.fg == .defaultColor && a.bg == .defaultColor
            && a.style == .none && a.underlineStyle == .none && a.underlineColor == nil
    }

    /// 正处在 alt 屏时，主屏那份只拿得到文本（见类型注释里那段拍板）。
    private static func emitNormalBufferTextOnly(_ t: Terminal, rows: Int, into out: inout [UInt8]) {
        let data = t.getBufferAsData(kind: .normal)
        guard let text = String(data: data, encoding: .utf8) else { return }
        var lines = text.components(separatedBy: "\n")
        // getBufferAsData 每行都补一个 \n，末尾必有一个空串；再把主屏底部那批
        // 从没写过的空行也裁掉 —— 照原样吐会把历史整体往上顶掉同样多行。
        while let last = lines.last, last.isEmpty { lines.removeLast() }
        guard !lines.isEmpty else { return }
        emit(&out, lines.joined(separator: "\r\n"))
    }

    // MARK: - 模式

    private static func emitModes(_ t: Terminal, probe: ModeProbe?, into out: inout [UInt8]) {
        emit(&out, t.applicationCursor ? "\u{1b}[?1h" : "\u{1b}[?1l")
        emit(&out, t.bracketedPasteMode ? "\u{1b}[?2004h" : "\u{1b}[?2004l")

        switch t.mouseMode {
        case .off:                 break
        case .x10:                 emit(&out, "\u{1b}[?9h")
        case .vt200:               emit(&out, "\u{1b}[?1000h")
        case .buttonEventTracking: emit(&out, "\u{1b}[?1002h")
        case .anyEvent:            emit(&out, "\u{1b}[?1003h")
        }
        // `?1006` 没有公开读口，只能问。问不到就不发 —— 发错比不发更坏。
        if decrqmIsSet(t, mode: 1006, probe: probe) == true {
            emit(&out, "\u{1b}[?1006h")
        }
    }

    private static func cursorHidden(_ t: Terminal, probe: ModeProbe?) -> Bool {
        decrqmIsSet(t, mode: 25, probe: probe) == false
    }

    /// DECRQM：喂 `\e[?N$p`，答复形如 `\e[?N;<state>$y`。
    /// state 1 = 已设置，2 = 已复位；其余（0 不认识 / 3、4 永久）当拿不到。
    private static func decrqmIsSet(_ t: Terminal, mode: Int, probe: ModeProbe?) -> Bool? {
        guard let probe else { return nil }
        let answer = probe(Array("\u{1b}[?\(mode)$p".utf8))
        guard let text = String(bytes: answer, encoding: .utf8) else { return nil }
        let prefix = "\u{1b}[?\(mode);"
        guard text.hasPrefix(prefix), text.hasSuffix("$y") else { return nil }
        let state = text.dropFirst(prefix.count).dropLast(2)
        switch state {
        case "1": return true
        case "2": return false
        default:  return nil
        }
    }

    // MARK: - SGR

    /// 一个属性的完整 SGR。**总是从 `0` 重来**，不做「相对上一个属性只发差量」的
    /// 优化：快照是一次性的，正确性比字节数重要得多，而差量编码正是这类代码最爱
    /// 出错的地方（漏掉一个复位，后面整屏跟着串色）。
    static func sgr(for a: Attribute) -> String {
        var parts = ["0"]

        if a.style.contains(.bold)       { parts.append("1") }
        if a.style.contains(.dim)        { parts.append("2") }
        if a.style.contains(.italic)     { parts.append("3") }
        if a.style.contains(.underline)  { parts.append("4") }
        if a.style.contains(.blink)      { parts.append("5") }
        if a.style.contains(.inverse)    { parts.append("7") }
        if a.style.contains(.invisible)  { parts.append("8") }
        if a.style.contains(.crossedOut) { parts.append("9") }

        switch a.underlineStyle {
        case .none:   break
        case .single: parts.append("4:1")
        case .double: parts.append("4:2")
        case .curly:  parts.append("4:3")
        case .dotted: parts.append("4:4")
        case .dashed: parts.append("4:5")
        }

        parts.append(contentsOf: colorParams(a.fg, base: 30, resetCode: 39))
        parts.append(contentsOf: colorParams(a.bg, base: 40, resetCode: 49))
        if let uc = a.underlineColor {
            parts.append(contentsOf: colorParams(uc, base: 50, resetCode: 59))
        }

        return "\u{1b}[" + parts.joined(separator: ";") + "m"
    }

    /// `base` 30 = 前景、40 = 背景、50 = 下划线色（下划线色没有 30..37 那档，
    /// 只有扩展形式 `58;…`，所以低位色也走扩展）。
    private static func colorParams(_ c: Attribute.Color, base: Int, resetCode: Int) -> [String] {
        let extended = base == 50 ? 58 : base + 8      // 38 / 48 / 58
        switch c {
        case .defaultColor, .defaultInvertedColor:
            return []                                   // SGR 0 已经把它复位了
        case .ansi256(let code):
            if base == 50 { return ["\(extended)", "5", "\(code)"] }
            if code < 8  { return ["\(base + Int(code))"] }
            if code < 16 { return ["\(base + 60 + Int(code) - 8)"] }
            return ["\(extended)", "5", "\(code)"]
        case .trueColor(let r, let g, let b):
            return ["\(extended)", "2", "\(r)", "\(g)", "\(b)"]
        }
    }

    // MARK: -

    private static func emit(_ out: inout [UInt8], _ s: String) {
        out.append(contentsOf: Array(s.utf8))
    }
}
#endif
