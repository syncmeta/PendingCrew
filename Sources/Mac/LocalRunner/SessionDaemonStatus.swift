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
        var lines = [
            "PendingCrew 后台正在运行",
            "PID：\(hello.pid)",
            "版本：\(hello.daemonBuild)（协议 \(hello.protocolVersion)）",
            "启动于：\(started)",
            "已连接前端：\(frontends.map(String.init) ?? "旧版后台未提供")",
            "运行中 session：\(sessions.count)",
        ]
        for session in sessions.sorted(by: { $0.sessionId < $1.sessionId }) {
            let title = session.run?.title.trimmingCharacters(in: .whitespacesAndNewlines)
            let label = title.flatMap { $0.isEmpty ? nil : $0 } ?? session.sessionId
            let crew = session.run?.crewId ?? "未知 crew"
            lines.append("- \(label) · \(crew) · \(session.sessionId)")
        }
        return lines.joined(separator: "\n")
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
