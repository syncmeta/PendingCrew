#if os(macOS)
import Foundation

/// Todo 落账编排（Task 10：从 `CrewChatView.sendTodo` 抽出的自由函数）：
/// `LocalTodoStore.add` 拿 #N → 群里发「To do +1: #N …」回执（走 `backend`，
/// macOS 上恒为 `LocalBackend`，落本地白板）→ 唤醒机长
/// （`CrewLocalMentionDelivery`，无定向 @ 默认给机长，与 sendTodo 原行为一致）。
///
/// 抽出来时它有两条来路（composer 直发 + relay 落地远端 iOS 的 `crew_todo_add`）；
/// 后者随 #63 第二期删除跨端遥控整层一起去掉了，现在只剩 composer 这一条。
///
/// 幂等由调用方负责：composer 每次点发送都是新增一条。
@MainActor
enum CrewLocalTodoLanding {
    /// `onError` surfaces failures from the (async, fire-and-forget) absent-target
    /// wake-up inside `injectAndWake` — the initial `postCrewMessage` receipt post
    /// still `throw`s synchronously to the caller's own `do`/`catch` (composer's
    /// `sendTodo` relies on that to skip clearing `draft` on failure), so this
    /// closure only covers the errors that would otherwise be silently dropped.
    ///
    /// `attachments`（Todo #52）：条目带的图/文件（已由
    /// `CrewLocalAttachmentPersist` 落进群聊那同一个附件目录）。三处都要带上它 ——
    /// 条目自己记一份（面板/详细窗口渲染缩略图）、群里那条「To do +1」挂一份
    /// （人在群里就看得见图）、注入给机长的文本追加绝对路径提示行（机器人才知道
    /// 去 Read）。
    @discardableResult
    static func land(
        crewId: String,
        text: String,
        attachments: [LocalWhiteboardAttachment] = [],
        backend: PendingCrewBackend,
        sessionRunner: CrewSessionRunner,
        onError: ((String) -> Void)? = nil
    ) async throws -> LocalTodoItem {
        // 没落盘就别去群里宣布（#577）：`add` 返回 nil = Todo 列表文件读不出来 /
        // 漏读，条目根本没写进去。此前照发「To do +1: #N」，人以为记下了，其实没有。
        guard let item = LocalTodoStore.shared.add(
            crewId: crewId, text: text, attachments: attachments) else {
            throw TodoLandingError.notPersisted
        }
        let message = "To do +1: #\(item.number) \(text)"
        try await backend.postCrewMessage(
            crewId: crewId, text: message, mentions: [],
            replyToId: nil, localAttachments: attachments)
        CrewLocalMentionDelivery.injectAndWake(
            crewId: crewId,
            mentions: [],
            text: LocalWhiteboardAttachment.appendingAgentHints(to: message, attachments),
            senderName: "人",
            sessionRunner: sessionRunner, backend: backend, onError: onError)
        return item
    }

    enum TodoLandingError: LocalizedError {
        case notPersisted

        var errorDescription: String? {
            "Todo 没能写进列表文件（这次读不出来或漏读，原有内容没被动过，"
                + "群聊白板上有系统警示），这条待办没有记下，请重试。"
        }
    }
}
#endif
