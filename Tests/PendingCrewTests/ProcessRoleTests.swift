#if os(macOS)
import XCTest

/// `ProcessRole` 判定表（前后端分离 P0，spec §6.2 闸门 1）。
///
/// 这条判定是「谁有资格跑长期定时器」的唯一真值 —— 判错的后果是双头
/// （两个进程各跑一套唤醒器往同一批账上写），所以每条分支都钉死。
final class ProcessRoleTests: XCTestCase {

    func testHelperArgvWinsOverEverything() {
        // helper 是短命子进程，无论总闸怎么设都不是编排者。
        for flag in [nil, "inproc", "daemon"] as [String?] {
            XCTAssertEqual(
                ProcessRole.resolve(argv: ["PendingCrew", "--mcp-serve", "--crew", "c1"],
                                    backendFlag: flag),
                .helper, "flag=\(String(describing: flag))")
        }
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew", "--mcp-hook"], backendFlag: nil), .helper)
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew", "--mcp-permission-hook"], backendFlag: nil),
            .helper)
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew", "--mcp-turn-hook"], backendFlag: nil), .helper)
    }

    func testDaemonArgvIsOrchestrator() {
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew", "--daemon"], backendFlag: nil), .orchestrator)
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew", "--daemon"], backendFlag: "daemon"),
            .orchestrator)
    }

    /// **总闸默认已翻**（P5a 收尾，设计 §9 P5a）：什么都不设 = 所有权在常驻后台，
    /// GUI 退化成 viewer。这一条就是「关掉 / 更新 app 而 session 不断」在代码里的样子。
    func testGuiIsViewerByDefaultNow() {
        XCTAssertEqual(ProcessRole.resolve(argv: ["PendingCrew"], backendFlag: nil), .viewer)
        XCTAssertEqual(ProcessRole.resolve(argv: ["PendingCrew"], backendFlag: ""), .viewer)
    }

    /// 老路仍留着，但现在要**显式**要：`inproc` = GUI 进程自己就是所有者。
    /// 它是这一期的回退开关（设计 §9 那张表的「回退方式」列）。
    func testInprocIsNowTheExplicitOptOut() {
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew"], backendFlag: "inproc"), .orchestrator)
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew"], backendFlag: " InProc "), .orchestrator)
    }

    /// 不认识的值按**新默认**兜底 = viewer。
    ///
    /// 翻默认之前这里兜的是 `.orchestrator`，理由是「没人管账比两个人管账更难发现」。
    /// 翻完之后那条理由不成立了：viewer 连不上时不会静默 —— 要么后台在跑，要么
    /// 按 §9.2 的表**拿到锁**之后临时接管，要么在界面上给一条可操作的错误。
    /// 三条出口没有一条是「没人管账且没人知道」。
    func testUnknownFlagFallsBackToTheNewDefault() {
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew"], backendFlag: "banana"), .viewer)
    }

    func testGuiIsViewerWhenBackendFlagIsDaemon() {
        XCTAssertEqual(ProcessRole.resolve(argv: ["PendingCrew"], backendFlag: "daemon"), .viewer)
    }

    func testFlagIsCaseInsensitiveAndTrimmed() {
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew"], backendFlag: " Daemon "), .viewer)
    }
}
#endif
