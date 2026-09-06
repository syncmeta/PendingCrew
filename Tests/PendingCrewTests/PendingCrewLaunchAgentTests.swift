#if os(macOS)
import Foundation
import XCTest

/// 开机自启那份 LaunchAgent（P5b·A）。
///
/// 这里钉两样东西：
/// 1. **plist 的内容**——它是一份"描述会变的事实"的静态文件，唯一不会过期的办法
///    就是让它被一个现算的值盯着。手改 plist → 红；改 Swift 那边 → 也红，直到
///    两边一致。哪边是事实源写在 plist 的注释里。
/// 2. **退出码那条分岔**——`--from-launchd` 存在的全部理由。
final class PendingCrewLaunchAgentTests: XCTestCase {

    // MARK: - plist 本体

    /// 仓库里那份 plist 必须逐键等于 `PendingCrewLaunchAgent.plist`。
    ///
    /// 变异自证：把 plist 里的 `SuccessfulExit` 改成 `<true/>`（或把 `--from-launchd`
    /// 删掉），这条立刻红。
    func testShippedPlistMatchesTheComputedPlan() throws {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()      // Tests/PendingCrewTests
            .deletingLastPathComponent()      // Tests
            .deletingLastPathComponent()      // 仓库根
            .appendingPathComponent("Resources/LaunchAgents")
            .appendingPathComponent(PendingCrewLaunchAgent.plistName)
        let data = try Data(contentsOf: url)
        let shipped = try XCTUnwrap(
            try PropertyListSerialization.propertyList(from: data, format: nil)
                as? [String: Any])
        let planned = PendingCrewLaunchAgent.plist

        XCTAssertEqual(Set(shipped.keys), Set(planned.keys),
                       "plist 的键和 Swift 那边对不上了 —— 事实源是 Swift，改那边")
        XCTAssertEqual(shipped["Label"] as? String, planned["Label"] as? String)
        XCTAssertEqual(shipped["BundleProgram"] as? String, planned["BundleProgram"] as? String)
        XCTAssertEqual(shipped["ProgramArguments"] as? [String],
                       planned["ProgramArguments"] as? [String])
        XCTAssertEqual(shipped["RunAtLoad"] as? Bool, planned["RunAtLoad"] as? Bool)
        XCTAssertEqual(shipped["ThrottleInterval"] as? Int, planned["ThrottleInterval"] as? Int)
        XCTAssertEqual(shipped["KeepAlive"] as? [String: Bool],
                       planned["KeepAlive"] as? [String: Bool])
    }

    /// **只在异常退出时重启。** 写成 `KeepAlive = true` 的话，用户正常停用之后
    /// launchd 立刻把它拉回来 —— 那不是技术细节，是把「我想关掉它」从人手里拿走。
    func testKeepAliveOnlyRestartsAfterAnUnsuccessfulExit() {
        XCTAssertEqual(PendingCrewLaunchAgent.plist["KeepAlive"] as? [String: Bool],
                       ["SuccessfulExit": false])
        XCTAssertNil(PendingCrewLaunchAgent.plist["KeepAlive"] as? Bool,
                     "KeepAlive 成了裸 true —— 这个后台就再也关不掉了")
    }

    /// 优雅退出的那个码，喂给这份 plist 的策略，必须得出「不重启」。
    /// 这是 `--daemon-stop` 之后进程真的停住的**唯一依据**。
    func testStoppingTheDaemonDoesNotGetItRestarted() {
        XCTAssertFalse(
            PendingCrewLaunchAgent.restartPolicy
                .wouldRestart(after: .exited(DaemonShutdownPolicy.gracefulExitCode)),
            "人按了停，launchd 又把它拉回来")
        XCTAssertTrue(
            PendingCrewLaunchAgent.restartPolicy.wouldRestart(after: .killedBySignal(SIGSEGV)),
            "崩了却不自拉，那这份 plist 就只剩自启没有自愈了")
    }

    /// plist 里的 flag 必须就是 `--daemon` 那个真身。两边各写一份字面量的话，
    /// 改了一边另一边不会有任何反应 —— plist 里那个会安静地变成没人认识的参数。
    func testProgramArgumentsUseTheRealDaemonFlag() {
        let args = PendingCrewLaunchAgent.plist["ProgramArguments"] as? [String]
        XCTAssertEqual(args?.first, PendingCrewLaunchAgent.bundleProgram, "argv[0]")
        XCTAssertEqual(args?.dropFirst().first, SessionDaemonMainFlag.daemon)
        XCTAssertTrue(args?.contains(PendingCrewLaunchAgent.launchdFlag) == true)
    }

    // MARK: - 退出码那条分岔（`--from-launchd` 存在的理由）

    /// **这条是这一笔里最容易被漏掉的一处。**
    ///
    /// 人开着 GUI 时，app 窗口占着编排锁。launchd 拉起来的那个 daemon 一看
    /// 「已经有编排者了」就该安静退出 —— 如果它退非 0，`SuccessfulExit=false`
    /// 会立刻把它拉回来，于是「人开着窗口」这个再正常不过的状态变成一个
    /// 10 秒一轮、只在日志里无声滚动的重启循环。
    func testLaunchdIsNotToldToRestartWhenTheAppWindowAlreadyOrchestrates() {
        let appHoldsIt = SessionDaemonHost.StartError
            .alreadyOrchestrated("app 窗口占着", holderIsDaemon: false)
        XCTAssertEqual(
            DaemonExitCode.forDaemonStart(appHoldsIt, launchedByLaunchd: true),
            DaemonExitCode.ok,
            "launchd 会把这个非 0 当异常退出，10 秒后再拉一次，永远")
    }

    /// 同一件事对 **app** 而言仍然是失败：它要的是一个 daemon，而它没拿到。
    /// 一个信号当两件事用是病根；这里是让第二个提问人自报身份，不是把答案改了。
    func testTheAppStillGetsTheHonestFailureForTheSameSituation() {
        let appHoldsIt = SessionDaemonHost.StartError
            .alreadyOrchestrated("app 窗口占着", holderIsDaemon: false)
        XCTAssertEqual(DaemonExitCode.forDaemonStart(appHoldsIt), DaemonExitCode.failed)
    }

    /// 「另一个 daemon 已经在跑」对谁都是成功。
    func testAnotherDaemonAlreadyRunningIsSuccessForBoth() {
        let daemonHoldsIt = SessionDaemonHost.StartError
            .alreadyOrchestrated("另一个 daemon", holderIsDaemon: true)
        XCTAssertEqual(DaemonExitCode.forDaemonStart(daemonHoldsIt), DaemonExitCode.ok)
        XCTAssertEqual(
            DaemonExitCode.forDaemonStart(daemonHoldsIt, launchedByLaunchd: true),
            DaemonExitCode.ok)
    }

    /// **真失败仍然要报失败**，launchd 该重试：数据根不可写这种事修好之后
    /// 它自己就起来了。别让上面那条分岔顺手把所有失败都吞成 0。
    func testRealFailuresStayNonZeroEvenUnderLaunchd() {
        for error in [SessionDaemonHost.StartError.lockUnavailable("数据根不可写"),
                      .listen(NSError(domain: "x", code: 1))] {
            XCTAssertEqual(DaemonExitCode.forDaemonStart(error, launchedByLaunchd: true),
                           DaemonExitCode.failed, "\(error)")
        }
    }

    func testLaunchdFlagIsRecognisedInArgv() {
        XCTAssertTrue(PendingCrewLaunchAgent.launchedByLaunchd(
            ["/x/PendingCrew", "--daemon", "--from-launchd"]))
        XCTAssertFalse(PendingCrewLaunchAgent.launchedByLaunchd(["/x/PendingCrew", "--daemon"]))
    }
}

/// 开机自启这个开关的决策层（`DaemonAutostartPlan`）。
///
/// 这里最要紧的一条不是"更新后要重新注册"，是**更新绝不许顺手把开关打开**——
/// 那是在人没点头的情况下改他机器上的登录项。
final class DaemonAutostartPlanTests: XCTestCase {

    func testAnUpdateNeverTurnsAutostartOnBehindTheUsersBack() {
        XCTAssertEqual(
            DaemonAutostartPlan.refresh(state: .off, registeredBuild: nil, currentBuild: "0.1.26"),
            .notNeeded,
            "人没开自启，一次更新就把登录项装上了 —— 这比「更新后不自启」严重得多")
    }

    func testAMissingServiceIsNotSilentlyRegistered() {
        XCTAssertEqual(
            DaemonAutostartPlan.refresh(state: .missing, registeredBuild: "0.1.25",
                                        currentBuild: "0.1.26"),
            .notNeeded)
    }

    /// SDK 头文件：app 换了可执行文件之后必须重新注册，否则**可能起不来**。
    /// 不处理的症状不是报错，是安静地不再自启、而开关看起来还是"已开启"。
    func testReregistersAfterTheAppWasUpdated() {
        guard case let .reregister(reason) = DaemonAutostartPlan.refresh(
            state: .on, registeredBuild: "0.1.25(9)", currentBuild: "0.1.26(10)") else {
            return XCTFail("更新之后没有重新注册")
        }
        XCTAssertTrue(reason.contains("0.1.26(10)"), reason)
    }

    func testDoesNothingWhenTheBuildIsUnchanged() {
        XCTAssertEqual(
            DaemonAutostartPlan.refresh(state: .on, registeredBuild: "0.1.26(10)",
                                        currentBuild: "0.1.26(10)"),
            .notNeeded)
    }

    /// 「等人去系统设置里点头」这一档也要跟着更新走 —— 它已经注册了。
    func testAwaitingApprovalStillFollowsTheApp() {
        guard case .reregister = DaemonAutostartPlan.refresh(
            state: .waitingForApproval, registeredBuild: "0.1.25", currentBuild: "0.1.26") else {
            return XCTFail("已注册但没生效的那一档被漏掉了")
        }
    }

    /// **「已注册」不等于「会起来」。** 把 `waitingForApproval` 当成开启，界面就会
    /// 显示"已开启"而它一次都不会自启，人查不出所以然。
    func testAwaitingApprovalIsNotShownAsOn() {
        XCTAssertFalse(DaemonAutostartState.waitingForApproval.isRunningAtLogin)
        XCTAssertTrue(DaemonAutostartState.on.isRunningAtLogin)
        XCTAssertTrue(DaemonAutostartState.waitingForApproval.text.contains("系统设置"),
                      "得告诉人去哪儿点这一下")
    }
}
#endif
