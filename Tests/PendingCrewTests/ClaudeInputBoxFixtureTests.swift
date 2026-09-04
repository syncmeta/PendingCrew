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
}
#endif
