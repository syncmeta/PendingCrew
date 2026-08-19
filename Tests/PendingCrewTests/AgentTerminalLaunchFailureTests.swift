#if os(macOS)
import XCTest

/// 端到端回归（#541）：真起一个注定失败的子进程，验证整条 fail-loud 链在
/// `AgentTerminalSession` 上真的接通了 —— 不只是纯判定函数对。
///
/// 事故语义：拉起失败的 session 必须 ① 不再自称 running（否则点名推导出「空闲」）、
/// ② 留下可读原因。此前两条都没有：后端一构造 status 就是 `.running`，
/// SwiftTerm 那边 fork 失败静默 return / 秒退丢事件，谁也不认账。
@MainActor
final class AgentTerminalLaunchFailureTests: XCTestCase {

    /// 可执行文件不存在 → forkpty 起得来但 execve 失败，子进程立刻 _exit(127)。
    /// 这正是「起来即死」：从头到尾一个字节没吐。必须报 launchFailed，不许留在 running。
    func testNonexistentExecutableIsReportedAsLaunchFailure() async throws {
        let session = AgentTerminalSession(
            config: SessionConfig(kind: .claudeCode, initialPrompt: nil),
            executable: "/nonexistent/definitely-not-a-real-agent-binary",
            workdir: NSTemporaryDirectory(),
            env: [:])

        try await waitUntil(timeout: 8) { session.health != nil }

        XCTAssertEqual(session.health?.kind, .launchFailed,
                       "拉起失败必须翻成 launchFailed —— 否则点名会把它算成「空闲」")
        XCTAssertNotEqual(session.status, .running,
                          "没跑起来的 session 不许继续自称 running")
        let detail = session.health?.detail ?? ""
        XCTAssertTrue(detail.contains("Claude Code"), "原因要说清是哪个工具：\(detail)")
        XCTAssertFalse(detail.isEmpty, "失败原因必须留痕可读")

        // 状态推导落到「拉起失败」档 —— 与机长点名读到的是同一份推导。
        XCTAssertEqual(
            CrewSessionStateDerivation.state(
                isRunning: session.status == .running,
                health: session.health, isWorking: session.isWorking),
            CrewSessionStateDerivation.launchFailed)
    }

    /// 正常跑得起来的进程不许被自检误伤：有输出 → 判 alive，health 保持干净。
    func testHealthyProcessIsNotFlaggedAsLaunchFailure() async throws {
        // 自带一个「吐一行然后常驻」的脚本 = 真 agent 起来后的形态（进程不退 +
        // 有输出）。用脚本而不是 /bin/cat 之类：spawn 的 argv 是 claude 的那套
        // flag，随手挑的系统命令会把它们当参数报错退出，测不到常驻这一面。
        let script = try makeExecutableScript("#!/bin/sh\necho pendingcrew-alive\nsleep 30\n")
        let session = AgentTerminalSession(
            config: SessionConfig(kind: .claudeCode, initialPrompt: nil),
            executable: script,
            workdir: NSTemporaryDirectory(),
            env: [:])
        defer { session.stop() }

        try await waitUntil(timeout: 8) {
            session.core.lastOutputAt != Date.distantPast
        }
        // 给看门狗几轮机会误报。
        try await Task.sleep(nanoseconds: 2_500_000_000)

        XCTAssertNil(session.health, "见过输出的 session 不该被判拉起失败")
        XCTAssertEqual(session.status, .running)
    }

    /// 用户主动停 ≠ 拉起失败：停在「还没输出」的当口也不许报警（否则每次手动停
    /// 一个刚起的 session 都会往白板刷一条假告警 @机长）。
    func testUserStopIsNotReportedAsLaunchFailure() async throws {
        let session = AgentTerminalSession(
            config: SessionConfig(kind: .claudeCode, initialPrompt: nil),
            executable: "/bin/cat",          // 挂着等输入，不吐字
            workdir: NSTemporaryDirectory(),
            env: [:])
        session.stop()
        try await Task.sleep(nanoseconds: 2_000_000_000)

        XCTAssertNotEqual(session.health?.kind, .launchFailed,
                          "主动停的 session 不该被自检倒打一耙")
    }

    // MARK: -

    /// 落一个可执行的临时脚本，返回路径（用例结束自动清理）。
    private func makeExecutableScript(_ body: String) throws -> String {
        let url = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("pendingcrew-launchtest-\(UUID().uuidString).sh")
        try body.write(to: url, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o755], ofItemAtPath: url.path)
        addTeardownBlock { try? FileManager.default.removeItem(at: url) }
        return url.path
    }

    /// 轮询等条件成立（避免固定 sleep 的脆弱等待）。
    private func waitUntil(
        timeout: TimeInterval, _ condition: @MainActor () -> Bool
    ) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if condition() { return }
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        XCTFail("等待超时（\(timeout)s）：条件始终没成立")
    }
}
#endif
