import Foundation

/// 托盘里的待发附件 → 落盘条目（`<attachments>/<crewId>/`）的**唯一**一条路。
///
/// 原本只长在 `CrewChatView.send` 里；Todo #52 让新建 Todo 与追问也能附图之后
/// 就有三个调用方了，抽出来是为了「图存哪、按什么规则命名、原名留不留」只有一份
/// 答案 —— 三条入口的图必须落在与群聊同一个目录、同一套通道里。
///
/// 两条来源分流照旧（见 `PendingComposerAttachment`）：
/// - `.staged`（拖入 / 文件选择器）→ `save(fileAt:)` 把暂存那份**搬**过去，
///   整份字节不进内存；
/// - `.data`（⌘V 位图 / 相册 / 拍照）→ `save(data:)`，字节本来就在内存里。
///
/// 只依赖 Foundation，跨平台可编 —— 但真正调它的是 macOS 本地落盘那条路
/// （iOS 走 edge `/v1/upload`，不落本机）。
enum CrewLocalAttachmentPersist {

    /// 一份附件没落成盘。附件是人要给 agent 看的东西，**不许静默少一张** ——
    /// 调用方据此中止本次发送并亮软报错。
    struct Failure: LocalizedError {
        let filename: String
        var errorDescription: String? { "附件保存失败：\(filename)" }
    }

    /// 逐个落盘，顺序与传入一致。任一失败即抛（已落盘的那几份留在目录里，
    /// 只是没被任何消息/条目引用 —— 比发出去一条缺图的消息好）。
    static func persist(
        _ items: [PendingComposerAttachment],
        crewId: String,
        root: URL = CrewChatAttachmentStore.defaultDirectory
    ) throws -> [LocalWhiteboardAttachment] {
        try items.map { att in
            // 图不记原名（气泡直接渲染缩略图，chip 文件名反而碍眼）；文件留原名。
            let name = att.isImage ? nil : att.filename
            let saved: LocalWhiteboardAttachment?
            if let staged = att.stagedURL {
                saved = CrewChatAttachmentStore.save(
                    fileAt: staged, mime: att.mime, filename: name, crewId: crewId, root: root)
            } else if let data = att.inMemoryData {
                saved = CrewChatAttachmentStore.save(
                    data: data, mime: att.mime, filename: name, crewId: crewId, root: root)
            } else {
                saved = nil
            }
            guard let saved else { throw Failure(filename: att.filename) }
            return saved
        }
    }

    /// 发完清场：暂存区里那些空壳目录（`.staged` 走 move 后文件已经搬走）收掉。
    /// `staging` 与 intake 时用的那个暂存根必须一致 —— `discardStaged` 只删这个根
    /// 底下的东西（别让它变成「能删任意路径」的口子），单测传临时目录。
    static func discardStaging(
        _ items: [PendingComposerAttachment],
        staging: URL = CrewChatAttachmentStore.stagingDirectory
    ) {
        for att in items {
            if let staged = att.stagedURL {
                CrewChatAttachmentStore.discardStaged(staged, staging: staging)
            }
        }
    }
}
