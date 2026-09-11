import Foundation

/// 本地白板落盘行 → 中栏渲染认的那个形状（`LocalWhiteboardMessage` →
/// `CrewWhiteboardEntry`）。
///
/// ## 为什么单独抽出来
///
/// 这段映射原本内联在 `PendingCrewBackend.listCrewWhiteboard` 里，而那个文件在
/// 单测 bundle 之外 —— 于是**整条本地渲染链一条断言都没有**。它断掉的症状是最难
/// 发现的那一类：字段在盘上好好的，气泡上什么都不显示，而「漏传了一个字段」和
/// 「这条消息本来就没有那个字段」**长得一模一样**（人类 Todo #132/#133 的引用
/// 胶囊尤其如此 —— 老消息本来就不长胶囊）。
///
/// 抽成纯函数之后它就是一段可以直接喂数据、直接断言的映射。
enum CrewLocalWhiteboardMapping {

    static func entry(_ m: LocalWhiteboardMessage) -> CrewWhiteboardEntry {
        CrewWhiteboardEntry(
            id: m.id,
            senderKind: m.senderKind,
            // session 作者 id 透传(原写死 nil)—— 中栏据此解析 session 名 + 点气泡
            // 跳右栏对应终端 + 回复定位到该 session。user 消息本就 nil(不受影响)。
            senderSessionId: m.senderSessionId,
            senderUserId: m.senderUserId,
            senderBotId: nil,
            messageKind: "instruction",
            summary: m.text,
            createdAt: m.createdAt,
            payload: CrewWhiteboardEntry.Payload(text: m.text),
            // 本地落盘附件 → 同形 CrewAttachment。`url` 用 file:// 绝对 URL，
            // 渲染端（CrewRemoteImage / FileAttachmentChip）据前缀分流本地读取。
            attachments: m.attachments.map { atts in
                atts.map { a in
                    CrewAttachment(
                        id: a.id, kind: nil, mime: a.mime, size: a.size,
                        width: nil, height: nil,
                        url: URL(fileURLWithPath: a.path).absoluteString,
                        filename: a.filename)
                }
            },
            // 发送者名收口在 CrewSenderNaming.localWireDisplayName:relay 远端名要显示,
            // 但本机人类自己发的消息(senderKind=="user")不折本地 senderName("人"),
            // 否则中栏 resolver 的 relay 守卫会把自己误判成 relay → 左对齐(#3)。
            senderDisplayName: CrewSenderNaming.localWireDisplayName(
                senderKind: m.senderKind, localName: m.senderName),
            // 本地白板消息没有成员表行 id —— 恒 nil。
            senderMemberId: nil,
            // #377 — 本地白板消息的回复引用(Phase 6 已加 LocalWhiteboardMessage.inReplyTo)。
            inReplyTo: m.inReplyTo,
            // Task 10 — 本地白板消息的定向 @（Phase 7 落的 LocalWhiteboardMention）
            // 映射回同形 CrewMention，中栏 mention 高亮 / 唤醒判定读同一个形状。
            mentions: m.mentions?.map { CrewMention(kind: $0.kind, targetId: $0.targetId) },
            // #132/#133 — 落盘时就是结构化的，这里原样透传给渲染端。
            references: m.references,
            // #143 — 作者写的那一行结论，原样透传；没写就是 nil，渲染端退回「猜」。
            headline: m.headline)
    }
}
