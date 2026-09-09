#if os(macOS)
import Foundation

/// PendingCrew 支持的本机 session 后端 kind。
///
/// 注：spec v2 §8.1 砍掉了 Kilo Code / opencode（不熟、降低复杂度），
/// 留到 v1.x 视需要再加。**不要**在这里塞回旧 enum case。
public enum LocalCodingAgentKind: String, CaseIterable, Sendable, Hashable, Codable {
    /// Anthropic Claude Code CLI（`claude`）
    case claudeCode = "claude_code"
    /// OpenAI Codex app-server（由 `codex app-server` 托管）
    case codex = "codex"
    /// 普通用户 shell（PTY）；不是 agent，不接 crew 编排与 MCP。
    case terminal = "terminal"

    /// 是否是能接收 crew 编排的 coding agent。纯终端必须始终为 false。
    public var isAgent: Bool { self != .terminal }

    /// 用户可见的简短显示名。**仅日志/UI 用**，不参与 wire/storage。
    public var displayName: String {
        switch self {
        case .claudeCode: return "Claude Code"
        case .codex: return "Codex"
        case .terminal: return "终端"
        }
    }

    /// 工具发现时在 `PATH` 上找的可执行名。
    var binaryName: String {
        switch self {
        case .claudeCode: return "claude"
        case .codex: return "codex"
        case .terminal:
            return URL(fileURLWithPath: ProcessInfo.processInfo.environment["SHELL"] ?? "/bin/zsh")
                .lastPathComponent
        }
    }

    /// 对应 server `crew_sessions.runner_kind` 的 wire 值(T4.5)。纯终端是本机人的
    /// 工具，不创建 server session，因此必须返回 nil，不能伪造 agent runner kind。
    public var serverRunnerKind: String? {
        switch self {
        case .claudeCode: return "local_claude_code"
        case .codex: return "local_codex"
        case .terminal: return nil
        }
    }

    /// crew 存的 `captainAgentKind`("claude_code"/"codex"/nil) → 本机 agent kind。
    /// **默认 Codex**(建 crew 时的默认;老 crew / edge crew 没记也走这里)。
    public static func captainDefault(_ raw: String?) -> LocalCodingAgentKind {
        switch raw {
        case "claude_code": return .claudeCode
        case "codex": return .codex
        default: return .codex
        }
    }

    enum MemberRestartError: LocalizedError, Equatable {
        case invalidRecordedKind(String)
        case unknownLegacyKind(String)
        case unreadableRecord(String)

        var errorDescription: String? {
            switch self {
            case .invalidRecordedKind(let raw):
                return "成员记录中的 runner 类型无效（\(raw)），已停止恢复；请修复记录后重试。"
            case .unknownLegacyKind(let name):
                return "成员「\(name)」没有 runner 记录，且无法从旧显示名确定类型，已停止恢复；请明确指定 runner 后重新启动。"
            case .unreadableRecord(let diagnostic):
                return "成员 runner 账本读取异常，已停止恢复：\(diagnostic)"
            }
        }
    }

    /// 已有记录是唯一事实源。只有确认无记录的旧成员允许从显示名推断；
    /// 无法识别时停止恢复，不得静默借用机长的 runner。
    static func restartingMember(recordedKind raw: String?, displayName: String,
                                 recordReadFailure: String? = nil) throws -> LocalCodingAgentKind {
        if let recordReadFailure {
            throw MemberRestartError.unreadableRecord(recordReadFailure)
        }
        if let raw {
            guard let kind = LocalCodingAgentKind(rawValue: raw), kind.isAgent else {
                throw MemberRestartError.invalidRecordedKind(raw)
            }
            return kind
        }
        guard let inferred = inferred(fromDisplayName: displayName) else {
            throw MemberRestartError.unknownLegacyKind(displayName)
        }
        return inferred
    }

    /// 仅为没有持久记录的旧成员从显示名反推 kind。显示名不是 runner 身份，
    /// 对不上时返回 nil，由恢复入口报错。
    public static func inferred(fromDisplayName name: String) -> LocalCodingAgentKind? {
        // 纯终端永不登记成成员，因此也不能从花名册反推出它并被 @ 唤醒。
        for kind in allCases where kind.isAgent && name.hasPrefix(kind.displayName) { return kind }
        return nil
    }
}
#endif
