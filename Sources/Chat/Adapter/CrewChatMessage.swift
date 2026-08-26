import Foundation

// ─────────────────────────────────────────────────────────────────────────────
// PendingCrew seam for the vendored BubbleView family (A3).
//
// `BubbleView` (copied verbatim from PendingBot in A4) has:
//
//   let message: ChatMessage
//
// This file provides:
//   • `CrewChatMessage` — a PendingCrew-local struct that mirrors EXACTLY the
//     members BubbleView reads off `message`, with no PendingBot-specific
//     heavyweight fields (citations storage, arena, recall, store coupling).
//   • `typealias ChatMessage = CrewChatMessage` — the seam that makes
//     BubbleView's `let message: ChatMessage` resolve to our type.
//   • `Attachment` — the value type BubbleView's `AttachmentGrid` iterates.
//   • `MessageCitation` — referenced by BubbleView's `citations: [MessageCitation]`
//     parameter (stub with only the shape BubbleView reads through MarkdownText).
//
// Deviations from PendingBot's ChatMessage are noted inline.
// ─────────────────────────────────────────────────────────────────────────────

/// PendingCrew-local message model. Mirrors the API surface BubbleView reads
/// so the vendored view compiles unchanged.
///
/// Key differences from PendingBot's `ChatMessage`:
/// - No Codable / DB coupling (we map from wire DTO in the adapter layer).
/// - No arena fields (`parent_message_id`, `bubble_group_id`, `model_slug`).
/// - `isMine` is a method matching PendingBot's signature:
///   `func isMine(currentUserId: String?) -> Bool`
///   but it checks a stored `senderUserId` rather than the DB `sender_id`.
/// - Status is a `String?` (same as PendingBot) — "sending" / "sent" / "failed" / nil.
struct CrewChatMessage: Identifiable, Hashable {
    // ── Core identity ─────────────────────────────────────────────────────────

    let id: String
    /// "user" | "bot" | "system"
    let sender_type: String        // intentional snake_case — BubbleView reads `message.sender_type`
    /// Sender's user-id (for "user" rows). BubbleView passes this to `isMine(_:)` logic.
    let sender_id: String

    // ── Content ───────────────────────────────────────────────────────────────

    let content: String
    let attachments: [Attachment]?

    // ── Send-state ────────────────────────────────────────────────────────────

    /// Optimistic-send state. nil = canonical row. "sending" / "sent" / "failed".
    let status: String?

    // ── Derived convenience ───────────────────────────────────────────────────

    /// BubbleView calls `message.isFailed` — mirrors PendingBot's computed var.
    var isFailed: Bool  { status == "failed" }
    /// BubbleView calls `message.isSending` — used by `userBubbleFill`.
    var isSending: Bool { status == "sending" }

    // ── Mine-detection ────────────────────────────────────────────────────────

    /// Single source of truth for mine-ness, set at construction time by
    /// `CrewChatAdapter` via `CrewSenderResolver` — which decides it from the
    /// author uid, not from the display name.
    let mine: Bool

    /// BubbleView calls `message.isMine(currentUserId: currentUserId)`.
    /// Delegates entirely to the stored `mine` flag — `CrewSenderResolver`
    /// is the single source of truth. The `currentUserId` parameter is accepted
    /// for API compatibility with the vendored BubbleView call-site but is not
    /// used here.
    func isMine(currentUserId: String?) -> Bool { mine }

    // ── Memberwise init ───────────────────────────────────────────────────────

    init(
        id: String,
        sender_type: String,
        sender_id: String,
        content: String,
        attachments: [Attachment]? = nil,
        status: String? = nil,
        mine: Bool = false
    ) {
        self.id = id
        self.sender_type = sender_type
        self.sender_id = sender_id
        self.content = content
        self.attachments = attachments
        self.status = status
        self.mine = mine
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Typealias — the seam. BubbleView declares `let message: ChatMessage`;
// with this alias the Swift compiler resolves `ChatMessage` to our local type.
// ─────────────────────────────────────────────────────────────────────────────
typealias ChatMessage = CrewChatMessage

// ─────────────────────────────────────────────────────────────────────────────
// Attachment
//
// BubbleView's AttachmentGrid reads:
//   attachments.filter(\.isImage)   → Attachment.isImage
//   ForEach(imageRows[row]) { att in ServerImage(path: att.url, …) }
//   att.url
//   FileAttachmentChip(attachment: att) → att.filename, att.size, att.mime, att.id
//
// Mirrors PendingBot's Attachment struct exactly (field names, types, semantics).
// ─────────────────────────────────────────────────────────────────────────────
struct Attachment: Identifiable, Hashable {
    let id: String
    let kind: String?
    let mime: String
    let size: Int?
    let width: Int?
    let height: Int?
    /// Relative URL path, e.g. `/v1/uploads/<id>`. Used by ServerImage and
    /// the zoomed ImageViewer.
    let url: String
    /// Original filename — set for non-image files so FileAttachmentChip
    /// can render an icon+name chip. nil for images.
    var filename: String? = nil

    /// True when this attachment should render as an inline image.
    var isImage: Bool { mime.lowercased().hasPrefix("image/") }
}

// ─────────────────────────────────────────────────────────────────────────────
// MessageCitation
//
// BubbleView passes `citations: [MessageCitation]` directly to MarkdownText.
// MarkdownText lives in PendingBot but will be vendored in A5; for now the
// type just needs to exist so BubbleView's declaration compiles.
//
// Deviation: PendingBot's MessageCitation also stores `snippet: String?`.
// We include it for shape-parity; PendingCrew does not currently surface
// web-search results, so it will always arrive as an empty array.
// ─────────────────────────────────────────────────────────────────────────────
struct MessageCitation: Hashable, Identifiable {
    let url: String
    let title: String
    let snippet: String?

    /// Identity is the URL — consistent with PendingBot's definition.
    var id: String { url }
}
