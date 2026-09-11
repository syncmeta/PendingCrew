#if os(macOS)
import Foundation
import XCTest

/// 「前端更新了，本机后端跟着换代」的判定。
///
/// 这一层最容易出的错不是漏换，是**拿不准的时候换了** —— 换一次就把正在干活的
/// session 全打断。所以这里的「安全的那一侧」跟退出印记那边**相反**，
/// 而两边都必须是有意选的，不是顺手写的。
final class BackendUpdatePlanTests: XCTestCase {

    private let mine = "0.1.34(1)"

    // MARK: - 该换的那一种

    func testReplacesWhenTheBackendIsOlderThanTheApp() {
        let d = BackendUpdatePlan.decide(
            backend: .running(build: "0.1.31(4)", pid: 42, sessionCount: 3), appBuild: mine)
        XCTAssertEqual(d, .replace(oldBuild: "0.1.31(4)", newBuild: mine, sessionCount: 3))
        XCTAssertTrue(d.shouldReplace)
    }

    /// 版本**只要不同就换**，不比大小 —— 回滚（后端比界面新）同样要换，
    /// 否则一个降级过的 app 会永远连着一个新后端，而那才是真正说不清的状态。
    func testReplacesWhenTheBackendIsNewerToo() {
        XCTAssertTrue(BackendUpdatePlan.decide(
            backend: .running(build: "0.2.0(1)", pid: 42, sessionCount: 0),
            appBuild: mine).shouldReplace)
    }

    // MARK: - 不该换的三种，每一种都要说清为什么

    func testSameBuildIsLeftAlone() {
        let d = BackendUpdatePlan.decide(
            backend: .running(build: mine, pid: 42, sessionCount: 9), appBuild: mine)
        XCTAssertFalse(d.shouldReplace)
        guard case let .leaveAlone(why) = d else { return XCTFail() }
        XCTAssertTrue(why.contains(mine), why)
    }

    func testNoBackendMeansNothingToReplace() {
        XCTAssertFalse(BackendUpdatePlan.decide(backend: .none, appBuild: mine).shouldReplace)
    }

    /// **这条是这个文件的重点。** 问不出版本时**不许换** —— 换错一次会把正在跑的
    /// session 全打断，而多等一轮什么都不会坏。
    /// （同一个三态形状，退出印记那边拿不准是「多问一次」，这里是「别动」：
    /// 安全的那一侧由误判的代价决定。）
    func testUndecidableNeverReplaces() {
        let d = BackendUpdatePlan.decide(
            backend: .undecidable("握手超时"), appBuild: mine)
        XCTAssertFalse(d.shouldReplace, "问不出版本却把后端换了 —— 正在跑的 session 全断")
    }

    /// **但也不许静默**：一个永远换不了代的后台，跟一个每次启动都被打断的后台一样糟，
    /// 只是它安静。所以 `.undecidable` 那条必须把原话带出来。
    func testUndecidableStillExplainsItself() {
        guard case let .leaveAlone(why) = BackendUpdatePlan.decide(
            backend: .undecidable("握手超时"), appBuild: mine) else { return XCTFail() }
        XCTAssertTrue(why.contains("握手超时"), "把原因吞了：\(why)")
    }

    /// 每一条 `leaveAlone` 都得能对人说清为什么，不许是空串。
    func testEveryLeaveAloneSaysWhy() {
        let states: [BackendUpdatePlan.BackendState] = [
            .none, .undecidable("x"), .running(build: mine, pid: 1, sessionCount: 0),
        ]
        for state in states {
            guard case let .leaveAlone(why) = BackendUpdatePlan.decide(
                backend: state, appBuild: mine) else { continue }
            XCTAssertGreaterThan(why.count, 5, "\(state) 的理由等于没说：\(why)")
        }
    }

    // MARK: - 换代前那句广播

    /// 有 session 在跑时**必须说会断**，而且要说之后会问他接不接回来 ——
    /// 人得分得清「预期内的断」和「出事了」。
    func testAnnouncementWarnsAboutTheInterruptionWhenSessionsAreRunning() {
        let text = BackendUpdatePlan.announcement(
            oldBuild: "0.1.31(4)", newBuild: mine, sessionCount: 3)
        XCTAssertTrue(text.contains("0.1.31(4)"), text)
        XCTAssertTrue(text.contains(mine), text)
        XCTAssertTrue(text.contains("3 个"), text)
        XCTAssertTrue(text.contains("中断"), text)
        XCTAssertTrue(text.contains("接回来"), "没说之后会问他接不接回来：\(text)")
    }

    /// 没有 session 在跑时**不许吓人**：说清这次不打断任何东西。
    func testAnnouncementDoesNotScareWhenNothingIsRunning() {
        let text = BackendUpdatePlan.announcement(
            oldBuild: "0.1.31(4)", newBuild: mine, sessionCount: 0)
        XCTAssertTrue(text.contains("不会打断"), text)
        XCTAssertFalse(text.contains("0 个"), "「中断 0 个 session」这种话别说：\(text)")
    }

    /// 鸡生蛋那句话跟实现待在一起，别让发版说明和代码各自漂。
    func testChickenAndEggNoteIsPresentAndSaysWhatToDo() {
        let note = BackendUpdatePlan.chickenAndEggNote
        XCTAssertTrue(note.contains("手动"), note)
        XCTAssertTrue(note.contains("此后自动"), note)
    }
}
#endif
