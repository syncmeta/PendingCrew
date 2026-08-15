import XCTest

final class CrewLoginIdentityTests: XCTestCase {
    func testFromCredentialWithProfile() {
        let id = CrewLoginIdentity(credential: FamilyCredential(
            token: "pfa", subjectId: "s", displayName: "阿狸", avatarSeed: "seed"))
        XCTAssertEqual(id.title, "阿狸")
        XCTAssertEqual(id.avatarSeed, "seed")
        XCTAssertFalse(id.isGeneric)
    }
    func testFromCredentialWithoutProfileFallsBackGeneric() {
        let id = CrewLoginIdentity(credential: FamilyCredential(token: "pfa", subjectId: "s"))
        XCTAssertEqual(id.title, "本机 PendingBot 账号")
        XCTAssertEqual(id.avatarSeed, "s")        // 头像 seed 退化到 subjectId
        XCTAssertTrue(id.isGeneric)
    }
}
