#if os(macOS)
import Foundation
import XCTest

/// 「我认识哪些后端」（人类 Todo #121）。
///
/// 这一层最要紧的三条都不是「能存能读」：
/// ① 本机那条**不是特例**、但也**删不掉**；
/// ② 远程那一档**绝不许静默降级成本机**；
/// ③ 存盘读不动时**不许显示成空列表**。
final class BackendRegistryTests: XCTestCase {

    private func tempURL() -> URL {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("pc-backends-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        addTeardownBlock {
            chmod(dir.path, 0o755)
            try? FileManager.default.removeItem(at: dir)
        }
        return dir.appendingPathComponent("backends.json")
    }

    private func remote(_ id: String) -> BackendRef {
        BackendRef(id: id, displayName: id, transport: .remote(url: "wss://\(id).example"))
    }

    // MARK: - 本机那条：是列表的一员，但删不掉

    func testTheLocalBackendIsAlwaysPresentAndFirst() {
        let refs = BackendRegistry.normalize([remote("a"), remote("b")])
        XCTAssertEqual(refs.first?.id, BackendRegistry.localId, "本机那条不在第一位")
        XCTAssertTrue(refs.first?.isBuiltIn == true)
        XCTAssertEqual(refs.count, 3)
    }

    func testTheLocalBackendCannotBeDeleted() {
        let refs = BackendRegistry.normalize([remote("a")])
        guard case let .refused(why) = BackendRegistry.removing(
            BackendRegistry.localId, from: refs) else {
            return XCTFail("内置的本机后端被删掉了 —— 删完这个界面没有任何后端可连")
        }
        XCTAssertTrue(why.contains("删不掉"), why)
    }

    func testOtherBackendsCanBeDeleted() {
        let refs = BackendRegistry.normalize([remote("a"), remote("b")])
        guard case let .removed(left) = BackendRegistry.removing("a", from: refs) else {
            return XCTFail("正常的后端删不掉")
        }
        XCTAssertEqual(left.map(\.id), [BackendRegistry.localId, "b"])
    }

    func testRemovingSomethingThatIsNotThereSaysSoRatherThanPretending() {
        guard case let .refused(why) = BackendRegistry.removing(
            "nope", from: BackendRegistry.normalize([])) else {
            return XCTFail("删一个不存在的却报成功")
        }
        XCTAssertTrue(why.contains("没有这个后端"), why)
    }

    /// 存盘里伪造一条 `isBuiltIn: true` 不作数 —— 内置只有一条，由我们说了算，
    /// 否则任何人往那个 json 里塞一行就能造出第二个「删不掉」的条目。
    func testAForgedBuiltInFlagIsIgnored() {
        var forged = remote("evil"); forged.isBuiltIn = true
        let refs = BackendRegistry.normalize([forged])
        XCTAssertEqual(refs.filter(\.isBuiltIn).map(\.id), [BackendRegistry.localId])
    }

    func testDuplicateIdsKeepOnlyTheFirst() {
        let refs = BackendRegistry.normalize([remote("a"), remote("a"), remote("b")])
        XCTAssertEqual(refs.map(\.id), [BackendRegistry.localId, "a", "b"])
    }

    // MARK: - 远程那一档：拒绝，而且绝不降级

    /// **这条是这个文件的重点。** 静默降级的症状是：人填了远程地址、界面显示
    /// 「已连接」，而他看到的其实是自己这台机器上的 session。那种错查不出来。
    func testRemoteIsRefusedAndNeverFallsBackToLocal() {
        guard case let .unsupported(why) = BackendRegistry.connectivity(
            of: remote("tokyo")) else {
            return XCTFail("远程后端被当成可连 —— 它还没实现")
        }
        XCTAssertTrue(why.contains("没有退回本机"), "没说清不会降级：\(why)")
        XCTAssertTrue(why.contains("tokyo"), "没带上他填的地址：\(why)")
    }

    func testLocalIsConnectable() {
        XCTAssertEqual(BackendRegistry.connectivity(of: BackendRegistry.builtInLocal()),
                       .supported)
    }

    // MARK: - 存 / 读

    func testSaveThenLoadRoundTrips() throws {
        let url = tempURL()
        try BackendRegistry.save(BackendRegistry.normalize([remote("a"), remote("b")]), to: url)
        guard case let .loaded(refs) = BackendRegistry.load(from: url) else {
            return XCTFail("存完读不回来")
        }
        XCTAssertEqual(refs.map(\.id), [BackendRegistry.localId, "a", "b"])
        XCTAssertTrue(refs[1].isRemote)
    }

    /// 内置那条**不写进存盘文件** —— 它每次现算；写进去会在 `PENDINGCREW_DATA_DIR`
    /// 挪走之后变成一条指向旧 socket 路径的假记录。
    func testTheBuiltInEntryIsNotPersisted() throws {
        let url = tempURL()
        try BackendRegistry.save(BackendRegistry.normalize([remote("a")]), to: url)
        let raw = try String(contentsOf: url, encoding: .utf8)
        XCTAssertFalse(raw.contains("\"\(BackendRegistry.localId)\""),
                       "内置那条被写进盘了：\(raw)")
    }

    func testNoFileYetIsFreshNotAnError() {
        guard case let .fresh(refs) = BackendRegistry.load(from: tempURL()) else {
            return XCTFail("全新机器被报成了错误")
        }
        XCTAssertEqual(refs.map(\.id), [BackendRegistry.localId])
    }

    /// **读不动不许显示成空列表** —— 人刚加过三个，界面却干干净净，
    /// 他会以为自己的配置被清了。
    func testUnreadableFileStillShowsLocalAndSaysWhy() throws {
        try XCTSkipIf(getuid() == 0, "root 绕过权限位")
        let url = tempURL()
        try BackendRegistry.save(BackendRegistry.normalize([remote("a")]), to: url)
        XCTAssertEqual(chmod(url.deletingLastPathComponent().path, 0), 0)
        defer { chmod(url.deletingLastPathComponent().path, 0o755) }

        let load = BackendRegistry.load(from: url)
        XCTAssertEqual(load.refs.map(\.id), [BackendRegistry.localId],
                       "读不动时连本机那条都没给 —— 界面会变成「一个后端都没有」")
        XCTAssertNotNil(load.problem, "读不动却一声不吭")
        XCTAssertTrue(load.problem?.contains("没有被删") == true, load.problem ?? "")
    }

    func testCorruptFileIsDegradedNotFresh() throws {
        let url = tempURL()
        try Data("{ 这不是 JSON".utf8).write(to: url)
        let load = BackendRegistry.load(from: url)
        XCTAssertNotNil(load.problem, "解不开却被当成「全新机器」")
        XCTAssertEqual(load.refs.map(\.id), [BackendRegistry.localId])
    }

    /// 不认识的 `kind` 要解码失败（而不是被猜成本机），这样上面那条降级才会生效。
    func testAnUnknownTransportKindFailsToDecodeRatherThanGuessing() throws {
        let url = tempURL()
        try Data(#"[{"id":"x","displayName":"x","kind":"carrierPigeon","address":"?"}]"#.utf8)
            .write(to: url)
        XCTAssertNotNil(BackendRegistry.load(from: url).problem,
                        "不认识的传输方式被猜成了某种能连的东西")
    }
}
#endif
