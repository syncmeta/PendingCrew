import Foundation
import XCTest

final class ReleaseScriptSourceContractTests: XCTestCase {
    func testMacReleaseSnapshotAndTagUseTheSameResolvedRef() throws {
        let repoRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent() // PendingCrewTests
            .deletingLastPathComponent() // Tests
            .deletingLastPathComponent() // repo root
        let scriptURL = repoRoot
            .appendingPathComponent("scripts/release/build-macos-update.sh")
        let script = try String(contentsOf: scriptURL, encoding: .utf8)

        XCTAssertTrue(script.contains("release_ref=${1:-main}"))
        XCTAssertTrue(script.contains("rev-parse --verify \"$release_ref^{commit}\""))
        XCTAssertTrue(script.contains("worktree add --detach \"$snap/src\" \"$release_commit\""))
        XCTAssertTrue(script.contains("snapshot_commit=$(git -C \"$snap/src\" rev-parse HEAD)"))
        XCTAssertTrue(script.contains("[ \"$snapshot_commit\" = \"$release_commit\" ]"))
        XCTAssertTrue(script.contains("\"$snap/src/CHANGELOG.md\""))
        XCTAssertTrue(script.contains("tag \"$tag_name\" \"$snapshot_commit\""))
        XCTAssertTrue(script.contains("[ \"$tagged_commit\" != \"$snapshot_commit\" ]"))

        XCTAssertFalse(script.contains("worktree add --detach \"$snap/src\" main"))
        XCTAssertFalse(script.contains("tag \"v$version\" main"))
    }
}
