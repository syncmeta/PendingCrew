import XCTest

final class AppUpdateSharedTests: XCTestCase {
    // MARK: - UpdateCheckGate 真值表

    func testUserInitiatedAlwaysPasses() {
        XCTAssertTrue(UpdateCheckGate.allows(userInitiated: true, busy: true),
                      "人手点的检查在忙时也必须放行")
        XCTAssertTrue(UpdateCheckGate.allows(userInitiated: true, busy: false))
    }

    func testBackgroundCheckBlockedWhileBusy() {
        XCTAssertFalse(UpdateCheckGate.allows(userInitiated: false, busy: true),
                       "有 session 在跑时后台检查必须拦下")
        XCTAssertTrue(UpdateCheckGate.allows(userInitiated: false, busy: false))
    }

    // MARK: - AppBuildStamp

    func testVersionDisplayWithFullStamp() {
        let info: [String: Any] = [
            "CFBundleShortVersionString": "0.2.0",
            "CFBundleVersion": "512",
            "BuildStampCommit": "abcdef0123456789abcdef0123456789abcdef01",
        ]
        XCTAssertEqual(AppBuildStamp.versionDisplay(info: info), "0.2.0 (512) · abcdef0")
    }

    func testVersionDisplayWithoutStampSaysSo() {
        let info: [String: Any] = [
            "CFBundleShortVersionString": "0.2.0",
            "CFBundleVersion": "512",
        ]
        XCTAssertEqual(AppBuildStamp.versionDisplay(info: info), "0.2.0 (512) · 无版本戳",
                       "缺戳要明说，不能拿占位值装有")
    }

    func testVersionDisplayTreatsShortCommitAsMissing() {
        let info: [String: Any] = [
            "CFBundleShortVersionString": "0.2.0",
            "CFBundleVersion": "512",
            "BuildStampCommit": "abc",
        ]
        XCTAssertEqual(AppBuildStamp.versionDisplay(info: info), "0.2.0 (512) · 无版本戳")
    }
}
