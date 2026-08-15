import XCTest

/// session 头像右下角**唯一**那颗状态点的语义（人类定调 2026-08-08：两个点合成一个）。
///
/// 钉死两件事：
/// 1. 五档语义各归其位 —— 干活/空闲/异常/待决策/已退出。
/// 2. **红只在「真需要人出手」时亮**：未读消息不得判红（这正是被删掉的右上角红点
///    的旧行为，人类明确要求不再点亮任何点）。
final class SessionStatusDotTests: XCTestCase {

    // MARK: - 五档语义

    func testWorkingIsGreen() {
        XCTAssertEqual(SessionStatusDotDerivation.dot(state: "working"), .working)
    }

    func testIdleIsYellow() {
        XCTAssertEqual(SessionStatusDotDerivation.dot(state: "idle"), .idle)
    }

    /// 异常（未登录 / key 失效）——进程活着但干不了活，人得出手。
    func testHealthErrorIsAttention() {
        XCTAssertEqual(SessionStatusDotDerivation.dot(state: "error"), .attention)
    }

    /// 卡住等人拍板（ask 待决策 / 终端弹了选择菜单）——不答就永远停在那。
    func testAwaitingDecisionIsAttention() {
        XCTAssertEqual(SessionStatusDotDerivation.dot(state: "awaitingDecision"), .attention)
    }

    /// 在等人回话（`ask` 挂着 / 说完停在一个问句上，Todo #25 层 2）——此前这类跟
    /// 「空闲」长得一模一样，人不进右栏就永远不知道它在等。
    func testAwaitingReplyIsAttention() {
        XCTAssertEqual(SessionStatusDotDerivation.dot(state: "awaitingReply"), .attention)
    }

    func testExitedIsGray() {
        XCTAssertEqual(SessionStatusDotDerivation.dot(state: "exited"), .exited)
    }

    // MARK: - 红的边界

    /// 额度到顶 / 拉起失败同属「需要人出手」。
    func testQuotaAndLaunchFailureAreAttention() {
        XCTAssertEqual(SessionStatusDotDerivation.dot(state: "rateLimited"), .attention)
        XCTAssertEqual(SessionStatusDotDerivation.dot(state: "launchFailed"), .attention)
    }

    /// **只有未读**绝不判红。未读不是状态词，压根不进这条推导 —— 一个空闲但有未读
    /// 的 session 只能是黄。这条守着人类那句「未读消息不再点亮任何点」。
    func testUnreadAloneNeverTurnsRed() {
        // 未读的 session 状态仍是 idle / working，各自照常。
        XCTAssertEqual(SessionStatusDotDerivation.dot(state: "idle"), .idle)
        XCTAssertEqual(SessionStatusDotDerivation.dot(state: "working"), .working)
        // 词表里根本没有任何「未读」相关的词能进红档。
        for state in ["unread", "hasUnread", "notification"] {
            XCTAssertNotEqual(
                SessionStatusDotDerivation.dot(state: state), .attention,
                "未读类状态词 \(state) 不得判成红")
        }
    }

    /// 只有红做呼吸动画 —— 动画是稀缺信号。
    func testOnlyAttentionBreathes() {
        XCTAssertTrue(SessionStatusDot.attention.breathes)
        for dot in SessionStatusDot.allCases where dot != .attention {
            XCTAssertFalse(dot.breathes, "\(dot) 不该做动画")
        }
    }

    // MARK: - 不画点 / 未知词

    func testNoStateDrawsNothing() {
        XCTAssertNil(SessionStatusDotDerivation.dot(state: nil))
        XCTAssertNil(SessionStatusDotDerivation.dot(state: ""))
    }

    /// 未知词保守落灰，绝不误报成绿（"在干活"）或红（"出事了"）。
    func testUnknownStateFallsBackToGray() {
        XCTAssertEqual(SessionStatusDotDerivation.dot(state: "brand_new_state"), .exited)
    }

    /// 远端成员用 server 的词表（running/queued），与本机词表在这里合流。
    func testRemoteMemberVocabulary() {
        XCTAssertEqual(SessionStatusDotDerivation.dot(state: "running"), .working)
        XCTAssertEqual(SessionStatusDotDerivation.dot(state: "queued"), .idle)
    }

    // MARK: - 防词表漂移

    #if os(macOS)
    /// `CrewSessionStateDerivation`（状态事实源）能吐出的**每一个**状态词都必须在这里
    /// 有明确归宿，且不许落进 `default` 的灰兜底 —— 否则新增状态会静默画成「已退出」。
    func testEveryDerivedStateHasAnExplicitDot() {
        let cases: [(String, SessionStatusDot)] = [
            (CrewSessionStateDerivation.state(
                isRunning: true, health: nil, isWorking: true), .working),
            (CrewSessionStateDerivation.state(
                isRunning: true, health: nil, isWorking: false), .idle),
            (CrewSessionStateDerivation.state(
                isRunning: false, health: nil, isWorking: false), .exited),
            (CrewSessionStateDerivation.state(
                isRunning: true, health: CrewSessionHealth(kind: .authRequired, detail: ""),
                isWorking: false), .attention),
            (CrewSessionStateDerivation.state(
                isRunning: true, health: CrewSessionHealth(kind: .usageLimit, detail: ""),
                isWorking: false), .attention),
            (CrewSessionStateDerivation.state(
                isRunning: true, health: CrewSessionHealth(kind: .rateLimited, detail: ""),
                isWorking: false), .attention),
            (CrewSessionStateDerivation.state(
                isRunning: true, health: CrewSessionHealth(kind: .launchFailed, detail: ""),
                isWorking: false), .attention),
            (CrewSessionStateDerivation.state(
                isRunning: true, health: nil, isWorking: false, awaitingDecision: true),
             .attention),
            (CrewSessionStateDerivation.state(
                isRunning: true, health: nil, isWorking: false, awaitingReply: true),
             .attention),
        ]
        for (state, expected) in cases {
            XCTAssertEqual(
                SessionStatusDotDerivation.dot(state: state), expected,
                "状态词 \(state) 的点色不对")
        }
        // 覆盖到每个 health kind，新增 kind 时这条会提醒补映射。
        XCTAssertEqual(CrewSessionHealth.Kind.allCases.count, 4)
    }
    #endif
}
