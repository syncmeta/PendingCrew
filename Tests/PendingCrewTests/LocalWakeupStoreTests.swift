import XCTest

/// LocalWakeupStore（#528）：定时唤醒账本 —— 「有约必赴」的持久化半边。
/// 基座三件套断言照 LocalWhiteboardStoreTests 的 #483 模式。
final class LocalWakeupStoreTests: XCTestCase {
    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("wakeup-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func wakeup(_ id: String, note: String = "继续干活") -> LocalWakeupStore.PendingWakeup {
        LocalWakeupStore.PendingWakeup(
            id: id, crewId: "c", sessionId: "sess-1",
            fireAt: "2026-07-25T12:00:00Z", note: note)
    }

    private func rawFileURL(_ dir: URL) -> URL { dir.appendingPathComponent("wakeups.json") }

    private func corruptArchives(_ dir: URL) throws -> [String] {
        try FileManager.default.contentsOfDirectory(atPath: dir.path)
            .filter { $0.hasPrefix("wakeups.json.corrupt-") }
    }

    func testRegisterListRemoveRoundtrip() {
        let s = LocalWakeupStore(directory: tempDir())
        XCTAssertTrue(s.list().isEmpty)
        XCTAssertTrue(s.register(wakeup("a")))
        XCTAssertTrue(s.register(wakeup("b")))
        XCTAssertEqual(s.list().map(\.id), ["a", "b"])
        s.remove(id: "a")
        XCTAssertEqual(s.list().map(\.id), ["b"])
    }

    func testDuplicateRegisterIsNoop() {
        let s = LocalWakeupStore(directory: tempDir())
        XCTAssertTrue(s.register(wakeup("a", note: "第一次")))
        // drain 重放同 id → 不重挂、不覆盖已有备注。
        XCTAssertFalse(s.register(wakeup("a", note: "重放")))
        XCTAssertEqual(s.list().map(\.note), ["第一次"])
    }

    func testPersistsAcrossInstances() {
        let dir = tempDir()
        _ = LocalWakeupStore(directory: dir).register(wakeup("a"))
        XCTAssertEqual(LocalWakeupStore(directory: dir).list().map(\.id), ["a"])
    }

    func testCorruptFileArchivedReportsAndNextRegisterDoesNotClobber() throws {
        // 曾经的致命路径（runner 旧 loadWakeups）：损坏 → 当空 → 下一次写以空
        // 重写、全部在途约定静默失约。现在：归档可找回 + onIncident 回调 fail-loud。
        let dir = tempDir()
        let garbage = Data("not json {{{".utf8)
        try garbage.write(to: rawFileURL(dir))
        let s = LocalWakeupStore(directory: dir)
        var reported: [MultiProcessJSONStore.LedgerIncident] = []
        XCTAssertTrue(s.register(wakeup("a"), onIncident: { reported.append($0) }))
        XCTAssertEqual(reported.count, 1)
        // 这条是**真解不开**那种事故（垃圾字节），才谈得上归档（2026-08-12 两分法）。
        XCTAssertFalse(reported[0].isDataIntact)
        XCTAssertEqual(s.list().map(\.id), ["a"])
        let archived = try corruptArchives(dir)
        XCTAssertEqual(archived.count, 1)
        XCTAssertEqual(try Data(contentsOf: dir.appendingPathComponent(archived[0])), garbage)
    }

    func testLenientDecodeDropsOnlyBadElements() throws {
        // 中间一条缺必填字段 → 只丢那条，好的约不连坐，不算整文件损坏（不归档）。
        let dir = tempDir()
        let json = """
        [{"id":"a","crewId":"c","sessionId":"s","fireAt":"2026-07-25T12:00:00Z","note":"one"},
         {"id":"b","crewId":"c","sessionId":"s","fireAt":"2026-07-25T12:00:01Z"},
         {"id":"c","crewId":"c","sessionId":"s","fireAt":"2026-07-25T12:00:02Z","note":"three"}]
        """
        try Data(json.utf8).write(to: rawFileURL(dir))
        let s = LocalWakeupStore(directory: dir)
        var reported = 0
        XCTAssertEqual(s.list(onIncident: { _ in reported += 1 }).map(\.note), ["one", "three"])
        XCTAssertEqual(reported, 0)
        XCTAssertTrue(try corruptArchives(dir).isEmpty)
    }

    func testConcurrentRegistersAcrossInstancesLoseNothing() {
        // 两个实例并发 register —— flock 后一条不丢。
        let dir = tempDir()
        let a = LocalWakeupStore(directory: dir)
        let b = LocalWakeupStore(directory: dir)
        DispatchQueue.concurrentPerform(iterations: 40) { i in
            _ = (i % 2 == 0 ? a : b).register(self.wakeup("w\(i)"))
        }
        XCTAssertEqual(a.list().count, 40)
    }

    // MARK: - #577 读不出来 / 空账本

    func testRemoveOnUnreadableFileDoesNotWipeTheLedger() throws {
        // remove 当初漏装拒写闸：读不出来 → 空表 → filter + 整写 = 全部在途约定
        // 一次抹光。现在必须拒写，原字节完好可找回。
        let dir = tempDir()
        let url = rawFileURL(dir)
        let original = Data(#"[{"id":"a","crewId":"c","sessionId":"s","fireAt":"2026-07-25T12:00:00Z","note":"有约必赴"}]"#.utf8)
        try original.write(to: url)
        XCTAssertEqual(chmod(url.path, 0), 0)
        defer { _ = chmod(url.path, S_IRUSR | S_IWUSR) }

        var reported: [MultiProcessJSONStore.LedgerIncident] = []
        LocalWakeupStore(directory: dir).remove(id: "a", onIncident: { reported.append($0) })

        XCTAssertGreaterThanOrEqual(reported.count, 1, "拒写要 fail-loud，不能悄悄跳过")
        XCTAssertTrue(reported.allSatisfy(\.isDataIntact), "读不出来 = 原件完好，不许说成损坏")
        // ⚠️ 2026-08-12 失效批注：这里原先还断言「原字节被归档为 .corrupt-*」。
        // 那个动作当晚把 wakeups.json 整份搬走、连 live 文件都没重建（在途约定
        // 直接消失）。现在拒写就只是拒写：原件留在原地、零归档。
        XCTAssertTrue(try corruptArchives(dir).isEmpty, "读不出来不许归档")
        XCTAssertEqual(chmod(url.path, S_IRUSR | S_IWUSR), 0)
        XCTAssertEqual(try Data(contentsOf: url), original, "在途约定必须原样留在原地")
    }

    /// 账本被归档走之后 **live 文件彻底不存在**（2026-08-12 事故实况：`wakeups.json`
    /// 被搬走且没重建，全机 `schedule_wakeup` 静默失效 5 个多小时）——
    /// 下一次 register 必须把它重建出来，而不是继续对着不存在的文件空转。
    ///
    /// 「文件不存在 = 合法空表 → 照常写」是 `MultiProcessJSONStore` 的既有语义，
    /// 但**这条路当时没人真验过**，而验收的代价是「全机没人再约得上唤醒且零报错」。
    /// 钉死它，别再靠假设。
    func testRegisterRebuildsLedgerAfterFileWasArchivedAway() throws {
        let dir = tempDir()
        let url = rawFileURL(dir)
        let s = LocalWakeupStore(directory: dir)
        XCTAssertTrue(s.register(wakeup("a")))

        // 复刻事故现场：整份账本被 rename 成 .corrupt-*，live 文件没了，.lock 还在。
        let archive = dir.appendingPathComponent("wakeups.json.corrupt-1786533000502")
        try FileManager.default.moveItem(at: url, to: archive)
        XCTAssertFalse(FileManager.default.fileExists(atPath: url.path))

        var incidents: [MultiProcessJSONStore.LedgerIncident] = []
        XCTAssertTrue(s.register(wakeup("b"), onIncident: { incidents.append($0) }),
                      "账本文件没了也必须能重新登记，否则唤醒这条腿静默断掉")
        XCTAssertTrue(incidents.isEmpty, "文件不存在是合法空表，不是事故，不该报警吓人")
        XCTAssertTrue(FileManager.default.fileExists(atPath: url.path), "live 账本必须被重建出来")
        XCTAssertEqual(LocalWakeupStore(directory: dir).list().map(\.id), ["b"], "重建后跨实例读得到")
        // 归档里的旧约原地不动，人工可找回。
        XCTAssertEqual(try corruptArchives(dir), [archive.lastPathComponent])
    }

    func testRegisterIntoLegitimatelyEmptyLedgerIsNotTreatedAsMisread() throws {
        // 最后一条唤醒触发后账本就是 `[]`。那是正常清空，不是漏读 ——
        // 拒写闸不许把它当读失败拦下（拦下就再也约不上，还平白留一个 .corrupt-*）。
        let dir = tempDir()
        let s = LocalWakeupStore(directory: dir)
        XCTAssertTrue(s.register(wakeup("a")))
        s.remove(id: "a")
        XCTAssertEqual(try Data(contentsOf: rawFileURL(dir)), Data("[]".utf8))

        XCTAssertTrue(s.register(wakeup("b")), "空账本必须还能再登记")
        XCTAssertEqual(s.list().map(\.id), ["b"])
        XCTAssertTrue(try corruptArchives(dir).isEmpty)
    }
}
