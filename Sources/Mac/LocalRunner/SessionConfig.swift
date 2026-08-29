#if os(macOS)
import Foundation

/// Per-crew/per-session Codex approval preference. Captain uses a stable crew key
/// because its local run id may change after an app restart; workers keep independent
/// session keys. Missing and malformed values intentionally fall back to auto_review.
final class CodexApprovalModeStore: @unchecked Sendable {
    static let shared = CodexApprovalModeStore()
    static let defaultsKey = "pendingcrew.codexApprovalModes.v1"

    enum Scope: Equatable {
        case captain
        case session(String)
    }

    private let defaults: UserDefaults
    private let defaultsKey: String
    private let lock = NSLock()

    init(defaults: UserDefaults = .standard,
         defaultsKey: String = CodexApprovalModeStore.defaultsKey) {
        self.defaults = defaults
        self.defaultsKey = defaultsKey
    }

    func reviewer(crewId: String, scope: Scope) -> CodexProtocol.ApprovalsReviewer {
        lock.lock()
        defer { lock.unlock() }
        let raw = values()[Self.storageKey(crewId: crewId, scope: scope)]
        return raw.flatMap(CodexProtocol.ApprovalsReviewer.init(rawValue:)) ?? .autoReview
    }

    func set(_ reviewer: CodexProtocol.ApprovalsReviewer, crewId: String, scope: Scope) {
        lock.lock()
        defer { lock.unlock() }
        var next = values()
        next[Self.storageKey(crewId: crewId, scope: scope)] = reviewer.rawValue
        defaults.set(next, forKey: defaultsKey)
    }

    private func values() -> [String: String] {
        defaults.dictionary(forKey: defaultsKey) as? [String: String] ?? [:]
    }

    private static func storageKey(crewId: String, scope: Scope) -> String {
        switch scope {
        case .captain: return "\(crewId)|captain"
        case .session(let sessionId): return "\(crewId)|session|\(sessionId)"
        }
    }
}

/// Per-session launch configuration (spec §1 启动带指令 / §11 per-session 配置).
/// Pure value type so `argv()` is unit-testable without spawning a process.
public struct SessionConfig: Sendable, Equatable {
    public var kind: LocalCodingAgentKind
    /// Claude uses `--model`; Codex sends `model` in thread/start or thread/resume.
    /// `nil` lets the corresponding runner use its default.
    public var model: String?
    /// Thinking effort: claude `--effort <level>` / codex app-server
    /// `thread/start.config.model_reasoning_effort`.
    /// Passthrough string — valid levels are runner-owned (UI validates; see spec probe §11).
    public var effort: String?
    /// The session's first instruction. Claude receives it over the PTY only after the
    /// TUI produces output; it must never be a process argument because `ps` exposes argv
    /// to every local session. Codex sends it over app-server after the handshake.
    public var initialPrompt: String?
    /// Resume an existing agent session by id.
    public var resumeSessionId: String?
    /// 新起 session 时**由我们指定**的 agent 侧会话号 → claude `--session-id <uuid>`
    /// （Todo #28）。自己指定就能立刻记账，重启直接 `--resume` 同一个 id，不用去猜
    /// `~/.claude/projects` 里哪个日志是它的。与 `resumeSessionId` 互斥（续跑时不带）。
    /// codex 用不上（threadId 由 app-server 握手时下发，握上手再记账）。
    public var newSessionId: String?
    /// claude permission mode. **Default "auto" = auto mode classifier (spec §9); never bypass.**
    public var permissionMode: String
    /// 隔离:on → session 跑在独立 git worktree（并行不打架 + claude containment §8）；
    /// off → 用 crew 共享目录。**编排参数,不进 argv。**
    public var isolation: Bool
    /// 世界观 system prompt 文件路径 → claude `--append-system-prompt-file`
    /// (**append**,不 replace claude 默认 system prompt;flag 已 `claude --help` 核实)。
    /// nil = 不注入。调用方用 `LocalSessionWorldModel` 渲染后写临时文件,把路径塞这里
    /// (spec local-first chunk 4 session↔crew 接线)。codex 路径暂不接(走 app-server
    /// 的 additionalContext,见 codex 参考)。
    public var appendSystemPromptFile: String?
    /// claude `--settings <file>`：per-session settings.json（含 PostToolUse hook =
    /// `pendingcrew-mcp hook`，每轮注入未读白板）。nil = 不带。flag 经 spike 实测。
    public var settingsFile: String?
    /// claude `--mcp-config <file>`：per-session mcp-config.json（挂 `pendingcrew-mcp
    /// serve`，提供 post_to_crew/read_whiteboard）。nil = 不带。flag 经 spike 实测。
    public var mcpConfigFile: String?

    public init(kind: LocalCodingAgentKind,
                model: String? = nil,
                effort: String? = nil,
                initialPrompt: String? = nil,
                resumeSessionId: String? = nil,
                permissionMode: String = "auto",
                isolation: Bool = false,
                appendSystemPromptFile: String? = nil,
                settingsFile: String? = nil,
                mcpConfigFile: String? = nil) {
        self.kind = kind
        self.model = model
        self.effort = effort
        self.initialPrompt = initialPrompt
        self.resumeSessionId = resumeSessionId
        self.permissionMode = permissionMode
        self.isolation = isolation
        self.appendSystemPromptFile = appendSystemPromptFile
        self.settingsFile = settingsFile
        self.mcpConfigFile = mcpConfigFile
    }

    /// Build the spawn argv for this session's agent CLI.
    ///
    /// **claude** (`claudeCode`): interactive TUI. The opening prompt is deliberately
    /// absent here and is delivered over the PTY by `AgentSessionCore` after startup.
    ///
    /// **codex**: runs as `codex app-server` (stdio JSON-RPC). The subcommand is the
    /// only token. All per-session values — effort, model, cwd, instructions, resume id,
    /// and the initial prompt — go over app-server after the handshake. PendingCrew does
    /// not model Codex sessions as interactive CLI invocations.
    ///
    /// **terminal**: the runner resolves the user's default shell and starts it as a
    /// login shell. Agent-only configuration and the initial prompt are deliberately
    /// ignored: a terminal is a human-operated PTY, not an agent session.
    public func argv() -> [String] {
        switch kind {
        case .claudeCode:
            var a: [String] = []
            // 续跑优先；否则若我们指定了会话号就带 --session-id（两者互斥，claude
            // 不接受同时给）。都没有 = 让 claude 自己生成（此路重启接不上，只作兜底）。
            if let id = resumeSessionId, !id.isEmpty {
                a += ["--resume", id]
            } else if let nid = newSessionId, !nid.isEmpty {
                a += ["--session-id", nid]
            }
            a += ["--permission-mode", permissionMode] // "auto", NOT --dangerously-skip-permissions
            if let f = appendSystemPromptFile, !f.isEmpty { a += ["--append-system-prompt-file", f] }
            if let s = settingsFile, !s.isEmpty { a += ["--settings", s] }
            if let mc = mcpConfigFile, !mc.isEmpty { a += ["--mcp-config", mc] }
            if let m = model, !m.isEmpty { a += ["--model", m] }
            if let e = effort, !e.isEmpty { a += ["--effort", e] }
            // Never append `initialPrompt`: process argv is readable through `ps`. A 2026-08-28
            // incident exposed another crew's full whiteboard this way and the observing agent
            // mistook that tool output for a task. `AgentSessionCore` owns PTY delivery instead.
            return a
        case .codex:
            return ["app-server"]
        case .terminal:
            return ["-l"]
        }
    }
}
#endif
