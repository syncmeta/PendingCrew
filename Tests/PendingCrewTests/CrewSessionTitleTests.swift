import XCTest
// CrewSessionTitle.swift 直接编进 PendingCrewTests target（见 project.yml），无需 import。

/// `CrewSessionTitle` clamp/derive/resolve —— session 精简标题单一真值的兜底逻辑。
final class CrewSessionTitleTests: XCTestCase {

    // MARK: - clamp

    func testClampKeepsShortTitle() {
        XCTAssertEqual(CrewSessionTitle.clamp("修登录按钮颜色"), "修登录按钮颜色")
    }

    func testClampTruncatesToMaxLen() {
        // 20 个「字」→ 截到 18。
        let long = String(repeating: "字", count: 20)
        XCTAssertEqual(CrewSessionTitle.clamp(long).count, 18)
    }

    func testClampCollapsesToFirstLineAndTrims() {
        XCTAssertEqual(CrewSessionTitle.clamp("  改群聊命名 \n 第二行忽略 "), "改群聊命名")
    }

    // MARK: - derive

    func testDeriveStripsBracketWrapper() {
        // 【…】包裹 → 抬出内文；后续正文接上，clamp 会截断到 ≤18。
        let t = CrewSessionTitle.derive(fromBrief: "【修右键@双@ bug】顺带核对行号")
        XCTAssertFalse(t.contains("【"))
        XCTAssertTrue(t.hasPrefix("修右键@双@ bug"))
        XCTAssertLessThanOrEqual(t.count, 18)
    }

    func testDeriveStripsProjectNamePrefix() {
        XCTAssertEqual(CrewSessionTitle.derive(fromBrief: "PendingCrew：修群聊状态圆点"), "修群聊状态圆点")
        XCTAssertEqual(CrewSessionTitle.derive(fromBrief: "大绿豆 修好友列表排序"), "修好友列表排序")
    }

    func testDeriveTakesFirstLineOnly() {
        XCTAssertEqual(CrewSessionTitle.derive(fromBrief: "加候选过滤\n还有一堆细节说明"), "加候选过滤")
    }

    func testDeriveClampsLongBrief() {
        let t = CrewSessionTitle.derive(fromBrief: String(repeating: "任务", count: 20))
        XCTAssertLessThanOrEqual(t.count, 18)
    }

    // MARK: - resolve

    func testResolvePrefersExplicitTitle() {
        XCTAssertEqual(
            CrewSessionTitle.resolve(explicit: "改群聊命名", brief: "PendingCrew 一大堆 brief 文字"),
            "改群聊命名")
    }

    func testResolveClampsExplicitTitle() {
        let t = CrewSessionTitle.resolve(explicit: String(repeating: "字", count: 30), brief: "x")
        XCTAssertEqual(t.count, 18)
    }

    func testResolveFallsBackToBriefWhenTitleEmpty() {
        XCTAssertEqual(
            CrewSessionTitle.resolve(explicit: "   ", brief: "PendingCrew：修状态圆点"),
            "修状态圆点")
        XCTAssertEqual(
            CrewSessionTitle.resolve(explicit: nil, brief: "修状态圆点"),
            "修状态圆点")
    }
}
