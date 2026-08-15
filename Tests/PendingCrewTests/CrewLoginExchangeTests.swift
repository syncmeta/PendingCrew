import XCTest

final class CrewLoginExchangeTests: XCTestCase {
    func testHappyPathMintsGrantThenTearsDownSession() async throws {
        var savedGrant: String?
        var savedFamily: FamilyCredential?
        var teardownCalled = false
        let exchange = CrewLoginExchange(
            issueFamily: { token in
                XCTAssertEqual(token, "jwt-123")
                return .init(token: "pfa_x", subjectId: "sid", displayName: "阿狸", avatarSeed: "seed-1")
            },
            mintGrant: { fam, sid in
                XCTAssertEqual(fam, "pfa_x"); XCTAssertEqual(sid, "sid")
                return "pdg_ok"
            },
            saveGrant: { savedGrant = $0 },
            saveFamily: { savedFamily = $0 },
            teardown: { teardownCalled = true }
        )
        try await exchange.run(accessToken: "jwt-123")
        XCTAssertEqual(savedGrant, "pdg_ok")
        XCTAssertEqual(savedFamily?.token, "pfa_x")
        XCTAssertEqual(savedFamily?.displayName, "阿狸")
        XCTAssertEqual(savedFamily?.avatarSeed, "seed-1")
        XCTAssertTrue(teardownCalled, "session 必须在兑换后被清")
    }

    func testTeardownRunsEvenWhenMintFails() async {
        var teardownCalled = false
        var grantSaved = false
        let exchange = CrewLoginExchange(
            issueFamily: { _ in .init(token: "pfa_x", subjectId: "sid", displayName: nil) },
            mintGrant: { _, _ in throw CrewLoginExchange.Failure.mintFailed },
            saveGrant: { _ in grantSaved = true },
            saveFamily: { _ in },
            teardown: { teardownCalled = true }
        )
        do { try await exchange.run(accessToken: "jwt"); XCTFail("应抛错") }
        catch {}
        XCTAssertTrue(teardownCalled, "失败路径也必须清 session（不留全权 session）")
        XCTAssertFalse(grantSaved, "mint 失败不该存 grant")
    }
}
