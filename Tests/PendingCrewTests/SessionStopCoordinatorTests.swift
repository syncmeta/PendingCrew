import XCTest

final class SessionStopCoordinatorTests: XCTestCase {
    private let local = SessionStopTarget(
        sessionId: "worker-local", crewId: "crew-a", displayName: "本组 worker", isRunning: true)

    func testCrossCrewTargetIsRejectedWithoutReceiptOrTermination() {
        let foreign = SessionStopTarget(
            sessionId: "worker-foreign", crewId: "crew-b", displayName: "外组 worker", isRunning: true)
        var events: [String] = []
        let result = SessionStopCoordinator.execute(
            requestCrewId: "crew-a", requesterSessionId: "captain-a",
            targetSessionId: foreign.sessionId, reason: "错误派活", targets: [foreign],
            writeReceipt: { _ in events.append("receipt") },
            terminate: { _ in events.append("terminate") })

        XCTAssertTrue(result.contains("拒绝终止"), result)
        XCTAssertTrue(result.contains("不是本 crew"), result)
        XCTAssertTrue(events.isEmpty)
    }

    func testMissingTargetIsExplicitErrorWithoutSideEffects() {
        var events: [String] = []
        let result = SessionStopCoordinator.execute(
            requestCrewId: "crew-a", requesterSessionId: "captain-a",
            targetSessionId: "typo-id", reason: "写错了", targets: [local],
            writeReceipt: { _ in events.append("receipt") },
            terminate: { _ in events.append("terminate") })

        XCTAssertTrue(result.contains("找不到 session typo-id"), result)
        XCTAssertTrue(result.contains("id 写错"), result)
        XCTAssertTrue(events.isEmpty)
    }

    func testExitedTargetIsExplicitErrorWithoutSideEffects() {
        let exited = SessionStopTarget(
            sessionId: "worker-old", crewId: "crew-a", displayName: "旧 worker", isRunning: false)
        var events: [String] = []
        let result = SessionStopCoordinator.execute(
            requestCrewId: "crew-a", requesterSessionId: "captain-a",
            targetSessionId: exited.sessionId, reason: "已经不用", targets: [exited],
            writeReceipt: { _ in events.append("receipt") },
            terminate: { _ in events.append("terminate") })

        XCTAssertTrue(result.contains("已退出"), result)
        XCTAssertTrue(events.isEmpty)
    }

    func testReceiptNamesWhoTargetAndReasonBeforeTermination() {
        var events: [String] = []
        var receipt = ""
        let result = SessionStopCoordinator.execute(
            requestCrewId: "crew-a", requesterSessionId: "captain-a",
            targetSessionId: local.sessionId, reason: "任务已经取消", targets: [local],
            writeReceipt: {
                receipt = $0
                events.append("receipt")
            },
            terminate: { target in events.append("terminate:\(target.sessionId)") })

        XCTAssertEqual(events, ["receipt", "terminate:worker-local"])
        XCTAssertTrue(receipt.contains("机长（captain-a）"), receipt)
        XCTAssertTrue(receipt.contains("本组 worker"), receipt)
        XCTAssertTrue(receipt.contains("worker-local"), receipt)
        XCTAssertTrue(receipt.contains("原因：任务已经取消"), receipt)
        XCTAssertTrue(result.contains("已终止"), result)
    }
}
