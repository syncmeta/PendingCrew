#if os(macOS)
import XCTest
import SwiftTerm

/// **真 · TUI 语料的往返测试**（前后端分离 P3 / 设计 §7.1 第二组语料）。
///
/// 合成语料负责覆盖面，这一组负责**真实**。随机 ANSI 测不出真 TUI 的怪癖，而快照
/// 编码器要还原的恰恰就是它 —— `Tests/Fixtures/tui-claude.bin` 是真的把 claude 在
/// PTY 里拉起来录下来的原始字节（含一次待决策菜单、一次改宽），录制器是
/// `AgentTuiFixtureRecorder`。
///
/// ## 三条与合成语料不同的做法
///
/// 1. **分批喂，不是一次喂完。** 真实的 PTY 字节是一批一批到的，转义序列会被拦腰
///    切在两批之间。一次喂完等于绕开了这条真实路径里最容易出事的地方。
/// 2. **在流的多个时刻各拍一次快照**，不是只在末尾拍一次。窗口可能在任何一刻连
///    上来 —— 包括 TUI 正画到一半的那一刻。只验末态等于只验了最太平的那个瞬间。
/// 3. **fixture 缺失时 fail，不 skip。** 仓库里已经有一批「本机绿、别人静默 skip」
///    的测试（`CrewChatOpenCostTests`，fixture 是 gitignored 的），那种绿是假的。
///    这两个 fixture 一共不到 20KB，**入库**，所以任何人 clone 下来都真的跑得到。
///
/// ## 它当场逮到了一个合成语料一次都没测出来的 bug
///
/// 真 claude 语料一进来就红：`26 行 ≠ 25 行`、第 7 行起整片错位。病因是**整行空白的
/// 折行续行凭空消失** —— SwiftTerm 是延迟折行，写满最后一格只是挂起，续行如果整行
/// 空白就没有任何字符去触发那一折，于是那一行不存在，后面所有内容整体上移一行。
/// 合成语料里造不出这种行；claude 的清屏（一串 `\e[2K\e[1A`）成串地造。
/// 修法见 `TerminalSnapshotEncoder.emitLines` 的注释。
final class TerminalSnapshotFixtureTests: XCTestCase {

    private static var fixtureDirectory: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Fixtures", isDirectory: true)
    }

    private func loadFixture(_ name: String,
                             file: StaticString = #filePath, line: UInt = #line) -> [UInt8]? {
        let url = Self.fixtureDirectory.appendingPathComponent(name)
        guard let data = try? Data(contentsOf: url) else {
            XCTFail("""
            缺 fixture：\(url.path)
            它是**入库**的，不该缺。要重录：
              TEST_RUNNER_PENDINGCREW_RECORD_TUI=all xcodebuild -project PendingCrew.xcodeproj \\
                -scheme PendingCrew -destination 'platform=macOS' \\
                -only-testing:PendingCrewTests/AgentTuiFixtureRecorder test
            这里故意 fail 而不是 skip —— 静默 skip 的测试看起来是绿的，其实一行都没跑。
            """, file: file, line: line)
            return nil
        }
        return [UInt8](data)
    }

    // MARK: -

    /// 把 fixture 分批喂进 T1，在若干个时刻各拍一次快照并逐格比对。
    private func assertRoundTripsThroughout(_ fixture: [UInt8], _ what: String,
                                           cols: Int = 80, rows: Int = 25,
                                           file: StaticString = #filePath, line: UInt = #line) {
        let t1 = HeadlessTerminalHarness(cols: cols, rows: rows)
        // 批大小故意不整齐 —— 转义序列被切在两批之间正是要验的那件事。
        let batchSizes = [1, 7, 64, 3, 512, 17, 2048, 5]
        var offset = 0
        var batchIndex = 0
        var checkpoints = 0
        // 每喂掉大约 1/6 就核一次，外加末尾那次。
        let stride = max(1, fixture.count / 6)
        var nextCheckpoint = stride

        while offset < fixture.count {
            let size = min(batchSizes[batchIndex % batchSizes.count], fixture.count - offset)
            t1.feed(Array(fixture[offset..<(offset + size)]))
            offset += size
            batchIndex += 1
            if offset >= nextCheckpoint {
                checkpoints += 1
                assertSnapshotMatches(t1, "\(what)：喂到第 \(offset) 字节",
                                      file: file, line: line)
                nextCheckpoint += stride
            }
        }
        checkpoints += 1
        assertSnapshotMatches(t1, "\(what)：末态", file: file, line: line)
        XCTAssertGreaterThanOrEqual(checkpoints, 3,
                                    "前置条件：至少要在三个时刻核过，只验末态不算",
                                    file: file, line: line)
    }

    private func assertSnapshotMatches(_ t1: HeadlessTerminalHarness, _ what: String,
                                       file: StaticString = #filePath, line: UInt = #line) {
        let snapshot = TerminalSnapshotEncoder.encode(t1.terminal, probe: t1.probe)
        let t2 = HeadlessTerminalHarness(cols: snapshot.cols, rows: snapshot.rows)
        t2.feed(snapshot.bytes)

        let a = TerminalFlattener.flatten(t1.terminal)
        let b = TerminalFlattener.flatten(t2.terminal)
        XCTAssertEqual(a.isAlternate, b.isAlternate, "\(what)：alt 屏状态", file: file, line: line)
        XCTAssertEqual(a.activeLines.count, b.activeLines.count, "\(what)：行数",
                       file: file, line: line)
        for (i, (l, r)) in zip(a.activeLines, b.activeLines).enumerated() where l != r {
            var detail = ""
            for (c, (lc, rc)) in zip(l.cells, r.cells).enumerated() where lc != rc {
                detail = "第 \(c) 格：\n    T1 \(lc)\n    T2 \(rc)"
                break
            }
            if detail.isEmpty { detail = "折行标记：T1 \(l.isWrapped) / T2 \(r.isWrapped)" }
            XCTFail("\(what)：第 \(i) 行对不上\n  T1 \(l)\n  T2 \(r)\n  \(detail)",
                    file: file, line: line)
            break
        }
        XCTAssertEqual(a.cursor, b.cursor, "\(what)：光标", file: file, line: line)
        XCTAssertEqual(a.scrollTop, b.scrollTop, "\(what)：滚动区上界", file: file, line: line)
        XCTAssertEqual(a.scrollBottom, b.scrollBottom, "\(what)：滚动区下界", file: file, line: line)
        XCTAssertEqual(a.bracketedPaste, b.bracketedPaste, "\(what)：?2004", file: file, line: line)
        XCTAssertEqual(a.applicationCursor, b.applicationCursor, "\(what)：?1",
                       file: file, line: line)
        XCTAssertEqual(a.mouseModeDescription, b.mouseModeDescription, "\(what)：鼠标模式",
                       file: file, line: line)
    }

    // MARK: - claude 的真 TUI

    func testClaudeTuiRoundTrips() throws {
        guard let fixture = loadFixture("tui-claude.bin") else { return }
        assertRoundTripsThroughout(fixture, "claude TUI")
    }

    /// fixture 里得真的有 TUI 的那些怪癖，否则「过了」什么也不说明。
    func testClaudeFixtureActuallyContainsTuiBehaviour() throws {
        guard let fixture = loadFixture("tui-claude.bin") else { return }
        let text = String(decoding: fixture, as: UTF8.self)
        XCTAssertGreaterThan(fixture.filter { $0 == 0x1b }.count, 200,
                             "前置条件：这得是一段真的在重绘的 TUI")
        XCTAssertTrue(text.contains("\u{1b}[?2004h"), "该开了括号粘贴")
        XCTAssertTrue(text.contains("\u{1b}[2K"), "该有整行擦除 —— 那正是「擦过的格子」那条路")
        XCTAssertTrue(text.contains("\u{1b}[?25l"), "该有隐藏光标")
        // **菜单那几行只有渲染完才是连续的字符串。** claude 是拿 `\e[4G` 这种列跳
        // 一个词一个词摆上去的（原始字节里是 `\e[4G2.\e[7GNo,`），所以在裸字节里
        // 搜「1. Yes, I trust this folder」永远搜不到 —— 得先让终端把它画出来。
        // 而且它后来被 claude 自己的清屏（一串 `\e[2K\e[1A`）擦掉了，所以只能
        // 一边喂一边看，不能喂完再看。
        var sawMenu = false
        let probe = HeadlessTerminalHarness()
        var offset = 0
        while offset < fixture.count {
            let end = min(offset + 256, fixture.count)
            probe.feed(Array(fixture[offset..<end]))
            offset = end
            // 词与词之间那些格子 claude 是**跳过去**的（`\e[7G`），从没写过 →
            // code 0，不是空格。渲染出来是一串 NUL，得先归一化成空格才认得出人话。
            let screen = (String(data: probe.terminal.getBufferAsData(kind: .active),
                                 encoding: .utf8) ?? "")
                .replacingOccurrences(of: "\0", with: " ")
            if screen.contains("1. Yes, I trust this folder") { sawMenu = true; break }
        }
        XCTAssertTrue(sawMenu, "这一段该含一次真的待决策菜单")
    }

    /// **claude 不走 alt 屏。** 这条不是顺手一测 —— 它是一条会影响别人怎么读设计的
    /// 事实：设计里默认「agent 的 TUI 大部分时间活在 alt 屏里」，claude 实测不是。
    /// 所以 §5.3.1 那条「alt 屏时主屏历史掉色」的降级，对 claude 这条腿**一次都不会
    /// 发生**。哪天 claude 改了，这条会红，那时该回去重新掂量那处降级。
    func testClaudeDoesNotUseAlternateScreen() throws {
        guard let fixture = loadFixture("tui-claude.bin") else { return }
        let text = String(decoding: fixture, as: UTF8.self)
        XCTAssertFalse(text.contains("\u{1b}[?1049h"), "claude 实测不进 alt 屏")
        XCTAssertFalse(text.contains("\u{1b}[?47h"), "claude 实测不进 alt 屏（老式切法）")

        let t = HeadlessTerminalHarness()
        t.feed(fixture)
        XCTAssertFalse(t.terminal.isCurrentBufferAlternate, "喂完之后也该还在主屏")
    }

    // MARK: - 人直接用的那个终端

    func testPlainShellRoundTrips() throws {
        guard let fixture = loadFixture("tui-shell.bin") else { return }
        assertRoundTripsThroughout(fixture, "普通 shell")
    }

    // MARK: - §7.2 reflow（真语料版）

    func testRealCorpusSurvivesResizes() throws {
        for name in ["tui-claude.bin", "tui-shell.bin"] {
            guard let fixture = loadFixture(name) else { return }
            let t1 = HeadlessTerminalHarness()
            t1.feed(fixture)
            let snapshot = TerminalSnapshotEncoder.encode(t1.terminal, probe: t1.probe)
            let t2 = HeadlessTerminalHarness(cols: snapshot.cols, rows: snapshot.rows)
            t2.feed(snapshot.bytes)

            for width in [60, 160] {
                t1.terminal.resize(cols: width, rows: t1.terminal.rows)
                t2.terminal.resize(cols: width, rows: t2.terminal.rows)
                let a = TerminalFlattener.flatten(t1.terminal)
                let b = TerminalFlattener.flatten(t2.terminal)
                XCTAssertEqual(a.activeLines.count, b.activeLines.count,
                               "\(name) resize(\(width))：行数")
                for (i, (l, r)) in zip(a.activeLines, b.activeLines).enumerated() where l != r {
                    XCTFail("\(name) resize(\(width))：第 \(i) 行对不上\n  T1 \(l)\n  T2 \(r)")
                    break
                }
            }
        }
    }

    // MARK: - §7.4 选中复制（真语料版）

    func testSelectionTextMatchesOnRealCorpus() throws {
        guard let fixture = loadFixture("tui-claude.bin") else { return }
        let t1 = HeadlessTerminalHarness()
        t1.feed(fixture)
        let snapshot = TerminalSnapshotEncoder.encode(t1.terminal, probe: t1.probe)
        let t2 = HeadlessTerminalHarness(cols: snapshot.cols, rows: snapshot.rows)
        t2.feed(snapshot.bytes)

        let start = Position(col: 0, row: 0)
        let end = Position(col: t1.terminal.cols - 1, row: t1.terminal.rows - 1)
        XCTAssertEqual(t1.terminal.getText(start: start, end: end),
                       t2.terminal.getText(start: start, end: end),
                       "拖选复制走的就是 getText，它一致就是选出来的文本一致")
    }
}
#endif
