import XCTest
@testable import PendingCrew

#if os(macOS)

/// `--daemon-status` 那段文本的判定（前后端分离 P5a）。
///
/// **为什么单独钉这几行**：daemon 的 `records` 在 session 退出之后**照旧留着**
/// （右栏的镜像还要看那份终端画面），所以 `listSessions` 回来的名单里天然混着
/// 已退出的条目。2026-09-04 的真 daemon 冒烟里，两个已经 `exited` 的 session
/// 被 `--daemon-status` 报成「运行中 session：2」—— 而共享账本 `crew-sessions.json`
/// 里它们已经是 `exited`。**两本账对不上时，人信的是命令行那本**，于是「后台还有活在跑」
/// 这个判断整个是错的，正好是关 app / 装更新之前最不该看错的那一栏。
final class SessionDaemonStatusTextTests: XCTestCase {

    private func hello(sessions: Int) -> SessionDaemonHello {
        .init(protocolVersion: 1, daemonBuild: "0.1.24(1)", capabilities: [],
              sessionCount: sessions, pid: 4242, viewerCount: 1, startedAt: 0)
    }

    private func summary(_ id: String, status: SessionWireStatus,
                         title: String) -> SessionSummary {
        .init(sessionId: id, stateSeq: 1,
              state: .init(status: status, isWorking: false, displayIsTyping: false,
                           health: nil, pendingDecision: nil, kind: "claude_code",
                           launchParameterProblem: nil, scrollState: nil),
              run: .init(crewId: "local-smoke", role: "worker", title: title,
                         taskBrief: "-", workingDirectory: "/tmp",
                         model: nil, effort: nil, pendingProfile: nil,
                         approvalsReviewer: nil, permissionModeOverride: nil,
                         startedAt: 0, runStatus: "running", exitCode: nil,
                         exitReason: nil))
    }

    func test_已退出的session不算进运行中() throws {
        let snapshot = SessionDaemonStatusSnapshot(
            hello: hello(sessions: 2),
            sessions: [summary("worker-alive", status: .running, title: "还在跑"),
                       summary("worker-dead", status: .exited(0), title: "已经退了")])
        let text = snapshot.text
        XCTAssertTrue(text.contains("运行中 session：1"),
                      "两条里只有一条在跑，却报成了别的数：\n\(text)")
        XCTAssertTrue(text.contains("还在跑"), "在跑的那条必须列出来：\n\(text)")
    }

    func test_已退出的session不混在运行中名单里() throws {
        let snapshot = SessionDaemonStatusSnapshot(
            hello: hello(sessions: 2),
            sessions: [summary("worker-alive", status: .running, title: "还在跑"),
                       summary("worker-dead", status: .exited(0), title: "已经退了")])
        let lines = snapshot.text.split(separator: "\n").map(String.init)
        guard let runningIndex = lines.firstIndex(where: { $0.hasPrefix("运行中 session：") })
        else { return XCTFail("没有「运行中 session：」这一行：\n\(snapshot.text)") }
        // 「运行中」那一行紧跟着的条目行只许是在跑的那些。
        let listed = lines[(runningIndex + 1)...].prefix { $0.hasPrefix("- ") }
        XCTAssertEqual(listed.count, 1, "运行中名单里混进了别的条目：\n\(snapshot.text)")
        XCTAssertFalse(listed.joined().contains("已经退了"),
                       "已退出的 session 被列进了运行中名单：\n\(snapshot.text)")
    }

    func test_全都退出时明说没有在跑的() throws {
        let snapshot = SessionDaemonStatusSnapshot(
            hello: hello(sessions: 1),
            sessions: [summary("worker-dead", status: .exited(1), title: "已经退了")])
        XCTAssertTrue(snapshot.text.contains("运行中 session：0"),
                      "一条都不在跑时要报 0：\n\(snapshot.text)")
    }
}

#endif
