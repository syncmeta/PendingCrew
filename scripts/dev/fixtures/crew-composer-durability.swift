import Foundation
import Darwin
import CryptoKit

/// Runs only the actual client draft store in a disposable private directory.
/// Separate write/read invocations prove process lifetime, not a second memory cache.
@main struct ComposerDurabilityProbe {
    @MainActor static func main() throws {
        let mode = CommandLine.arguments[1]
        let root = URL(fileURLWithPath: CommandLine.arguments[2])
        if mode.hasPrefix("offline-") { try offline(mode, root: root); return }
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
    @MainActor private static func offline(_ mode: String, root: URL) throws {
        let fm = FileManager.default
        let record = "00000000-0000-4000-8000-000000000001"
        // Slash-bearing opaque dbID must remain intact, never split into fields.
        let namespace = record.uppercased() + "/00000000-0000-4000-8000-000000000002/opaque/db/1"
        let a = namespace + "/crew-a/topic-a", b = namespace + "/crew-b/topic-b"
        let textA = "  A\n" + String(repeating: "a", count: 1024 * 1024) + "\n原文🙂\n"
        let textB = " B · 第二份未发送输入\n"
        func filename(_ ns: String) -> String { "composer-drafts-" + SHA256.hash(data: Data(ns.utf8)).map { String(format: "%02x", $0) }.joined() + ".json" }
        func directory(_ data: URL, _ id: String = record) throws -> URL {
            let path = data.appendingPathComponent("frontend-presentation/unknown-submissions/" + id)
            try fm.createDirectory(at: path, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            return path
        }
        func check(_ value: Bool, _ why: String) throws { if !value { throw Failure.failed(why) } }
        func write(_ data: Data, _ file: URL) throws {
            try data.write(to: file)
            try fm.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
        }
        func refused(_ data: URL, containing: String? = nil) throws {
            let result = CrewOfflineComposerDrafts.read(dataDirectory: data)
            try check(!result.complete && !result.issues.isEmpty, "unsafe/partial read claimed complete empty")
            try check(result.issues.allSatisfy { $0.hasPrefix("未完整读取，原文件保留：") }, "partial status missing")
            if let containing { try check(result.issues.contains { $0.contains(containing) }, "wrong refusal") }
        }
        let parent = root.appendingPathComponent("frontend-presentation/unknown-submissions/" + record)
        let file = parent.appendingPathComponent(filename(namespace))
        switch mode {
        case "offline-write":
            _ = try directory(root)
            let store = CrewComposerDraftStore(file: file, namespace: namespace)
            try check(store.save(.init(text: textA, stagedMentions: [.init(token: "@机长", mention: .captain)], replyTarget: .init(mention: .captain, replyToId: "old-message", quotedSender: "机长", quotedSnippet: "原引用")), for: a, crew: "crew-a", channel: "topic-a"), "writer A failed")
            try check(store.save(.init(text: textB), for: b, crew: "crew-b", channel: "topic-b"), "writer B failed")
        case "offline-read":
            let before = try Data(contentsOf: file)
            let result = CrewOfflineComposerDrafts.read(dataDirectory: root)
            try check(result.complete && result.items.count == 2, "cross-process offline A/B not found")
            try check(result.items.first { $0.key == a }?.text == textA && result.items.first { $0.key == b }?.text == textB, "offline full bytes changed")
            try check(result.items.first { $0.key == a }?.mentionTokens == ["@机长"] && result.items.first { $0.key == a }?.reply?.contains("原引用") == true, "stored context lost")
            try check(result.items.allSatisfy { $0.record == record && $0.namespace == namespace }, "opaque identity changed")
            try check(try Data(contentsOf: file) == before, "offline read modified original")
            let missing = root.appendingPathComponent("never-created")
            let empty = CrewOfflineComposerDrafts.read(dataDirectory: missing)
            try check(empty.complete && empty.missing && empty.items.isEmpty && !fm.fileExists(atPath: missing.path), "missing read created directory")
        case "offline-negative":
            let original = try Data(contentsOf: file)
            let wrongHash = parent.appendingPathComponent("composer-drafts-" + String(repeating: "0", count: 64) + ".json")
            try fm.moveItem(at: file, to: wrongHash); try refused(root)
            try check(try Data(contentsOf: wrongHash) == original, "bad hash file changed")
            try fm.moveItem(at: wrongHash, to: file)
            try fm.setAttributes([.posixPermissions: 0o644], ofItemAtPath: file.path); try refused(root)
            try check(try Data(contentsOf: file) == original, "unsafe-mode file changed")
            try fm.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
            try fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: parent.path); try refused(root)
            try check(try Data(contentsOf: file) == original, "unsafe directory changed original")
            try fm.setAttributes([.posixPermissions: 0o700], ofItemAtPath: parent.path)
            let linkedRoot = root.appendingPathComponent("linked-data")
            try fm.createSymbolicLink(atPath: linkedRoot.path, withDestinationPath: root.path); try refused(linkedRoot)
            try fm.removeItem(at: linkedRoot)
            for relative in ["frontend-presentation", "frontend-presentation/unknown-submissions"] {
                let ancestor = root.appendingPathComponent(relative)
                let keptAncestor = root.appendingPathComponent("preserved-ancestor")
                try fm.moveItem(at: ancestor, to: keptAncestor)
                try fm.createSymbolicLink(atPath: ancestor.path, withDestinationPath: keptAncestor.path)
                try refused(root)
                let relativeFile = relative == "frontend-presentation" ? "unknown-submissions/" + record + "/" + filename(namespace) : record + "/" + filename(namespace)
                try check(try Data(contentsOf: keptAncestor.appendingPathComponent(relativeFile)) == original, "intermediate symlink touched original")
                try fm.removeItem(at: ancestor); try fm.moveItem(at: keptAncestor, to: ancestor)
            }
            let foreignNamespace = namespace.replacingOccurrences(of: record.uppercased(), with: "00000000-0000-4000-8000-000000000099")
            var foreign = try JSONSerialization.jsonObject(with: original) as! [String: Any]
            foreign["namespace"] = foreignNamespace
            let originalEntries = foreign["entries"] as! [String: Any]
            foreign["entries"] = Dictionary(uniqueKeysWithValues: originalEntries.map { ($0.key.replacingOccurrences(of: namespace, with: foreignNamespace), $0.value) })
            let foreignFile = parent.appendingPathComponent(filename(foreignNamespace))
            let foreignBytes = try JSONSerialization.data(withJSONObject: foreign)
            try write(foreignBytes, foreignFile); try refused(root)
            try check(try Data(contentsOf: foreignFile) == foreignBytes, "wrong parent changed original")
            try fm.removeItem(at: foreignFile)
            let preserved = parent.appendingPathComponent("preserved-original")
            try fm.moveItem(at: file, to: preserved)
            try fm.createSymbolicLink(atPath: file.path, withDestinationPath: preserved.path); try refused(root)
            try check(try Data(contentsOf: preserved) == original, "symlink target changed")
            try fm.removeItem(at: file); try fm.moveItem(at: preserved, to: file)
            let hardLink = parent.appendingPathComponent("preserved-hardlink")
            try fm.linkItem(at: file, to: hardLink); try refused(root)
            try check(try Data(contentsOf: hardLink) == original, "hardlink original changed")
            try fm.removeItem(at: hardLink)
            let keptDirectory = root.appendingPathComponent("preserved-record")
            try fm.moveItem(at: parent, to: keptDirectory)
            try fm.createSymbolicLink(atPath: parent.path, withDestinationPath: keptDirectory.path); try refused(root)
            try fm.removeItem(at: parent); try fm.moveItem(at: keptDirectory, to: parent)
            for mutation in ["json", "format", "namespace", "entry"] {
                var bytes = Data("{broken original".utf8)
                if mutation != "json" {
                    var object = try JSONSerialization.jsonObject(with: original) as! [String: Any]
                    if mutation == "format" { object["format"] = 999 }
                    if mutation == "namespace" { object["namespace"] = "other-app/authority" }
                    if mutation == "entry" { let entry = (object["entries"] as! [String: Any])[a]!; object["entries"] = ["foreign/key": entry] }
                    bytes = try JSONSerialization.data(withJSONObject: object)
                }
                try write(bytes, file); try refused(root)
                try check(try Data(contentsOf: file) == bytes, "rejected original overwritten")
            }
            try write(original, file)
            let safeNamespace = namespace + "-safe"
            let safe = parent.appendingPathComponent(filename(safeNamespace))
            let safeStore = CrewComposerDraftStore(file: safe, namespace: safeNamespace)
            try check(safeStore.save(.init(text: "safe remainder"), for: safeNamespace + "/crew-c/topic-c"), "safe remainder save failed")
            try write(Data("{broken".utf8), file)
            let partial = CrewOfflineComposerDrafts.read(dataDirectory: root)
            try check(!partial.complete && partial.items.contains { $0.text == "safe remainder" }, "bad file hid safe items")
            try write(original, file)
        case "offline-limits":
            let tooLargeRoot = root.appendingPathComponent("file-limit")
            let tooLargeParent = try directory(tooLargeRoot)
            let tooLargeFile = tooLargeParent.appendingPathComponent(filename(namespace))
            let writer = CrewComposerDraftStore(file: tooLargeFile, namespace: namespace)
            try check(writer.save(.init(text: String(repeating: "x", count: CrewOfflineComposerDrafts.fileLimit)), for: a), "existing writer was limited")
            let original = try Data(contentsOf: tooLargeFile)
            try refused(tooLargeRoot, containing: "8MiB")
            try check(try Data(contentsOf: tooLargeFile) == original, "oversized original changed")
            let directoryRoot = root.appendingPathComponent("record-limit")
            for number in 1...257 { _ = try directory(directoryRoot, String(format: "00000000-0000-4000-8000-%012d", number)) }
            try refused(directoryRoot, containing: "记录目录")
            let fileRoot = root.appendingPathComponent("matching-file-limit")
            let manyParent = try directory(fileRoot)
            for number in 1...257 {
                let ns = namespace + String(number)
                let store = CrewComposerDraftStore(file: manyParent.appendingPathComponent(filename(ns)), namespace: ns)
                try check(store.save(.init(text: "safe item " + String(number)), for: ns + "/crew/topic"), "limit fixture save failed")
            }
            let capped = CrewOfflineComposerDrafts.read(dataDirectory: fileRoot)
            try check(!capped.complete && capped.items.count == 256, "file limit silently lost or over-read entries")
            let totalRoot = root.appendingPathComponent("total-limit")
            let totalParent = try directory(totalRoot)
            for number in 1...5 {
                let ns = namespace + String(number)
                let store = CrewComposerDraftStore(file: totalParent.appendingPathComponent(filename(ns)), namespace: ns)
                try check(store.save(.init(text: String(repeating: "y", count: CrewOfflineComposerDrafts.fileLimit - 2048)), for: ns + "/crew/topic"), "total fixture save failed")
            }
            let total = CrewOfflineComposerDrafts.read(dataDirectory: totalRoot)
            try check(!total.complete && total.items.count == 4 && total.issues.contains { $0.contains("32MiB") }, "cumulative read cap failed")
        default: throw Failure.failed("unknown offline mode")
        }
        print("PASS production offline composer: " + mode + "; existing files only, full text, no RPC/Keychain/network/session or write-back")
    }
    enum Failure: Error { case failed(String) }
}
