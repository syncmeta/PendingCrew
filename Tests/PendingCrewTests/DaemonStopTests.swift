#if os(macOS)
import Darwin
import Foundation
import XCTest

/// `--daemon-stop`（P5b 的停用入口）与它依赖的三态锁探测。
///
/// 这条命令的危险不在"停不掉"——停不掉是看得见的。危险在**它报了成功而后台还活着**：
/// 退出码 0 会让 `--daemon-stop && rm -rf 数据根` 一路走下去，删在一个正在写的目录上。
final class DaemonStopTests: XCTestCase {

    private func holder(kind: String, pid: Int32) -> SessionOrchestratorLock.Holder {
        .init(kind: kind, pid: pid, startTimeSeconds: 1, startTimeMicroseconds: 2,
              dataRoot: "/tmp/x", acquiredAt: Date(timeIntervalSince1970: 0))
    }

    /// 固定 presence 序列的假 stopper：每次探测取下一个，用完停在最后一个上。
    private func stopper(_ sequence: [SessionOrchestratorLock.Presence],
                         signals: UnsafeMutablePointer<[Int32]>,
                         termResult: Int32 = 0,
                         timeout: TimeInterval = 8) -> DaemonStopper {
        var index = 0
        var clock = Date(timeIntervalSince1970: 0)
        return DaemonStopper(
            presence: {
                let value = sequence[min(index, sequence.count - 1)]
                index += 1
                return value
            },
            sendTerm: { pid in signals.pointee.append(pid); return termResult },
            tick: { clock += 0.1 },
            now: { clock },
            timeout: timeout)
    }

    // MARK: - 报成功的两种情形，都必须是「期望状态真的成立」

    func testNothingRunningIsSuccessBecauseTheDesiredStateAlreadyHolds() {
        var signals: [Int32] = []
        let outcome = stopper([.none], signals: &signals).stop()
        XCTAssertEqual(outcome, .alreadyStopped("PendingCrew 后台进程没有在运行。"))
        XCTAssertEqual(outcome.exitCode, 0, "本来就没在跑 = 期望状态成立 = 成功")
        XCTAssertEqual(signals, [], "没有后台却发了信号，说明我们打错了对象")
    }

    func testStopsTheDaemonAndConfirmsTheLockWasReleased() {
        var signals: [Int32] = []
        let outcome = stopper([.held(holder(kind: "daemon", pid: 4321)), .none],
                              signals: &signals).stop()
        XCTAssertEqual(outcome, .stopped(pid: 4321))
        XCTAssertEqual(outcome.exitCode, 0)
        XCTAssertEqual(signals, [4321])
    }

    /// 探到它、发信号时它刚好没了。期望状态成立，别报错吓人。
    func testAlreadyGoneBetweenTheProbeAndTheSignalIsSuccess() {
        var signals: [Int32] = []
        let outcome = stopper([.held(holder(kind: "daemon", pid: 7))],
                              signals: &signals, termResult: ESRCH).stop()
        XCTAssertEqual(outcome, .alreadyStopped("PendingCrew 后台进程已经退出。"))
        XCTAssertEqual(outcome.exitCode, 0)
    }

    // MARK: - 绝不许报成功的几种情形

    /// **这条是这个文件的重点。** 「我读不到那把锁」不是「没有后台在跑」——
    /// 压成同一个答案，就是 `CrewDirectory` 把「读不动」说成「查无此号」那个 bug
    /// 换了个出口，而这个出口后面接的是 `rm -rf`。
    func testCannotTellIsNeverReportedAsStopped() {
        var signals: [Int32] = []
        let outcome = stopper([.undecidable("打不开 /x/orchestrator.lock：Permission denied")],
                              signals: &signals).stop()
        XCTAssertNotEqual(outcome.exitCode, 0,
                          "说不清后台在不在却退 0 —— 后面那条 rm -rf 会删在正在写的目录上")
        XCTAssertTrue(outcome.text.contains("说不清"), outcome.text)
        XCTAssertEqual(signals, [])
    }

    func testHeldByAnUnidentifiableProcessIsRefusedRatherThanGuessed() {
        var signals: [Int32] = []
        let outcome = stopper([.heldByUnknown("锁被占着，但读不出持有者")], signals: &signals).stop()
        XCTAssertNotEqual(outcome.exitCode, 0)
        XCTAssertEqual(signals, [], "认不出是谁就发信号，等于对着一个不知道是什么的进程开枪")
    }

    /// 同一把编排锁也可能被 **app 窗口**拿着。这条命令要停的是后台进程，
    /// 分不清的话它会去 SIGTERM 一个正开着的 GUI。
    func testNeverSignalsTheAppWindow() {
        var signals: [Int32] = []
        let outcome = stopper([.held(holder(kind: "app", pid: 999))], signals: &signals).stop()
        XCTAssertEqual(signals, [], "把 GUI 当后台停掉了")
        XCTAssertNotEqual(outcome.exitCode, 0, "什么都没停却报成功")
        XCTAssertTrue(outcome.text.contains("⌘Q"), "得告诉人怎么才停得掉：\(outcome.text)")
    }

    /// 停不下来时**大声说停不下来**，不要偷偷升级到 SIGKILL ——
    /// SIGKILL 跳过收尾，会把它底下的 agent 子进程全变成孤儿，
    /// 而避免这件事正是优雅退出存在的全部理由。
    func testDoesNotEscalateToSigkill() {
        var signals: [Int32] = []
        let stuck = SessionOrchestratorLock.Presence.held(holder(kind: "daemon", pid: 555))
        let outcome = stopper([stuck], signals: &signals, timeout: 1).stop()
        XCTAssertEqual(signals, [555], "只该发一次 SIGTERM；多出来的那次就是 SIGKILL")
        XCTAssertNotEqual(outcome.exitCode, 0)
        XCTAssertTrue(outcome.text.contains("kill -9"), "至少要把强杀的办法交给人：\(outcome.text)")
    }

    /// 发完信号之后锁变得读不出来：**这也不算停掉了**。
    func testLosingSightOfItAfterSignallingIsNotSuccess() {
        var signals: [Int32] = []
        let outcome = stopper([.held(holder(kind: "daemon", pid: 8)),
                               .undecidable("目录不可读")], signals: &signals).stop()
        XCTAssertNotEqual(outcome.exitCode, 0)
        XCTAssertTrue(outcome.text.contains("确认不了"), outcome.text)
    }

    /// 等待上限必须**大于**优雅退出自己的预算，否则我们会在它正常收尾的半路上
    /// 宣布「停不掉」，而它下一秒就退了。
    func testWaitBudgetOutlastsTheGracefulDrain() {
        let dataRoot = URL(fileURLWithPath: "/tmp/whatever")
        XCTAssertGreaterThan(DaemonStopper(dataRoot: dataRoot).timeout,
                             DaemonShutdownPolicy.drainBudget)
    }

    // MARK: - 三态探测本身（真文件系统）

    func testNoLockFileMeansNobodyIsOrchestrating() throws {
        let root = try makeTempDir()
        XCTAssertEqual(SessionOrchestratorLock.presence(dataRoot: root), .none)
    }

    /// **不许用 `fileExists` 判「有没有锁文件」**：数据根不可读时它同样返回 false，
    /// 于是「我进不去这个目录」会被说成「这里没有编排者」。
    /// 平时准、恰好在你要诊断的那种故障下说谎 —— 这条测试就是那种故障。
    func testAnUnreadableDataRootIsUndecidableNotEmpty() throws {
        try XCTSkipIf(getuid() == 0, "root 绕过权限位，这条用例在 root 下没有意义")
        let root = try makeTempDir()
        // 先造出锁文件，再把目录锁死 —— 「文件在，但我进不去」。
        guard case .acquired = SessionOrchestratorLock.acquire(dataRoot: root, kind: "daemon") else {
            return XCTFail("取锁失败")
        }
        XCTAssertEqual(chmod(root.path, 0), 0)
        defer { chmod(root.path, 0o755) }

        guard case let .undecidable(detail) = SessionOrchestratorLock.presence(dataRoot: root) else {
            return XCTFail("目录读不进去却答成了别的 —— 这正是那条 rm -rf 的前一步")
        }
        XCTAssertTrue(detail.contains("orchestrator.lock"), detail)
    }

    func testAHeldLockNamesItsHolder() throws {
        let root = try makeTempDir()
        guard case let .acquired(handle) = SessionOrchestratorLock.acquire(
            dataRoot: root, kind: "daemon") else { return XCTFail("取锁失败") }
        defer { _ = handle }

        guard case let .held(holder) = SessionOrchestratorLock.presence(dataRoot: root) else {
            return XCTFail("锁明明被本进程占着")
        }
        XCTAssertEqual(holder.kind, "daemon")
        XCTAssertEqual(holder.pid, ProcessInfo.processInfo.processIdentifier)
    }

    /// 锁被占着、但内容读不出来 —— 第三态。**不许退化成 `.none`**。
    func testHeldButUnreadableContentIsItsOwnAnswer() throws {
        let root = try makeTempDir()
        guard case let .acquired(handle) = SessionOrchestratorLock.acquire(
            dataRoot: root, kind: "daemon") else { return XCTFail("取锁失败") }
        defer { _ = handle }

        // 就地把内容改成不是 JSON 的东西（不换文件，免得把 flock 一起换掉）。
        let path = root.appendingPathComponent(SessionOrchestratorLock.fileName).path
        let fd = open(path, O_WRONLY | O_TRUNC)
        XCTAssertGreaterThanOrEqual(fd, 0)
        _ = "not json".withCString { write(fd, $0, 8) }
        close(fd)

        guard case .heldByUnknown = SessionOrchestratorLock.presence(dataRoot: root) else {
            return XCTFail("有人占着锁，却答成了「没人」或「读不到目录」")
        }
    }

    private func makeTempDir() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("pc-stop-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        addTeardownBlock {
            chmod(url.path, 0o755)
            try? FileManager.default.removeItem(at: url)
        }
        return url
    }
}
#endif
