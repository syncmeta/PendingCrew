#if os(macOS)
import Foundation

/// 「session 关掉再点恢复，agent 自己的对话上下文也接上」的判定层（人类 Todo #28）。
///
/// 背景：`CrewSessionRunner.restartMember` 一直复用原 localSessionId（身份/成员登记/
/// 白板游标延续），但从不带 agent 侧会话号，所以 agent 每次重启都是新脑子。两条续接
/// 通道本来就有（claude `--resume <id>` / codex `thread/resume`），缺的只是把会话号记
/// 下来 + 重启时决定要不要用它。
///
/// claude 侧的干净做法：**会话号由我们指定**（`claude --session-id <uuid>`，已在
/// claude 2.1.226 实测：指定后 `--resume <同一个 uuid>` 能复述上一轮内容，且续跑写回
/// 同一个 `.jsonl`、id 不变），所以不用去猜 `~/.claude` 里哪个日志是它的。
///
/// 纯 Foundation + 可注入，不碰文件系统（存在性判定由调用方以闭包喂进来），供单测直接跑。
enum AgentSessionResume {
    /// 重启一个既有成员时，agent 侧该怎么起。
    enum Decision: Equatable {
        /// 带着这个会话号续跑（claude `--resume` / codex `thread/resume`）。
        case resume(id: String)
        /// 接不回来 → 老老实实新起一轮，并把原因如实说出去（不装死）。
        case fresh(reason: FreshReason)
    }

    /// 为什么续不上 —— 每一种都要能对人说清楚。
    enum FreshReason: Equatable {
        /// 压根没记过会话号（这个成员是本特性上线前起的）。
        case noRecord
        /// 记了，但 agent 那边的会话没了（日志被清 / 换机器 / CLI 说无此会话）。
        case transcriptMissing(id: String)
    }

    /// 决定重启时带不带会话号。
    ///
    /// - Parameters:
    ///   - recordedId: 账本里记的 agent 侧会话号（`LocalAgentSessionStore`）。
    ///   - transcriptAvailable: 这个会话号在 agent 那边还在不在。claude 能查日志文件；
    ///     codex 查不了（thread 存在与否要发请求才知道），传 `{ _ in true }` 让它乐观
    ///     续跑 —— 真续不上由 backend 的 resume→start 降级兜住，同样会 fail-loud。
    static func decide(recordedId: String?,
                       transcriptAvailable: (String) -> Bool) -> Decision {
        guard let id = recordedId, !id.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return .fresh(reason: .noRecord)
        }
        return transcriptAvailable(id) ? .resume(id: id) : .fresh(reason: .transcriptMissing(id: id))
    }

    /// 我们自己指定给一个新 claude session 的会话号（小写 uuid，claude `--session-id` 要求）。
    static func newClaudeSessionId(_ uuid: UUID = UUID()) -> String {
        uuid.uuidString.lowercased()
    }

    /// claude 的会话日志路径：`~/.claude/projects/<工作目录 slug>/<会话号>.jsonl`。
    /// slug = 绝对路径里每个 `/` 和 `.` 都换成 `-`（实测目录名如
    /// `-Users-hey-…-dev--pendingcrew-worktrees-…`）。
    static func claudeTranscriptURL(sessionId: String, workdir: String, home: URL) -> URL {
        home.appendingPathComponent(".claude/projects", isDirectory: true)
            .appendingPathComponent(projectSlug(forWorkdir: workdir), isDirectory: true)
            .appendingPathComponent(sessionId + ".jsonl")
    }

    /// 工作目录 → claude 项目目录名。
    static func projectSlug(forWorkdir workdir: String) -> String {
        String(workdir.map { ($0 == "/" || $0 == ".") ? "-" : $0 })
    }

    /// 续不上时塞进这轮 brief 的开头 —— 让 agent 自己知道「我不是接着上次，是新开的」，
    /// 别对着空脑子假装记得。能续上则 nil（不加噪音）。
    static func briefNotice(for decision: Decision) -> String? {
        guard case .fresh(let reason) = decision else { return nil }
        switch reason {
        case .noRecord:
            return "【注意】你和这个成员此前的对话上下文没能接回来（没有记录），"
                + "这是**新开的一轮**。群里的白板会自动注入，别假装记得终端里聊过的细节。"
        case .transcriptMissing(let id):
            return "【注意】原会话（\(id)）在本机已找不到，接不回来了，这是**新开的一轮**。"
                + "群里的白板会自动注入，别假装记得终端里聊过的细节。"
        }
    }

    /// 续不上时往群里说的那句（fail-loud，人能看见）。能续上则 nil。
    static func whiteboardNotice(memberName: String, decision: Decision) -> String? {
        guard case .fresh(let reason) = decision else { return nil }
        let tail: String
        switch reason {
        case .noRecord: tail = "此前没记下它的会话号"
        case .transcriptMissing: tail = "原会话在本机已找不到"
        }
        return "「\(memberName)」的原对话接不回来了（\(tail)），这一轮是**新开的**——"
            + "群里的上下文它还在，但终端里聊过的细节要重讲。"
    }
}
#endif
