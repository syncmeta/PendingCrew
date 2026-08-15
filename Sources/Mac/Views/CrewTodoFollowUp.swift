#if os(macOS)
import SwiftUI

/// Todo **追问**编排（Todo #12 的重开逻辑，Todo #21 一般化成追问）：
/// 数据 → 发群 → 机长感知。概览面板不提供入口（只保留概览），入口在详细窗口，
/// 编排逻辑放这儿共用。
///
/// 为什么追问必须走这条编排、而不是只写盘：人在 Todo 里追问一句，如果只落在本地
/// JSON 上，群里没人看见、机长也不知道 —— 那条追问就石沉大海了。所以三步一个都不能少。
@MainActor
enum CrewTodoFollowUp {
    /// 沿「To do +1」链路：先落数据（追问进时间线；completed 的顺带翻回 pending），
    /// 再以人类身份发白板「Todo #N 追问 / 已重开：…」，最后默认唤醒机长分诊。
    /// 白板发送失败不回滚数据（本地已落是事实），落一条系统注记兜底。
    ///
    /// 返回发出去的那句话；条目不存在（或已删）→ nil（调用方亮错）。
    /// `attachments`（Todo #52）：追问带的图（已由 `CrewLocalAttachmentPersist`
    /// 落进群聊那同一个附件目录）—— 挂到这条追问上、挂到群里那句话上、并把绝对
    /// 路径提示追加进注入给机长的文本里。
    @discardableResult
    static func perform(crewId: String, number: Int, note: String,
                        attachments: [LocalWhiteboardAttachment] = [],
                        runner: CrewSessionRunner, appModel: AppModel) async -> String? {
        // 措辞按追问前的状态定：完成的被追问叫「重开」，其余叫「追问」。
        let wasCompleted = LocalTodoStore.shared
            .item(crewId: crewId, number: number)?.status == "completed"
        guard LocalTodoStore.shared.followUp(
            crewId: crewId, number: number, note: note, attachments: attachments) != nil
        else { return nil }
        let verb = wasCompleted ? "已重开" : "追问"
        let message = note.isEmpty
            ? "Todo #\(number) \(verb)"
            : "Todo #\(number) \(verb)：\(note)"
        if let backend = appModel.backend {
            do {
                try await backend.postCrewMessage(
                    crewId: crewId, text: message, mentions: [],
                    attachmentIds: [], replyToId: nil, localAttachments: attachments)
            } catch {
                // 数据已翻回,只是群里没吱声 —— 落一条系统注记兜底,机长下轮白板注入能看到。
                LocalWhiteboardStore.shared.appendSessionMessage(
                    crewId: crewId, sessionId: "system",
                    text: "\(message)（发群失败：\(error.localizedDescription)，本地兜底注记）",
                    senderName: "系统")
            }
        } else {
            LocalWhiteboardStore.shared.appendSessionMessage(
                crewId: crewId, sessionId: "system",
                text: "\(message)（backend 不可用，本地兜底注记）",
                senderName: "系统")
        }
        wakeCaptain(
            crewId: crewId,
            message: LocalWhiteboardAttachment.appendingAgentHints(to: message, attachments),
            runner: runner, appModel: appModel)
        return message
    }

    /// 机长感知复用 composer / Todo 落地的统一编排：idle 立即直投，busy 留给
    /// runner 在 turn 完成后补投，没在跑则直接拉起。不要在这里再维护一份忙闲门禁。
    private static func wakeCaptain(crewId: String, message: String,
                                    runner: CrewSessionRunner, appModel: AppModel) {
        CrewLocalMentionDelivery.injectAndWake(
            crewId: crewId,
            mentions: [],
            text: message,
            senderName: "人",
            sessionRunner: runner,
            backend: appModel.backend,
            onError: { detail in
                LocalWhiteboardStore.shared.appendSessionMessage(
                    crewId: crewId, sessionId: "system",
                    text: "Todo 追问后自动拉起机长失败：\(detail)。",
                    senderName: "系统")
            })
    }
}
#endif
