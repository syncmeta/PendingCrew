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
    }
}
#endif
