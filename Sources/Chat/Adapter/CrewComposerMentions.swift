// Pure-logic core for the crew composer's @-mention support (Phase 6).
//
// Foundation-only so it compiles into the PendingCrewTests bundle (no
// `@testable import`; the source is listed directly in the test target) and
// stays cross-platform (macOS + iOS composer share it).
//
// Three concerns live here, all side-effect-free:
//   1. `CrewMentionCandidate` + `crewMentionCandidates(...)` — roster → the
//      pick list shown in the @ autocomplete popover, with prefix filtering.
//   2. `CrewComposerMentionParser` — detect an in-progress `@<prefix>` token at
//      the caret in the draft, and the token-insertion / staged-list bookkeeping
//      (insert a readable `@name ` token + record one `CrewMention`; drop a
//      token → drop its staged mention).
//   3. `CrewReplyTarget` — derive the auto-@ mention for "reply to this message"
//      from a whiteboard entry's sender.
//
// The UI layer (`CrewChatView`) drives this from the `$draft` binding via
// `.onChange` + a popover overlay — it never forks the vendored `ComposerView`.
import Foundation

// MARK: - Candidate model

/// One row in the @-mention autocomplete popover. `mention` is what gets staged
/// when picked; `token` is the readable text inserted into the draft (e.g.
/// `@小绿`). `id` is stable for SwiftUI `ForEach`.
struct CrewMentionCandidate: Identifiable, Equatable {
    enum Kind: Equatable { case session, captain, broadcast, human }

    let id: String
    let kind: Kind
    /// Display label shown in the popover row (e.g. "小绿", "全体", "机长").
    let label: String
    /// The `CrewMention` staged when this candidate is picked.
    let mention: CrewMention
    /// 头像 seed(captain=captainBotId、session=sessionId、human=userId)——
    /// picker 行用真头像与成员列表/气泡一致;nil(broadcast)退回 glyph 图标。
    var avatarSeed: String? = nil

    /// Readable token inserted into the draft for this candidate. Always
    /// `@<label>` — the trailing space is added by the parser at insert time.
    var token: String { "@\(label)" }
}

/// Build the @-mention candidate list from a crew roster.
///
/// Order (stable, matches how people think about a crew): broadcast first
/// (the common "tell everyone"), then captain, then each session member, then
/// human members. `prefix` filters by case-insensitive substring of the label
/// (empty prefix → all). `selfMemberId` drops the local human from the list
/// (you don't @ yourself).
///
/// - `broadcastLabel` / `captainLabel` are injected so the caller controls copy
///   (kept out of this Foundation core's hardcoded strings minimally).
func crewMentionCandidates(
    members: [CrewMember],
    captainBotId: String?,
    prefix: String,
    selfMemberId: String?,
    broadcastLabel: String = "全体",
    captainLabel: String = "机长"
) -> [CrewMentionCandidate] {
    var out: [CrewMentionCandidate] = []

    // Broadcast — fan out to everyone. Always offered.
    out.append(CrewMentionCandidate(
        id: "broadcast", kind: .broadcast, label: broadcastLabel, mention: .broadcast))

    // Captain — only if the crew has one.
    if let capId = captainBotId {
        out.append(CrewMentionCandidate(
            id: "captain", kind: .captain, label: captainLabel, mention: .captain,
            avatarSeed: capId))
    }

    // Session members — each routes to that session's mailbox.
    for m in members where m.memberKind == "code_session" {
        guard let sid = m.codeSessionId else { continue }
        let label = m.displayName ?? CrewSenderNaming.sessionFallback(sid)
        out.append(CrewMentionCandidate(
            id: "session:\(sid)", kind: .session, label: label,
            mention: .session(sid), avatarSeed: sid))
    }

    // Human members — `{kind:"human", target_id:userId}`. Skip self.
    for m in members where m.memberKind == "human" {
        if let sid = selfMemberId, m.id == sid { continue }
        guard let uid = m.userId else { continue }
        let label = m.displayName ?? "成员"
        out.append(CrewMentionCandidate(
            id: "human:\(uid)", kind: .human, label: label,
            mention: CrewMention(kind: "human", targetId: uid), avatarSeed: uid))
    }

    let needle = prefix.trimmingCharacters(in: .whitespaces)
    guard !needle.isEmpty else { return out }
    return out.filter { $0.label.range(of: needle, options: .caseInsensitive) != nil }
}

// MARK: - In-progress @-token detection + staged bookkeeping

/// One staged mention paired with the readable token currently sitting in the
/// draft. The pairing lets us reconcile: if the user deletes the token text,
/// the staged mention is dropped (see `reconcile`).
struct CrewStagedMention: Equatable {
    let token: String        // e.g. "@小绿" (no trailing space)
    let mention: CrewMention
}

/// Caret-anchored `@<prefix>` detection + token insertion + staged-list
/// reconciliation. All pure string math — no SwiftUI, no caret object (we
/// operate on "the trailing in-progress token", which is what an inline
/// composer popover needs without a UITextView selection range).
enum CrewComposerMentionParser {

    /// The active `@<prefix>` the user is mid-typing at the *end* of the draft,
    /// or nil if there's no open mention token to complete.
    ///
    /// We only treat a trailing run as an open token (the caret is at the end in
    /// the common typing flow). An `@` opens a token; it closes on whitespace.
    /// So `"hi @gr"` → prefix `"gr"`; `"hi @gr "` → nil (closed by the space);
    /// `"hi@gr"` → nil (no boundary before `@`).
    struct ActiveQuery: Equatable {
        /// The prefix typed after `@` (may be empty right after typing `@`).
        let prefix: String
        /// Offset of the `@` in the draft (UTF-16-agnostic String.Index distance
        /// from startIndex), so the caller can replace `@prefix` on pick.
        let atOffset: Int
    }

    /// Detect an open `@`-token at the tail of `draft`. Returns nil when the
    /// last token is already closed (trailing space) or there's no `@` to
    /// complete.
    static func activeQuery(in draft: String) -> ActiveQuery? {
        guard let atRange = draft.range(of: "@", options: .backwards) else { return nil }
        let afterAt = draft[atRange.upperBound...]
        // A space (or newline) after the `@…` means the token is closed.
        if afterAt.rangeOfCharacter(from: .whitespacesAndNewlines) != nil { return nil }
        // The `@` must start the draft or follow whitespace — not be mid-word
        // (an email "a@b" is not a mention).
        if atRange.lowerBound > draft.startIndex {
            let before = draft[draft.index(before: atRange.lowerBound)]
            if !before.isWhitespace { return nil }
        }
        let offset = draft.distance(from: draft.startIndex, to: atRange.lowerBound)
        return ActiveQuery(prefix: String(afterAt), atOffset: offset)
    }

    /// Result of picking a candidate: the new draft text (with the in-progress
    /// `@prefix` replaced by `@label `) and the staged mention to record.
    struct Insertion: Equatable {
        let newDraft: String
        let staged: CrewStagedMention
    }

    /// Replace the open `@<prefix>` token at the tail of `draft` with the
    /// candidate's `@label ` (trailing space so the user keeps typing after it),
    /// and produce the `CrewStagedMention` to add. Returns nil if there's no
    /// open token to replace (defensive — caller only calls this while a query
    /// is active).
    static func insert(
        candidate: CrewMentionCandidate, into draft: String
    ) -> Insertion? {
        guard let q = activeQuery(in: draft) else { return nil }
        let atIndex = draft.index(draft.startIndex, offsetBy: q.atOffset)
        let head = String(draft[draft.startIndex..<atIndex])
        let newDraft = head + candidate.token + " "
        return Insertion(
            newDraft: newDraft,
            staged: CrewStagedMention(token: candidate.token, mention: candidate.mention))
    }

    /// Drop staged mentions whose token text no longer appears in the draft.
    ///
    /// The composer has no rich text — a mention "chip" is just the literal
    /// `@label` substring. If the user backspaces through it (or edits it away),
    /// the staged mention must follow. We keep a staged mention iff its exact
    /// token substring is still present. Duplicate tokens (two `@小绿`) are
    /// matched by count so deleting one of two drops exactly one staged entry.
    static func reconcile(staged: [CrewStagedMention], draft: String) -> [CrewStagedMention] {
        var remainingDraft = draft
        var kept: [CrewStagedMention] = []
        for s in staged {
            if let r = remainingDraft.range(of: s.token) {
                kept.append(s)
                // Consume this occurrence so a second identical token must find
                // its own occurrence further along.
                remainingDraft.removeSubrange(r)
            }
        }
        return kept
    }

    /// Append a mention to the tail of `draft` **without** replacing an open
    /// `@token` (that's `insert`'s job at the caret). Used by the "右键头像 → @"
    /// path where there's no in-progress token — the user picked a sender from
    /// their avatar, not by typing. Inserts a single leading space when the
    /// draft is non-empty and doesn't already end with whitespace, then
    /// `@label ` (trailing space so typing continues cleanly). Dedups by mention
    /// target: if that exact target is already staged, it's a no-op (draft
    /// still reclaimed) so hammering the menu doesn't pile up `@X @X @X`.
    ///
    /// Two guards keep this from producing a stray double-`@`:
    ///  1. **Reclaim a half-open `@<prefix>`** — the user may have typed `@`
    ///     (opening the picker) and then reached for the avatar instead. That
    ///     trailing `@`/`@gr` must be dropped before appending, else the result
    ///     reads `@ @Name` / `@gr @Name`.
    ///  2. **Normalize the token to a single leading `@`** — an author whose
    ///     display name itself carries a leading `@` would otherwise make the
    ///     token `@@Name`.
    static func appendMention(
        _ staged: CrewStagedMention, to draft: String, existing: [CrewStagedMention]
    ) -> (newDraft: String, staged: [CrewStagedMention]) {
        let normalizedToken = "@" + staged.token.drop(while: { $0 == "@" })
        let normalized = CrewStagedMention(token: normalizedToken, mention: staged.mention)
        // Reclaim any half-open `@<prefix>` at the tail before we touch the draft
        // (do this even on the dedup no-op so the stray `@` never lingers).
        var base = draft
        if let q = activeQuery(in: base) {
            let atIndex = base.index(base.startIndex, offsetBy: q.atOffset)
            base = String(base[base.startIndex..<atIndex])
        }
        guard !existing.contains(where: { $0.mention == normalized.mention }) else {
            return (base, existing)
        }
        let sep = (base.isEmpty || base.last?.isWhitespace == true) ? "" : " "
        return (base + sep + normalizedToken + " ", existing + [normalized])
    }

    /// Collapse a staged list into the de-duplicated `[CrewMention]` to send.
    /// Same logical target staged twice (e.g. via reply auto-@ + manual @) is
    /// sent once.
    static func mentionsToSend(_ staged: [CrewStagedMention]) -> [CrewMention] {
        var seen: [CrewMention] = []
        for s in staged where !seen.contains(s.mention) {
            seen.append(s.mention)
        }
        return seen
    }
}

// MARK: - Reply-target derivation

/// The mention + quoted-reference a "reply to this message" produces. `mention`
/// auto-@s the original sender; `quoted*` drive the composer's reply banner.
struct CrewReplyTarget: Equatable {
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

enum CrewReplyTargetBuilder {

    /// Max length of the quoted snippet shown in the reply banner.
    static let snippetLimit = 80

    /// Derive the reply target from a whiteboard entry + the resolved sender.
    ///
    /// - `entry`: the message being replied to.
    /// - `senderName`: the display name (from `CrewSenderResolver`) — drives the
    ///   banner and the staged token label.
    ///
    /// Mention routing by the entry's `senderKind`:
    ///   * session → `.session(senderSessionId)` (route back to that run).
    ///   * user/human → `{kind:"human", target_id: senderUserId}` (if known).
    ///   * bot/captain/other → no auto-@ (bots aren't @-routable as a reply
    ///     target in this path; reply still records `reply_to`).
    static func make(entry: CrewWhiteboardEntry, senderName: String) -> CrewReplyTarget {
        let mention: CrewMention?
        switch entry.senderKind {
        case "session":
            mention = entry.senderSessionId.map { CrewMention.session($0) }
        case "captain":
            // 机长 run 的发言（本地 senderKind "captain"）：回复即 @机长 —— 走
            // .captain 让唤醒端解析到当前 captain run，比钉死发消息时的 sessionId
            // 更稳（机长重启后 id 会换）。
            mention = .captain
        case "user", "human":
            mention = entry.senderUserId.map { CrewMention(kind: "human", targetId: $0) }
        default:
            mention = nil
        }
        return CrewReplyTarget(
            mention: mention,
            replyToId: entry.id,
            quotedSender: senderName,
            quotedSnippet: snippet(entry.displayText)
        )
    }

    /// Single-line, length-capped snippet for the reply banner.
    static func snippet(_ text: String) -> String {
        let oneLine = text
            .replacingOccurrences(of: "\n", with: " ")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if oneLine.count <= snippetLimit { return oneLine }
        return String(oneLine.prefix(snippetLimit)) + "…"
    }
}

// MARK: - Reply-reference resolution (#377)

/// The quoted reference rendered inside/above a bubble that replies to another
/// message. `senderName` + `snippet` come from the referenced message (resolved
/// locally from the already-loaded entries — no server join). `found == false`
/// degrades to a generic "回复了一条消息" label when the replied-to message is
/// outside the loaded window.
struct CrewReplyReference: Equatable {
    let senderName: String
    let snippet: String
    /// false when the replied-to id wasn't in the loaded entries (degraded).
    let found: Bool

    /// Generic placeholder shown when the referenced message isn't loaded.
    static let notLoaded = CrewReplyReference(
        senderName: "", snippet: "回复了一条消息", found: false)
}

enum CrewReplyReferenceResolver {

    /// Max length of the quoted snippet shown in a reply bubble's quote strip.
    static let snippetLimit = 64

    /// Resolve the reply reference for an entry against the loaded entries.
    ///
    /// - `entry`:    the (replying) entry — its `inReplyTo` points at the
    ///               quoted message. nil `inReplyTo` ⇒ returns nil (not a reply).
    /// - `entries`:  the full loaded whiteboard window to search by id.
    /// - `members` / `captainBotId` / `localUserId`: passed through to
    ///   `CrewSenderResolver` so the quoted sender shows its display name.
    ///
    /// Returns nil when `entry` isn't a reply. Returns `CrewReplyReference`
    /// (with `found == false`) when it IS a reply but the target isn't loaded,
    /// so the UI shows a non-crashing placeholder instead of nothing.
    static func resolve(
        for entry: CrewWhiteboardEntry,
        in entries: [CrewWhiteboardEntry],
        members: [CrewMember],
        captainBotId: String?,
        localUserId: String?
    ) -> CrewReplyReference? {
        guard let targetId = entry.inReplyTo else { return nil }
        guard let referenced = entries.first(where: { $0.id == targetId }) else {
            return .notLoaded
        }
        let sender = CrewSenderResolver.resolve(
            referenced, members: members, captainBotId: captainBotId, localUserId: localUserId)
        return CrewReplyReference(
            senderName: sender.displayName,
            snippet: snippet(referenced.displayText),
            found: true)
    }

    /// Single-line, length-capped snippet for the in-bubble quote strip.
    static func snippet(_ text: String) -> String {
        let oneLine = text
            .replacingOccurrences(of: "\n", with: " ")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if oneLine.count <= snippetLimit { return oneLine }
        return String(oneLine.prefix(snippetLimit)) + "…"
    }
}
