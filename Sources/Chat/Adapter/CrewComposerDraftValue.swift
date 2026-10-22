import Foundation

// Original value-only composer DTOs, shared unchanged by App, local draft
// persistence and the personal sync actor. No renderer or sender lookup.

struct CrewMention: Codable, Equatable, Sendable {
    let kind: String          // 'session' | 'captain' | 'broadcast' | 'human'
    let targetId: String?

    enum CodingKeys: String, CodingKey {
        case kind
        case targetId = "target_id"
    }

    static func session(_ id: String) -> CrewMention { .init(kind: "session", targetId: id) }
    static let captain = CrewMention(kind: "captain", targetId: nil)
    static let broadcast = CrewMention(kind: "broadcast", targetId: nil)
}

struct CrewStagedMention: Codable, Equatable, Sendable {
    let token: String        // e.g. "@小绿" (no trailing space)
    let mention: CrewMention
}

struct CrewReplyTarget: Codable, Equatable, Sendable {
    /// nil when the original sender can't be turned into a mention target
    /// (e.g. an unknown / relay sender with no resolvable id) — the reply still
    /// carries `replyToId`, it just doesn't auto-@.
    let mention: CrewMention?
    /// The id of the message being replied to (→ `reply_to` on the wire).
    let replyToId: String
    /// Display name of the original sender (banner).
    let quotedSender: String
    /// Short snippet of the replied-to message (banner).
    let quotedSnippet: String

    /// Build a staged mention for the auto-@, if any.
    var staged: CrewStagedMention? {
        guard let mention else { return nil }
        return CrewStagedMention(token: "@\(quotedSender)", mention: mention)
    }
}

extension CrewComposerDraftStore {
    struct Draft: Codable, Equatable, Sendable {
        var text: String = ""
        var stagedMentions: [CrewStagedMention] = []
        var replyTarget: CrewReplyTarget?
        var isEmpty: Bool {
            text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                && stagedMentions.isEmpty && replyTarget == nil
        }
    }
}
