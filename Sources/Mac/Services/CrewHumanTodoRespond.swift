#if os(macOS)
import SwiftUI

/// 人类回应一条**人类 Todo** 的编排（Todo #62 ③）。
///
/// 方向和 `CrewTodoFollowUp` 正好相反：那边是人类追问 agent 的活，这边是 agent
/// 请人拍板、人拍完把答复送回去。三步一步都不能少，顺序由共享剧本
/// `TodoLandingFlow` 说了算（落账 → 发群 → 叫醒），**这里只做 I/O**：
///
///   1. 回应落进 `.human` 那本账（`LocalTodoStore.respond`）；
///   2. 群里发一行「回应 人类 To Do #N：…」，挂 `[broadcast, 提问者]` ——
///      全组看得见（群聊是白板不是私信），只叫醒当初提这件事的那个 session；
///   3. 白板唯一唤醒器叫醒它。提问者已经退出 / 条目没记下是谁提的 / 机长自己提的 → 按
///      `HumanTodoWakePlan` 回落机长转达，**绝不静默丢**。
///
/// 走的是 ③ 那条共用的路，**没有为它另开特殊通道**：mentions 经
/// `LocalBackend.postCrewMessage` 真落盘，可见性/消歧/唤醒读的都是同一份。
@MainActor
enum CrewHumanTodoRespond {

    /// 返回 nil = 三步全成；非 nil = 该显示给人的那句话（措辞出自 `TodoLandingFlow`，
    /// 这里不自己拼）。
    static func perform(crewId: String, item: LocalTodoItem, text: String,
                        runner: CrewSessionRunner, appModel: AppModel) async -> String? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        // 步骤 1：落账。没落上就到此为止 —— 不发群、不叫醒（#577）。
        guard LocalTodoStore.shared(.human).respond(
            crewId: crewId, number: item.number,
            sessionId: Self.humanResponderId, senderName: "人", text: trimmed) != nil
        else {
            return TodoLandingFlow.notPersistedReceipt(ledger: .human, action: .responded)
        }

        let plan = HumanTodoWakePlan.plan(
            createdBySessionId: item.createdBySessionId,
            runningSessionIds: Set(runner.runs.filter { $0.status == .running }.map(\.sessionId)),
            captainSessionId: runner.runs.first {
                $0.crewId == crewId && $0.role == .captain && $0.status == .running
            }?.sessionId)
        let mentions = TodoLandingFlow.mentions(.responded, wake: plan)
        let announce = TodoLedger.human.responseAnnouncement(number: item.number, text: trimmed)
        // 回落原因必须跟着同一条白板消息进 agentText；过去它只塞进 composer 的
        // 第二条直投通道，白板唤醒一旦接管就会丢。现在白板既是真相也是唯一投递源。
        let delivered = [announce, plan.fallbackNote]
            .compactMap { $0 }
            .joined(separator: "\n")

        // 步骤 2：发群。失败**不回滚**已落的账（本地已落是事实），但回执必须如实。
        guard let backend = appModel.backend else {
            return TodoLandingFlow.receipt(
                ledger: .human, action: .responded, number: item.number,
                reached: .persisted, detail: "backend 不可用")
        }
        do {
            try await backend.postCrewMessage(
                crewId: crewId, text: delivered, mentions: mentions,
                replyToId: nil, localAttachments: [])
        } catch {
            return TodoLandingFlow.receipt(
                ledger: .human, action: .responded, number: item.number,
                reached: .persisted, detail: error.localizedDescription)
        }

        // 步骤 3 由 `CrewLocalMentionWaker` 消费上面这条白板 message id：同进程
        // changes 与目录事件即使都到，也只会进入一次 deferred-wake 队列。
        return nil
    }

    /// 人类回应记在 `responses` 里时占的 sessionId 位。渲染看的是 `senderName`
    /// （「人」），这个值只是让那条回应有个稳定的来源标识。
    static let humanResponderId = "human"
}
#endif
