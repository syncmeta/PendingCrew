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

    func testGuiIsOrchestratorWhenBackendFlagAbsentOrInproc() {
        // P0~P3 期间总闸没设 / 设成 inproc —— GUI 进程**就是**所有者。
        XCTAssertEqual(ProcessRole.resolve(argv: ["PendingCrew"], backendFlag: nil), .orchestrator)
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew"], backendFlag: "inproc"), .orchestrator)
        // 不认识的值按 inproc 兜底：写错环境变量不该让 app 变成没人管账的空壳。
        XCTAssertEqual(
            ProcessRole.resolve(argv: ["PendingCrew"], backendFlag: "banana"), .orchestrator)
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
