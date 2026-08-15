import XCTest

/// FamilyCredentialStore set→get→clear roundtrip。
///
/// 容忍 headless：无正常签名（ad-hoc / CLI 构建）时 keychain-access-groups
/// entitlement 不生效，SecItem* 全部 errSecMissingEntitlement → set 后 get
/// 返回 nil。这种情况下 XCTSkip，真实读写验证留给真机/Xcode GUI 签名环境。
final class FamilyCredentialStoreTests: XCTestCase {
    override func tearDown() {
        FamilyCredentialStore.clear()
        super.tearDown()
    }

    func testSetGetClearRoundtrip() throws {
        let cred = FamilyCredential(token: "pfa_test_token_0123456789", subjectId: "subject-abc")
        FamilyCredentialStore.set(cred)

        let fetched = FamilyCredentialStore.get()
        try XCTSkipIf(fetched == nil, "keychain entitlement 不可用（headless/ad-hoc 签名），共享组读写留真机验证")

        XCTAssertEqual(fetched, cred)
        XCTAssertEqual(fetched?.token, "pfa_test_token_0123456789")
        XCTAssertEqual(fetched?.subjectId, "subject-abc")

        FamilyCredentialStore.clear()
        XCTAssertNil(FamilyCredentialStore.get())
    }
}
