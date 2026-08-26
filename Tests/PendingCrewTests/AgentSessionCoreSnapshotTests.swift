#if os(macOS)
import XCTest
import SwiftTerm

/// `AgentSessionCore.snapshot()`（前后端分离 P3）。
///
/// 这里只钉两件事，都是**只有在 core 这一层才成立**的：
/// 1. 拍快照时那几个模式是真的问出来的（`?25` / `?1006` 在 SwiftTerm 里没有公开读口）。
/// 2. **问出来的答复一个字节都不许漏进 PTY。** 问法是往终端喂 DECRQM，答复从
///    `TerminalDelegate.send` 出来 —— 而那条路平时是原样送给子进程的。漏过去的话
///    agent 的输入框里会凭空多出一串 `\u{1b}[?25;2$y`。
///
/// **两条都当场证过会红**（2026-08-26）：把 `send(source:data:)` 里那道拦截关掉，
/// 两条同时红 —— 一条报「答复漏进 PTY 了」，另一条报「快照里没有 ?25l」。后者会红
/// 是因为答复流去了子进程、`probe` 收到空手，于是模式压根问不出来。**这两件事是
/// 同一道拦截的正反面，所以一道红证同时覆盖两条。**
@MainActor
final class AgentSessionCoreSnapshotTests: XCTestCase {

    private func waitUntil(_ timeout: TimeInterval = 8, _ cond: @MainActor () -> Bool) async -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if cond() { return true }
            try? await Task.sleep(nanoseconds: 50_000_000)
        }
        return cond()
    }

    private func makeShellCore() -> AgentSessionCore {
        AgentSessionCore(
            config: SessionConfig(kind: .terminal),
            mode: .plainShell,
            executable: "/bin/sh",
            workdir: NSTemporaryDirectory(),
            env: ["TERM": "xterm-256color", "PATH": "/usr/bin:/bin"])
    }

    /// 拍快照不许在子进程那边留下任何痕迹。
    ///
    /// 判据是直接的：PTY 是开回显的，所以答复只要送进去就会被回显到屏幕上。屏幕上
    /// 出现 `$y`（DECRQM 答复的固定结尾）就说明漏了。
    func testSnapshotDoesNotLeakModeQueryRepliesIntoThePty() async throws {
        let core = makeShellCore()
        defer { core.stop() }
        core.write(Array("echo BEFORE_SNAPSHOT\n".utf8))
        _ = await waitUntil { core.screenText(maxLines: 60).contains("BEFORE_SNAPSHOT") }

        let snapshot = core.snapshot()
        XCTAssertGreaterThan(snapshot.bytes.count, 0, "前置条件：该拍出东西来")

        // 再跑一条命令：如果刚才那份答复漏进了 PTY，它此刻已经被回显、或者被 sh
        // 当成一行输入报错了。
        core.write(Array("echo AFTER_SNAPSHOT\n".utf8))
        let ok = await waitUntil { core.screenText(maxLines: 60).contains("AFTER_SNAPSHOT") }
        XCTAssertTrue(ok, "前置条件：快照之后 shell 该还是好的")

        let screen = core.screenText(maxLines: 60)
        XCTAssertFalse(screen.contains("$y"),
                       "DECRQM 的答复漏进 PTY 了 —— agent 的输入框里会凭空多出这串字节：\n\(screen)")
        XCTAssertFalse(screen.contains("[?25"), "同上：\n\(screen)")
    }

    /// **拦截必须窄：宁可漏拿，也不许错吞。**（机长 2026-08-26 的硬要求。）
    ///
    /// delegate 那条 `send` 上跑的不只是我们问出来的答复 —— agent 自己问的光标位置
    /// 报告、设备属性、括号粘贴回应全从那儿走。多吞一条，agent 就会去等一个永远不
    /// 来的回答，**而那种坏法不报错、只是不动了**，几乎查不出来。
    func testInterceptionMatchesOnlyTheExactModeBeingAsked() {
        func bytes(_ s: String) -> ArraySlice<UInt8> { Array(s.utf8)[...] }

        XCTAssertTrue(AgentSessionCore.isDecrpmAnswer(bytes("\u{1b}[?25;2$y"), forMode: 25),
                      "问 ?25 就该认 ?25 的答复")
        // 同一种形状、**模式号不同** —— 这一条是最要紧的：它长得几乎一样，
        // 错吞了也看不出来。
        XCTAssertFalse(AgentSessionCore.isDecrpmAnswer(bytes("\u{1b}[?1006;1$y"), forMode: 25),
                       "模式号不是问的那个就必须放行")
        XCTAssertFalse(AgentSessionCore.isDecrpmAnswer(bytes("\u{1b}[24;5R"), forMode: 25),
                       "光标位置报告是 agent 自己问的，必须放行")
        XCTAssertFalse(AgentSessionCore.isDecrpmAnswer(bytes("\u{1b}[?62;c"), forMode: 25),
                       "设备属性回应必须放行")
        XCTAssertFalse(AgentSessionCore.isDecrpmAnswer(bytes("\u{1b}[?25;"), forMode: 25),
                       "缺尾巴的半截不算答复")
        XCTAssertFalse(AgentSessionCore.isDecrpmAnswer(bytes("hello"), forMode: 25),
                       "普通字节当然放行")
    }

    /// 正反两面的端到端：**同一次查询窗口里**，问的那条被吞掉、没问的那条照常到 PTY。
    ///
    /// 这条比上面的判据测试更狠：它用一条**同时问两个模式**的查询，而 core 只挂着
    /// `?25`。于是终端会连着吐两条形状一模一样、只有模式号不同的答复 —— 只吞对的
    /// 那条，另一条必须原样落进子进程。
    func testNonMatchingResponseStillReachesThePtyDuringAProbe() async throws {
        let core = makeShellCore()
        defer { core.stop() }
        _ = await waitUntil { core.isProcessRunning }

        let answer = core.probeTerminal(mode: 25,
                                        query: Array("\u{1b}[?25$p\u{1b}[?2004$p".utf8))
        let captured = String(decoding: answer, as: UTF8.self)
        XCTAssertTrue(captured.contains("[?25;"), "问的那条该被收下：\(captured.debugDescription)")
        XCTAssertFalse(captured.contains("[?2004;"),
                       "没问的那条不许被吞：\(captured.debugDescription)")

        // 没被吞的那条已经写进 sh 的标准输入了。补一个回车让它执行，
        // sh 会把那串字节原样报进「命令找不到」里 —— 那就是它到过 PTY 的证据。
        core.write(Array("\n".utf8))
        let arrived = await waitUntil { core.screenText(maxLines: 60).contains("2004") }
        XCTAssertTrue(arrived,
                      "没问的那条答复该原样到达子进程：\n\(core.screenText(maxLines: 60))")
        XCTAssertFalse(core.screenText(maxLines: 60).contains("25;"),
                       "问的那条不许漏过去")
    }

    /// 接上问答闭环之后，光标可见性才进得了快照。没接的话这条会红。
    func testSnapshotCarriesCursorVisibility() async throws {
        let core = makeShellCore()
        defer { core.stop() }
        core.write(Array("printf '\\033[?25l'; echo HIDDEN\n".utf8))
        _ = await waitUntil { core.screenText(maxLines: 60).contains("HIDDEN") }

        let text = String(decoding: core.snapshot().bytes, as: UTF8.self)
        XCTAssertTrue(text.contains("\u{1b}[?25l"),
                      "光标是隐藏的，快照里就该有 ?25l —— 它没有公开读口，只能问出来")
    }
}
#endif
