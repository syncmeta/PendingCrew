#if os(macOS)
import XCTest
import SwiftTerm

/// 快照往返属性测试（前后端分离 P3 / 设计 §7.1）。
///
/// 形状永远是同一个：
/// ```
/// 语料 → 无画面 Terminal T1 → TerminalSnapshotEncoder → 字节流 → 干净 Terminal T2
/// 断言 T1 与 T2 逐格相等
/// ```
/// **这组测试是用来发现 encoder 哪儿不对的，不是用来给写好的 encoder 盖章的。**
/// 所以断言只许往细里写，不许往宽里改：某类语料对不齐，说明还原路径与内核不等价，
/// 那是真问题。
final class TerminalSnapshotEncoderTests: XCTestCase {

    // MARK: - 往返

    /// 喂一段语料，返回 (T1, T2)。
    private func roundTrip(_ corpus: String,
                           cols: Int = 80, rows: Int = 25, scrollback: Int = 10_000)
        -> (HeadlessTerminalHarness, HeadlessTerminalHarness) {
        let t1 = HeadlessTerminalHarness(cols: cols, rows: rows, scrollback: scrollback)
        t1.feed(corpus)
        let snapshot = TerminalSnapshotEncoder.encode(t1.terminal, probe: t1.probe)
        let t2 = HeadlessTerminalHarness(cols: snapshot.cols, rows: snapshot.rows,
                                         scrollback: scrollback)
        t2.feed(snapshot.bytes)
        return (t1, t2)
    }

    private func assertEquivalent(_ t1: HeadlessTerminalHarness, _ t2: HeadlessTerminalHarness,
                                  _ what: String,
                                  file: StaticString = #filePath, line: UInt = #line) {
        let a = TerminalFlattener.flatten(t1.terminal)
        let b = TerminalFlattener.flatten(t2.terminal)

        XCTAssertEqual(a.isAlternate, b.isAlternate, "\(what)：alt 屏状态", file: file, line: line)
        XCTAssertEqual(a.activeLines.count, b.activeLines.count,
                       "\(what)：活跃缓冲区行数（回滚 + 视口）", file: file, line: line)
        for (i, (l, r)) in zip(a.activeLines, b.activeLines).enumerated() where l != r {
            XCTFail("\(what)：第 \(i) 行对不上\n  T1 \(l)\n  T2 \(r)\n  \(firstCellDiff(l, r))",
                    file: file, line: line)
            // 第一处不同就够定位了，整屏刷屏反而看不见
            break
        }
        XCTAssertEqual(a.cursor, b.cursor, "\(what)：光标位置", file: file, line: line)
        XCTAssertEqual(a.scrollTop, b.scrollTop, "\(what)：滚动区上界", file: file, line: line)
        XCTAssertEqual(a.scrollBottom, b.scrollBottom, "\(what)：滚动区下界", file: file, line: line)
        XCTAssertEqual(a.applicationCursor, b.applicationCursor, "\(what)：?1 应用光标键",
                       file: file, line: line)
        XCTAssertEqual(a.bracketedPaste, b.bracketedPaste, "\(what)：?2004 括号粘贴",
                       file: file, line: line)
        XCTAssertEqual(a.mouseModeDescription, b.mouseModeDescription, "\(what)：鼠标上报模式",
                       file: file, line: line)
    }

    /// 逐行比较只报得出「第几行不一样」，而属性对不上时两行的**文本是一样的** ——
    /// 不把格子摊开就只能看到两行空白。这个函数就是为了不让人对着两行空白发呆。
    private func firstCellDiff(_ l: FlatLine, _ r: FlatLine) -> String {
        if l.isWrapped != r.isWrapped {
            return "折行标记：T1 \(l.isWrapped) / T2 \(r.isWrapped)"
        }
        for (i, (lc, rc)) in zip(l.cells, r.cells).enumerated() where lc != rc {
            return "第 \(i) 格：\n    T1 \(lc)\n    T2 \(rc)"
        }
        return "格数：T1 \(l.cells.count) / T2 \(r.cells.count)"
    }

    // MARK: - §7.1 合成语料

    func testPlainText() {
        let (t1, t2) = roundTrip("hello world\r\nsecond line\r\nthird")
        assertEquivalent(t1, t2, "纯文本")
    }

    func testSgrFamily() {
        var corpus = ""
        for (i, sgr) in ["1", "2", "3", "4", "5", "7", "8", "9"].enumerated() {
            corpus += "\u{1b}[\(sgr)mstyle\(i)\u{1b}[0m "
        }
        corpus += "\r\n"
        for code in [0, 1, 7, 8, 9, 15, 16, 88, 255] {
            corpus += "\u{1b}[38;5;\(code)mfg\(code)\u{1b}[0m"
            corpus += "\u{1b}[48;5;\(code)mbg\(code)\u{1b}[0m"
        }
        corpus += "\r\n"
        corpus += "\u{1b}[38;2;12;34;56mtruefg\u{1b}[0m"
        corpus += "\u{1b}[48;2;200;100;50mtruebg\u{1b}[0m\r\n"
        corpus += "\u{1b}[4:3munder-curly\u{1b}[0m\u{1b}[4:4munder-dotted\u{1b}[0m\r\n"
        corpus += "\u{1b}[4:2;58;2;9;8;7munder-color\u{1b}[0m\r\n"
        let (t1, t2) = roundTrip(corpus)
        assertEquivalent(t1, t2, "SGR 全家")
    }

    func testWideAndCombiningCharacters() {
        let corpus = "中文宽字符测试\r\n"
            + "mixed 中 en 文 mix\r\n"
            + "e\u{0301}cole combining\r\n"
            + "\u{1b}[31m红色中文\u{1b}[0m\r\n"
        let (t1, t2) = roundTrip(corpus)
        assertEquivalent(t1, t2, "宽字符与组合字符")
    }

    func testCursorMovementAndSparseScreen() {
        // 光标乱跳、屏幕上留下大片「从没写过」的格子 —— 那些格子是 code 0，
        // 不是空格；用空格填会让它们变成「画过的空白」。
        let corpus = "\u{1b}[5;10Hisland-a\u{1b}[12;40Hisland-b\u{1b}[2;2Hc\u{1b}[8;1Hd"
        let (t1, t2) = roundTrip(corpus)
        assertEquivalent(t1, t2, "稀疏屏幕")
    }

    func testEraseAndClear() {
        let corpus = "line one\r\nline two\r\nline three\u{1b}[2;1H\u{1b}[K"
            + "\u{1b}[3;5H\u{1b}[1Krest\r\n\u{1b}[41m\u{1b}[K\u{1b}[0m"
        let (t1, t2) = roundTrip(corpus)
        assertEquivalent(t1, t2, "清行清屏")
    }

    func testScrollRegionDECSTBM() {
        var corpus = "\u{1b}[3;10r"
        for i in 0..<20 { corpus += "row\(i)\r\n" }
        corpus += "\u{1b}[5;3H"
        let (t1, t2) = roundTrip(corpus)
        assertEquivalent(t1, t2, "滚动区域 DECSTBM")
    }

    func testWrappedLongLineKeepsWrapFlag() {
        // 折行标记丢了不会在这里当场翻车，但会在 §7.2 改宽度时炸 —— 所以这里直接
        // 把 isWrapped 钉进逐行比较（FlatLine 带着它）。
        let corpus = String(repeating: "abcdefghij", count: 25) + "\r\nafter"
        let (t1, t2) = roundTrip(corpus)
        assertEquivalent(t1, t2, "自动折行")
        XCTAssertTrue(TerminalFlattener.flatten(t1.terminal).activeLines.contains { $0.isWrapped },
                      "前置条件：这段语料本来就该产生折行")
    }

    /// **折行的续行正好落在屏幕最后一行。**（机长 2026-08-26 点出来的边界。）
    ///
    /// 还原折行的手法是「垫满右边界 → 写一个字符把折触发掉 → `\e[A` 上去把垫的
    /// 擦回 code 0 → 回来」。这套的前提是**那两行此刻都还寻址得到**。而续行落在
    /// 最后一行时，触发那一折会让整屏**向上滚一格**：要擦的那一行不在原来的行号上了。
    ///
    /// 它到底还对不对，不是想出来的 —— `\e[A` 是**相对光标**的，滚动把光标和那一行
    /// 一起带走了，所以相对关系不变。这条测试就是把这句话钉住：错了也看不出来，
    /// 只会让历史悄悄少一行。
    private func lastRowWrapCorpus(prefixLines: Int, cols: Int = 80) -> String {
        var corpus = ""
        for i in 0..<prefixLines { corpus += "pre\(i)\r\n" }
        // 一条比一行还长的行 —— 在最后一行上折下去，屏幕跟着滚一格。
        corpus += String(repeating: "W", count: cols + cols / 2)
        // 折下去之后光标在**续行**上。要制造的状态是「**前半行**没写满、而下一行
        // 是它的折行续行」，所以必须先 `\e[A` 上到前半行再擦 —— 直接擦会擦到续行，
        // 那条路根本走不到需要垫空格的分支。
        //
        // 这个坑是红证抓出来的：第一版就是直接擦，两条测试照样绿，而同一时刻真
        // claude 语料是红的 —— 也就是说它们当时什么都没测到。
        corpus += "\u{1b}[A\u{1b}[\(50)G\u{1b}[K"
        return corpus
    }

    /// 前置条件里最要紧的一条：**那一对折行的前半行必须真的没写满**。写满了就不需要
    /// 垫空格、也就走不到「上去把垫的擦回来」那条路 —— 测试会绿，但什么也没测到。
    private func assertPairNeedsPadding(_ flat: FlatTerminal, _ what: String,
                                        file: StaticString = #filePath, line: UInt = #line) {
        guard flat.activeLines.count >= 2 else {
            return XCTFail("\(what)：连两行都没有", file: file, line: line)
        }
        let first = flat.activeLines[flat.activeLines.count - 2]
        let trailingBlank = first.cells.reversed().prefix { $0.char == "\0" }.count
        XCTAssertGreaterThan(trailingBlank, 0,
                             "\(what)：前半行必须留着没写过的尾巴，否则垫空格那条路走不到",
                             file: file, line: line)
    }

    func testWrappedContinuationLandingOnLastRow() {
        let rows = 25
        let corpus = lastRowWrapCorpus(prefixLines: rows - 1)
        let (t1, t2) = roundTrip(corpus, rows: rows)
        let flat = TerminalFlattener.flatten(t1.terminal)
        XCTAssertTrue(flat.activeLines.suffix(2).first?.isWrapped == false
                      && flat.activeLines.last?.isWrapped == true,
                      "前置条件：最后两行该正好是一对折行")
        XCTAssertGreaterThan(flat.activeLines.count, rows,
                             "前置条件：那一折该真的把屏幕滚过一格（有历史了）")
        assertPairNeedsPadding(flat, "落在最后一行")
        assertEquivalent(t1, t2, "折行续行落在最后一行")
    }

    /// 同一个边界，叠上回滚缓冲区溢出 —— 那一折造成的滚动此刻正把一行挤出上限。
    func testWrappedContinuationOnLastRowWhileScrollbackOverflows() {
        let cap = AgentSessionCore.scrollbackLines
        let corpus = lastRowWrapCorpus(prefixLines: cap + 500)
        let (t1, t2) = roundTrip(corpus)
        let a = TerminalFlattener.flatten(t1.terminal)
        XCTAssertGreaterThan(a.activeLines.count, cap,
                             "前置条件：这一趟该真的把回滚上限撑满")
        assertPairNeedsPadding(a, "落在最后一行 + 回滚溢出")
        assertEquivalent(t1, t2, "落在最后一行 + 回滚溢出")
        XCTAssertEqual(a.activeLines.count,
                       TerminalFlattener.flatten(t2.terminal).activeLines.count,
                       "回滚长度不许因为那一折少一行")
    }

    func testModesArePreserved() {
        let corpus = "\u{1b}[?1h\u{1b}[?2004h\u{1b}[?1002h\u{1b}[?1006hmodes on"
        let (t1, t2) = roundTrip(corpus)
        assertEquivalent(t1, t2, "DEC 模式")
        XCTAssertTrue(t1.terminal.bracketedPasteMode, "前置条件：语料该真的开了 ?2004")
    }

    func testHiddenCursorIsPreserved() {
        let (t1, t2) = roundTrip("text\u{1b}[?25l")
        assertEquivalent(t1, t2, "光标隐藏")
        XCTAssertEqual(t1.probe(Array("\u{1b}[?25$p".utf8)),
                       t2.probe(Array("\u{1b}[?25$p".utf8)),
                       "?25 光标可见性该一致")
    }

    // MARK: - alt 屏（断言必须发生在**还处在 alt 屏**的状态下）

    /// P1 踩过的盲点：语料若是「进 alt 屏、写几行、再退回主屏」而断言发生在退回
    /// **之后**，等于根本没验到 alt 屏本身。agent 的 TUI 大部分时间就活在 alt 屏里。
    func testAlternateScreenWhileStillInIt() {
        let corpus = "history line 1\r\nhistory line 2\r\n"
            + "\u{1b}[?1049h"
            + "\u{1b}[H\u{1b}[32mTUI top\u{1b}[0m\r\n\u{1b}[7mstatus bar\u{1b}[0m\r\nbody"
        let (t1, t2) = roundTrip(corpus)
        XCTAssertTrue(t1.terminal.isCurrentBufferAlternate,
                      "前置条件：断言必须发生在还处在 alt 屏的时候")
        assertEquivalent(t1, t2, "alt 屏（未退出）")
    }

    func testAlternateScreenExitRestoresMainScreen() {
        let corpus = "history line 1\r\nhistory line 2\r\n"
            + "\u{1b}[?1049hTUI\u{1b}[?1049l"
        let (t1, t2) = roundTrip(corpus)
        XCTAssertFalse(t1.terminal.isCurrentBufferAlternate, "前置条件：这条该已经退回主屏")
        assertEquivalent(t1, t2, "alt 屏（已退出）")
    }

    /// **这条钉的是一处已知的不完整，不是 bug。**（机长 + 父机长 2026-08-26 拍板走 A。）
    ///
    /// SwiftTerm 只公开活跃那一份缓冲区，所以**正处在 alt 屏时**主屏那份只拿得到
    /// 文本、拿不到属性。这条测试把「拿不到的东西」和「拿得到却对不上」分开钉：
    /// 文本必须一个字不差；颜色则明确断言**会掉**。
    ///
    /// 哪天 SwiftTerm 把 `bufferFromKind(kind:)` 开成 public，这条会因为「颜色居然
    /// 没掉」而红 —— 那正是回来收掉这处降级的线索。
    ///
    /// **这两条断言都当场证过会红**（2026-08-26，按 CONTRIBUTING「先证明它会红」）：
    /// - 往 `emitNormalBufferTextOnly` 里硬塞一个 `\e[31m`（假装拿得到属性）→
    ///   `("ansi256(code: 1)") is not equal to ("defaultColor")`。
    /// - 让 `emitNormalBufferTextOnly` 直接 return（假装主屏文本没还原）→
    ///   文本那条当场红。
    /// 没红过的断言只是一句注释，这两条不是。
    func testKnownGap_normalBufferAttributesAreLostWhileInAlternateScreen() {
        let corpus = "\u{1b}[31mred history\u{1b}[0m\r\nplain history\r\n"
            + "\u{1b}[?1049hTUI body"
        let t1 = HeadlessTerminalHarness()
        t1.feed(corpus)
        XCTAssertTrue(t1.terminal.isCurrentBufferAlternate, "前置条件：必须还在 alt 屏里")

        let snapshot = TerminalSnapshotEncoder.encode(t1.terminal, probe: t1.probe)
        let t2 = HeadlessTerminalHarness()
        t2.feed(snapshot.bytes)

        // 文本不丢 —— 这是底线。
        let text1 = String(data: t1.terminal.getBufferAsData(kind: .normal), encoding: .utf8) ?? ""
        let text2 = String(data: t2.terminal.getBufferAsData(kind: .normal), encoding: .utf8) ?? ""
        XCTAssertEqual(trimTrailingBlankLines(text1), trimTrailingBlankLines(text2),
                       "alt 屏时主屏那段历史的**文本**必须一个字不丢")

        // 颜色掉了 —— 明确写死，别让它悄悄变成「反正也对」。
        t1.feed("\u{1b}[?1049l")
        t2.feed("\u{1b}[?1049l")
        let redInT1 = firstCellAttribute(t1.terminal, row: 0)
        let redInT2 = firstCellAttribute(t2.terminal, row: 0)
        XCTAssertEqual(redInT1?.fg, .ansi256(code: 1), "前置条件：T1 那行历史本来是红的")
        XCTAssertEqual(redInT2?.fg, .defaultColor,
                       "已知降级：正处在 alt 屏时，主屏历史的颜色拿不到（只拿得到文本）。"
                       + "这条要是红了，说明 SwiftTerm 开放了非活跃缓冲区 —— 回去收掉这处降级。")
    }

    private func trimTrailingBlankLines(_ s: String) -> String {
        var lines = s.components(separatedBy: "\n")
        while let last = lines.last, last.trimmingCharacters(in: .whitespaces).isEmpty {
            lines.removeLast()
        }
        return lines.joined(separator: "\n")
    }

    private func firstCellAttribute(_ t: Terminal, row: Int) -> Attribute? {
        guard let line = t.getScrollInvariantLine(row: t.buffer.totalLinesTrimmed + row) else {
            return nil
        }
        return line[0].attribute
    }

    // MARK: - §7.3 回滚缓冲区长度

    func testScrollbackOverflowKeepsLastTenThousandLines() {
        // 上限用产品里的那个常量，不另写一个数 —— 那个数改了，这条测试要跟着改，
        // 而不是继续绿着测一个已经不存在的上限。
        let cap = AgentSessionCore.scrollbackLines
        var corpus = ""
        for i in 0..<(cap + 2_000) { corpus += "line-\(i)\r\n" }
        let (t1, t2) = roundTrip(corpus, scrollback: cap)
        assertEquivalent(t1, t2, "\(cap + 2000) 行溢出")

        let a = TerminalFlattener.flatten(t1.terminal)
        let b = TerminalFlattener.flatten(t2.terminal)
        XCTAssertGreaterThan(a.activeLines.count, cap,
                             "前置条件：得真的撑满过上限，否则这条什么也没测")
        XCTAssertLessThan(a.activeLines.count, cap + 2_000,
                          "前置条件：也得真的**溢出**过 —— 没裁掉任何行就不叫溢出")
        XCTAssertEqual(a.activeLines.count, b.activeLines.count, "还原后的总行数")
        // Todo #34 踩过的坑：首行不许是断在半截的碎片。
        let firstText = b.activeLines.first?.text.trimmingCharacters(in: .whitespaces) ?? ""
        XCTAssertTrue(firstText.isEmpty || firstText.hasPrefix("line-"),
                      "回滚缓冲区的首行必须是完整的一行，不是半截碎片：\(firstText)")
    }

    // MARK: - §7.4 选中复制

    func testSelectionTextMatches() {
        var corpus = ""
        for i in 0..<50 { corpus += "\u{1b}[3\(i % 8)mselectable row \(i)\u{1b}[0m\r\n" }
        let (t1, t2) = roundTrip(corpus)
        assertEquivalent(t1, t2, "选中复制语料")

        let start = Position(col: 3, row: 0)
        let end = Position(col: 10, row: t1.terminal.rows - 1)
        XCTAssertEqual(t1.terminal.getText(start: start, end: end),
                       t2.terminal.getText(start: start, end: end),
                       "getText 是拖选复制走的同一条路，它一致就是选出来的文本一致")
    }

    // MARK: - §7.2 reflow

    func testReflowStaysConsistentAcrossResizes() {
        var corpus = ""
        for i in 0..<40 {
            corpus += "\u{1b}[3\(i % 8)m" + String(repeating: "w\(i % 10)", count: 30)
                + "\u{1b}[0m\r\n"
        }
        let (t1, t2) = roundTrip(corpus)
        assertEquivalent(t1, t2, "reflow 前")

        for width in [60, 160] {
            t1.terminal.resize(cols: width, rows: t1.terminal.rows)
            t2.terminal.resize(cols: width, rows: t2.terminal.rows)
            assertEquivalent(t1, t2, "resize(\(width)) 之后")
        }
    }
}
#endif
