#if os(macOS)
import XCTest
import SwiftTerm

/// **尺子本身准不准** —— 拿录下来的**真** claude 首屏字节验
/// `ClaudeInputBox`（P5a）。`StartupPromptDeliveryTests` 验的是「规矩」（什么时候
/// 才许投），这一组验的是「尺子」（判据认不认得真画面）。两边合起来才完整：
/// 一把认不出真 TUI 的尺子，配再严的规矩也只会把每个 session 都判成异常。
///
/// 语料是 `Tests/Fixtures/tui-claude.bin`（`AgentTuiFixtureRecorder` 在真 PTY 里
/// 把 claude 拉起来录的原始字节，入库）。**不自己凭印象编 ANSI 序列** —— 这条 bug
/// 的病根恰恰是「以为首批字节到了就等于画面好了」，凭印象编的语料复现不了它。
final class ClaudeInputBoxFixtureTests: XCTestCase {

    private static var fixtureDirectory: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Fixtures", isDirectory: true)
    }

    private func loadFixture(_ name: String) throws -> [UInt8] {
        let url = Self.fixtureDirectory.appendingPathComponent(name)
        guard let data = try? Data(contentsOf: url) else {
            XCTFail("""
            缺 fixture：\(url.path)
            它是**入库**的，不该缺。这里故意 fail 而不是 skip —— 静默 skip 的测试
            看起来是绿的，其实一行都没跑。
            """)
            throw XCTSkip("missing fixture")
        }
        return [UInt8](data)
    }

    /// 与 `AgentSessionCore.screenRows()` 同一个读法（逐行渲染 + 去掉尾部空行）。
    private func screenRows(_ terminal: Terminal) -> [String] {
        var lines: [String] = []
        for row in 0..<terminal.rows {
            guard let line = terminal.getLine(row: row) else { continue }
            lines.append(line.translateToString(trimRight: true))
        }
        while let last = lines.last, last.trimmingCharacters(in: .whitespaces).isEmpty {
            lines.removeLast()
        }
        return lines
    }

    /// 真 claude 的启动过程按顺序经过两屏：先是「是否信任此文件夹」的对话框，
    /// 答完之后才是 banner + 输入框。判据必须两屏都认得，而且**顺序对得上** ——
    /// 对话框那一屏绝不许被判成「输入框就绪」（旧代码正是在那一屏把回车投了进去，
    /// 选中 `No, exit`，session 秒退且零输出）。
    func testWalksRealClaudeStartupFromTrustDialogToReadyInputBox() throws {
        let fixture = try loadFixture("tui-claude.bin")
        // 录的时候就是 80×25（`TerminalOptions.default`）。
        let harness = HeadlessTerminalHarness(cols: 80, rows: 25)

        var sawDialog = false
        var sawReady = false
        var dialogCameFirst = false
        var readyRowWhenFirstReady: String?
        var observedInputRows: [String] = []

        var offset = 0
        while offset < fixture.count {
            let size = min(64, fixture.count - offset)
            harness.feed(Array(fixture[offset..<(offset + size)]))
            offset += size

            let rows = screenRows(harness.terminal)
            let dialog = ClaudeInputBox.blockingDialog(rows)
            let input = ClaudeInputBox.inputRow(rows)

            if dialog != nil {
                sawDialog = true
                XCTAssertNil(
                    input,
                    """
                    对话框在场的那一屏被判成了「输入框就绪」—— brief 会被当成按键投进去，\
                    回车选中「No, exit」，session 秒退。喂到第 \(offset) 字节，画面：
                    \(rows.joined(separator: "\n"))
                    """)
            }
            if let input {
                if !sawReady {
                    sawReady = true
                    dialogCameFirst = sawDialog
                    readyRowWhenFirstReady = input
                }
                if observedInputRows.last != input { observedInputRows.append(input) }
            }
        }

        XCTAssertTrue(sawDialog, "真 claude 的首屏就是信任对话框，判据必须认得出来")
        XCTAssertTrue(sawReady, "答完对话框之后画面上出现了输入框，判据必须认得出来")
        XCTAssertTrue(dialogCameFirst, "顺序应当是「先对话框、后输入框」")
        XCTAssertNotNil(readyRowWhenFirstReady)

        // **真 claude 的「空」输入框会自己变。** 它先画一个空框，随后往里摆一句
        // 灰色示例提示（录这段 fixture 那次是 `Try "how do I log an error?"`），
        // 用户开始打字之后又换成正文。这是拿真字节跑出来才知道的事，也正是
        // 「落地」判据只能写成**「这一行跟刚就绪时不一样了」**的原因 —— 假设它
        // 「空 → 非空」会误判，逐字匹配 brief 更会（大段输入被折成 `[Pasted text …]`）。
        XCTAssertTrue(
            observedInputRows.contains(where: { $0.contains("Try") }),
            """
            真 claude 就绪后的输入行里应当出现过示例提示。观察到的输入行序列：
            \(observedInputRows)
            """)
        let hint = try XCTUnwrap(observedInputRows.first(where: { $0.contains("Try") }))
        XCTAssertFalse(
            StartupPromptDelivery.landed(row: hint, baseline: hint),
            "示例提示原样待着 = 什么都没进去")
        XCTAssertTrue(
            StartupPromptDelivery.landed(row: "[Pasted text #1 +212 lines]", baseline: hint),
            "brief 进去之后那一行会变 —— 这才是「落地」")
    }

    /// 认出来的确实是那个对话框，不是碰巧撞上的别的编号列表。
    func testRecognisesTheRealTrustDialogOptions() throws {
        let fixture = try loadFixture("tui-claude.bin")
        let harness = HeadlessTerminalHarness(cols: 80, rows: 25)

        var found: PendingTerminalDecision?
        var offset = 0
        while offset < fixture.count, found == nil {
            let size = min(64, fixture.count - offset)
            harness.feed(Array(fixture[offset..<(offset + size)]))
            offset += size
            found = ClaudeInputBox.blockingDialog(screenRows(harness.terminal))
        }

        let decision = try XCTUnwrap(found, "真 claude 首屏的信任对话框没被认出来")
        XCTAssertEqual(decision.options.count, 2)
        XCTAssertTrue(decision.options[0].contains("trust"), "选项 1：\(decision.options[0])")
        XCTAssertTrue(decision.options[1].contains("exit"), "选项 2：\(decision.options[1])")
    }

    // MARK: - 今天的现场：**没有编号**的信任框（`tui-claude-trust.bin`）

    /// 上面两条吃的 `tui-claude.bin` 是更早版本的 claude 录的，那时候选项还带编号
    /// （`1. Yes, I trust this folder` / `2. No, exit`）。**现在的 claude 不带了**：
    ///
    /// ```
    /// ❯  No, exit
    ///    Yes, I trust this folder
    ///  Enter to confirm · Esc to cancel
    /// ```
    ///
    /// 这段语料是 2026-09-06 在一个**全新的、claude 从没信任过的**临时目录里现录的
    /// （`AgentTuiFixtureRecorder` 的 `trust` 档；跑在已信任目录里的探针走不到这一屏，
    /// 录出来是 banner + 输入框，看着有东西其实测不到现场）。
    ///
    /// 两条判据在这一屏上都翻车，而且**翻得静悄悄**：
    /// - `blockingDialog` 判「没有对话框」（解析器要求 `1.` `2.` 开头）；
    /// - 同一屏被 `inputRow` 判成「输入框已就绪」（那行以 `❯` 开头、又不像编号选项），
    ///   于是开场 brief 会被当按键打进这个框、再回车 —— 而默认高亮停在 `No, exit`。
    private func trustDialogScreen() throws -> [String] {
        let fixture = try loadFixture("tui-claude-trust.bin")
        let harness = HeadlessTerminalHarness(cols: 80, rows: 25)
        harness.feed(fixture)
        // **用生产那条读法**（`AgentSessionCore.screenRows()` → `TerminalScreenText`），
        // 不用本文件上面那个 `translateToString` 私有读法 —— 后者把没写过的格原样留成
        // NUL，跟生产里喂给判据的文本不是同一份东西。
        return TerminalScreenText.rows(of: harness.terminal)
    }

    func testRecognisesTodaysUnnumberedTrustDialog() throws {
        let rows = try trustDialogScreen()
        let decision = try XCTUnwrap(
            ClaudeInputBox.blockingDialog(rows),
            """
            屏幕上明明摆着「是否信任这个文件夹」，判据却说没有对话框。画面：
            \(rows.joined(separator: "\n"))
            """)
        XCTAssertEqual(decision.options.count, 2, "选项：\(decision.options)")
        XCTAssertTrue(decision.options.contains { $0.contains("trust") }, "\(decision.options)")
        XCTAssertTrue(decision.options.contains { $0.contains("exit") }, "\(decision.options)")
        XCTAssertTrue(
            decision.prompt.contains("trust") || decision.prompt.contains("safety"),
            "问句得说清在问什么，否则群里那条通知等于没说：\(decision.prompt)")
    }

    /// 在等人回答的那一屏**绝不是**「输入框就绪」。这条挂了 = brief 被打进框里、
    /// 回车落在默认高亮的 `No, exit` 上。
    func testUnnumberedTrustDialogIsNotMistakenForAReadyInputBox() throws {
        let rows = try trustDialogScreen()
        XCTAssertNil(
            ClaudeInputBox.inputRow(rows),
            """
            信任框那一屏被判成了「输入框就绪」。画面：
            \(rows.joined(separator: "\n"))
            """)
    }

    /// **这条就是今天那个 P0 本身**：屏幕上有信任框 → 待决策那条出口必须出一条，
    /// 而不是「起来了、一直空闲」。
    ///
    /// 喂法照生产：字节喂 `feed`（`AgentSessionCore.scanOutput` 就这么喂），
    /// 每拍带着**渲染完的画面**去 `poll`（`pollPendingDecision` 挂在 0.6s busyTimer 上）。
    func testTrustDialogOnScreenRaisesAPendingDecision() throws {
        let fixture = try loadFixture("tui-claude-trust.bin")
        let harness = HeadlessTerminalHarness(cols: 80, rows: 25)
        let tracker = PendingDecisionTracker()
        let t0 = Date()

        var offset = 0
        while offset < fixture.count {
            let size = min(64, fixture.count - offset)
            let chunk = Array(fixture[offset..<(offset + size)])
            harness.feed(chunk)
            tracker.feed(chunk[...])
            offset += size
            _ = tracker.poll(now: t0, screen: TerminalScreenText.rows(of: harness.terminal))
        }

        let rows = TerminalScreenText.rows(of: harness.terminal)
        // 稳定窗过了才认（同 `PendingDecisionTracker.stableWindow`）——半成品画面不算。
        let event = tracker.poll(
            now: t0.addingTimeInterval(PendingDecisionTracker.stableWindow + 0.1), screen: rows)
        guard case let .appeared(decision)? = event else {
            return XCTFail("""
            信任框在屏幕上稳稳挂着，待决策出口却一个字都没有 —— 这正是「session 起来了、
            群里一片安静、看着一直空闲」的那条路。实际事件：\(String(describing: event))
            画面：
            \(rows.joined(separator: "\n"))
            """)
        }
        XCTAssertTrue(decision.options.contains { $0.contains("trust") }, "\(decision.options)")

        // 进得去也要出得来（#545）：框答掉/滚过去之后必须清，否则又是一个谎报状态。
        tracker.feed(Array("\n⏺ 好的，开始干活了。\n正在读文件…\n还在读…\n".utf8)[...])
        harness.feed(Array("\u{1b}[2J\u{1b}[H⏺ 好的，开始干活了。\r\n正在读文件…\r\n".utf8))
        XCTAssertEqual(
            tracker.poll(now: t0.addingTimeInterval(PendingDecisionTracker.stableWindow + 1),
                         screen: TerminalScreenText.rows(of: harness.terminal)),
            .cleared,
            "框没了就必须清掉待决策")
    }
}
#endif
