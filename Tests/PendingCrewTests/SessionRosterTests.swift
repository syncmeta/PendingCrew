#if os(macOS)
import Foundation
import XCTest

/// P4 viewer：daemon 的 roster 怎么变成右栏里那份镜像。
@MainActor
final class SessionRosterTests: XCTestCase {

    private func state(kind: String = "claude_code") -> SessionProtocolState {
        .init(status: .running, isWorking: false, displayIsTyping: false,
              health: nil, pendingDecision: nil, kind: kind,
              launchParameterProblem: nil, scrollState: nil)
    }

    private func meta(crewId: String = "c1", role: String = "worker",
                      runStatus: String = "running") -> SessionRunSummary {
        .init(crewId: crewId, role: role, title: "标题", taskBrief: "活",
              workingDirectory: "/tmp/w", model: "opus", effort: nil,
              pendingProfile: nil, approvalsReviewer: nil, permissionModeOverride: nil,
              startedAt: 1_700_000_000, runStatus: runStatus, exitCode: nil,
              exitReason: nil, awaitingReply: nil)
    }

    private func summary(_ id: String, kind: String = "claude_code",
                         run: SessionRunSummary? = nil) -> SessionSummary {
        .init(sessionId: id, stateSeq: 1, state: state(kind: kind), run: run ?? meta())
    }

    // MARK: - 增删改

    func test_本地没有的才新建已有的只更新() {
        let plan = SessionRosterReconciliation.plan(
            incoming: [summary("a"), summary("b")], localSessionIds: ["a"])
        XCTAssertEqual(plan.update.map(\.sessionId), ["a"])
        XCTAssertEqual(plan.create.map(\.sessionId), ["b"])
        XCTAssertTrue(plan.remove.isEmpty)
    }

    /// **这条钉的是右栏不会每两秒闪一下。** roster 是全量、每拍都来；把已有的当新的
    /// 建一遍，正开着的那个 session 就会换一个 `runID`，选中态跟着跳。
    func test_同一份roster重复喂不产生任何新建() {
        let incoming = [summary("a"), summary("b")]
        let plan = SessionRosterReconciliation.plan(
            incoming: incoming, localSessionIds: ["a", "b"])
        XCTAssertTrue(plan.create.isEmpty)
        XCTAssertTrue(plan.remove.isEmpty)
        XCTAssertEqual(plan.update.count, 2)
    }

    func test_daemon那边没了的本地要删() {
        let plan = SessionRosterReconciliation.plan(
            incoming: [summary("a")], localSessionIds: ["a", "gone"])
        XCTAssertEqual(plan.remove, ["gone"])
    }

    /// 旧 daemon 不带编排身份（§4.4 向前兼容）——**跳过，不用默认值建一个**：
    /// 顶着空 crewId 的镜像挂在右栏上比不显示更难查。
    func test_没有编排身份的条目跳过而不是拿默认值建一个() {
        let bare = SessionSummary(sessionId: "x", stateSeq: 1, state: state(), run: nil)
        let plan = SessionRosterReconciliation.plan(incoming: [bare], localSessionIds: [])
        XCTAssertTrue(plan.create.isEmpty)
        XCTAssertTrue(plan.update.isEmpty)
    }

    func test_认不出的kind跳过() {
        let unknown = SessionSummary(sessionId: "x", stateSeq: 1,
                                     state: state(kind: "future_agent"), run: meta())
        XCTAssertTrue(SessionRosterReconciliation.plan(
            incoming: [unknown], localSessionIds: []).create.isEmpty)
    }

    // MARK: - §4.4 向前兼容

    func test_旧app解新daemon的summary不报错只是没有编排身份() throws {
        let json = try JSONSerialization.data(withJSONObject: [
            "sessionId": "s", "stateSeq": 3,
            "state": ["status": ["kind": "running"], "isWorking": false,
                      "displayIsTyping": false, "kind": "codex"],
            "run": ["crewId": "c", "role": "worker", "title": "t", "taskBrief": "b",
                    "workingDirectory": "/tmp", "startedAt": 1, "runStatus": "running",
                    "未来新增的字段": "无视我"],
        ])
        let decoded = try JSONDecoder().decode(SessionSummary.self, from: json)
        XCTAssertEqual(decoded.run?.crewId, "c")

        // 反过来：没有 run 字段的旧 daemon 报文照样解得开，不抛。
        let old = try JSONSerialization.data(withJSONObject: [
            "sessionId": "s", "stateSeq": 3,
            "state": ["status": ["kind": "running"], "isWorking": false,
                      "displayIsTyping": false, "kind": "codex"],
        ])
        XCTAssertNil(try JSONDecoder().decode(SessionSummary.self, from: old).run)
    }

    func test_编排身份可往返() throws {
        let value = meta(crewId: "c9", role: "captain", runStatus: "cancelled")
        let data = try JSONEncoder().encode(value)
        XCTAssertEqual(try JSONDecoder().decode(SessionRunSummary.self, from: data), value)
    }
}
#endif
