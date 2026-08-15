#if os(macOS)
import XCTest
// LocalRunner 源码直接编进本 target（见 project.yml），terminateTree 直接可见。

/// 回归测试：交互式 claude/codex 常截获 SIGTERM —— `stop()` 必须能升级到
/// SIGKILL，否则"停不掉"。这里用一个 `trap '' TERM` 忽略 SIGTERM 的子进程
/// 复现"单发 SIGTERM 杀不掉"，再验证 `terminateTree` 的 SIGKILL 升级能终结它。
final class AgentTerminationTests: XCTestCase {

    func testTerminateTreeKillsSigtermIgnoringProcess() async throws {
        let p = Process()
        // perl 显式把 SIGTERM 设成 IGNORE —— 铁定截获 SIGTERM 的进程（比 bash
        // trap 可靠），模拟截获 SIGTERM 的交互式 agent。perl 在 macOS 必装。
        p.executableURL = URL(fileURLWithPath: "/usr/bin/perl")
        p.arguments = ["-e", "$SIG{TERM}='IGNORE'; sleep 60"]
        try p.run()
        let pid = p.processIdentifier
        // 等 perl 装上 SIG_IGN handler 再发信号 —— 否则 SIGTERM 在 handler 就位前
        // 到达会走默认动作杀掉进程（启动竞态，与被测逻辑无关）。
        try await Task.sleep(nanoseconds: 700_000_000)
        XCTAssertEqual(kill(pid, 0), 0, "子进程应在运行")

        // 模拟 SwiftTerm terminate() 发的 SIGTERM —— 被 SIG_IGN 吞掉。
        kill(pid, SIGTERM)
        try await Task.sleep(nanoseconds: 400_000_000)
        XCTAssertEqual(kill(pid, 0), 0, "SIGTERM 被忽略 → 仍存活（复现 bug）")

        // 升级：宽限后 SIGKILL —— 必须杀得掉。
        await terminateTree(pid: pid, graceSeconds: 0.2)
        p.waitUntilExit()
        XCTAssertFalse(p.isRunning, "terminateTree 应 SIGKILL 终结幸存进程")
    }
}

/// 终止原因分类单测（Todo #10 ①）：hit-limit 要从正常结束/手动停/一般失败里分出来。
final class SessionExitReasonTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_000_000)

    func testUserStopWinsEvenWhenRateLimited() {
        // 主动停优先：即便正卡限额,用户停了就不自动续跑。
        XCTAssertEqual(SessionExitReason.classify(
            cancelled: true, exitCode: nil,
            lastHealthKind: .rateLimited, healthAt: now.addingTimeInterval(-5), now: now),
            .userStopped)
    }

    func testRecentUsageLimitClassifiesHitLimit() {
        XCTAssertEqual(SessionExitReason.classify(
            cancelled: false, exitCode: 1,
            lastHealthKind: .usageLimit, healthAt: now.addingTimeInterval(-30), now: now),
            .hitLimit)
    }

    func testRecentRateLimitMenuClassifiesHitLimitEvenExitZero() {
        // 卡限额菜单后进程退出（无论 code）→ hit limit,要挂唤醒续跑。
        XCTAssertEqual(SessionExitReason.classify(
            cancelled: false, exitCode: 0,
            lastHealthKind: .rateLimited, healthAt: now.addingTimeInterval(-60), now: now),
            .hitLimit)
    }

    func testStaleQuotaHealthDoesNotMisclassify() {
        // 几小时前撞过墙、恢复后正常退出 → 不算 hit limit（health 是 sticky 首报）。
        XCTAssertEqual(SessionExitReason.classify(
            cancelled: false, exitCode: 0,
            lastHealthKind: .usageLimit, healthAt: now.addingTimeInterval(-7200), now: now),
            .completed)
    }

    func testAuthHealthIsNotHitLimit() {
        XCTAssertEqual(SessionExitReason.classify(
            cancelled: false, exitCode: 1,
            lastHealthKind: .authRequired, healthAt: now.addingTimeInterval(-5), now: now),
            .failed)
    }

    func testPlainExitCodes() {
        XCTAssertEqual(SessionExitReason.classify(
            cancelled: false, exitCode: 0, lastHealthKind: nil, healthAt: nil, now: now),
            .completed)
        XCTAssertEqual(SessionExitReason.classify(
            cancelled: false, exitCode: nil, lastHealthKind: nil, healthAt: nil, now: now),
            .completed)
        XCTAssertEqual(SessionExitReason.classify(
            cancelled: false, exitCode: 2, lastHealthKind: nil, healthAt: nil, now: now),
            .failed)
    }
}
#endif
