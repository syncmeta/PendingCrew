import XCTest

/// crew 状态点聚合优先级（crew-sidebar-status spec §3）：红 > 黄 > 绿 > 灰；
/// 无 session 记录且无 attention → nil（不画）。
final class CrewStatusAggregationTests: XCTestCase {
    private func signal(alive: Bool = true, working: Bool = false, health: Bool = false,
                        awaiting: Bool = false) -> CrewSessionStatusSignal {
        CrewSessionStatusSignal(isAlive: alive, isWorking: working, hasHealthIssue: health,
                                isAwaitingReply: awaiting)
    }

    // MARK: - 各色的基础条件

    func testNoSessionsNoAttentionIsNil() {
        XCTAssertNil(CrewStatusAggregation.dot(sessions: [], attentionReason: nil))
    }

    func testAllIdleOrExitedIsGray() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(
                sessions: [signal(alive: true), signal(alive: false)],
                attentionReason: nil),
            .gray)
    }

    func testAnyWorkingIsGreen() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(
                sessions: [signal(), signal(working: true)],
                attentionReason: nil),
            .green)
    }

    func testAttentionIsYellowEvenWithoutSessions() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(sessions: [], attentionReason: "需要人类拍板"),
            .yellow)
    }

    func testHealthIssueOnAliveSessionIsRed() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(
                sessions: [signal(alive: true, health: true)],
                attentionReason: nil),
            .red)
    }

    // 已退出 session 的残留 health 不算红（红要求进程存活）——全退 → 灰。
    func testHealthIssueOnExitedSessionIsNotRed() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(
                sessions: [signal(alive: false, health: true)],
                attentionReason: nil),
            .gray)
    }

    /// 群里有人卡着等回复 → crew 这一级也要红（Todo #25 层 2）。侧栏折起来时人只看得到
    /// 这一级，冒不上来就得逐个展开才发现有人在等。
    func testAwaitingReplyOnAliveSessionIsRed() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(
                sessions: [signal(), signal(awaiting: true)],
                attentionReason: nil),
            .red)
    }

    /// 已退出 session 的残留待回复不算红（同 health 那条）——它不再需要人过来。
    func testAwaitingReplyOnExitedSessionIsNotRed() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(
                sessions: [signal(alive: false, awaiting: true)],
                attentionReason: nil),
            .gray)
    }

    /// 有人在等回复时，别被「另一个 session 在干活」的绿盖过去 —— 红是最高优先级。
    func testAwaitingReplyBeatsWorkingGreen() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(
                sessions: [signal(working: true), signal(awaiting: true)],
                attentionReason: "机长点的黄"),
            .red)
    }

    // MARK: - 优先级 红 > 黄 > 绿 > 灰

    func testRedBeatsYellowGreenGray() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(
                sessions: [signal(working: true), signal(alive: true, health: true), signal(alive: false)],
                attentionReason: "也有 attention"),
            .red)
    }

    func testYellowBeatsGreenAndGray() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(
                sessions: [signal(working: true), signal()],
                attentionReason: "要人看"),
            .yellow)
    }

    func testGreenBeatsGray() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(
                sessions: [signal(alive: false), signal(working: true)],
                attentionReason: nil),
            .green)
    }

    // 空串 attention 不算点亮（防御：正常路径 raise 已拒空）。
    func testEmptyAttentionReasonNotYellow() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(sessions: [signal()], attentionReason: ""),
            .gray)
        XCTAssertNil(CrewStatusAggregation.dot(sessions: [], attentionReason: ""))
    }

    // session 级头像右上红点（旧 `sessionAttention`）已随「两点合一」删除，
    // 相应用例迁到 `SessionStatusDotTests`（右下角那颗唯一状态点的语义）。
}
