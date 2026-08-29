import XCTest

/// crew 状态点聚合优先级（Todo #71）：红（运行错误）> 黄（有给人类的 Todo）>
/// 绿（正在工作）> nil（静止/退出/无 session，均不画）。
final class CrewStatusAggregationTests: XCTestCase {
    private func signal(alive: Bool = true, working: Bool = false, health: Bool = false,
                        awaiting: Bool = false) -> CrewSessionStatusSignal {
        CrewSessionStatusSignal(isAlive: alive, isWorking: working, hasHealthIssue: health,
                                isAwaitingReply: awaiting)
    }

    // MARK: - 各色的基础条件

    func testNoSessionsIsNil() {
        XCTAssertNil(CrewStatusAggregation.dot(sessions: [], attentionReason: nil))
    }

    func testAllIdleOrExitedHasNoIndicator() {
        XCTAssertNil(CrewStatusAggregation.dot(
            sessions: [signal(alive: true), signal(alive: false)],
            attentionReason: nil))
    }

    func testAnyWorkingIsGreen() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(
                sessions: [signal(), signal(working: true)],
                attentionReason: nil),
            .green)
    }

    func testAttentionWithoutHumanTodoDoesNotLightIndicator() {
        XCTAssertNil(CrewStatusAggregation.dot(
            sessions: [], attentionReason: "需要人类拍板"))
    }

    func testHealthIssueOnAliveSessionIsRed() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(
                sessions: [signal(alive: true, health: true)],
                attentionReason: nil),
            .red)
    }

    // 拉起失败时进程已经退出，但它仍是错误，不能因为不 alive 就静默。
    func testHealthIssueOnExitedSessionIsStillRed() {
        XCTAssertEqual(CrewStatusAggregation.dot(
            sessions: [signal(alive: false, health: true)],
            attentionReason: nil), .red)
    }

    /// 等回复不是运行错误；需要人类处理的事应落人类 Todo，再由黄点表达。
    func testAwaitingReplyAloneHasNoIndicator() {
        XCTAssertNil(CrewStatusAggregation.dot(
            sessions: [signal(), signal(awaiting: true)],
            attentionReason: nil))
    }

    /// 已退出 session 的残留待回复同样不画。
    func testAwaitingReplyOnExitedSessionIsNotRed() {
        XCTAssertNil(CrewStatusAggregation.dot(
            sessions: [signal(alive: false, awaiting: true)],
            attentionReason: nil))
    }

    /// 等回复本身不染红；同 crew 另有 session 工作时照常显示绿。
    func testWorkingGreenIsNotOverriddenByAwaitingReplyOrAttention() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(
                sessions: [signal(working: true), signal(awaiting: true)],
                attentionReason: "机长点的黄"),
            .green)
    }

    // MARK: - 优先级 红 > 黄 > 绿 > nil

    func testRedBeatsTodoAndWorking() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(
                sessions: [signal(working: true), signal(alive: true, health: true), signal(alive: false)],
                attentionReason: "也有 attention",
                humanTodoUnanswered: 2),
            .red)
    }

    func testTodoYellowBeatsWorking() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(
                sessions: [signal(working: true), signal()],
                attentionReason: nil,
                humanTodoUnanswered: 1),
            .yellow)
    }

    func testGreenBeatsInactiveSessions() {
        XCTAssertEqual(
            CrewStatusAggregation.dot(
                sessions: [signal(alive: false), signal(working: true)],
                attentionReason: nil),
            .green)
    }

    // attentionReason 不再参与颜色；空串/非空都不能让静止 crew 亮点。
    func testAttentionReasonDoesNotControlIndicator() {
        XCTAssertNil(CrewStatusAggregation.dot(sessions: [signal()], attentionReason: "提醒"))
        XCTAssertNil(CrewStatusAggregation.dot(sessions: [], attentionReason: ""))
    }

    func testOnlyTodoYellowBreathes() {
        XCTAssertTrue(CrewStatusDotColor.yellow.breathes)
        XCTAssertFalse(CrewStatusDotColor.red.breathes)
        XCTAssertFalse(CrewStatusDotColor.green.breathes)
    }

    // session 级头像右上红点（旧 `sessionAttention`）已随「两点合一」删除，
    // 相应用例迁到 `SessionStatusDotTests`（右下角那颗唯一状态点的语义）。
}
