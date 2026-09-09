#if os(macOS)
import XCTest
// Sources compiled directly into the test bundle (see project.yml) — no module import needed.

/// 「只看这一条计划」—— 引用胶囊里那颗「计划 #N」的落点（人类 Todo #132/#133）。
///
/// 没有它，那颗胶囊只能做到「把驾驶舱打开」，剩下的还得人自己在一整页里找 #N。
/// 那正是硬口径里说的装饰品：**它有反应，但没把人送到目的地。**
final class CockpitPlanFocusingTests: XCTestCase {

    private func item(_ n: Int) -> CockpitTaskItem {
        CockpitTaskItem(id: "plan:\(n)", title: "第 \(n) 条", statusRaw: "doing",
                        origin: .captainPlan, updated: nil, badge: "#\(n)")
    }
    private func number(_ i: CockpitTaskItem) -> Int? {
        Int(i.id.dropFirst("plan:".count))
    }
    private var all: [CockpitTaskItem] { [item(1), item(2), item(3)] }

    func test_落在指定那条上() {
        let rows = CockpitPlanFocusing.focused(all, number: 2, numberOf: number)
        XCTAssertEqual(rows.map(\.badge), ["#2"])
        XCTAssertTrue(CockpitPlanFocusing.isFocused(all, focused: rows))
    }

    func test_没给号就是完整列表() {
        let rows = CockpitPlanFocusing.focused(all, number: nil, numberOf: number)
        XCTAssertEqual(rows.count, 3)
        XCTAssertFalse(CockpitPlanFocusing.isFocused(all, focused: rows))
    }

    func test_指到不存在的号回落成完整列表而不是空白() {
        // 计划会被改、会被重排。一颗指着已经没有的 #N 的胶囊如果落进一片空白，
        // 人看到的是一个坏掉的界面。
        let rows = CockpitPlanFocusing.focused(all, number: 99, numberOf: number)
        XCTAssertEqual(rows.count, 3)
        // 回落时不许说「你正看着一条」。
        XCTAssertFalse(CockpitPlanFocusing.isFocused(all, focused: rows))
    }

    func test_一共就一条时不算落在单条上() {
        let one = [item(1)]
        let rows = CockpitPlanFocusing.focused(one, number: 1, numberOf: number)
        XCTAssertEqual(rows.count, 1)
        // 「‹ 全部」在这时出现是句废话 —— 全部就是这一条。
        XCTAssertFalse(CockpitPlanFocusing.isFocused(one, focused: rows))
    }
}
#endif
