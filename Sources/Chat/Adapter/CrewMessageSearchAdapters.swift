import Foundation

/// 三个搜索入口各自的数据源只在这里做形状转换；字段口径仍由
/// `CrewMessageSearchDocument` / `CrewMessageSearch` 统一定义。
enum CrewMessageSearchAdapters {
    static func local(
        _ message: LocalWhiteboardMessage,
        crewId: String,
        crewTitle: String
    ) -> CrewMessageSearchDocument {
        CrewMessageSearchDocument(
            crewId: crewId,
            crewTitle: crewTitle,
            messageId: message.id,
            text: message.text,
            senderName: localSenderName(message),
            senderId: message.senderSessionId ?? message.senderUserId,
            createdAt: message.createdAt,
            attachmentMetadata: (message.attachments ?? []).flatMap { attachment in
                [attachment.filename.nonEmpty
                    ?? URL(fileURLWithPath: attachment.path).lastPathComponent,
                 attachment.mime]
            })
    }

    static func entry(
        _ message: CrewWhiteboardEntry,
        crewId: String,
        crewTitle: String
    ) -> CrewMessageSearchDocument {
        CrewMessageSearchDocument(
            crewId: crewId,
            crewTitle: crewTitle,
            messageId: message.id,
            text: message.displayText,
            senderName: entrySenderName(message),
            senderId: message.senderSessionId ?? message.senderUserId
                ?? message.senderBotId ?? message.senderMemberId,
            createdAt: message.createdAt,
            attachmentMetadata: (message.attachments ?? []).flatMap { attachment in
                [attachment.filename.nonEmpty
                    ?? URL(string: attachment.url)?.lastPathComponent
                    ?? attachment.url,
                 attachment.mime]
            })
    }

    static func localSenderName(_ message: LocalWhiteboardMessage) -> String {
        if let name = message.senderName.nonEmpty { return name }
        switch message.senderKind {
        case "user": return "人类"
        case "captain": return "机长"
        case "session": return CrewSenderNaming.sessionFallback(message.senderSessionId)
        default: return message.senderKind
        }
    }

    static func entrySenderName(_ message: CrewWhiteboardEntry) -> String {
        if let name = message.senderDisplayName.nonEmpty { return name }
        switch message.senderKind {
        case "user": return "人类"
        case "captain": return "机长"
        case "session": return CrewSenderNaming.sessionFallback(message.senderSessionId)
        default: return message.senderKind
        }
    }
}

private extension Optional where Wrapped == String {
    var nonEmpty: String? {
        guard let self, !self.isEmpty else { return nil }
        return self
    }
}
