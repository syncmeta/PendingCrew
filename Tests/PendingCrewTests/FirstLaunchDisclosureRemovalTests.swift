import Foundation
import XCTest

final class FirstLaunchDisclosureRemovalTests: XCTestCase {
    func testAppHasNoFirstLaunchDisclosureGate() throws {
        let repoRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent() // PendingCrewTests
            .deletingLastPathComponent() // Tests
            .deletingLastPathComponent() // repo root
        let appSource = try String(
            contentsOf: repoRoot.appendingPathComponent("Sources/PendingCrewApp.swift"),
            encoding: .utf8
        )

        XCTAssertFalse(
            appSource.contains("FirstLaunchDisclosureGate"),
            "首次启动不应再被披露 sheet 阻挡"
        )
        XCTAssertFalse(
            FileManager.default.fileExists(
                atPath: repoRoot
                    .appendingPathComponent("Sources/Mac/Views/FirstLaunchDisclosureView.swift")
                    .path
            ),
            "整套首次启动披露视图和 UserDefaults 接受状态应已删除"
        )
    }
}
