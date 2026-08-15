import XCTest

/// 2026-08-12 P0 的回归闸：**读失败 ≠ 内容损坏**。
///
/// 那天晚上四轮误杀，19–24 份完全合法的 JSON 被归档并从空重建，约 2000+ 条群聊
/// 历史从 live 文件消失。病根是 `open()` 撞上 fd 上限（launchd 给 GUI app 的
/// `RLIMIT_NOFILE` 软上限只有 256）→ Foundation 把 EMFILE 包成
/// `NSFileReadNoPermissionError`（「你没有权限查看此文件」）→ 上层当成文件损坏
/// → quarantine + 重建。
///
/// 这一组测试钉死的不变式：**读不出来的时候，原文件一个字节都不许动，
/// 目录里也不许多出任何 `.corrupt-*` 归档。**
///
/// 复现手段是 `chmod 000`（拿到的正是事故当天那个 `NSFileReadNoPermissionError`），
/// 不去真把机器逼到 fd 耗尽 —— 那会波及正在跑的 app。root 下 chmod 拦不住读，
/// 所以 root 环境自动跳过。
final class MultiProcessJSONStoreReadFailureTests: XCTestCase {
    private struct Row: Codable, Equatable {
        let id: String
        let text: String
    }

    private var dir: URL!

    override func setUp() {
        super.setUp()
        dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("mpjs-readfail-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    }

    override func tearDown() {
        if let dir {
            // 还原权限，否则临时目录清不掉。
            for url in (try? FileManager.default.contentsOfDirectory(
                at: dir, includingPropertiesForKeys: nil)) ?? [] {
                try? FileManager.default.setAttributes(
                    [.posixPermissions: 0o644], ofItemAtPath: url.path)
            }
            try? FileManager.default.removeItem(at: dir)
        }
        super.tearDown()
    }

    // MARK: - 工具

    private func writeRows(_ rows: [Row], to url: URL) throws {
        try JSONEncoder().encode(rows).write(to: url, options: .atomic)
    }

    private func makeUnreadable(_ url: URL) throws -> Bool {
        guard getuid() != 0 else { return false }   // root 无视权限位
        try FileManager.default.setAttributes([.posixPermissions: 0o000], ofItemAtPath: url.path)
        // 自检：真的读不动了才算成功布置（沙盒/文件系统差异下可能仍可读）。
        return (try? Data(contentsOf: url)) == nil
    }

    private func archives() -> [URL] {
        ((try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil))
            ?? []).filter { $0.lastPathComponent.contains(".corrupt-") }
    }

    private func rawBytes(_ url: URL) -> Data? {
        try? FileManager.default.setAttributes(
            [.posixPermissions: 0o644], ofItemAtPath: url.path)
        return try? Data(contentsOf: url)
    }

    // MARK: - 核心不变式

    /// 读不出来 → 抛错、**原文件一个字节没动、零归档**。守卫拆掉这条会红。
    func testUnreadableFileIsNeverQuarantinedAndBytesUntouched() throws {
        let url = dir.appendingPathComponent("rows.json")
        let rows = [Row(id: "1", text: "历史一"), Row(id: "2", text: "历史二")]
        try writeRows(rows, to: url)
        let before = try XCTUnwrap(try? Data(contentsOf: url))

        guard try makeUnreadable(url) else {
            throw XCTSkip("当前环境下 chmod 000 仍可读（root？），这条复现不成立")
        }

        var incidents: [MultiProcessJSONStore.LedgerIncident] = []
        XCTAssertThrowsError(
            try MultiProcessJSONStore.loadRowsLockedReportingFailure(
                Row.self, at: url, onIncident: { incidents.append($0) }),
            "读不出来必须抛给调用方，不许伪装成空表")

        XCTAssertTrue(incidents.isEmpty, "严格读的读失败走 throw，不许从回调报成损坏")
        XCTAssertTrue(archives().isEmpty, "读不出来时目录里不许出现任何 .corrupt-* 归档")
        XCTAssertTrue(FileManager.default.fileExists(atPath: url.path), "原文件必须还在原地")
        XCTAssertEqual(rawBytes(url), before, "原文件一个字节都不许改动")
    }

    /// 白板 append 撞上读不出来 → 拒写 + 抛错，历史原样保留、零归档。
    /// （8-12 那天这条路径是「归档 + 从一条系统警示重建」，2000+ 条就是这么没的。）
    func testWhiteboardAppendOnUnreadableFileKeepsHistoryAndRefusesWrite() throws {
        let store = LocalWhiteboardStore(directory: dir)
        store.appendUserMessage(crewId: "c", text: "历史一")
        store.appendUserMessage(crewId: "c", text: "历史二")

        let url = dir.appendingPathComponent("c.json")
        let before = try XCTUnwrap(try? Data(contentsOf: url))
        guard try makeUnreadable(url) else {
            throw XCTSkip("当前环境下 chmod 000 仍可读（root？），这条复现不成立")
        }

        XCTAssertThrowsError(
            try store.appendSessionMessageReportingFailure(
                crewId: "c", sessionId: "s", text: "新消息"),
            "读不出来时必须如实报「没发出去」，不许回一句已发送")

        XCTAssertTrue(archives().isEmpty, "白板读不出来时不许归档原件")
        XCTAssertEqual(rawBytes(url), before, "白板历史必须原样保留")
    }

    /// 反向：**真**解不开（两次读到的都是垃圾字节）仍然照常归档 + fail-loud。
    /// 这条守住上一版 #483/#576 的行为没被这次修复顺手削掉。
    func testGenuinelyUndecodableFileStillQuarantines() throws {
        let url = dir.appendingPathComponent("rows.json")
        try Data("{ 这不是 JSON ".utf8).write(to: url)

        var incidents: [MultiProcessJSONStore.LedgerIncident] = []
        let rows = try MultiProcessJSONStore.loadRowsLockedReportingFailure(
            Row.self, at: url, onIncident: { incidents.append($0) })

        XCTAssertTrue(rows.isEmpty)
        XCTAssertEqual(incidents.count, 1, "确认损坏必须回调 fail-loud")
        XCTAssertEqual(archives().count, 1, "确认损坏应当留下一份归档供人工找回")
        XCTAssertFalse(FileManager.default.fileExists(atPath: url.path), "损坏文件已被挪走")
    }

    /// 拒写闸：读到空表但磁盘非空 → 拒写、**不归档**（8-12 之前这里也会搬走原件）。
    func testEmptyRewriteGuardRefusesWithoutArchiving() throws {
        let url = dir.appendingPathComponent("rows.json")
        try writeRows([Row(id: "1", text: "历史")], to: url)
        let before = try XCTUnwrap(try? Data(contentsOf: url))

        var incidents: [MultiProcessJSONStore.LedgerIncident] = []
        let refused = MultiProcessJSONStore.refuseEmptyRewriteIfNonEmptyFile(
            [Row](), at: url, onRefusal: { incidents.append($0) })

        XCTAssertTrue(refused, "空表 + 非空文件必须拒写")
        XCTAssertEqual(incidents.count, 1)
        XCTAssertTrue(incidents[0].isDataIntact, "拒写不再归档，原件完好")
        XCTAssertTrue(archives().isEmpty)
        XCTAssertEqual(try? Data(contentsOf: url), before)
    }

    /// 合法的空数组（账本被正常清空）不该被拒写闸误当成漏读。
    func testLegitimateEmptyArrayIsNotRefused() throws {
        let url = dir.appendingPathComponent("rows.json")
        try writeRows([Row](), to: url)
        XCTAssertFalse(
            MultiProcessJSONStore.refuseEmptyRewriteIfNonEmptyFile([Row](), at: url))
    }

    // MARK: - 两种事故两套信号（措辞在这次事故里是真实的成本项）

    /// 宽松版包装也必须**报出** `.unreadable`，而且不能报成 `.corrupt` ——
    /// 8-12 那晚 24 次读失败全被描述成「损坏，已归档」，十几个机长跑去翻归档，
    /// 发现文件好好的；当晚一半的无效轮次是这句假描述造成的。
    func testUnreadableIsReportedAsUnreadableNotCorrupt() throws {
        let url = dir.appendingPathComponent("rows.json")
        try writeRows([Row(id: "1", text: "历史")], to: url)
        guard try makeUnreadable(url) else {
            throw XCTSkip("当前环境下 chmod 000 仍可读（root？），这条复现不成立")
        }

        var incidents: [MultiProcessJSONStore.LedgerIncident] = []
        let rows: [Row] = MultiProcessJSONStore.loadRowsLocked(
            Row.self, at: url, onIncident: { incidents.append($0) })

        XCTAssertTrue(rows.isEmpty)
        XCTAssertEqual(incidents.count, 1, "读失败以前是彻底静默的，现在必须报出来")
        guard case .unreadable = incidents[0] else {
            return XCTFail("读不出来必须报 .unreadable，不许报成 .corrupt")
        }
        XCTAssertTrue(incidents[0].isDataIntact, "读不出来 = 原件完好")
        XCTAssertTrue(incidents[0].summary.contains("不是文件损坏"), incidents[0].summary)
        XCTAssertFalse(incidents[0].summary.contains("已归档"), incidents[0].summary)
    }

    /// 反过来：真解不开才说「已归档」，且不许说成「原件完好」。
    func testCorruptSaysArchivedAndNotIntact() throws {
        let url = dir.appendingPathComponent("rows.json")
        try Data("{ 这不是 JSON ".utf8).write(to: url)

        var incidents: [MultiProcessJSONStore.LedgerIncident] = []
        let rows: [Row] = MultiProcessJSONStore.loadRowsLocked(
            Row.self, at: url, onIncident: { incidents.append($0) })

        XCTAssertTrue(rows.isEmpty)
        XCTAssertEqual(incidents.count, 1)
        guard case .corrupt = incidents[0] else { return XCTFail("两次都解不开应报 .corrupt") }
        XCTAssertFalse(incidents[0].isDataIntact)
        XCTAssertTrue(incidents[0].summary.contains("已归档"), incidents[0].summary)
    }

    /// 拒写闸开火报的是 `.misread`（原件完好），不是损坏。
    func testMisreadIsItsOwnSignal() throws {
        let url = dir.appendingPathComponent("rows.json")
        try writeRows([Row(id: "1", text: "历史")], to: url)

        var incidents: [MultiProcessJSONStore.LedgerIncident] = []
        XCTAssertTrue(MultiProcessJSONStore.refuseEmptyRewriteIfNonEmptyFile(
            [Row](), at: url, onRefusal: { incidents.append($0) }))
        XCTAssertEqual(incidents.count, 1)
        guard case .misread = incidents[0] else { return XCTFail("拒写闸应报 .misread") }
        XCTAssertTrue(incidents[0].isDataIntact)
    }

    // MARK: - 错误分类（只影响要不要重试，不影响要不要销毁）

    func testTransientClassification() {
        let emfile = NSError(domain: NSPOSIXErrorDomain, code: Int(EMFILE))
        let noPermission = NSError(domain: NSCocoaErrorDomain,
                                   code: NSFileReadNoPermissionError)
        // 事故现场的真实形状：Cocoa 壳里裹着 POSIX errno。
        let wrapped = NSError(
            domain: NSCocoaErrorDomain, code: NSFileReadNoPermissionError,
            userInfo: [NSUnderlyingErrorKey: emfile])
        let corrupt = NSError(domain: NSCocoaErrorDomain, code: NSFileReadCorruptFileError)

        XCTAssertTrue(MultiProcessJSONStore.isTransientReadFailure(emfile))
        XCTAssertTrue(MultiProcessJSONStore.isTransientReadFailure(noPermission))
        XCTAssertTrue(MultiProcessJSONStore.isTransientReadFailure(wrapped))
        XCTAssertFalse(MultiProcessJSONStore.isTransientReadFailure(corrupt))

        let missing = NSError(domain: NSCocoaErrorDomain, code: NSFileReadNoSuchFileError)
        XCTAssertTrue(MultiProcessJSONStore.isNotFound(missing))
        XCTAssertFalse(MultiProcessJSONStore.isNotFound(emfile))
    }

    func testMissingFileIsLegitimateEmptyNotFailure() throws {
        let url = dir.appendingPathComponent("nope.json")
        XCTAssertNil(try MultiProcessJSONStore.readDataIfExists(at: url))
        XCTAssertTrue(
            try MultiProcessJSONStore.loadRowsLockedReportingFailure(Row.self, at: url).isEmpty)
    }

    // MARK: - fd 软上限（触发闸）

    func testSoftLimitTargetRaisesToHardLimitCeiling() {
        // launchd 给 GUI app 的实况：软 256 / 硬 unlimited。
        XCTAssertEqual(
            FileDescriptorLimit.targetSoftLimit(soft: 256, hard: FileDescriptorLimit.unlimited),
            FileDescriptorLimit.desiredSoftLimit)
        // 硬上限比想要的低 → 顶到硬上限为止。
        XCTAssertEqual(FileDescriptorLimit.targetSoftLimit(soft: 256, hard: 4096), 4096)
        // 已经够高 → 不动（不降级）。
        XCTAssertEqual(
            FileDescriptorLimit.targetSoftLimit(soft: 200_000, hard: FileDescriptorLimit.unlimited), 200_000)
    }

    func testRaiseSoftLimitIsIdempotentAndNeverLowers() {
        var before = rlimit()
        XCTAssertEqual(getrlimit(RLIMIT_NOFILE, &before), 0)
        let first = FileDescriptorLimit.raiseSoftLimitToHardLimit()
        let second = FileDescriptorLimit.raiseSoftLimitToHardLimit()
        XCTAssertGreaterThanOrEqual(first, before.rlim_cur)
        XCTAssertEqual(first, second)
    }
}
