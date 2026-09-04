#if os(macOS)
import Foundation

/// **把 codex 的 reduced transcript 变成文本**的唯一口径。
///
/// codex 没有终端 —— 它过江的不是字节流而是结构化条目，所以「看一眼它现在是什么样」
/// 在 codex 上等于「看这份对话记录」。这段映射本来长在 `CodexAppServerBackend`
/// （daemon 侧 `screenText` / `inspect_session` 用），现在无画面探针
/// （`--daemon-attach`）也要同一份文本 —— 于是抽到这里，**两个消费者一份文本**。
///
/// 与 `TerminalScreenText` 是一对：那个管「格子 → 文本」，这个管「条目 → 文本」。
/// 两条路各有各的权威源，但都只有一个口径。
enum CodexTranscriptText {

    /// 空的时候**明说为空**，不返回空串 —— 空串在输出里跟「拿不到」分不开，
    /// 而这两件事对「内容连续」这条证据的意义完全不同。
    static let emptyDescription = "（transcript 为空）"

    static func render(items: [CodexThreadItem], maxLines: Int) -> String {
        let tail = items.suffix(max(0, maxLines))
        guard !tail.isEmpty else { return emptyDescription }
        return tail.map(line(of:)).joined(separator: "\n")
    }

    static func line(of item: CodexThreadItem) -> String {
        switch item.kind {
        case let .userMessage(text): return "[输入] \(text.prefix(200))"
        case let .agentMessage(text, _): return "[回复] \(text.prefix(300))"
        case let .reasoning(summary, content):
            return "[思考] \((summary ?? content ?? "…").prefix(200))"
        case let .plan(text): return "[计划] \(text.prefix(200))"
        case let .commandExecution(command):
            return "[命令] \(command.command.prefix(160))"
                + (command.exitCode.map { " → exit \($0)" } ?? "")
        case let .fileChange(change): return "[改文件] \(change.summary ?? change.status ?? "?")"
        case let .toolCall(name, status): return "[工具] \(name) \(status ?? "")"
        case let .webSearch(query): return "[搜索] \(query ?? "")"
        case let .unknown(type): return "[\(type)]"
        }
    }
}
#endif
