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
                // 目录得留着 +x，否则 removeItem 进不去（`diagnostics/` 就是一个）。
                var isDir: ObjCBool = false
                _ = FileManager.default.fileExists(atPath: url.path, isDirectory: &isDir)
                try? FileManager.default.setAttributes(
                    [.posixPermissions: isDir.boolValue ? 0o755 : 0o644],
                    ofItemAtPath: url.path)
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

    // MARK: - 读失败的可诊断读数（2026-09-08：这一条被判过一次「没事」的根因）

    /// 2026-09-01 那次排查是这么结束的：机长在 Agent Todo #97 里写下
    /// 「旧提示未保存底层 errno，无法事后证明具体是哪种系统错误」，于是翻了
    /// completed。**他说的就是这一层。**
    ///
    /// Foundation 给的那句「未能打开文件“x.json”，因为你没有查看它的权限」对
    /// EPERM / EACCES / EMFILE **一字不差**——而这三个是完全不同的病：
    /// 句柄耗尽（8-12 那次）、文件权限、环境层拒绝（沙盒 / TCC）。
    /// 分它们的唯一判据是底层 errno，而 errno **本来就装在同一个错误对象里**。
    func testDiagnosisSeparatesTheThreeErrnosThatShareOneSentence() {
        var sentences = Set<String>()
        for (code, name) in [(EPERM, "EPERM"), (EACCES, "EACCES"), (EMFILE, "EMFILE")] {
            let wrapped = NSError(
                domain: NSCocoaErrorDomain, code: NSFileReadNoPermissionError,
                userInfo: [NSUnderlyingErrorKey:
                            NSError(domain: NSPOSIXErrorDomain, code: Int(code))])
            sentences.insert(wrapped.localizedDescription)
            let d = MultiProcessJSONStore.diagnose(wrapped, fallbackPath: nil)
            XCTAssertEqual(d.posixCode, code, "errno 必须被取出来")
            XCTAssertEqual(d.posixName, name)
            XCTAssertTrue(d.line.contains(name), d.line)
        }
        XCTAssertEqual(sentences.count, 1,
                       "前提核对：三个 errno 的 localizedDescription 确实一模一样，"
                       + "所以光有那句话永远分不出是哪一种")
    }

    /// 绝对路径同理：那句话里**永远只有文件名**（Foundation 的 localizedDescription
    /// 就是这么写的，跟数据根对不对没关系），而 `NSFilePath` 本来就在 userInfo 里。
    /// 丢掉它，「读的进程用的数据根跟写的是不是同一个」就永远查不了。
    func testDiagnosisKeepsTheAbsolutePathThatTheSentenceThrowsAway() {
        let path = "/Users/x/Library/Application Support/PendingCrew/whiteboards/c.approvals.json"
        let wrapped = NSError(
            domain: NSCocoaErrorDomain, code: NSFileReadNoPermissionError,
            userInfo: [NSFilePathErrorKey: path,
                       NSUnderlyingErrorKey:
                        NSError(domain: NSPOSIXErrorDomain, code: Int(EPERM))])
        XCTAssertFalse(wrapped.localizedDescription.contains(path),
                       "前提核对：那句话里确实没有路径")
        let d = MultiProcessJSONStore.diagnose(wrapped, fallbackPath: nil)
        XCTAssertEqual(d.path, path)
        XCTAssertTrue(d.line.contains(path), d.line)
    }

    /// 错误对象没带路径时，退到**读的人自己用的那个 URL** —— 那才是回答
    /// 「他读的到底是哪个根」的东西，不能因为 Foundation 没附上就空着。
    func testDiagnosisFallsBackToTheURLTheReaderActuallyUsed() {
        let bare = NSError(domain: NSCocoaErrorDomain, code: NSFileReadNoPermissionError)
        let url = URL(fileURLWithPath: "/tmp/somewhere/else/c.approvals.json")
        let d = MultiProcessJSONStore.diagnose(bare, fallbackPath: url)
        XCTAssertEqual(d.path, url.path)
        XCTAssertEqual(d.cocoaCode, NSFileReadNoPermissionError)
        XCTAssertNil(d.posixCode, "没带 errno 就不许编一个出来")
        XCTAssertTrue(d.line.contains("errno 没带上"), d.line)
    }

    /// 端到端：真造一次读失败，`.unreadable` 那句话必须同时带上 errno 与绝对路径。
    /// 这两样都在错误对象里，是我们自己在写文案时扔掉的。
    func testUnreadableSummaryCarriesErrnoAndAbsolutePath() throws {
        let url = dir.appendingPathComponent("rows.json")
        try writeRows([Row(id: "1", text: "历史")], to: url)
        guard try makeUnreadable(url) else {
            throw XCTSkip("当前环境下 chmod 000 仍可读（root？），这条复现不成立")
        }

        var incidents: [MultiProcessJSONStore.LedgerIncident] = []
        _ = MultiProcessJSONStore.loadRowsLocked(
            Row.self, at: url, onIncident: { incidents.append($0) })

        let summary = try XCTUnwrap(incidents.first).summary
        XCTAssertTrue(summary.contains("EACCES"), "少了 errno 就分不出是哪种病：\(summary)")
        XCTAssertTrue(summary.contains(url.path), "少了绝对路径就查不了数据根：\(summary)")
        // 8-12 立的两条不变式一个字都不许被这次改动削掉。
        XCTAssertTrue(summary.contains("不是文件损坏"), summary)
        XCTAssertFalse(summary.contains("已归档"), summary)
    }

    /// 白板写路径那句回执同理 —— 两个现场（approvals 读失败 / 白板写失败）走的是
    /// 同一个基座的同一次 `open()`，文案却各写各的，于是同一个病看起来像两件事。
    func testWhiteboardUnreadableReceiptCarriesErrnoAndAbsolutePath() throws {
        let store = LocalWhiteboardStore(directory: dir)
        store.appendUserMessage(crewId: "c", text: "历史")
        let url = dir.appendingPathComponent("c.json")
        guard try makeUnreadable(url) else {
            throw XCTSkip("当前环境下 chmod 000 仍可读（root？），这条复现不成立")
        }

        var message = ""
        XCTAssertThrowsError(
            try store.appendSessionMessageReportingFailure(
                crewId: "c", sessionId: "s", text: "新消息")
        ) { message = ($0 as NSError).localizedDescription }

        XCTAssertTrue(message.contains("EACCES"), message)
        XCTAssertTrue(message.contains(url.path), message)
        // 这句话原本写着「读不出来且**归档失败**」—— 读不出来时我们从来没试过归档，
        // 那是假的，而假描述在 8-12 那晚是真实的成本项（机长跑去翻不存在的归档）。
        XCTAssertFalse(message.contains("归档失败"), message)
    }

    /// **只写不读**的持久痕迹。为什么必须另有一条：读失败的播报是往白板 append，
    /// 而 append 自己要先把白板读出来 —— 同一刻两个都读不出来时，那条播报就被
    /// `_ = try?` 静默吞掉。于是「这个错到底发作过多少次」永远查不清。
    func testEveryReadFailureLeavesADurableTraceThatNeedsNoRead() throws {
        let url = dir.appendingPathComponent("rows.json")
        try writeRows([Row(id: "1", text: "历史")], to: url)
        guard try makeUnreadable(url) else {
            throw XCTSkip("当前环境下 chmod 000 仍可读（root？），这条复现不成立")
        }

        _ = try? MultiProcessJSONStore.loadRowsLockedReportingFailure(Row.self, at: url)

        let log = MultiProcessJSONStore.readFailureLogURL(besideFileAt: url)
        XCTAssertTrue(FileManager.default.fileExists(atPath: log.path),
                      "读失败必须留下一条不依赖任何读操作的痕迹")
        let text = try String(contentsOf: log, encoding: .utf8)
        XCTAssertTrue(text.contains("EACCES"), text)
        XCTAssertTrue(text.contains(url.path), text)
        XCTAssertTrue(text.contains("pid="), text)
        XCTAssertTrue(text.contains("argv="), text)
        XCTAssertTrue(archives().isEmpty, "留痕不许顺手动原件")
        XCTAssertEqual(rawBytes(url)?.isEmpty, false, "原件必须还在")
    }

    /// 痕迹文件不许无限长，也不许**悄悄停止记录** —— 到顶就轮转一次，
    /// 新的照常写得进去。（「从不发声的检查」那类失效正是这么来的。）
    func testDurableTraceRotatesInsteadOfSilentlyStopping() throws {
        let url = dir.appendingPathComponent("rows.json")
        try writeRows([Row(id: "1", text: "历史")], to: url)
        let log = MultiProcessJSONStore.readFailureLogURL(besideFileAt: url)
        try FileManager.default.createDirectory(
            at: log.deletingLastPathComponent(), withIntermediateDirectories: true)
        let filler = String(repeating: "x", count: MultiProcessJSONStore.readFailureLogMaxBytes + 1)
        try Data(filler.utf8).write(to: log)

        guard try makeUnreadable(url) else {
            throw XCTSkip("当前环境下 chmod 000 仍可读（root？），这条复现不成立")
        }
        _ = try? MultiProcessJSONStore.loadRowsLockedReportingFailure(Row.self, at: url)

        let rotated = log.deletingLastPathComponent()
            .appendingPathComponent(log.lastPathComponent + ".1")
        XCTAssertTrue(FileManager.default.fileExists(atPath: rotated.path), "满了要轮转出去")
        let text = try String(contentsOf: log, encoding: .utf8)
        XCTAssertTrue(text.contains("EACCES"), "轮转之后新的一条照样写得进去：\(text.prefix(200))")
        XCTAssertLessThan(text.count, MultiProcessJSONStore.readFailureLogMaxBytes)
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


/// 新文件**在数据根之外出生**（2026-09-12）。
///
/// 为什么这件事需要一把尺子：出生地事后在磁盘上**看不出来** —— 落地之后
/// 那个文件长得跟 `.atomic` 写出来的一模一样。所以 `writeStaged` 把用过的
/// 落脚路径返回来，这里才有得断言。改回 `Data.write(options:.atomic)` 时
/// 这两条会红。
final class StagedWriteTests: XCTestCase {

    func test_临时文件不在目标目录里出生() throws {
        let dir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("staged-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let target = dir.appendingPathComponent("ledger.json")

        // 落脚点由测试给，免得这条判据跟着 CI 有没有 Caches 目录一起红。
        let staging = dir.deletingLastPathComponent()
            .appendingPathComponent("staging-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: staging) }
        let staged = try MultiProcessJSONStore.writeStaged(
            Data("[1,2]".utf8), to: target, staging: staging)

        guard let staged else {
            return XCTFail("走了退路（原地原子写）——这台机器上落脚点应该可用")
        }
        // ⚠️ 比 `.path`，别比 URL：`deletingLastPathComponent()` 返回**带尾斜杠**的
        // URL，跟不带尾斜杠的 `dir` 用 `==` 永远不等 —— 那样写这条断言恒真，
        // 变异测试里刀都切进去了它还是绿的（2026-09-12 亲手撞上）。
        XCTAssertNotEqual(
            staged.deletingLastPathComponent().standardizedFileURL.path,
            dir.standardizedFileURL.path,
            "临时文件建在了目标目录里，等于没改：\(staged.path)")
        XCTAssertEqual(try Data(contentsOf: target), Data("[1,2]".utf8))
        XCTAssertEqual(
            try FileManager.default.contentsOfDirectory(atPath: dir.path), ["ledger.json"],
            "目标目录里留下了别的东西")
    }

    func test_替换已存在的文件() throws {
        let dir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("staged-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let target = dir.appendingPathComponent("ledger.json")
        try Data("旧的".utf8).write(to: target)

        try MultiProcessJSONStore.writeStaged(Data("新的".utf8), to: target)

        XCTAssertEqual(String(data: try Data(contentsOf: target), encoding: .utf8), "新的")
    }

    /// 上面那条用的是注入的落脚点，所以**生产那个落脚点在哪**要单独钉一条 ——
    /// 否则它哪天被改回数据根里面，上面那条照样绿。
    func test_生产的落脚点在数据根之外() {
        let root = PendingCrewDataRoot.url.standardizedFileURL.path
        let staging = PendingCrewDataRoot.stagingDirectory.standardizedFileURL.path
        XCTAssertFalse(staging.hasPrefix(root),
                       "落脚点落在数据根里了，新文件照样带标记：\(staging)")
        // 那个故障拦的是整个 Application Support 底下新出生的文件，不只是数据根。
        XCTAssertFalse(staging.contains("/Application Support/"),
                       "落脚点在 Application Support 底下：\(staging)")
    }
}
