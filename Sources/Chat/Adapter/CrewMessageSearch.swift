import Foundation

/// 群聊搜索三入口（当前群、跨群、session MCP）共用的数据形状。
///
/// 这里故意不依赖 SwiftUI、store 或 MCP：调用方只负责把各自的消息映成 document，
/// 匹配字段、中文行为、时间边界和排序只在 `CrewMessageSearch` 保留一份。
struct CrewMessageSearchDocument: Identifiable, Equatable, Sendable {
    let crewId: String
    let crewTitle: String
    let messageId: String
    let text: String
    let senderName: String
    let senderId: String?
    let createdAt: String
    let attachmentMetadata: [String]

    var id: String { "\(crewId):\(messageId)" }
}

enum CrewMessageSearchField: String, Equatable, Hashable, Sendable {
    case text
    case sender
    case attachment
    case time
}

struct CrewMessageSearchResult: Identifiable, Equatable, Sendable {
    let document: CrewMessageSearchDocument
    let matchedFields: Set<CrewMessageSearchField>

    var id: String { document.id }
}

enum CrewMessageSearchOrder: Sendable {
    case source
    case newestFirst
}

enum CrewMessageSearch {
    static let defaultLimit = 50
    static let maximumLimit = 200

    static func search(
        _ documents: [CrewMessageSearchDocument],
        query: String,
        after: Date? = nil,
        before: Date? = nil,
        limit: Int = defaultLimit,
        order: CrewMessageSearchOrder = .newestFirst
    ) -> [CrewMessageSearchResult] {
        let tokens = query
            .split(whereSeparator: \.isWhitespace)
            .map { normalized(String($0)) }
            .filter { !$0.isEmpty }
        guard !tokens.isEmpty else { return [] }

        let matched = documents.enumerated().compactMap {
            (offset, document) -> (Int, Date?, CrewMessageSearchResult)? in
            let date = parseISO(document.createdAt)
            if after != nil || before != nil {
                guard let date else { return nil }
                if let after, date < after { return nil }
                if let before, date > before { return nil }
            }

            let fields: [(CrewMessageSearchField, String)] = [
                (.text, normalized(document.text)),
                (.sender, normalized([document.senderName, document.senderId]
                    .compactMap { $0 }.joined(separator: "\n"))),
                (.attachment, normalized(document.attachmentMetadata.joined(separator: "\n"))),
                (.time, normalized(document.createdAt)),
            ]
            var hitFields = Set<CrewMessageSearchField>()
            for token in tokens {
                let hits = fields.compactMap { field, haystack in
                    haystack.contains(token) ? field : nil
                }
                guard !hits.isEmpty else { return nil }
                hitFields.formUnion(hits)
            }
            return (offset, date, CrewMessageSearchResult(
                document: document, matchedFields: hitFields))
        }

        let ordered: [(Int, Date?, CrewMessageSearchResult)]
        switch order {
        case .source:
            ordered = matched
        case .newestFirst:
            ordered = matched.sorted { lhs, rhs in
                switch (lhs.1, rhs.1) {
                case let (left?, right?) where left != right: return left > right
                default: return lhs.0 < rhs.0
                }
            }
        }
        let boundedLimit = min(maximumLimit, max(1, limit))
        return ordered.prefix(boundedLimit).map(\.2)
    }

    /// Unicode 字符串统一做大小写、音标与全/半角折叠；中文本身不分词，直接子串匹配。
    static func normalized(_ value: String) -> String {
        value.folding(
            options: [.caseInsensitive, .diacriticInsensitive, .widthInsensitive],
            locale: Locale(identifier: "en_US_POSIX"))
    }

    /// 搜索/API 的时间都收标准 ISO8601；带或不带小数秒都接受。
    static func parseISO(_ value: String) -> Date? {
        let plain = ISO8601DateFormatter()
        if let date = plain.date(from: value) { return date }
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fractional.date(from: value)
    }
}
