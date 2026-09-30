#if os(macOS)
import XCTest

final class CaptainAutomaticRecoveryClaimStoreTests: XCTestCase {
    func test_同一crew跨store重开及回滚新source也只能claim一次() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("captain-auto-recovery-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: directory) }
        let first = CaptainAutomaticRecoveryClaimStore(directory: directory)
        XCTAssertTrue(try first.claim(crewId: "crew", sourceSessionId: "captain-old"))

        let reopened = CaptainAutomaticRecoveryClaimStore(directory: directory)
        XCTAssertFalse(try reopened.claim(crewId: "crew", sourceSessionId: "captain-rollback"),
                       "重启或回滚产生新 session id 后也不得再次自动启动")
        XCTAssertTrue(try reopened.claim(crewId: "other-crew", sourceSessionId: "captain-old"),
                      "不同 crew 的独立硬失效不应被全局误挡")
        XCTAssertEqual(try reopened.records().count, 2)
    }

    func test_坏claim账本拒绝覆盖并保留原字节() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("captain-auto-recovery-corrupt-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = CaptainAutomaticRecoveryClaimStore(directory: directory)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let original = Data("[{\"crewId\":\"partial\"}]".utf8)
        try original.write(to: store.fileURL)

        XCTAssertThrowsError(try store.claim(crewId: "crew", sourceSessionId: "captain-old"))
        XCTAssertEqual(try Data(contentsOf: store.fileURL), original)
    }
}
#endif
