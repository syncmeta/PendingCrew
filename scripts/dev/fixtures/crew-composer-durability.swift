import Foundation
import Darwin

/// Runs only the actual client draft store in a disposable private directory.
/// Separate write/read invocations prove process lifetime, not a second memory cache.
@main struct ComposerDurabilityProbe {
    @MainActor static func main() throws {
        let mode = CommandLine.arguments[1]
        let root = URL(fileURLWithPath: CommandLine.arguments[2])
        let file = root.appendingPathComponent("drafts.json")
        let namespace = "synthetic-authority/verified-db/local"
        let a = namespace + "/crew-a/topic-a"
        let b = namespace + "/crew-b/topic-b"
        func require(_ value: Bool, _ why: String) throws { if !value { throw Failure.failed(why) } }
        let store = CrewComposerDraftStore(file: file, namespace: namespace)
        let full = CrewComposerDraftStore.Draft(text: "  UNSENT A · synthetic\n原文🙂\n", stagedMentions: [.init(token: "@机长", mention: .captain)], replyTarget: .init(mention: .captain, replyToId: "original-message", quotedSender: "机长", quotedSnippet: "原话"))
        switch mode {
        case "write":
            try require(store.save(full, for: a, crew: "crew-a", channel: "topic-a"), "A save failed")
            try require(store.save(.init(text: "UNSENT B"), for: b, crew: "crew-b", channel: "topic-b"), "B save failed")
        case "read":
            try require(store.storageError == nil && store.draft(for: a) == full, "reopened A text/mentions/reply changed")
            try require(store.draft(for: b).text == "UNSENT B", "reopened B changed")
            try require(store.entries[a]?.crew == "crew-a" && store.entries[a]?.channel == "topic-a", "read-only destination lost")
            try require(store.draft(for: namespace + "/crew-a/topic-b").isEmpty, "topic leak")
            try require(store.draft(for: "other-host/verified-db/local/crew-a/topic-a").isEmpty, "authority leak")
            let other = CrewComposerDraftStore(file: file, namespace: "other-host/verified-db/local")
            let original = try Data(contentsOf: file)
            try require(other.storageError != nil && other.draft(for: a).isEmpty, "mismatched identity not refused")
            try require(!other.save(.init(text: "must not replace original"), for: "other-host/verified-db/local/c/t"), "mismatched overwrite admitted")
            try require(try Data(contentsOf: file) == original, "identity refusal overwrote file")
        case "states":
            let shared = CrewComposerDraftStore.durable(file: file, namespace: namespace)
            let reselected = CrewComposerDraftStore.durable(file: file, namespace: namespace)
            try require(shared === reselected, "reselected authority has two competing writers")
            let oldWindow = shared.snapshot(for: a)
            reselected.save(.init(text: "new window edit"), for: a, crew: "crew-a", channel: "topic-a")
            try require(!shared.clearIfUnchanged(oldWindow) && reselected.draft(for: a).text == "new window edit", "old workspace completion erased new window input")
            let original = store.snapshot(for: a)
            // Failed/Unknown/refused operations have no success callback: preserve verbatim.
            try require(store.draft(for: a) == full, "failed/Unknown input lost")
            store.save(.init(text: "new edit"), for: a, crew: "crew-a", channel: "topic-a")
            try require(!store.clearIfUnchanged(original), "old success erased new edit")
            store.save(full, for: a, crew: "crew-a", channel: "topic-a")
            try require(!store.clearIfUnchanged(original), "ABA erased by old success")
            let now = store.snapshot(for: a)
            try require(store.clearIfUnchanged(now), "confirmed unchanged success not cleared")
            // Actual successful UI reset's subsequent onChange saves an empty
            // whole composer; no old mention/reply may resurrect the sent draft.
            try require(store.save(.init(text: "", stagedMentions: [], replyTarget: nil), for: a, crew: "crew-a", channel: "topic-a"), "empty success onChange failed")
            try require(store.draft(for: a).isEmpty, "success onChange resurrected reply/mention")
            try require(store.draft(for: b).text == "UNSENT B", "successful A erased B")
            let reopened = CrewComposerDraftStore(file: file, namespace: namespace)
            try require(reopened.draft(for: a).isEmpty && reopened.draft(for: b).text == "UNSENT B", "durable clear/other topic failed")
            let priorBytes = try Data(contentsOf: file)
            try FileManager.default.setAttributes([.posixPermissions: 0o500], ofItemAtPath: root.path)
            defer { try? FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: root.path) }
            try require(!store.save(.init(text: "visible unsaved edit"), for: b, crew: "crew-b", channel: "topic-b"), "failed disk write claimed durable")
            try require(store.storageError != nil && store.draft(for: b).text == "visible unsaved edit", "write failure lost memory/error")
            try require(!store.clearIfUnchanged(store.snapshot(for: b)) && store.draft(for: b).text == "visible unsaved edit", "failed clear lost input")
            try require(try Data(contentsOf: file) == priorBytes, "failed write changed old bytes")
        case "damaged":
            let bytes = Data("{broken original".utf8)
            try bytes.write(to: file); try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
            let damaged = CrewComposerDraftStore(file: file, namespace: namespace)
            try require(damaged.storageError != nil, "damaged read silently treated empty")
            try require(!damaged.save(full, for: a), "damaged file overwritten")
            try require(damaged.draft(for: a) == full && (try Data(contentsOf: file)) == bytes, "damaged refusal lost memory/original")
            let future = Data("{\"format\":999,\"namespace\":\"\(namespace)\",\"entries\":{}}".utf8)
            try future.write(to: file)
            let unknown = CrewComposerDraftStore(file: file, namespace: namespace)
            try require(unknown.storageError != nil && !unknown.save(full, for: a), "unknown version overwritten")
            try require(try Data(contentsOf: file) == future, "unknown version bytes changed")
        case "symlink":
            try FileManager.default.removeItem(at: file)
            try FileManager.default.createSymbolicLink(atPath: file.path, withDestinationPath: root.appendingPathComponent("missing-target").path)
            let linked = CrewComposerDraftStore(file: file, namespace: namespace)
            try require(linked.storageError != nil && !linked.save(full, for: a), "dangling symlink overwritten")
            try require(try FileManager.default.destinationOfSymbolicLink(atPath: file.path) == root.appendingPathComponent("missing-target").path, "link changed")
        default: throw Failure.failed("unknown mode")
        }
        print("PASS actual composer store: \(mode); no RPC, Core, App, credential, or automatic submission")
    }
    enum Failure: Error { case failed(String) }
}
