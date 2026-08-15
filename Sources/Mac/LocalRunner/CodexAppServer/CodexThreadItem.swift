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
/// The public `Kind`/structs are intentionally unchanged so the transcript view
/// is unaffected; only the decoding maps real keys onto them. Each branch keeps
/// a defensive fallback to the old flat key so a bare-string variant still
/// decodes. New arms (hookPrompt/imageView/imageGeneration/review/…) fall
/// through to `.unknown` by design.
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
    }

    struct FileChange: Equatable {
        let status: String?
        let summary: String?    // derived from changes[].path
    }

    private enum Keys: String, CodingKey {
        case id, type, text, phase, summary, content, command, cwd, status
        case aggregatedOutput, exitCode, name, query
        case server, tool, namespace, changes
    }

    /// One `content` entry of a `userMessage` (the `UserInput` union); we only
    /// surface text entries (image/skill/mention are dropped from the reduced view).
    private struct UserInputLite: Decodable { let type: String?; let text: String? }
    /// One `changes` entry of a `fileChange` (`FileUpdateChange`); path is what we render.
    private struct FileChangeLite: Decodable { let path: String? }

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
            kind = .commandExecution(.init(
                command: s(.command) ?? "",
                cwd: s(.cwd),
                status: s(.status),
                aggregatedOutput: s(.aggregatedOutput),
                exitCode: try? c.decode(Int.self, forKey: .exitCode)
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
#endif
