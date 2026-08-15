import XCTest

/// 新建 crew 的「最近工作目录」MRU 单测（Todo #29）：置顶去重、上限截断、
/// 旧单值 key 迁移、不存在路径过滤。
final class RecentWorkingDirectoriesTests: XCTestCase {

    // MARK: - 置顶去重

    func testPromotingPutsPathFirst() {
        XCTAssertEqual(RecentWorkingDirectories.promoting("/c", into: ["/a", "/b"]),
                       ["/c", "/a", "/b"])
    }

    func testPromotingDedupesExisting() {
        XCTAssertEqual(RecentWorkingDirectories.promoting("/b", into: ["/a", "/b", "/c"]),
                       ["/b", "/a", "/c"])
    }

    func testPromotingIgnoresEmptyPath() {
        XCTAssertEqual(RecentWorkingDirectories.promoting("", into: ["/a"]), ["/a"])
    }

    // MARK: - 上限截断

    func testPromotingTruncatesToLimit() {
        let list = ["/a", "/b", "/c", "/d", "/e"]
        XCTAssertEqual(RecentWorkingDirectories.promoting("/f", into: list),
                       ["/f", "/a", "/b", "/c", "/d"])
    }

    // MARK: - 旧 key 迁移

    func testMigratedAppendsLegacyWhenMissing() {
        XCTAssertEqual(RecentWorkingDirectories.migrated(list: ["/a"], legacy: "/old"),
                       ["/a", "/old"])
    }

    func testMigratedKeepsLegacyOnlyOnce() {
        XCTAssertEqual(RecentWorkingDirectories.migrated(list: ["/a", "/old"], legacy: "/old"),
                       ["/a", "/old"])
    }

    func testMigratedFromLegacyOnly() {
        XCTAssertEqual(RecentWorkingDirectories.migrated(list: nil, legacy: "/old"), ["/old"])
        XCTAssertEqual(RecentWorkingDirectories.migrated(list: nil, legacy: nil), [])
    }

    // MARK: - 不存在路径过滤

    func testExistingFiltersMissingPaths() {
        let alive: Set<String> = ["/a", "/c"]
        XCTAssertEqual(RecentWorkingDirectories.existing(["/a", "/b", "/c"]) { alive.contains($0) },
                       ["/a", "/c"])
    }

    // MARK: - UserDefaults 通道（load 迁移 + record 置顶）

    func testLoadMigratesLegacyAndFilters() throws {
        let suite = "RecentWorkingDirectoriesTests.load"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defaults.removePersistentDomain(forName: suite)
        defaults.set(["/a", "/gone"], forKey: RecentWorkingDirectories.key)
        defaults.set("/old", forKey: RecentWorkingDirectories.legacyKey)

        let loaded = RecentWorkingDirectories.load(from: defaults) { $0 != "/gone" }
        XCTAssertEqual(loaded, ["/a", "/old"])
        defaults.removePersistentDomain(forName: suite)
    }

    func testRecordPromotesAndKeepsLegacyKeyInSync() throws {
        let suite = "RecentWorkingDirectoriesTests.record"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defaults.removePersistentDomain(forName: suite)
        defaults.set("/old", forKey: RecentWorkingDirectories.legacyKey)

        RecentWorkingDirectories.record("/new", in: defaults)
        XCTAssertEqual(defaults.stringArray(forKey: RecentWorkingDirectories.key), ["/new", "/old"])
        XCTAssertEqual(defaults.string(forKey: RecentWorkingDirectories.legacyKey), "/new")

        RecentWorkingDirectories.record("/old", in: defaults)
        XCTAssertEqual(defaults.stringArray(forKey: RecentWorkingDirectories.key), ["/old", "/new"])
        defaults.removePersistentDomain(forName: suite)
    }
}
