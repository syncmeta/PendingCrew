import XCTest

/// 侧栏「总机长」视图那个刷新按钮的判定（人类 Todo #145）。
///
/// 这里量的是**按下去到底会不会发出去、发的是什么**。
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

    /// 发出去那句话**必须点名 `arrange_crews`**：总机长收到的是一条普通人类消息，
    /// 不点名它得自己猜该调哪个工具。
    func test_发出去那句话点名了工具名和reason() {
        XCTAssertTrue(ChiefResortRequest.requestText.contains("arrange_crews"),
                      ChiefResortRequest.requestText)
        XCTAssertTrue(ChiefResortRequest.requestText.contains("reason"),
                      ChiefResortRequest.requestText)
    }

    /// **文案里不许出现「总结」** —— 今天没有「给每个机组各写一句」这种字段，
    /// 承诺了就是让总机长去做一件它做不到的事，然后人以为按钮坏了。
    /// 这条撞红时先看 `CrewArrangement` 是不是真的长出了那个字段；
    /// 长出来了，改的是这条断言；没长出来，改回文案。
    func test_文案不承诺它做不到的总结() {
        XCTAssertFalse(ChiefResortRequest.requestText.contains("总结"),
                       "按钮承诺了每个机组一句摘要，而 CrewArrangement 里没有那个字段："
                       + ChiefResortRequest.requestText)
    }

    /// 发出去那句话要说清是按钮触发的 —— 否则总机长会当成人坐在那儿打的字。
    func test_发出去那句话说清了是按钮触发的() {
        XCTAssertTrue(ChiefResortRequest.requestText.contains("刷新按钮"),
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
}
