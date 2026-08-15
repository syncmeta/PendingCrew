import XCTest

final class InMemoryAuthStorageTests: XCTestCase {
    func testStoreRetrieveRemove() {
        let s = InMemoryKeyValueStore()
        XCTAssertNil(s.get("k"))
        s.set("k", Data("v".utf8))
        XCTAssertEqual(s.get("k"), Data("v".utf8))
        s.remove("k")
        XCTAssertNil(s.get("k"))
    }
}
