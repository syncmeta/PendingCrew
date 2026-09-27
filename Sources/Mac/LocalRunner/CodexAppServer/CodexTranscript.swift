import Combine
import Foundation

/// Observable transcript built from codex streaming notifications. v1 renders on
/// `item/completed` (authoritative final per item); deltas/item-started are opted out.
@MainActor
final class CodexTranscript: ObservableObject {
    @Published private(set) var items: [CodexThreadItem] = []
    enum InputDelivery: String { case queued, accepted, started, failed }
    @Published private(set) var inputDelivery: [String: InputDelivery] = [:]
    private var pendingInputIDs: [String] = []
    private var pendingAliases: [String: String] = [:]
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
        case "pendingcrew/inputQueued":
            guard let id = params["id"] as? String,
                  let text = params["text"] as? String,
                  !id.isEmpty, !text.isEmpty,
                  !pendingInputIDs.contains(id), pendingAliases[id] == nil else { break }
            if params["source"] as? String == "server",
               let localID = pendingInputIDs.first(where: { candidate in
                   guard candidate.hasPrefix("pendingcrew-client-input-"),
                         !pendingAliases.values.contains(candidate),
                         let item = items.first(where: { $0.id == candidate }),
                         case let .userMessage(value) = item.kind else { return false }
                   return value == text
               }) {
                pendingAliases[id] = localID
                break
            }
            pendingInputIDs.append(id)
            items.append(CodexThreadItem(id: id, kind: .userMessage(text: text)))
            inputDelivery[id] = .queued
        case "pendingcrew/inputAccepted", "pendingcrew/inputFailed":
            let candidates: Set<InputDelivery> = method == "pendingcrew/inputAccepted"
                ? [.queued] : [.queued, .accepted, .started]
            let id = (params["id"] as? String).map { pendingAliases[$0] ?? $0 }
                ?? pendingInputID(matching: params["text"] as? String, states: candidates)
            if let id, inputDelivery[id] != nil {
                inputDelivery[id] = method == "pendingcrew/inputAccepted" ? .accepted : .failed
            }
        case "turn/started":
            activeTurnId = (params["turn"] as? [String: Any])?["id"] as? String
            turnActive = true
            if let id = pendingInputIDs.first(where: { inputDelivery[$0] == .accepted || inputDelivery[$0] == .queued }) {
                inputDelivery[id] = .started
            }
        case "turn/completed":
            let completedId = (params["turn"] as? [String: Any])?["id"] as? String
            // A completion belongs to one turn. An independently scheduled old
            // notification must never clear a newer active turn.
            if let activeTurnId, let completedId, activeTurnId != completedId { break }
            if let turn = params["turn"] as? [String: Any],
               let status = turn["status"] as? String, status != "completed",
               let id = pendingInputIDs.first(where: { inputDelivery[$0] == .started }) {
                inputDelivery[id] = .failed
            }
            turnActive = false; activeTurnId = nil
        case _ where method.hasPrefix("item/"):
            // Item/reasoning/tool activity is first-hand proof of a live turn even
            // if turn/started was delayed or lost by the app-side relay.
            turnActive = true
            applyItem(method: method, params: params)
            return
        default:
            break
        }
        applyItem(method: method, params: params)
    }

    private func applyItem(method: String, params: [String: Any]) {
        switch method {
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
            else if case let .userMessage(text) = item.kind,
                    let matchedIDs = matchingPendingInputs(for: text),
                    let firstID = matchedIDs.first,
                    let idx = items.firstIndex(where: { $0.id == firstID }) {
                items[idx] = item
                for id in matchedIDs {
                    if id != firstID { items.removeAll { $0.id == id } }
                    inputDelivery.removeValue(forKey: id)
                }
                pendingInputIDs.removeAll { matchedIDs.contains($0) }
                pendingAliases = pendingAliases.filter { !matchedIDs.contains($0.value) }
            } else { items.append(item) }
        default:
            break   // item/started, 其它 deltas, token usage — v1 不渲染
        }
    }

    private func pendingInputID(matching text: String?,
                                states: Set<InputDelivery> = [.queued, .accepted, .started]) -> String? {
        pendingInputIDs.first { id in
            guard let state = inputDelivery[id], states.contains(state),
                  let item = items.first(where: { $0.id == id }),
                  case let .userMessage(value) = item.kind else { return false }
            return text == nil || value == text
        }
    }

    private func matchingPendingInputs(for text: String) -> [String]? {
        if let exact = pendingInputID(matching: text) { return [exact] }
        // Native compaction coalesces several queued sends into one turn/start.
        // Its authoritative user item therefore contains the joined input.
        var candidateIDs: [String] = []
        var parts: [String] = []
        for id in pendingInputIDs {
            if inputDelivery[id] == .failed { continue }
            guard let item = items.first(where: { $0.id == id }),
                  case let .userMessage(value) = item.kind else { break }
            candidateIDs.append(id)
            parts.append(value)
            if parts.joined(separator: "\n\n") == text { return candidateIDs }
        }
        return nil
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
