#if os(macOS)
import Foundation

/// Observable transcript built from codex streaming notifications. v1 renders on
/// `item/completed` (authoritative final per item); deltas/item-started are opted out.
@MainActor
final class CodexTranscript: ObservableObject {
    @Published private(set) var items: [CodexThreadItem] = []
    @Published private(set) var turnActive = false
    @Published private(set) var activeTurnId: String?
    /// 唤醒回执用的单调活动序号。`isWorking` 是瞬时态，短 turn 可在首拍前从
    /// true 回 false；turn/item 事件一旦到达，这个序号就不会倒退。
    private(set) var activityRevision: UInt64 = 0

    /// Reasoning streams as deltas keyed by `itemId`. Under ChatGPT auth the final
    /// `item/completed` reasoning item carries an EMPTY `summary` (the real
    /// chain-of-thought is `encrypted_content`), so the only human-readable text is
    /// the `item/reasoning/summaryTextDelta` stream — accumulate it here and keep it
    /// when the empty completed item lands (otherwise 思考过程 渲染成空白, #4).
    private var reasoningSummary: [String: String] = [:]
    private var reasoningContent: [String: String] = [:]

    func apply(method: String, params: [String: Any]) {
        // item/started、工具调用与流式 delta 即使尚未形成可渲染行，也已经是
        // “消息被消费并开始处理”的硬证据；malformed item 同理算协议活动。
        if method == "turn/started" || method == "turn/completed"
            || method.hasPrefix("item/") {
            activityRevision &+= 1
        }
        switch method {
        case "turn/started":
            activeTurnId = (params["turn"] as? [String: Any])?["id"] as? String
            turnActive = true
        case "turn/completed":
            turnActive = false; activeTurnId = nil
        case "item/reasoning/summaryTextDelta":
            guard let id = params["itemId"] as? String,
                  let delta = params["delta"] as? String else { return }
            reasoningSummary[id, default: ""] += delta
            upsertReasoning(id: id)
        case "item/reasoning/textDelta":
            guard let id = params["itemId"] as? String,
                  let delta = params["delta"] as? String else { return }
            reasoningContent[id, default: ""] += delta
            upsertReasoning(id: id)
        case "item/reasoning/summaryPartAdded":
            // 新的一段 summary —— 用空行隔开,避免两段思考连成一坨。
            guard let id = params["itemId"] as? String,
                  let cur = reasoningSummary[id], !cur.isEmpty else { return }
            reasoningSummary[id] = cur + "\n\n"
        case "item/completed":
            guard let itemObj = params["item"] as? [String: Any],
                  let data = try? JSONSerialization.data(withJSONObject: itemObj),
                  let decoded = try? JSONDecoder().decode(CodexThreadItem.self, from: data) else { return }
            let item = mergeReasoning(decoded)
            if let idx = items.firstIndex(where: { $0.id == item.id }) { items[idx] = item }
            else { items.append(item) }
        default:
            break   // item/started, 其它 deltas, token usage — v1 不渲染
        }
    }

    /// Upsert a `reasoning` row from the deltas accumulated so far for `id` — the
    /// thinking shows live as it streams (and survives an empty `item/completed`).
    private func upsertReasoning(id: String) {
        let item = CodexThreadItem(id: id, kind: .reasoning(
            summary: reasoningSummary[id], content: reasoningContent[id]))
        if let idx = items.firstIndex(where: { $0.id == id }) { items[idx] = item }
        else { items.append(item) }
    }

    /// For a completed `reasoning` item prefer its own non-empty text, else fall
    /// back to the streamed deltas (ChatGPT-auth reasoning completes empty). Any
    /// other item kind passes through unchanged.
    private func mergeReasoning(_ item: CodexThreadItem) -> CodexThreadItem {
        guard case let .reasoning(s, c) = item.kind else { return item }
        let summary = (s?.isEmpty == false) ? s : reasoningSummary[item.id]
        let content = (c?.isEmpty == false) ? c : reasoningContent[item.id]
        return CodexThreadItem(id: item.id, kind: .reasoning(summary: summary, content: content))
    }
}
#endif
