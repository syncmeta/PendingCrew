import XCTest

/// 侧栏「总机长」视图那个刷新按钮的判定（人类 Todo #145）。
///
/// 这里量的是**按下去到底会不会发出去、发的是什么、回执说没说实话**。
/// 判定刻意住在 `ChiefResortRequest` 而不是 `CrewChiefListView` 里，就是为了
/// 这几条能存在 —— 长在 View 里的判定进不了 test bundle，本仓已经为这个
/// 撞过好几次「规则有测试、接线没有」。
final class ChiefResortRequestTests: XCTestCase {

    private let t0 = Date(timeIntervalSince1970: 1_757_600_000)

    // MARK: - 会发出去的那条路

    func test_从没按过_会发出去() {
        let d = ChiefResortRequest.decide(chiefCrewId: "pendingcrew-chief",
                                          lastRequestedAt: nil, now: t0)
        XCTAssertEqual(d, .send(text: ChiefResortRequest.requestText))
    }

    func test_冷却窗过了_再按还会发() {
        let d = ChiefResortRequest.decide(
            chiefCrewId: "pendingcrew-chief",
            lastRequestedAt: t0.addingTimeInterval(-ChiefResortRequest.cooldown - 1),
            now: t0)
        XCTAssertEqual(d, .send(text: ChiefResortRequest.requestText))
    }

    /// 发出去那句话**必须点名工具和参数名**：总机长收到的是一条普通人类消息，
    /// 不点名它得自己猜该怎么调。
    func test_发出去那句话点名了工具名和参数名() {
        for needle in ["arrange_crews", "summaries", "crew_ids", "reason"] {
            XCTAssertTrue(ChiefResortRequest.requestText.contains(needle),
                          "没点名 \(needle)：" + ChiefResortRequest.requestText)
        }
    }

    /// **反过来了**（人类 2026-09-13：「我希望是总机长重新总结和排序」）。
    ///
    /// 上一版这里钉的是「文案里不许出现总结」，理由是 `CrewArrangement` 里没有给每个
    /// 机组各写一句的字段。现在有了（`arrange_crews(summaries:)` → `CrewChiefSummaryStore`，
    /// 带防过期），按钮就必须要它总结 —— 只要排序不要摘要，人按完看到的每一行还是旧话。
    func test_文案要求总结每个机组() {
        XCTAssertTrue(ChiefResortRequest.requestText.contains("总结"),
                      "按钮只请它排序、没请它总结：" + ChiefResortRequest.requestText)
        XCTAssertTrue(ChiefResortRequest.requestText.contains("每个机组"),
                      ChiefResortRequest.requestText)
    }

    /// 发出去那句话要说清是按钮触发的 —— 否则总机长会当成人坐在那儿打的字。
    /// `arrange_crews` 的工具描述也靠这几个字认出「这是刷新请求」。
    func test_发出去那句话说清了是按钮触发的() {
        XCTAssertTrue(ChiefResortRequest.requestText.contains("刷新按钮触发"),
                      ChiefResortRequest.requestText)
    }

    // MARK: - 不发的两种，各自要把理由说出来

    func test_没有总机组时_不发并说清为什么() {
        for id in [String?.none, "", "   "] {
            guard case let .refuse(why) = ChiefResortRequest.decide(
                chiefCrewId: id, lastRequestedAt: nil, now: t0) else {
                return XCTFail("chiefCrewId=\(String(describing: id)) 时竟然要发出去")
            }
            XCTAssertTrue(why.contains("总机组"), why)
        }
    }

    func test_冷却窗内连按_不发并告诉人还要等多久() {
        guard case let .refuse(why) = ChiefResortRequest.decide(
            chiefCrewId: "pendingcrew-chief",
            lastRequestedAt: t0.addingTimeInterval(-10), now: t0) else {
            return XCTFail("冷却窗内竟然又发了一条")
        }
        XCTAssertTrue(why.contains("10 秒前"), why)
        XCTAssertTrue(why.contains("50 秒"), "没告诉人还要等多久：\(why)")
    }

    /// 时钟往回跳时别把负数印给人看（「刚请求过（-3600 秒前）」）。
    func test_上次请求时间在未来_不印负数() {
        guard case let .refuse(why) = ChiefResortRequest.decide(
            chiefCrewId: "pendingcrew-chief",
            lastRequestedAt: t0.addingTimeInterval(3600), now: t0) else {
            return XCTFail("时间倒挂时竟然发了出去")
        }
        XCTAssertFalse(why.contains("-"), "把负数印给人看了：\(why)")
        XCTAssertTrue(why.contains("0 秒前"), why)
    }

    // MARK: - 回执照实说（2026-09-13：按了、群里没有、回执说已请）

    private struct DiskFull: LocalizedError {
        var errorDescription: String? { "磁盘满了" }
    }

    func test_写进去了_回执说已请并开冷却() {
        let r = ChiefResortRequest.receipt(for: .success(nil))
        XCTAssertTrue(r.text.contains("已请总机长重新总结并排序"), r.text)
        XCTAssertFalse(r.text.contains("没发出去"), r.text)
        XCTAssertTrue(r.startsCooldown)
    }

    func test_写进去但白板出过事_回执带上事故原文() {
        let r = ChiefResortRequest.receipt(for: .success("白板文件损坏，已归档"))
        XCTAssertTrue(r.text.contains("白板文件损坏，已归档"), r.text)
        XCTAssertTrue(r.startsCooldown)
    }

    func test_没写进去_回执照实说且不开冷却() {
        let r = ChiefResortRequest.receipt(for: .failure(DiskFull()))
        XCTAssertTrue(r.text.contains("没发出去"), r.text)
        XCTAssertTrue(r.text.contains("磁盘满了"), "没把原因带上：\(r.text)")
        XCTAssertFalse(r.text.contains("已请"), "没写进去却说已请：\(r.text)")
        // 开了冷却，人重按会被自己的冷却窗挡住，而那条消息根本没发出去。
        XCTAssertFalse(r.startsCooldown)
    }

    /// 用**真的**读不出来的白板造出「存进待发件箱」那种失败（原件 chmod 0、目录可写）——
    /// 那个错误类型是白板 store 私有的，手搓一个出来测，测的就只是手搓的那个。
    func test_白板读不出来但存进了待发件箱_说会自动补发且开冷却() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("resort-spool-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let url = dir.appendingPathComponent("pendingcrew-chief.json")
        try Data("[]".utf8).write(to: url)
        XCTAssertEqual(chmod(url.path, 0), 0)
        defer {
            _ = chmod(url.path, S_IRUSR | S_IWUSR)
            try? FileManager.default.removeItem(at: dir)
        }
        let store = LocalWhiteboardStore(directory: dir)

        let result = Result<String?, Error> {
            try store.appendUserMessageReportingFailure(
                crewId: "pendingcrew-chief", text: ChiefResortRequest.requestText, senderName: "人")
        }
        guard case .failure(let error) = result else {
            return XCTFail("原件读不出来竟然写进去了 —— 这条的前提没立住")
        }
        XCTAssertTrue(LocalWhiteboardStore.wasPreservedForRetry(error),
                      "前提没立住：没存进待发件箱（\(error.localizedDescription)）")
        let r = ChiefResortRequest.receipt(for: result)
        XCTAssertTrue(r.text.contains("待发件箱"), r.text)
        XCTAssertFalse(r.text.contains("已请"), r.text)
        // 恢复时会自动补发 —— 再按一次只会补出两条。
        XCTAssertTrue(r.startsCooldown)
    }

    func test_不是待发件箱那种失败_不当成已存下() {
        XCTAssertFalse(LocalWhiteboardStore.wasPreservedForRetry(DiskFull()))
    }

    /// **整条链**：真往一个写不进去的白板里发人类消息 → 抛 → 回执不说「已请」。
    ///
    /// 这条防的正是 2026-09-13 那个吞错点：人类身份那条写入路径要是又退回 `try?`，
    /// 这里拿到的是 `.success(nil)`，回执会说「已请」，当场红。
    func test_人类消息写不进白板时_回执不说已请() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("resort-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let notADirectory = root.appendingPathComponent("whiteboards")
        try Data("占位的文件，不是目录".utf8).write(to: notADirectory)
        let store = LocalWhiteboardStore(directory: notADirectory)

        let result = Result<String?, Error> {
            try store.appendUserMessageReportingFailure(
                crewId: "pendingcrew-chief", text: ChiefResortRequest.requestText, senderName: "人")
        }
        let r = ChiefResortRequest.receipt(for: result)
        XCTAssertFalse(r.text.contains("已请"), "写不进去，回执却说已请：\(r.text)")
    }
}
