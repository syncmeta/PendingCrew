import Foundation

/// Window-local drafts. Keys include the destination, never whichever picker is current
/// when a request eventually completes. This model does not own or send messages.
struct CrewdDrafts {
    struct Context: Hashable {
        enum Kind: Hashable { case message, initialTask }
        let kind: Kind
        let crew: String
        let channel: String
        let session: String
        static func message(crew: String, channel: String, session: String) -> Self {
            Self(kind: .message, crew: crew, channel: channel, session: session)
        }
        static func initialTask(crew: String, session: String) -> Self {
            Self(kind: .initialTask, crew: crew, channel: "", session: session)
        }
    }
    struct Snapshot: Equatable {
        let context: Context
        let text: String
        fileprivate let revision: UUID?
    }
    private struct Entry {
        var text: String
        var revision: UUID
    }
    private var entries: [Context: Entry] = [:]
    var allSnapshots:[Snapshot] {entries.map{Snapshot(context:$0.key,text:$0.value.text,revision:$0.value.revision)}.filter{!$0.text.isEmpty}}
    func text(for context: Context) -> String { entries[context]?.text ?? "" }
    mutating func set(_ text: String, for context: Context) {
        guard self.text(for: context) != text else { return }
        // Keep an empty entry's revision: an edit away and back must not be mistaken
        // for the exact draft that started an older request.
        entries[context] = Entry(text: text, revision: UUID())
    }
    func snapshot(for context: Context) -> Snapshot {
        Snapshot(context: context, text: text(for: context), revision: entries[context]?.revision)
    }
    @discardableResult mutating func clearIfUnchanged(_ snapshot: Snapshot) -> Bool {
        guard let entry = entries[snapshot.context], entry.text == snapshot.text,
              entry.revision == snapshot.revision else { return false }
        entries.removeValue(forKey: snapshot.context)
        return true
    }
}

/// New-chat input belongs to its original authority; no backend action until Send.
struct PendingCrewDraft: Equatable {
    let id: String
    var title: String
    var parentCrewId: String?
    var workingDirectory: String?
    var executor: String = "codex"
    var firstMessage: String = ""
}
