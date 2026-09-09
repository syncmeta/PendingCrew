import XCTest

/// (D)：驾驶舱那本账按**谁有资格决定**拆开（人类 Todo #115）。
///
/// 起因是一道 `guard isCaptain` 把四个不同的动作当成了一件事，于是
/// **worker 报不了进度** —— 而群里绝大多数消息是 worker 发的，
/// 人类要的「账能及时更新」在那半边根本不成立。
final class CrewCockpitWritePermissionTests: XCTestCase {

    private func decide(_ c: CrewMessageCategory, captain: Bool) -> CrewCockpitWritePermission.Decision {
        CrewCockpitWritePermission.decide(category: c, isCaptain: captain)
    }

    // MARK: - worker 能做的：报进度、报卡住

    /// **这两条是这一单的正身。** 干活的人才知道进展；卡住了要立刻可见，
    /// 等机长转述就晚了。
    func test_worker报得了进度和卡住() {
        XCTAssertEqual(decide(.progress, captain: false), .allowed)
        XCTAssertEqual(decide(.blocked, captain: false), .allowed)
    }

    // MARK: - worker 不能做的：新增条目、翻完成

    func test_worker不能往板上新增条目_而且要给出路() {
        guard case let .refused(msg) = decide(.plan, captain: false) else {
            return XCTFail("worker 能自己往板上加条目了 —— 几十个 worker 各自加，那块板就废了")
        }
        XCTAssertTrue(msg.contains("progress"), "没给出路①（挂到已有那条上）：\(msg)")
        XCTAssertTrue(msg.contains("finding"), "没给出路③（它其实不构成一件要推进的事）：\(msg)")
    }

    /// **完成是验收判断，不是自我声明。**
    func test_worker不能翻完成_而且要说清为什么() {
        guard case let .refused(msg) = decide(.done, captain: false) else {
            return XCTFail("worker 能给自己销号了 —— 那跟「翻 completed 必须带凭据」自相矛盾")
        }
        XCTAssertTrue(msg.contains("验收"), "没说清这是验收判断不是自我声明：\(msg)")
        XCTAssertTrue(msg.contains("progress"), "没给出路（按进度报上去让机长核）：\(msg)")
    }

    // MARK: - 机长四样都能做

    func test_机长四样都能做() {
        for c in [CrewMessageCategory.plan, .progress, .blocked, .done] {
            XCTAssertEqual(decide(c, captain: true), .allowed, c.rawValue)
        }
    }

    // MARK: - 不碰驾驶舱的分类，这道门一概不管

    func test_不落驾驶舱的分类不受这道门影响() {
        for c in [CrewMessageCategory.humanTodo, .todoResponse, .handoff,
                  .ack, .question, .finding, .note] {
            XCTAssertEqual(decide(c, captain: false), .allowed, c.rawValue)
        }
    }
}

/// 落账前的纯解析（`CrewCockpitLanding`）。
///
/// 这几条看着琐碎，但每一条对着一个**具体的骗人形态**：标题另立一把尺子会跟折叠
/// 分叉、报进度顺手把卡点清掉会让人类看板上的「卡在哪条」凭空消失。
final class CrewCockpitLandingTests: XCTestCase {

    func test_计划号收三种形状() {
        XCTAssertEqual(CrewCockpitLanding.number(7), 7)
        XCTAssertEqual(CrewCockpitLanding.number(7.0), 7)
        XCTAssertEqual(CrewCockpitLanding.number(NSNumber(value: 7)), 7)
        XCTAssertEqual(CrewCockpitLanding.number(" 7 "), 7)
        XCTAssertNil(CrewCockpitLanding.number("七"))
        XCTAssertNil(CrewCockpitLanding.number(nil))
    }

    /// 标题复用折叠那把尺子 —— **不许另立一套截断规则**。
    /// 两把尺子迟早分叉，而分叉之后没有人会发现（同一段文字在气泡上和板上不一样长）。
    func test_标题跟折叠摘要同一把尺子() {
        let long = String(repeating: "阻", count: 120)
        let title = CrewCockpitLanding.title(from: long)
        XCTAssertEqual(title.count, CrewMessageFold.summaryCap + 1, "多出来的那一个是省略号：\(title)")
        XCTAssertTrue(title.hasSuffix("…"))
    }

    func test_标题取第一行且剥掉标记() {
        XCTAssertEqual(CrewCockpitLanding.title(from: "## 把闸门接上\n\n细节在下面"), "把闸门接上")
        XCTAssertEqual(CrewCockpitLanding.title(from: "\n\n  接上闸门  \n第二行"), "接上闸门")
    }

    /// **报进度不等于解了卡。**
    ///
    /// 反面很具体：一条 `blocked` 的计划被人顺手报一句进度，如果这里翻回
    /// `in_progress`，`CockpitPlan.validate` 会跟着把卡点引用清掉 ——
    /// 而人类看板看的正是那个引用（「卡在哪条待我拍板的事上」）。
    /// 那不是「状态不准」，是**一条人在等的事从板上消失了**。
    func test_报进度不许把卡住翻回进行中() {
        XCTAssertNil(CrewCockpitLanding.statusForProgress(current: .blocked))
        XCTAssertNil(CrewCockpitLanding.statusForProgress(current: .done))
        XCTAssertNil(CrewCockpitLanding.statusForProgress(current: .inProgress))
        XCTAssertEqual(CrewCockpitLanding.statusForProgress(current: .notStarted),
                       CockpitPlanStatus.inProgress.rawValue)
    }

    func test_卡点账本只认两本() {
        XCTAssertEqual(CrewCockpitLanding.blockerLedger(nil), .ok("human"))
        XCTAssertEqual(CrewCockpitLanding.blockerLedger("AGENT"), .ok("agent"))
        guard case .refused = CrewCockpitLanding.blockerLedger("cockpit") else {
            return XCTFail("认了第三本账")
        }
    }
}
