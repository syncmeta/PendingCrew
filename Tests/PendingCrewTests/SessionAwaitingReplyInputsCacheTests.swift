import XCTest

final class SessionAwaitingReplyInputsCacheTests: XCTestCase {
    private func tempDir() -> URL {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("awaiting-inputs-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    func testQuestionMarkerTracksChangesWithoutLegacyApprovalReads() {
        let dir = tempDir()
        defer { try? FileManager.default.removeItem(at: dir) }
        let key = SessionAwaitingReplyInputsCache.RunKey(crewId: "crew", sessionId: "session")
        let cache = SessionAwaitingReplyInputsCache(directory: dir)
        XCTAssertNil(cache.refresh(runs: [key])[key]?.trailingQuestion)

        let marker = SessionTurnMarker(directory: dir, crewId: "crew", sessionId: "session")
        marker.write(.init(lastMessageId: "m1", lastTurnId: "t1", awaitingQuestion: "A 还是 B？"))
        XCTAssertEqual(cache.refresh(runs: [key])[key]?.trailingQuestion, "A 还是 B？")
        let reads = cache.markerReadCount
        _ = cache.refresh(runs: [key])
        XCTAssertEqual(cache.markerReadCount, reads)

        marker.clearAwaitingQuestion()
        XCTAssertNil(cache.refresh(runs: [key])[key]?.trailingQuestion)
    }
}
