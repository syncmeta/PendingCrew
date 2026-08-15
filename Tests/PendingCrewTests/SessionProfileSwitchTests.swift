import XCTest
// SessionProfileSwitch 直接编进 PendingCrewTests target，无需 import。

/// 中途切模型/effort 的回显判定单测（#544）。
///
/// 全部 fixture 取自**实测**：本机 claude 2.1.220 在真 PTY 里注入 `/model haiku`
/// / `/effort low`，把去 ANSI 后的输出原样搬进来。关键坑就在这些 fixture 里 ——
/// claude TUI 排版靠光标移动、词间空格根本不进字节流，所以拿到的是
/// `SetmodeltoHaiku 4.5andsaved…` 而不是 `Set model to Haiku 4.5 and saved…`。
final class SessionProfileSwitchTests: XCTestCase {

    // MARK: - 实测回显

    /// 真 PTY 实测：`/model haiku` 生效后的那一行（空格被光标移动吃掉的原貌）。
    private let realModelEcho = "❯ /model haiku\n  ⎿  SetmodeltoHaiku 4.5andsavedasyourdefaultfornewsessions\n"
    /// 真 PTY 实测：`/effort low` 生效后的那两行。
    private let realEffortEcho = "❯ /effort low\n  ⎿  Set effortleveltolow(savedasyourdefaultfornewsessions):Quick,straightforwardimplementationwith\n     minimal overhead\n"

    func testRealModelEchoCountsAsApplied() {
        let v = SessionProfileEchoVerdict.classify(realModelEcho, knob: .model)
        XCTAssertEqual(v?.isApplied, true)
    }

    func testRealEffortEchoCountsAsApplied() {
        let v = SessionProfileEchoVerdict.classify(realEffortEcho, knob: .effort)
        XCTAssertEqual(v?.isApplied, true)
    }

    /// 有空格的正常排版同样要认（不同终端宽度/换行下 claude 会真吐空格）。
    func testSpacedEchoAlsoCountsAsApplied() {
        XCTAssertEqual(
            SessionProfileEchoVerdict.classify(
                "Set model to Opus 5 for this session only", knob: .model)?.isApplied, true)
    }

    // MARK: - 明确拒绝

    func testUnknownModelIsRejected() {
        let v = SessionProfileEchoVerdict.classify("Unknown model 'fable5'. Run /model …", knob: .model)
        guard case let .rejected(quote)? = v else { return XCTFail("应判为 rejected，实得 \(String(describing: v))") }
        XCTAssertTrue(quote.contains("Unknown model"))
    }

    func testEffortPinIsRejected() {
        let line = "Not applied: the launch-effort pin holds effort at high this session."
        guard case .rejected? = SessionProfileEchoVerdict.classify(line, knob: .effort) else {
            return XCTFail("launch-effort pin 必须报失败，不能当成功")
        }
    }

    func testEffortEnvOverrideIsRejected() {
        let line = "Not applied: CLAUDE_CODE_EFFORT_LEVEL=high overrides effort this session"
        guard case .rejected? = SessionProfileEchoVerdict.classify(line, knob: .effort) else {
            return XCTFail("env 覆盖必须报失败")
        }
    }

    func testFailedToSetEffortIsRejected() {
        guard case .rejected? = SessionProfileEchoVerdict.classify(
            "Failed to set effort level: bad value", knob: .effort) else {
            return XCTFail("应判为 rejected")
        }
    }

    // MARK: - 不能误报

    /// `/model` 的帮助提示里也有「set the model」字样 —— 不能当成生效回显。
    func testModelHelpHintIsNotAConfirmation() {
        let hint = "Run /model to open the model selection menu, or /model [modelName] to set the model."
        XCTAssertNil(SessionProfileEchoVerdict.classify(hint, knob: .model))
    }

    /// `/effort` 的菜单标题是「Set effort level for model usage」——差一个介词，别认。
    func testEffortMenuTitleIsNotAConfirmation() {
        XCTAssertNil(SessionProfileEchoVerdict.classify("Set effort level for model usage", knob: .effort))
    }

    /// 刚注入、只有自己那行回显时还没有结论。
    func testEchoedCommandAloneIsInconclusive() {
        XCTAssertNil(SessionProfileEchoVerdict.classify("❯ /model opus", knob: .model))
        XCTAssertNil(SessionProfileEchoVerdict.classify("❯ /effort max", knob: .effort))
    }

    /// 档位不串台：model 的生效回显不能满足 effort 的判定，反之亦然。
    func testKnobsDoNotCrossTalk() {
        XCTAssertNil(SessionProfileEchoVerdict.classify(realModelEcho, knob: .effort))
        XCTAssertNil(SessionProfileEchoVerdict.classify(realEffortEcho, knob: .model))
    }

    /// 同一窗里成功/失败都出现时，宁可报失败也不谎报成功。
    func testRejectionWinsOverConfirmation() {
        let mixed = "Set model to Opus 5\nUnknown model 'nope'"
        guard case .rejected? = SessionProfileEchoVerdict.classify(mixed, knob: .model) else {
            return XCTFail("失败优先")
        }
    }

    // MARK: - 扫描器（去 ANSI + 跨 chunk + 结论锁定）

    private func feed(_ sc: SessionProfileEchoScanner, _ s: String) -> SessionProfileSwitchOutcome? {
        let b = Array(s.utf8)
        return sc.feed(b[...])
    }

    func testScannerStripsAnsiAndConfirms() {
        let sc = SessionProfileEchoScanner(knob: .model)
        // 真终端里词与词之间是光标定位序列（CSI …G/C），不是空格。
        XCTAssertNil(feed(sc, "\u{1b}[2K\u{1b}[38;5;244m❯ /model haiku\u{1b}[0m\n"))
        XCTAssertEqual(feed(sc, "  ⎿  \u{1b}[1mSet\u{1b}[3Gmodel\u{1b}[9Gto\u{1b}[12GHaiku 4.5\n")?.isApplied, true)
    }

    func testScannerJoinsPhraseAcrossChunks() {
        let sc = SessionProfileEchoScanner(knob: .model)
        XCTAssertNil(feed(sc, "Set mo"))
        XCTAssertEqual(feed(sc, "del to Sonnet 5\n")?.isApplied, true)
    }

    func testScannerLocksFirstVerdict() {
        let sc = SessionProfileEchoScanner(knob: .model)
        XCTAssertEqual(feed(sc, "Set model to Haiku 4.5\n")?.isApplied, true)
        // 之后 TUI 继续重绘/出别的字样，结论不再翻。
        XCTAssertEqual(feed(sc, "Unknown model 'x'\n")?.isApplied, true)
    }

    // MARK: - 真 PTY 原始字节（端到端最要紧的一条）

    /// 真 claude 2.1.220 的 PTY 原始字节（base64），截自「注入 `/model haiku` 后
    /// 那条确认行」。**不是手写的近似** —— 里面是
    /// `Set\u{1b}[10Gmodel\u{1b}[16Gto\u{1b}[19G\u{1b}[1mHaiku 4.5…`：
    /// 词间是光标前移序列（CSI …G）而不是空格。任何按「Set model to」原样匹配的
    /// 实现在这条上都会漏判，本用例就是钉死这一点。
    private let realPtyBytesB64 = "ICAgICAgICAgICAgICAgICAgDRtbMUIbWzQ5bRtbMzg7NTsyNDZtICDijr8gIBtbMzltU2V0G1sxMEdtb2RlbBtbMTZHdG8bWzE5RxtbMW1IYWlrdSA0LjUbWzI5RxtbMjJtYW5kG1szM0dzYXZlZBtbMzlHYXMbWzQyR3lvdXIbWzQ3R2RlZmF1bHQbWzU1R2ZvchtbNTlHbmV3G1s2M0dzZXNzaW9ucw0bWzFCG1tLDRtbMUIbWzM4OzU7MjQ0beKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgOKUgA=="

    func testRealPtyBytesAreRecognisedAsApplied() throws {
        let data = try XCTUnwrap(Data(base64Encoded: realPtyBytesB64))
        let sc = SessionProfileEchoScanner(knob: .model)
        XCTAssertEqual(sc.feed(Array(data)[...])?.isApplied, true)
    }

    /// PTY 是流，一条确认行会被切成任意大小的 chunk 送达 —— 逐字节喂也要认出来。
    func testRealPtyBytesRecognisedWhenSplitByteByByte() throws {
        let data = try XCTUnwrap(Data(base64Encoded: realPtyBytesB64))
        let sc = SessionProfileEchoScanner(knob: .model)
        for byte in data { sc.feed([byte][...]) }
        XCTAssertEqual(sc.outcome?.isApplied, true)
    }

    // MARK: - 命令拼装

    func testCommandLineMatchesWhatAHumanWouldType() {
        XCTAssertEqual(SessionProfileSwitchCommand(knob: .model, value: "opus").line, "/model opus")
        XCTAssertEqual(SessionProfileSwitchCommand(knob: .effort, value: "xhigh").line, "/effort xhigh")
        XCTAssertEqual(SessionProfileSwitchCommand(knob: .model, value: "opus").summary, "模型→opus")
    }
}
