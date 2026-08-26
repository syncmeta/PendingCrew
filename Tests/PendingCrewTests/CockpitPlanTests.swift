import XCTest

/// 机长作战板（人类 Todo #66）—— 纯逻辑 + 存储。
///
/// 这本账的两条硬约束都在这儿钉住：**「卡住」必须指明卡在哪条人类 Todo**，
/// 以及**悬空引用要显式说出来、不许静默降级**。
final class CockpitPlanTests: XCTestCase {

    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory.appendingPathComponent("plan-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private let humanRef = CockpitPlanBlocker(ledger: "human", number: 7)

    // MARK: - 守卫：blocked ⇔ 有引用

    func testBlockedWithoutBlockerIsRefused() {
        let r = CockpitPlan.validate(next: .blocked, incomingBlocker: nil, existingBlocker: nil)
        guard case let .failure(refusal) = r else { return XCTFail("应当拒绝") }
        XCTAssertEqual(refusal, .blockedWithoutBlocker)
        XCTAssertTrue(refusal.summary.contains("卡在人身上"))
    }

    func testBlockedKeepsExistingBlockerWhenNoneGiven() {
        // 已经卡着了，这次只追加一句进度 —— 不该逼机长把 #N 再写一遍。
        let r = CockpitPlan.validate(next: .blocked, incomingBlocker: nil, existingBlocker: humanRef)
        guard case let .success(b) = r else { return XCTFail("应当放行") }
        XCTAssertEqual(b, humanRef)
    }

    func testLeavingBlockedClearsBlocker() {
        // 卡点解了还挂着「卡在 #7」，下一个人只能靠猜。
        for next in [CockpitPlanStatus.inProgress, .done, .notStarted] {
            guard case let .success(b) = CockpitPlan.validate(
                next: next, incomingBlocker: nil, existingBlocker: humanRef)
            else { return XCTFail("应当放行") }
            XCTAssertNil(b, "\(next.rawValue) 不该还挂着卡点")
        }
    }

    func testUnknownStatusIsRefused() {
        let r = CockpitPlan.validate(nextRaw: "pending", incomingBlocker: nil, existingBlocker: nil)
        guard case let .failure(refusal) = r else { return XCTFail("应当拒绝") }
        // 故意不认 Todo 的三档 —— 两本账的状态词不是一回事，串了迟早出事。
        XCTAssertEqual(refusal, .unknownStatus("pending"))
    }

    func testNoStatusChangeKeepsBlockerAndAcceptsNewOne() {
        guard case let .success(kept) = CockpitPlan.validate(
            next: nil, incomingBlocker: nil, existingBlocker: humanRef) else { return XCTFail() }
        XCTAssertEqual(kept, humanRef)
        let moved = CockpitPlanBlocker(ledger: "agent", number: 3)
        guard case let .success(updated) = CockpitPlan.validate(
            next: nil, incomingBlocker: moved, existingBlocker: humanRef) else { return XCTFail() }
        XCTAssertEqual(updated, moved)
    }

    // MARK: - 引用：带账本、不静默降级

    func testBlockerLabelDistinguishesLedgers() {
        // 两本账各自从 #1 起，裸 #N 有歧义 —— 标签必须把「哪本」说出来。
        XCTAssertEqual(CockpitPlanBlocker(ledger: "human", number: 1).label, "人类 Todo #1")
        XCTAssertEqual(CockpitPlanBlocker(ledger: "agent", number: 1).label, "Todo #1")
    }

    func testBlockerStateUnverifiedWhenCallerCannotCheckHumanLedger() {
        // 调用点接不上那本账时**如实承认没查**，绝不默认「在」。
        // （Todo #62 已合 main，两个生产调用点都接上了；这条守的是「接不上时说实话」
        // 这个语义本身，不是某一条线的进度。）
        let state = CockpitPlan.blockerState(humanRef, agentTodoExists: { _ in true }, humanTodoExists: nil)
        guard case let .unverified(reason) = state else { return XCTFail("应当是未核实") }
        XCTAssertTrue(reason.contains("查不了"))
        // 反向钉死：这句不许再提某条 Todo 的进度 —— 接完之后那种措辞就成了假话。
        XCTAssertFalse(reason.contains("#62"))
    }

    func testBlockerStateForHumanLedgerOnceWired() {
        // 接上之后：查得到 → present；人类把它删了（软删，`list` 不返回）→ missing。
        // **不许因为查不到就退回 present 或偷偷改状态。**
        XCTAssertEqual(
            CockpitPlan.blockerState(humanRef, agentTodoExists: { _ in false },
                                     humanTodoExists: { $0 == 7 }), .present)
        XCTAssertEqual(
            CockpitPlan.blockerState(humanRef, agentTodoExists: { _ in true },
                                     humanTodoExists: { _ in false }), .missing)
    }

    func testTwoLedgersAreLookedUpSeparately() {
        // 两本各自从 #1 起：同一个 #1 必须查各自那本，串本就是静默读错账。
        let human1 = CockpitPlanBlocker(ledger: "human", number: 1)
        let agent1 = CockpitPlanBlocker(ledger: "agent", number: 1)
        // 只有 agent 那本有 #1
        XCTAssertEqual(CockpitPlan.blockerState(agent1, agentTodoExists: { $0 == 1 },
                                                humanTodoExists: { _ in false }), .present)
        XCTAssertEqual(CockpitPlan.blockerState(human1, agentTodoExists: { $0 == 1 },
                                                humanTodoExists: { _ in false }), .missing)
    }

    func testBlockerStateForAgentLedger() {
        let ref = CockpitPlanBlocker(ledger: "agent", number: 3)
        XCTAssertEqual(CockpitPlan.blockerState(ref, agentTodoExists: { $0 == 3 }, humanTodoExists: nil), .present)
        XCTAssertEqual(CockpitPlan.blockerState(ref, agentTodoExists: { _ in false }, humanTodoExists: nil), .missing)
    }

    func testMissingBlockerIsSaidOutLoud() {
        // 人类把 #7 删了：既不偷偷把状态改回「进行中」，也不显示一个点不开的 #7。
        let line = CockpitPlan.blockerLine(humanRef, state: .missing)
        XCTAssertTrue(line.contains("人类 Todo #7"))
        XCTAssertTrue(line.contains("找不到了"))
        XCTAssertTrue(line.contains("重新指认"))
    }

    func testUnverifiedBlockerSaysSo() {
        let line = CockpitPlan.blockerLine(humanRef, state: .unverified(reason: "还没接线"))
        XCTAssertTrue(line.contains("未核实"))
    }

    // MARK: - 照妖镜：多久没碰过

    func testLastUpdatedLabelBuckets() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        XCTAssertEqual(CockpitPlan.lastUpdatedLabel(now.addingTimeInterval(-30), now: now), "最后更新 刚刚")
        XCTAssertEqual(CockpitPlan.lastUpdatedLabel(now.addingTimeInterval(-600), now: now), "最后更新 10 分钟前")
        XCTAssertEqual(CockpitPlan.lastUpdatedLabel(now.addingTimeInterval(-7200), now: now), "最后更新 2 小时前")
        XCTAssertEqual(CockpitPlan.lastUpdatedLabel(now.addingTimeInterval(-6 * 86400), now: now), "最后更新 6 天前")
        XCTAssertEqual(CockpitPlan.lastUpdatedLabel(nil, now: now), "")
    }

    func testStatusLineReadsLikeTheDesignAskedFor() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        XCTAssertEqual(
            CockpitPlan.statusLine(statusRaw: "in_progress",
                                   updated: now.addingTimeInterval(-3 * 86400), now: now),
            "进行中 · 最后更新 3 天前")
        // 认不出来的档不连坐：按「没做」显示，但文件里的原值不动（见 store 那侧）。
        XCTAssertEqual(CockpitPlan.statusLine(statusRaw: "???", updated: nil, now: now), "没做")
    }

    // MARK: - 归段（进 glance）

    func testBandMapping() {
        XCTAssertEqual(CockpitTaskLedger.band("not_started"), .next)
        XCTAssertEqual(CockpitTaskLedger.band("in_progress"), .doing)
        // 卡住是「开了工却推不动」，不是「还没开始」。
        XCTAssertEqual(CockpitTaskLedger.band("blocked"), .doing)
        XCTAssertEqual(CockpitTaskLedger.band("done"), .done)
    }

    // MARK: - 存储

    func testAddAssignsIncreasingNumbersAndStartsNotStarted() throws {
        let s = CockpitPlanStore(directory: tempDir())
        let a = try XCTUnwrap(s.add(crewId: "c", title: "把 #62 合完", bySessionId: "cap", byName: "机长"))
        let b = try XCTUnwrap(s.add(crewId: "c", title: "起 B 段"))
        XCTAssertEqual([a.number, b.number], [1, 2])
        XCTAssertEqual(a.status, CockpitPlanStatus.notStarted.rawValue)
        XCTAssertEqual(a.createdByName, "机长")
        XCTAssertTrue(a.updates.isEmpty)
    }

    func testAddRejectsBlankTitle() {
        let s = CockpitPlanStore(directory: tempDir())
        XCTAssertNil(s.add(crewId: "c", title: "   "))
    }

    func testNumbersAreScopedPerCrew() {
        let s = CockpitPlanStore(directory: tempDir())
        XCTAssertEqual(s.add(crewId: "c1", title: "a")?.number, 1)
        XCTAssertEqual(s.add(crewId: "c2", title: "b")?.number, 1)
        XCTAssertEqual(s.add(crewId: "c1", title: "c")?.number, 2)
    }

    func testProgressIsAppendedNeverOverwritten() throws {
        let s = CockpitPlanStore(directory: tempDir())
        _ = s.add(crewId: "c", title: "接线 blockedBy")
        guard case .success = s.update(crewId: "c", number: 1, progress: "读完了 a473363",
                                       statusRaw: "in_progress") else { return XCTFail() }
        guard case let .success(item) = s.update(crewId: "c", number: 1, progress: "单测先喂假数据")
        else { return XCTFail() }
        XCTAssertEqual(item.updates.map(\.text), ["读完了 a473363", "单测先喂假数据"])
        XCTAssertEqual(item.status, "in_progress", "只写进度不该动状态")
        XCTAssertEqual(item.updates[0].status, "in_progress")
        XCTAssertNil(item.updates[1].status)
    }

    func testStoreRefusesBlockedWithoutBlockerAndWritesNothing() throws {
        let s = CockpitPlanStore(directory: tempDir())
        _ = s.add(crewId: "c", title: "等人拍板")
        guard case let .failure(f) = s.update(crewId: "c", number: 1, statusRaw: "blocked")
        else { return XCTFail("应当拒绝") }
        XCTAssertEqual(f, .refused(.blockedWithoutBlocker))
        let after = try XCTUnwrap(s.item(crewId: "c", number: 1))
        XCTAssertEqual(after.status, CockpitPlanStatus.notStarted.rawValue, "拒绝时一个字都不该落盘")
        XCTAssertTrue(after.updates.isEmpty)
    }

    func testBlockedRoundTripAndClearOnLeave() throws {
        let s = CockpitPlanStore(directory: tempDir())
        _ = s.add(crewId: "c", title: "等人拍板")
        guard case let .success(blocked) = s.update(
            crewId: "c", number: 1, progress: "问了，等回话",
            statusRaw: "blocked", blocker: humanRef) else { return XCTFail() }
        XCTAssertEqual(blocked.blockedBy, humanRef)
        guard case let .success(freed) = s.update(crewId: "c", number: 1, statusRaw: "done")
        else { return XCTFail() }
        XCTAssertNil(freed.blockedBy)
        XCTAssertEqual(freed.status, "done")
    }

    func testUpdateWithNothingIsRefused() {
        let s = CockpitPlanStore(directory: tempDir())
        _ = s.add(crewId: "c", title: "x")
        XCTAssertEqual(s.update(crewId: "c", number: 1), .failure(.nothingToDo))
    }

    func testUpdateUnknownNumberIsRefused() {
        let s = CockpitPlanStore(directory: tempDir())
        XCTAssertEqual(s.update(crewId: "c", number: 9, progress: "..."), .failure(.notFound))
    }

    func testUpdatedAtMovesOnRealChange() throws {
        let s = CockpitPlanStore(directory: tempDir())
        let created = try XCTUnwrap(s.add(crewId: "c", title: "x"))
        guard case let .success(touched) = s.update(crewId: "c", number: 1, progress: "动了一下")
        else { return XCTFail() }
        let iso = ISO8601DateFormatter()
        let t0 = try XCTUnwrap(iso.date(from: created.updatedAt))
        let t1 = try XCTUnwrap(iso.date(from: touched.updatedAt))
        XCTAssertGreaterThanOrEqual(t1, t0)
    }

    func testDropIsSoftAndKeepsNumberOutOfCirculation() throws {
        let s = CockpitPlanStore(directory: tempDir())
        _ = s.add(crewId: "c", title: "排错了")
        XCTAssertTrue(s.drop(crewId: "c", number: 1))
        XCTAssertNil(s.item(crewId: "c", number: 1))
        // 号码不复用：下一条是 #2，不是把 #1 发第二遍。
        XCTAssertEqual(s.add(crewId: "c", title: "重排")?.number, 2)
        XCTAssertFalse(s.drop(crewId: "c", number: 1), "已撤下的不该再撤一次")
    }

    func testUnknownStatusInFileIsKeptNotRewritten() throws {
        // 逐条 lenient 的姿态：认不出来的档按「没做」渲染，但**原值留在文件里**。
        let dir = tempDir()
        let s = CockpitPlanStore(directory: dir)
        _ = s.add(crewId: "c", title: "x")
        let file = dir.appendingPathComponent("c.plan.json")
        var raw = try String(contentsOf: file, encoding: .utf8)
        raw = raw.replacingOccurrences(of: "not_started", with: "somethingelse")
        try raw.write(to: file, atomically: true, encoding: .utf8)
        let reread = try XCTUnwrap(CockpitPlanStore(directory: dir).item(crewId: "c", number: 1))
        XCTAssertEqual(reread.status, "somethingelse")
        XCTAssertNil(CockpitPlan.status(reread.status))
    }

    func testPlanLedgerIsSeparateFromTodoLedger() throws {
        // 第六本账就是第六本账：同一个 crewId 下两本各写各的文件，互不串号。
        let dir = tempDir()
        let plans = CockpitPlanStore(directory: dir)
        let todos = LocalTodoStore(directory: dir)
        _ = plans.add(crewId: "c", title: "机长排的")
        _ = todos.add(crewId: "c", text: "人类派的")
        XCTAssertEqual(plans.list(crewId: "c").map(\.title), ["机长排的"])
        XCTAssertEqual(todos.list(crewId: "c").map(\.text), ["人类派的"])
        XCTAssertTrue(FileManager.default.fileExists(atPath: dir.appendingPathComponent("c.plan.json").path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: dir.appendingPathComponent("c.todos.json").path))
    }
}
