import XCTest

/// 督办租约的纯判定层（人类 Todo #107：**别人停了我不知道**）。
///
/// 现场：后台重启带走了三条子 crew 的机长，半小时没人管，靠人工核账才发现 ——
/// session 只有被 @ 才会醒。人类原话：「如果一个会话把事情交给了某个人，要做一个
/// 超时，超时还没动静要自己唤醒去看看怎么回事，要定期监督，直到不需要再醒了、
/// 做完了，再把这个唤醒解除。保持监督和警觉是常态，而休息下来不是常态。」
///
/// 这一族测的是那条**分寸**落成代码之后的每一环：解除条件只有一个、退避而不是
/// 连环叫、同一笔委托只有一个在途唤醒。
final class SupervisionLeaseTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_780_000_000)

    private func decide(_ status: CockpitPlanStatus?, sinceHours: Double = 3) -> SupervisionLease.Decision {
        SupervisionLease.decide(
            planNumber: 12, planTitle: "让 codex 修建子 crew 丢 brief",
            planStatusRaw: status?.rawValue,
            leaseSince: now.addingTimeInterval(-sinceHours * 3600), now: now)
    }

    // MARK: - 解除条件（这整件事的命门）

    /// 唯一能让督办消音的路径：把那条计划翻到 done 或 blocked。
    func testOnlyDoneOrBlockedDischargesTheLease() {
        XCTAssertEqual(decide(.done), .discharge(.resolved(.done)))
        XCTAssertEqual(decide(.blocked), .discharge(.resolved(.blocked)))
    }

    /// 「进行中」和「没做」都**不是结果** —— 一条排了三天还写着「没做」的活，
    /// 正是最该被叫醒过问的那种。
    func testInProgressAndNotStartedStillRemind() {
        guard case .remind = decide(.inProgress) else { return XCTFail("进行中不是结果") }
        guard case .remind = decide(.notStarted) else { return XCTFail("没做更不是结果") }
    }

    /// 计划从板上撤下（软删）/ 根本查不到 → 没有可督办的对象了，解除。
    /// 这不是后门：撤下会把这条从板面上整个拿掉，不是「看一眼就算办完」。
    func testPlanGoneFromBoardDischarges() {
        XCTAssertEqual(decide(nil), .discharge(.goneFromBoard))
    }

    // MARK: - 到期说什么

    func testRemindTextNamesThePlanTheElapsedTimeAndTheOnlyWayOut() {
        guard case let .remind(text) = decide(.inProgress, sinceHours: 3) else {
            return XCTFail("该提醒")
        }
        XCTAssertTrue(text.contains("#12"), text)
        XCTAssertTrue(text.contains("让 codex 修建子 crew 丢 brief"), text)
        XCTAssertTrue(text.contains("3 小时"), "要说清多久没有结果：\(text)")
        XCTAssertTrue(text.contains("done"), text)
        XCTAssertTrue(text.contains("blocked"), text)
        XCTAssertTrue(text.contains("plan_update"), "要说清从哪儿翻：\(text)")
    }

    /// 措辞不许退化成「去看一眼」—— 那正是人类点名要避免的语义（看一眼就算办完）。
    ///
    /// ⚠️ 这条尺子只量**有没有给出口**，不量有没有出现那几个词：文案里恰恰要
    /// 明写「这里没有『我知道了』这种动作」，按词禁会把最该说的那句话也禁掉。
    /// （第一版就是按词禁的，当场被自己的正确文案判红。）
    func testRemindTextNeverOffersAnAcknowledgeShortcut() {
        guard case let .remind(text) = decide(.inProgress) else { return XCTFail("该提醒") }
        for shortcut in ["去看一眼就行", "稍后提醒", "可以顺延", "标记已读"] {
            XCTAssertFalse(text.contains(shortcut), "督办不给「看一眼就算办完」的出口：\(text)")
        }
        XCTAssertTrue(text.contains("没有「我知道了」这种动作"),
                      "而且要主动把这条出口不存在说出来：\(text)")
    }

    /// 挂上督办的时刻丢了（老数据 / 时间戳解不开）→ 仍然要提醒，只是不谎报时长。
    func testMissingLeaseSinceStillRemindsWithoutFabricatingADuration() {
        guard case let .remind(text) = SupervisionLease.decide(
            planNumber: 7, planTitle: "某条活", planStatusRaw: CockpitPlanStatus.inProgress.rawValue,
            leaseSince: nil, now: now) else { return XCTFail("该提醒") }
        XCTAssertTrue(text.contains("#7"))
        XCTAssertFalse(text.contains("已 0"), "算不出来就别编一个时长：\(text)")
    }

    // MARK: - 退避（不是固定间隔连环叫）

    func testBackoffDoublesEachTimeAReminderActuallyLands() {
        let base: TimeInterval = 40 * 60
        XCTAssertEqual(SupervisionLease.reschedule(baseSeconds: base, step: 0, delivered: true),
                       .init(after: base * 2, step: 1))
        XCTAssertEqual(SupervisionLease.reschedule(baseSeconds: base, step: 1, delivered: true),
                       .init(after: base * 4, step: 2))
        XCTAssertEqual(SupervisionLease.reschedule(baseSeconds: base, step: 2, delivered: true),
                       .init(after: base * 8, step: 3))
    }

    func testBackoffIsCappedSoAnUnresolvedLeaseNeverGoesFullySilent() {
        let base: TimeInterval = 40 * 60
        XCTAssertEqual(SupervisionLease.reschedule(baseSeconds: base, step: 9, delivered: true),
                       .init(after: base * SupervisionLease.maxBackoffMultiplier, step: 10))
    }

    /// 没送到（持有人不在跑、这一刻没人可叫）**不算叫过一次** —— 档位不推进、
    /// 间隔不翻倍。否则一次重启就能把督办退避到几小时以后，正好复刻 #107。
    func testUndeliveredReminderDoesNotAdvanceTheBackoff() {
        let base: TimeInterval = 40 * 60
        XCTAssertEqual(SupervisionLease.reschedule(baseSeconds: base, step: 2, delivered: false),
                       .init(after: base * 4, step: 2))
    }

    // MARK: - 同一笔委托最多一个在途唤醒

    /// 靠**确定性 id** 保证，不靠调用方自觉：同一条计划算出来的 id 恒等，
    /// 而 `LocalWakeupStore.register` 同 id 是 no-op。
    func testLeaseIdIsDeterministicPerPlan() {
        XCTAssertEqual(SupervisionLease.id(crewId: "c", planNumber: 12),
                       SupervisionLease.id(crewId: "c", planNumber: 12))
        XCTAssertNotEqual(SupervisionLease.id(crewId: "c", planNumber: 12),
                          SupervisionLease.id(crewId: "c", planNumber: 13))
        XCTAssertNotEqual(SupervisionLease.id(crewId: "c", planNumber: 12),
                          SupervisionLease.id(crewId: "d", planNumber: 12))
    }

    func testRegisteringTheSameLeaseTwiceIsANoop() {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("lease-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let store = LocalWakeupStore(directory: dir)
        let w = LocalWakeupStore.PendingWakeup(
            id: SupervisionLease.id(crewId: "c", planNumber: 1), crewId: "c", sessionId: "cap",
            fireAt: "2026-09-07T10:00:00Z", note: "-", planNumber: 1,
            leaseSince: "2026-09-07T09:00:00Z", leaseBaseSeconds: 2400, leaseStep: 0)
        XCTAssertTrue(store.register(w))
        XCTAssertFalse(store.register(w), "同一笔委托同一时刻最多一个在途唤醒")
        XCTAssertEqual(store.list().count, 1)
    }

    /// 退避重排换的是 `fireAt` / `leaseStep`，**id 与挂单时刻不动** —— 时长得从
    /// 最初交出去那一刻算，不是从上一次响铃算。
    func testNextKeepsTheSameIdAndOriginalLeaseSince() {
        let w = LocalWakeupStore.PendingWakeup(
            id: SupervisionLease.id(crewId: "c", planNumber: 1), crewId: "c", sessionId: "cap",
            fireAt: "2026-09-07T10:00:00Z", note: "-", planNumber: 1,
            leaseSince: "2026-09-07T09:00:00Z", leaseBaseSeconds: 2400, leaseStep: 0)
        let next = SupervisionLease.next(w, delivered: true, now: now)
        XCTAssertEqual(next.id, w.id)
        XCTAssertEqual(next.leaseSince, w.leaseSince)
        XCTAssertEqual(next.leaseStep, 1)
        XCTAssertEqual(McpServer.parseISO(next.fireAt)?.timeIntervalSince(now) ?? 0,
                       2400 * 2, accuracy: 1)
    }

    // MARK: - 参数卫生

    /// 下限存在的理由：比这更短的督办就是空转轮询，与世界观里「别空转轮询」直接
    /// 打架 —— 那条并没有被推翻，被推翻的只是「没有委托时也该安静」这半。
    func testTooShortAnIntervalIsRefusedAsBusyPolling() {
        guard case let .refused(why) = SupervisionLease.parseMinutes(1) else {
            return XCTFail("1 分钟的督办就是空转轮询，该拒")
        }
        XCTAssertTrue(why.contains("\(Int(SupervisionLease.minMinutes))"), why)
        XCTAssertEqual(SupervisionLease.parseMinutes(4.9), .refused(why))
        XCTAssertEqual(SupervisionLease.parseMinutes(5), .minutes(5))
    }

    func testAbsurdlyLongIntervalIsRefused() {
        guard case .refused = SupervisionLease.parseMinutes(1441) else {
            return XCTFail("超过一天的督办该拒")
        }
        XCTAssertEqual(SupervisionLease.parseMinutes(1440), .minutes(1440))
    }

    /// 没给这个参数 = 不挂督办（**worker 默认不挂**，机长也只在自己想挂时挂）。
    /// 注意它与「给了但不合法」是**两种**结论，压成一个 nil 就会把拒绝说成没要求。
    func testOmittingTheParameterMeansNoLeaseAndIsNotTheSameAsRefusal() {
        XCTAssertEqual(SupervisionLease.parseMinutes(nil), SupervisionLease.ParsedMinutes.none)
        XCTAssertNotEqual(SupervisionLease.parseMinutes(nil), SupervisionLease.parseMinutes(1))
    }

    /// JSON 数字经 `JSONSerialization` 可能是 Int 也可能是 Double，两种都得认。
    func testAcceptsBothIntegerAndDoubleShapes() {
        XCTAssertEqual(SupervisionLease.parseMinutes(Int(40)), .minutes(40))
        XCTAssertEqual(SupervisionLease.parseMinutes(Double(40)), .minutes(40))
        guard case .refused = SupervisionLease.parseMinutes("四十分钟") else {
            return XCTFail("认不出来的形状该拒，不该静默当没给")
        }
    }
}
