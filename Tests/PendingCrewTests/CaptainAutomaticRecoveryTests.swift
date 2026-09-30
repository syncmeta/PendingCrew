#if os(macOS)
import XCTest

final class CaptainAutomaticRecoveryTests: XCTestCase {
    private func context(
        trigger: CaptainAutomaticRecoveryTrigger,
        isCaptain: Bool = true,
        sourceKind: LocalCodingAgentKind = .claudeCode,
        hasOtherLiveCaptain: Bool = false
    ) -> CaptainAutomaticRecoveryContext {
        .init(isCaptain: isCaptain, sourceKind: sourceKind,
              hasOtherLiveCaptain: hasOtherLiveCaptain, trigger: trigger)
    }

    func test_明确终止失败的Claude机长可进入一次性救援资格() {
        XCTAssertEqual(CaptainAutomaticRecoveryPolicy.decide(context(trigger: .endedFailed)), .recover)
    }

    func test_当前run明确认证或启动失败可进入一次性救援资格() {
        XCTAssertEqual(CaptainAutomaticRecoveryPolicy.decide(
            context(trigger: .authenticationRequired)), .recover)
        XCTAssertEqual(CaptainAutomaticRecoveryPolicy.decide(
            context(trigger: .launchFailed)), .recover)
    }

    func test_旧epoch的expectedLive仅在typedProbe确认进程不存在时可救援() {
        XCTAssertEqual(CaptainAutomaticRecoveryPolicy.decide(context(trigger: .expectedLiveMissing(
            recordIsCaptain: true, recordBelongsToOldDaemonEpoch: true, probe: .missing))), .recover)
        XCTAssertEqual(CaptainAutomaticRecoveryPolicy.decide(context(trigger: .expectedLiveMissing(
            recordIsCaptain: true, recordBelongsToOldDaemonEpoch: true, probe: .unreadable(errno: 1)))), .alertOnly)
    }

    func test_无响应worker和已有liveCaptain永远只告警() {
        XCTAssertEqual(CaptainAutomaticRecoveryPolicy.decide(context(trigger: .unresponsive)), .alertOnly)
        XCTAssertEqual(CaptainAutomaticRecoveryPolicy.decide(context(
            trigger: .authenticationRequired, isCaptain: false)), .alertOnly)
        XCTAssertEqual(CaptainAutomaticRecoveryPolicy.decide(context(
            trigger: .launchFailed, hasOtherLiveCaptain: true)), .alertOnly)
        XCTAssertEqual(CaptainAutomaticRecoveryPolicy.decide(context(
            trigger: .endedFailed, sourceKind: .codex)), .alertOnly,
                       "Codex 自身失败不能跨 runner 递归接任")
    }

    /// 同一个 onEnded 如果已确认是 Claude resume 被拒绝，旧逻辑会一边排无 resume
    /// 重启、一边因 `.failed` 排 Codex handoff。两条都是真启动入口，必须先由一个
    /// 互斥决策拥有本次结束，而不是靠后续 roster 时序碰运气。
    func test_结束时resume拒绝不得与P2硬失效并排调度() throws {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let runner = try String(contentsOf: root.appendingPathComponent(
            "Sources/Mac/Services/CrewSessionRunner.swift"))
        XCTAssertFalse(runner.contains(
            "if r.exitReason == .failed {\n                    self?.recoverConfirmedUnavailableCaptain(r, trigger: .endedFailed)"),
            "onEnded 必须先统一决定 resume 降级或 P2 救援，不能并排排两条启动任务")
        XCTAssertTrue(runner.contains("sourceRunID: run.runID"),
                      "current-run 硬失效必须把 roster 身份带入异步 handoff")
        XCTAssertTrue(runner.contains("self.runs.contains(where: { $0.runID == sourceRunID })"),
                      "resume fallback 移除旧 run 后，迟到 handoff 不得跨 runner 接任")
    }

    func test_同一次结束最多选择一次backendLaunch且resume降级优先() {
        let route = CaptainEndedRecoveryOwnership.decide(
            resumeFallbackScheduled: true, isCaptain: true, exitReason: .failed)
        let launches: [LocalCodingAgentKind] = switch route {
        case .retryClaudeWithoutResume: [.claudeCode]
        case .automaticCaptainRecovery: [.codex]
        case .none: []
        }
        XCTAssertEqual(route, .retryClaudeWithoutResume)
        XCTAssertEqual(launches, [.claudeCode])
        XCTAssertEqual(launches.count, 1, "同一 terminal event 不得安排第二个 backend launch")
    }

    func test_没有resume降级时仅失败captain才选择P2接任() {
        XCTAssertEqual(CaptainEndedRecoveryOwnership.decide(
            resumeFallbackScheduled: false, isCaptain: true, exitReason: .failed),
                       .automaticCaptainRecovery)
        XCTAssertEqual(CaptainEndedRecoveryOwnership.decide(
            resumeFallbackScheduled: false, isCaptain: false, exitReason: .failed), .none)
    }
}
#endif
