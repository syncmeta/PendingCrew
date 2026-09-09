import XCTest

/// `crew_status` 的写侧判定（人类 Todo #136）。
///
/// 这批用例钉的是一条我自己前后说过两遍不同话的口径：
/// **40 是提醒线（超了照写），200 才是硬闸。**
/// 定成这样的理由是「显示宽度不该变成写入端的合法性判据」——
/// 侧栏那行以后变宽了，40 这个数不会跟着变，于是它会开始拒掉本来能显示的内容。
final class CrewStatusIntakeTests: XCTestCase {

    private func text(_ n: Int) -> String { String(repeating: "状", count: n) }

    func test_没填就是没填() {
        XCTAssertEqual(CrewStatusIntake.decide(nil, isCaptain: true), .none)
        XCTAssertEqual(CrewStatusIntake.decide("   ", isCaptain: true), .none,
                       "写了个空格不算填过 —— 那是最廉价的一种假账")
    }

    func test_四十字以内静静收下() {
        XCTAssertEqual(CrewStatusIntake.decide(text(40), isCaptain: true),
                       .accepted(text(40), hint: nil))
    }

    /// **超 40 照写**，只是回执多一句。红这条 = 有人把提醒线做成了硬闸。
    func test_超过四十字照样写进去只是提醒一句() {
        guard case let .accepted(value, hint) =
                CrewStatusIntake.decide(text(41), isCaptain: true) else {
            return XCTFail("41 字被拒了 —— 40 是提醒线不是硬闸")
        }
        XCTAssertEqual(value, text(41), "不许在写入端偷偷砍短")
        XCTAssertNotNil(hint, "超了得说一句，否则人不知道侧栏会截")
    }

    func test_超过两百字拒收() {
        guard case let .refused(why) = CrewStatusIntake.decide(text(201), isCaptain: true) else {
            return XCTFail("201 字该拒")
        }
        XCTAssertTrue(why.contains("没有发出去"),
                      "拒了必须说清整条消息也没发 —— 半截状态最骗人：\(why)")
    }

    func test_两百字整还收() {
        guard case .accepted = CrewStatusIntake.decide(text(200), isCaptain: true) else {
            return XCTFail("闸是「最多 200」，200 本身该过")
        }
    }

    /// worker 填不了 —— 侧栏那行是**整组**的状态，不是某个 worker 手上那件活。
    func test_worker填不了而且要给出路() {
        guard case let .refused(why) = CrewStatusIntake.decide("在跑全量", isCaptain: false) else {
            return XCTFail("worker 不该填得动整组状态")
        }
        XCTAssertTrue(why.contains("机长"), why)
        XCTAssertTrue(why.contains("出路"), "只说不行不给出路，它下次还是会填：\(why)")
    }
}
