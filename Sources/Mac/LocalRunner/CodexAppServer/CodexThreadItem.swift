#if os(macOS)
import Foundation

/// A codex `ThreadItem` reduced to what the transcript renders. `.unknown`
/// keeps forward-compat: a codex upgrade adding a new arm degrades to a raw
/// row instead of failing the decode.
///
/// Inbound field names reconciled against a real `codex app-server
/// generate-json-schema` capture (codex-cli **0.137.0**, `v2/ThreadItem`) on
/// 2026-06-16 — see docs/tech-debt.md. The shapes that bit the first pass
/// (they were written from the prose reference and silently mis-decoded):
/// - `userMessage` carries `content: [UserInput]` (text lives in `.type=="text"`
///   entries), **not** a top-level `text`.
/// - `reasoning.summary` / `reasoning.content` are `[String]`, **not** strings.
/// - `fileChange` carries `changes: [FileUpdateChange]` (`{path,kind,diff}`),
///   **not** a `summary`.
/// - tool calls carry `server`+`tool` (mcp) / `namespace?`+`tool` (dynamic) /
///   `tool` (collab) — **not** a `name`.
/// The reduced `Kind` shape stays compatible with the transcript view; decoding
/// maps real keys onto it and retains each action's exact command for Todo #90.
/// Each branch keeps a defensive fallback to the old flat key so a bare-string
/// variant still decodes. New arms (hookPrompt/imageView/imageGeneration/review/…)
/// fall through to `.unknown` by design.
struct CodexThreadItem: Decodable, Identifiable, Equatable {
    let id: String
    let kind: Kind

    enum Kind: Equatable {
        case userMessage(text: String)
        case agentMessage(text: String, phase: String?)
        case reasoning(summary: String?, content: String?)
        case plan(text: String)
        case commandExecution(CommandExec)
        case fileChange(FileChange)
        case toolCall(name: String, status: String?)   // mcp/dynamic/collab folded
        case webSearch(query: String?)
        case unknown(type: String)
    }

    struct CommandExec: Equatable {
        let command: String
        let cwd: String?
        let status: String?
        let aggregatedOutput: String?
        let exitCode: Int?
        /// app-server 已经做好的 best-effort 语义解析。UI 用它生成折叠态的「读取
        /// 档案 / 搜索档案 / 执行指令」摘要；shell 原文只在展开详情显示（#83/#90）。
        var actions: [CommandAction] = []
    }

    struct CommandAction: Equatable {
        enum Kind: String, Hashable { case read, listFiles, search, unknown }
        let kind: Kind
        /// Exact sub-command supplied by app-server. The collapsed activity row never
        /// prints this whole string; the disclosure detail does (Todo #90).
        let command: String?
        let name: String?
        let path: String?
        let query: String?
    }

    struct FileChange: Equatable {
        let status: String?
        let summary: String?    // derived from changes[].path
    }

    private enum Keys: String, CodingKey {
        case id, type, text, phase, summary, content, command, cwd, status
        case aggregatedOutput, exitCode, name, query, commandActions
        case server, tool, namespace, changes
    }

    /// One `content` entry of a `userMessage` (the `UserInput` union); we only
    /// surface text entries (image/skill/mention are dropped from the reduced view).
    private struct UserInputLite: Decodable { let type: String?; let text: String? }
    /// One `changes` entry of a `fileChange` (`FileUpdateChange`); path is what we render.
    private struct FileChangeLite: Decodable { let path: String? }
    private struct CommandActionLite: Decodable {
        let type: String
        let command: String?
        let name: String?
        let path: String?
        let query: String?
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        id = (try? c.decode(String.self, forKey: .id)) ?? UUID().uuidString
        let type = (try? c.decode(String.self, forKey: .type)) ?? "unknown"
        func s(_ k: Keys) -> String? {
            let v = try? c.decode(String.self, forKey: k)
            return (v?.isEmpty == true) ? nil : v
        }
        // real schema is array-of-strings; tolerate a bare string for fwd/back-compat
        func joinedStrings(_ k: Keys) -> String? {
            if let arr = try? c.decode([String].self, forKey: k) {
                let joined = arr.joined(separator: "\n")
                return joined.isEmpty ? nil : joined
            }
            return s(k)
        }
        switch type {
        case "userMessage":
            let parts = (try? c.decode([UserInputLite].self, forKey: .content)) ?? []
            let text = parts
                .filter { $0.type == nil || $0.type == "text" }
                .compactMap { $0.text }
                .joined(separator: "\n")
            kind = .userMessage(text: text.isEmpty ? (s(.text) ?? "") : text)
        case "agentMessage":
            kind = .agentMessage(text: s(.text) ?? "", phase: s(.phase))
        case "reasoning":
            kind = .reasoning(summary: joinedStrings(.summary), content: joinedStrings(.content))
        case "plan":
            kind = .plan(text: s(.text) ?? "")
        case "commandExecution":
            let actionRows = (try? c.decode([CommandActionLite].self, forKey: .commandActions)) ?? []
            let actions = actionRows.map {
                CommandAction(
                    kind: CommandAction.Kind(rawValue: $0.type) ?? .unknown,
                    command: $0.command, name: $0.name, path: $0.path, query: $0.query)
            }
            kind = .commandExecution(.init(
                command: s(.command) ?? "",
                cwd: s(.cwd),
                status: s(.status),
                aggregatedOutput: s(.aggregatedOutput),
                exitCode: try? c.decode(Int.self, forKey: .exitCode),
                actions: actions
            ))
        case "fileChange":
            let changes = (try? c.decode([FileChangeLite].self, forKey: .changes)) ?? []
            let paths = changes.compactMap { $0.path }
            kind = .fileChange(.init(
                status: s(.status),
                summary: paths.isEmpty ? s(.summary) : paths.joined(separator: ", ")
            ))
        case "mcpToolCall", "dynamicToolCall", "collabAgentToolCall":
            // mcp = server+tool · dynamic = namespace?+tool · collab = tool (string enum)
            let tool = s(.tool)
            let name: String
            if let server = s(.server), let tool { name = "\(server).\(tool)" }
            else if let ns = s(.namespace), let tool { name = "\(ns)/\(tool)" }
            else { name = tool ?? s(.name) ?? type }
            kind = .toolCall(name: name, status: s(.status))
        case "webSearch":
            kind = .webSearch(query: s(.query))
        default:
            kind = .unknown(type: type)
        }
    }

    /// Synthetic construction — `CodexTranscript` builds/updates `reasoning` items
    /// from the streamed delta notifications. Under ChatGPT auth the final
    /// `item/completed` reasoning item completes with an empty `summary` (the real
    /// chain-of-thought is `encrypted_content`), so the only readable text is the
    /// `item/reasoning/summaryTextDelta` stream we accumulate. See CodexTranscript.
    init(id: String, kind: Kind) {
        self.id = id
        self.kind = kind
    }
}

/// Human-facing collapsed copy plus the exact, selectable facts shown after expansion.
/// Keeping this pure makes the "specific enough when closed, complete enough when open"
/// contract independently testable from SwiftUI (Todo #90).
struct CodexActivityPresentation: Equatable {
    struct Detail: Equatable, Identifiable {
        let label: String
        let value: String
        var id: String { label + "\u{0}" + value }
    }

    let icon: String
    let headline: String
    let details: [Detail]
    let bad: Bool

    static func command(_ command: CodexThreadItem.CommandExec) -> Self {
        let failed = command.exitCode.map { $0 != 0 } ?? (command.status == "failed")
        let primary = command.actions.first { $0.kind != .unknown }
        let program = programName(
            from: command.actions.compactMap(\.command) + [command.command])

        let verb: String
        let subject: String?
        if failed {
            verb = "执行指令失败"
            subject = program
        } else if let primary, primary.kind == .search {
            verb = "已搜索档案"
            subject = compactPath(primary.path) ?? primary.query
        } else if let primary, primary.kind == .read || primary.kind == .listFiles {
            verb = "已读取档案"
            subject = primary.name ?? compactPath(primary.path)
        } else {
            verb = command.status == "inProgress" ? "正在执行指令" : "已执行指令"
            subject = program
        }

        var details: [Detail] = []
        if !command.command.isEmpty {
            details.append(.init(label: "完整指令", value: command.command))
        }
        if let cwd = command.cwd, !cwd.isEmpty {
            details.append(.init(label: "工作目录", value: cwd))
        }
        let paths = unique(command.actions.compactMap(\.path))
        if !paths.isEmpty {
            details.append(.init(label: "涉及档案", value: paths.joined(separator: "\n")))
        }
        let queries = unique(command.actions.compactMap(\.query))
        if !queries.isEmpty {
            details.append(.init(label: "搜索内容", value: queries.joined(separator: "\n")))
        }
        if let exitCode = command.exitCode {
            details.append(.init(label: "退出状态", value: String(exitCode)))
        } else if let status = command.status, !status.isEmpty {
            details.append(.init(label: "状态", value: status))
        }
        return .init(
            icon: failed ? "exclamationmark.triangle" : icon(for: primary?.kind),
            headline: headline(verb, subject), details: details, bad: failed)
    }

    static func fileChange(_ change: CodexThreadItem.FileChange) -> Self {
        let failed = change.status == "failed"
        let paths = change.summary?
            .split(separator: ",", omittingEmptySubsequences: true)
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty } ?? []
        let subject = compactList(paths)
        var details: [Detail] = []
        if !paths.isEmpty {
            details.append(.init(label: "涉及档案", value: paths.joined(separator: "\n")))
        }
        if let status = change.status, !status.isEmpty {
            details.append(.init(label: "状态", value: status))
        }
        return .init(
            icon: failed ? "exclamationmark.triangle" : "doc.badge.ellipsis",
            headline: headline(failed ? "修改档案失败" : "已修改档案", subject),
            details: details, bad: failed)
    }

    static func tool(name: String, status: String?) -> Self {
        let failed = status == "failed"
        var details = [Detail(label: "工具", value: name)]
        if let status, !status.isEmpty { details.append(.init(label: "状态", value: status)) }
        return .init(
            icon: failed ? "exclamationmark.triangle" : "wrench.and.screwdriver",
            headline: headline(failed ? "调用工具失败" : "已调用工具", name),
            details: details, bad: failed)
    }

    static func webSearch(query: String?) -> Self {
        let clean = query?.trimmingCharacters(in: .whitespacesAndNewlines)
        let value = clean?.isEmpty == false ? clean : nil
        return .init(
            icon: "globe", headline: headline("已搜索网页", value),
            details: value.map { [.init(label: "搜索内容", value: $0)] } ?? [], bad: false)
    }

    private static func headline(_ verb: String, _ subject: String?) -> String {
        guard let subject, !subject.isEmpty else { return verb }
        return verb + " · " + subject
    }

    private static func icon(for kind: CodexThreadItem.CommandAction.Kind?) -> String {
        switch kind {
        case .search: return "magnifyingglass"
        case .read, .listFiles: return "book"
        default: return "terminal"
        }
    }

    private static func compactPath(_ raw: String?) -> String? {
        guard let raw, !raw.isEmpty else { return nil }
        let trimmed = raw.hasSuffix("/") ? String(raw.dropLast()) : raw
        let last = URL(fileURLWithPath: trimmed).lastPathComponent
        return last.isEmpty ? raw : last
    }

    private static func compactList(_ values: [String]) -> String? {
        guard let first = values.first else { return nil }
        let compact = compactPath(first) ?? first
        return values.count == 1 ? compact : "\(compact)（另 \(values.count - 1) 项）"
    }

    private static func unique(_ values: [String]) -> [String] {
        var seen: Set<String> = []
        return values.filter { !$0.isEmpty && seen.insert($0).inserted }
    }

    /// Best-effort executable name for the collapsed row. This deliberately does not
    /// reproduce a shell parser: it only finds the first real program, skipping common
    /// setup-only segments/wrappers. The exact command is always available in details.
    private static func programName(from commands: [String]) -> String? {
        for command in commands where !command.isEmpty {
            for segment in shellSegments(command) {
                var words = shellWords(segment)
                guard !words.isEmpty else { continue }
                if ["cd", "export", "set", "unset", "source", "."].contains(words[0]) {
                    continue
                }
                while let first = words.first, isEnvironmentAssignment(first) {
                    words.removeFirst()
                }
                while let wrapper = words.first,
                      ["env", "sudo", "command", "time", "nice", "nohup"].contains(
                        URL(fileURLWithPath: wrapper).lastPathComponent) {
                    words.removeFirst()
                    while let first = words.first,
                          first.hasPrefix("-") || isEnvironmentAssignment(first) {
                        words.removeFirst()
                    }
                }
                guard let first = words.first else { continue }
                let name = URL(fileURLWithPath: first).lastPathComponent
                if !name.isEmpty { return name }
            }
        }
        return nil
    }

    private static func shellSegments(_ command: String) -> [String] {
        command
            .replacingOccurrences(of: "&&", with: "\n")
            .replacingOccurrences(of: "||", with: "\n")
            .replacingOccurrences(of: ";", with: "\n")
            .replacingOccurrences(of: "|", with: "\n")
            .split(separator: "\n")
            .map(String.init)
    }

    private static func shellWords(_ command: String) -> [String] {
        var result: [String] = []
        var current = ""
        var quote: Character?
        var escaped = false
        for character in command {
            if escaped { current.append(character); escaped = false; continue }
            if character == "\\" { escaped = true; continue }
            if let active = quote {
                if character == active { quote = nil } else { current.append(character) }
            } else if character == "\"" || character == "'" {
                quote = character
            } else if character.isWhitespace {
                if !current.isEmpty { result.append(current); current = "" }
            } else {
                current.append(character)
            }
        }
        if escaped { current.append("\\") }
        if !current.isEmpty { result.append(current) }
        return result
    }

    private static func isEnvironmentAssignment(_ word: String) -> Bool {
        guard let equal = word.firstIndex(of: "="), equal != word.startIndex else { return false }
        let name = word[..<equal]
        return name.allSatisfy { $0 == "_" || $0.isLetter || $0.isNumber }
    }
}
#endif
