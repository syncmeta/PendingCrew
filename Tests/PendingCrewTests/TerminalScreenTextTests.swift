#if os(macOS)
import Foundation
import SwiftTerm
import XCTest

/// **从终端格子里把文本弄出来**这件事的口径（2026-09-04 真机抓到）。
///
/// 现场：`--daemon-attach` 打出来的真 claude 首屏是 `ClaudeCodev2.1.260`、
/// `automodeon`、`我在，attach探针测试中。` —— 词与词之间的空格没了。逐字节看权威
/// 那份（`AgentSessionCore.screenText`，也就是 `inspect_session` 看到的那份）：
/// ```
/// ' ▐▛███▛█\x00\x00\x00Claude\x00Code\x00v2.1.260'
/// '⏺ 我\x00在\x00，\x00attach\x00探\x00针\x00测\x00试\x00中\x00。\x00'
/// ```
/// **本该是空格的位置是 NUL**，而且不只在全角字后面（`Claude\x00Code` 是纯 ASCII）。
/// 病根：TUI 用绝对定位画屏，跳过去没写的格在 SwiftTerm 里就是 `\0`，
/// 而 `translateToString` 原样把它交出来 —— 终端里 NUL 不显示，所以肉眼看着一直是对的，
/// **只有把这份东西当文本用的时候才露馅**（inspect_session 的输出、白板里那句
/// 「它最后一句话」，都吃这条）。
///
/// ⚠️ **NUL 有两种，一视同仁就会错另一边**：
/// - 没被写过的格：`width == 1`、字符 `\0` → **该是空格**
/// - 全角字的后半格：`width == 0` → **该丢掉**（映射成空格会把「我在」变成「我 在」）
///
/// 所以判定必须逐格读 `width`，不能在 `translateToString` 的结果上做字符串替换 ——
/// 那条路上这两种 NUL 长得一模一样。
@MainActor
final class TerminalScreenTextTests: XCTestCase {

    // MARK: - 两种 NUL

    func test_没被写过的格是空格不是NUL也不是粘在一起() {
        // 真 claude 首屏就是这个形状：写几个字，跳到某一列再写。
        let text = renderThroughSnapshot("\u{1b}[2J\u{1b}[1;1HClaude\u{1b}[1;13Hv2.1.260")

        XCTAssertEqual(text, "Claude      v2.1.260",
                       "跳过去没写的格该是空格 —— 删掉它会把词粘在一起，留着 NUL 又是脏文本")
        XCTAssertFalse(text.contains("\0"), "文本里不许有 NUL：\(debugBytes(text))")
    }

    func test_全角字不被拆开也不多出空格() {
        let text = renderThroughSnapshot("\u{1b}[2J\u{1b}[1;1H我在，attach 探针测试中。")

        XCTAssertEqual(text, "我在，attach 探针测试中。",
                       "全角字的后半格是同一个字的一部分，不是一个空格")
        XCTAssertFalse(text.contains("\0"), debugBytes(text))
    }

    /// 两种 NUL 混在同一行 —— 这才是现场那一行的真实形状。
    func test_全角与跳格混在同一行时各按各的规矩() {
        let text = renderThroughSnapshot("\u{1b}[2J\u{1b}[1;1H我在\u{1b}[1;9Hattach 中")

        XCTAssertEqual(text, "我在    attach 中")
        XCTAssertFalse(text.contains("\0"), debugBytes(text))
    }

    func test_行尾没写过的格不留出一串尾随空格() {
        let text = renderThroughSnapshot("\u{1b}[2J\u{1b}[1;1Hhi")
        XCTAssertEqual(text, "hi")
    }

    // MARK: - 权威那份（`inspect_session` 走的就是它）

    /// 这条跑**真 PTY**：口径要是只在探针那半边修对，`inspect_session` 的输出
    /// 和白板里那句「它最后一句话」照旧带 NUL。
    func test_权威画面同一口径() async throws {
        let core = makeShellCore()
        defer { core.stop() }
        core.write(Array("printf '\\033[2J\\033[1;1HClaude\\033[1;13Hv2.1.260\\n'\n".utf8))
        let arrived = await waitUntil { core.screenText(maxLines: 60).contains("v2.1.260") }
        XCTAssertTrue(arrived, "前置条件：那一行该画到屏幕上")

        let screen = core.screenText(maxLines: 60)
        XCTAssertFalse(screen.contains("\0"),
                       "权威画面里不许有 NUL：\(debugBytes(screen))")
        XCTAssertTrue(screen.contains("Claude      v2.1.260"),
                      "跳格该还原成空格，而不是把词粘在一起：\(debugBytes(screen))")
    }

    /// **同一份缓冲区，两个消费者，一份文本。** 探针那条路（快照字节 → 无画面终端 →
    /// 文本）与权威那条路（直接读权威终端）必须给出同一个字符串 —— 不然「探针打出来
    /// 的画面」就不是 daemon 里那份画面。
    func test_探针渲染与权威画面逐字相同() async throws {
        let core = makeShellCore()
        defer { core.stop() }
        let script = "printf '\\033[2J\\033[1;1HClaude\\033[1;13Hv2.1.260\\n"
            + "我在，attach 探针测试中。\\n'\n"
        core.write(Array(script.utf8))
        let arrived = await waitUntil { core.screenText(maxLines: 60).contains("探针测试中") }
        XCTAssertTrue(arrived, "前置条件：那两行该画到屏幕上")

        let snapshot = core.snapshot()
        let viaProbe = SessionSnapshotTextRenderer.render(
            snapshotBytes: snapshot.bytes, cols: snapshot.cols, rows: snapshot.rows,
            maxLines: 60)
        XCTAssertEqual(viaProbe, core.screenText(maxLines: 60))
    }

    // MARK: - 器材

    /// 走**真的那条路**：语料 → 无画面终端 → 快照编码 → 探针渲染器。
    /// 直接构造字节流会让这组测试量不到编码那一段。
    private func renderThroughSnapshot(_ corpus: String, cols: Int = 40, rows: Int = 6) -> String {
        let source = HeadlessTerminalHarness(cols: cols, rows: rows)
        source.feed(corpus)
        let snapshot = TerminalSnapshotEncoder.encode(source.terminal, probe: source.probe)
        return SessionSnapshotTextRenderer.render(
            snapshotBytes: snapshot.bytes, cols: snapshot.cols, rows: snapshot.rows,
            maxLines: 100)
    }

    /// 失败信息里必须看得见 NUL —— 它在终端上不显示，直接贴原文等于贴一片看不出
    /// 毛病的字。
    private func debugBytes(_ text: String) -> String {
        text.replacingOccurrences(of: "\0", with: "\\x00")
    }

    private func makeShellCore() -> AgentSessionCore {
        AgentSessionCore(
            config: SessionConfig(kind: .terminal),
            mode: .plainShell,
            executable: "/bin/sh",
            workdir: NSTemporaryDirectory(),
            env: ["TERM": "xterm-256color", "PATH": "/usr/bin:/bin"])
    }

    private func waitUntil(_ timeout: TimeInterval = 8,
                           _ condition: @MainActor () -> Bool) async -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if condition() { return true }
            try? await Task.sleep(nanoseconds: 50_000_000)
        }
        return condition()
    }
}
#endif
