#if os(macOS)
import Foundation

/// P5 的无界面自检结果。事实来自 daemon 的协议握手与 roster，不读本地注释或猜版本。
struct SessionDaemonStatusSnapshot: Equatable {
    var hello: SessionDaemonHello
    var sessions: [SessionSummary]

    var text: String {
        let started = hello.startedAt.map {
            ISO8601DateFormatter().string(from: Date(timeIntervalSince1970: $0))
        } ?? "旧版后台未提供"
        let frontends = hello.viewerCount.map { max(0, $0 - 1) }
        // **daemon 的 records 在 session 退出后照旧留着**（右栏镜像还要看那份终端画面），
        // 所以 `listSessions` 回来的名单天然混着已退出的条目。按状态分开数、分开列 ——
        // 2026-09-04 的真 daemon 冒烟里这一栏把两个 `exited` 报成「运行中 2」，
        // 而共享账本里它们已经是 exited。**两本账打架时人信的是命令行那本**，
        // 而这一栏正是关 app / 装更新之前最不该看错的东西。
        let ordered = sessions.sorted { $0.sessionId < $1.sessionId }
        let running = ordered.filter { $0.state.status == .running }
        let finished = ordered.filter { $0.state.status != .running }
        var lines = [
            "PendingCrew 后台正在运行",
            "PID：\(hello.pid)",
            "版本：\(hello.daemonBuild)（协议 \(hello.protocolVersion)）",
            "启动于：\(started)",
            "已连接前端：\(frontends.map(String.init) ?? "旧版后台未提供")",
            "运行中 session：\(running.count)",
        ]
        lines.append(contentsOf: running.map(Self.line))
        if !finished.isEmpty {
            // 已退出的**不省略**：它们还占着 daemon 里的记录（画面留着给人看），
            // 直接不显示会让「后台里到底还有什么」这个问题永远差一块。
            lines.append("已退出但画面还留着：\(finished.count)")
            lines.append(contentsOf: finished.map(Self.line))
        }
        return lines.joined(separator: "\n")
    }

    private static func line(_ session: SessionSummary) -> String {
        let title = session.run?.title.trimmingCharacters(in: .whitespacesAndNewlines)
        let label = title.flatMap { $0.isEmpty ? nil : $0 } ?? session.sessionId
        let crew = session.run?.crewId ?? "未知 crew"
        return "- \(label) · \(crew) · \(session.sessionId)"
    }
}

/// 建一条短命协议连接问实况。连接本身不 attach session，也不启动/停止任何东西。
@MainActor
enum SessionDaemonStatusProbe {
    enum ProbeError: Error, CustomStringConvertible {
        case timeout

        var description: String { "后台已占用 socket，但状态握手超时" }
    }

    static func query(paths: PendingCrewDaemonPaths = .standard(),
                      timeout: TimeInterval = 2) throws -> SessionDaemonStatusSnapshot {
        let link = try UnixSocketTransport.connect(toPath: paths.socket)
        defer { link.close() }

        let client = SessionProtocolClient(
            link: link, capabilities: [], appBuild: "daemon-status")
        var hello: SessionDaemonHello?
        var list: SessionList?
        client.onDaemonHello = { hello = $0 }
        client.onSessionList = { list = $0 }
        client.connect()
        client.requestSessionList()

        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline, (hello == nil || list == nil) {
            _ = RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.01))
        }
        guard let hello, let list else { throw ProbeError.timeout }
        return .init(hello: hello, sessions: list.sessions)
    }
}
#endif
