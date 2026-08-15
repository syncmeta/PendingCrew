import XCTest

final class FamilyCredentialCodecTests: XCTestCase {
    func testDecodesLegacyPayloadWithoutProfile() throws {
        let legacy = #"{"token":"pfa_abc","subjectId":"11111111-1111-1111-1111-111111111111"}"#
        let cred = try JSONDecoder().decode(FamilyCredential.self, from: Data(legacy.utf8))
        XCTAssertEqual(cred.token, "pfa_abc")
        XCTAssertNil(cred.displayName)
        XCTAssertNil(cred.avatarSeed)
    }

    func testRoundTripsProfileFields() throws {
        let cred = FamilyCredential(token: "pfa_x", subjectId: "sid", displayName: "阿狸", avatarSeed: "seed-9")
        let data = try JSONEncoder().encode(cred)
        let back = try JSONDecoder().decode(FamilyCredential.self, from: data)
        XCTAssertEqual(back.displayName, "阿狸")
        XCTAssertEqual(back.avatarSeed, "seed-9")
    }
}
