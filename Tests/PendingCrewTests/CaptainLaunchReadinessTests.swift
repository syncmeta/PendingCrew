#if os(macOS)
import XCTest

/// 机长交接拉起自检的回归（2026-09-06 现场）。
///
/// 事故语义：从 daemon 侧换 Claude Code 机长，三次全部报「25 秒内没有给出真实
/// 就绪信号」并回滚 —— 而 claude 自己的记录说它渲染了 32 帧、跑满 27.19 秒。
/// 也就是说 claude 好好的，是自检看不见它。看不见的原因是**按具体类认后端**：
/// daemon 里造的是 `HeadlessSessionBackend`（无画面内核），既不是 inproc 的
/// `AgentTerminalSession` 门面，也不是 viewer 侧的 `RemoteSessionBackend`。
///
/// 这组测试的形状因此是：**同一个内核、同样吐过字节，三种后端形态必须给出同一个
/// 答案**。少认一种，这里就红。
@MainActor
final class CaptainLaunchReadinessTests: XCTestCase {

    // MARK: - 真起进程的那半（后端形态覆盖）

    /// 假 TUI：吐一批字节就停在那儿等输入 —— 和 claude 起来后的形状一样
    /// （进程活着、吐过字、还没人给它活干）。
    private func makeFakeTui() throws -> String {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("pendingcrew-readiness-\(UUID().uuidString).sh")
        try """
        #!/bin/sh
        printf 'fake-tui-ready\\r\\n'
        printf '\\342\\235\\257 '
        sleep 30
        """.write(to: url, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o755], ofItemAtPath: url.path)
        addTeardownBlock { try? FileManager.default.removeItem(at: url) }
        return url.path
    }

    @discardableResult
    private func waitUntil(
        _ timeout: TimeInterval = 8, _ cond: @MainActor () -> Bool
    ) async -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if cond() { return true }
            try? await Task.sleep(nanoseconds: 50_000_000)
        }
        return cond()
    }

    /// **这条就是那个 bug。** daemon 进程里 claude 的后端是 `HeadlessSessionBackend`；
    /// 它吐过字节之后，自检必须认得出来。认不出来 = 一个活得好好的 claude 被判死。
    func testHeadlessBackendIsRecognizedAsLaunchReady() async throws {
        let backend = HeadlessSessionBackend(
            config: SessionConfig(kind: .claudeCode),
            mode: .agent,
            executable: try makeFakeTui(),
            workdir: NSTemporaryDirectory(),
            env: [:])
        defer { backend.stop() }

        let sawOutput = await waitUntil { backend.core.lastOutputAt != .distantPast }
        XCTAssertTrue(sawOutput, "前置条件：无画面内核必须真的收到过 PTY 字节")

        XCTAssertTrue(
            CaptainLaunchReadiness.observedLaunchSignal(backend),
            "daemon 里 claude 的后端是 HeadlessSessionBackend —— 自检认不出它，"
                + "就会把一个正在跑的 claude 判成没起来并回滚杀掉（2026-09-06 现场）")
    }

    /// 强度对照（其一）：inproc 的门面后端，新判据必须与旧判据
    /// （`core.lastOutputAt != .distantPast`）逐拍一致 —— 修的是「够不够得着」，
    /// 不是「放不放宽」。
    func testFacadeBackendMatchesTheRawKernelSignal() async throws {
        let session = AgentTerminalSession(
            config: SessionConfig(kind: .claudeCode),
            executable: try makeFakeTui(),
            workdir: NSTemporaryDirectory(),
            env: [:])
        defer { session.stop() }

        // 吐字之前：两边都必须是「还没看到」。
        XCTAssertEqual(
            CaptainLaunchReadiness.observedLaunchSignal(session),
            session.core.lastOutputAt != .distantPast,
            "首字节到达之前，新旧判据必须一致地说「还没就绪」")

        let sawOutput = await waitUntil { session.core.lastOutputAt != .distantPast }
        XCTAssertTrue(sawOutput, "前置条件：门面后端也要真的收到过 PTY 字节")
        XCTAssertEqual(
            CaptainLaunchReadiness.observedLaunchSignal(session),
            session.core.lastOutputAt != .distantPast,
            "首字节到达之后，新旧判据必须一致地说「就绪」")
    }

    /// 三种后端形态**同源**：都拿同一个 `AgentSessionCore` 的同一个信号。
    /// 这条是防「下一次又只补一半名单」的：任一种漏掉就红。
    func testEveryClaudeBackendShapeAnswersTheSameSignal() async throws {
        let script = try makeFakeTui()
        let headless = HeadlessSessionBackend(
            config: SessionConfig(kind: .claudeCode), mode: .agent,
            executable: script, workdir: NSTemporaryDirectory(), env: [:])
        let facade = AgentTerminalSession(
            config: SessionConfig(kind: .claudeCode),
            executable: script, workdir: NSTemporaryDirectory(), env: [:])
        defer { headless.stop(); facade.stop() }

        _ = await waitUntil {
            headless.core.lastOutputAt != .distantPast
                && facade.core.lastOutputAt != .distantPast
        }
        for backend in [headless as any SessionBackend, facade as any SessionBackend] {
            XCTAssertTrue(
                CaptainLaunchReadiness.observedLaunchSignal(backend),
                "\(type(of: backend)) 吐过字节了，自检必须认得出来")
        }
    }

    // MARK: - 纯判定那半（不起进程）

    func testReadySignalEndsTheWait() {
        XCTAssertEqual(
            CaptainLaunchReadiness.step(
                kind: .claudeCode, isRunning: true, health: nil,
                observedSignal: true, ledgerAgentSessionId: nil, elapsed: 1),
            .ready)
    }

    func testKeepsWaitingInsideTheWindow() {
        XCTAssertEqual(
            CaptainLaunchReadiness.step(
                kind: .claudeCode, isRunning: true, health: nil,
                observedSignal: false, ledgerAgentSessionId: nil, elapsed: 1),
            .keepWaiting)
    }

    func testLaunchFailedHealthIsTerminalImmediately() {
        let health = CrewSessionHealth(kind: .launchFailed, detail: "子进程没能启动")
        guard case .failed(let detail) = CaptainLaunchReadiness.step(
            kind: .claudeCode, isRunning: true, health: health,
            observedSignal: false, ledgerAgentSessionId: nil, elapsed: 1)
        else { return XCTFail("launchFailed 必须立刻终局") }
        XCTAssertEqual(detail, "子进程没能启动", "原因要原样带出去，别丢")
    }

    /// codex 的账本兜底照旧有效（它跨的是协议边界，与本次修复解决的问题不是一件事）。
    func testCodexLedgerRecordStillCountsAsReady() {
        XCTAssertEqual(
            CaptainLaunchReadiness.step(
                kind: .codex, isRunning: true, health: nil,
                observedSignal: false, ledgerAgentSessionId: "thread-42", elapsed: 1),
            .ready)
    }

    /// …但**只对 codex 成立**：claude 的会话号是我们自己指定的，写进账本只证明我们
    /// 传了参数，不证明 claude 起来了。拿它当就绪信号就是「把死机长判成活的」。
    func testClaudeLedgerRecordIsNotAReadySignal() {
        XCTAssertEqual(
            CaptainLaunchReadiness.step(
                kind: .claudeCode, isRunning: true, health: nil,
                observedSignal: false, ledgerAgentSessionId: "7a14d488", elapsed: 1),
            .keepWaiting)
    }

    /// 超时文案必须说出自己观测到了什么 —— 上一版那句把「我没看见」写成了「它没给」。
    func testTimeoutDetailSaysWhatWasObserved() {
        guard case .failed(let detail) = CaptainLaunchReadiness.step(
            kind: .claudeCode, isRunning: true, health: nil,
            observedSignal: false, ledgerAgentSessionId: nil, elapsed: 26)
        else { return XCTFail("过了观察窗必须终局") }
        XCTAssertTrue(detail.contains("26 秒"), "要说等了多久：\(detail)")
        XCTAssertTrue(detail.contains("PTY"), "要说观测的是哪个信号：\(detail)")
    }
}
#endif
