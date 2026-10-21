import Foundation
import Darwin

/// Client-only unsent input. It is never a replay queue or a server receipt.
/// The authority journal supplies the private file and authenticated namespace.
@MainActor
final class CrewComposerDraftStore {
    static let shared = CrewComposerDraftStore()
    private static var authorities: [String: CrewComposerDraftStore] = [:]
    /// Retired and reselected workspaces in this process share one writer for
    /// the same authenticated file. A late reply cannot overwrite a new window.
    static func durable(file: URL, namespace: String) -> CrewComposerDraftStore {
        let key = file.standardizedFileURL.path + "\n" + namespace
        if let existing = authorities[key] { return existing }
        let store = CrewComposerDraftStore(file: file, namespace: namespace)
        authorities[key] = store
        return store
    }

    struct Draft: Codable, Equatable {
        var text: String = ""
        var stagedMentions: [CrewStagedMention] = []
        var replyTarget: CrewReplyTarget?
        var isEmpty: Bool {
            text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                && stagedMentions.isEmpty && replyTarget == nil
        }
    }
    struct Entry: Codable {
        let draft: Draft
        let revision: UUID
        let crew: String?
        let channel: String?
    }
    struct Snapshot {
        let key: String
        let draft: Draft
        fileprivate let revision: UUID?
    }
    private struct FileContents: Codable {
        let format: Int
        let namespace: String
        let entries: [String: Entry]
    }
    private var drafts: [String: Entry] = [:]
    private var file: URL?
    private var namespace = ""
    private var storageBlocked = false
    private(set) var storageError: String?
    var entries: [String: Entry] { drafts }

    init() {}
    init(file: URL, namespace: String) {
        self.file = file; self.namespace = namespace
        do {
            try checkDirectory(file.deletingLastPathComponent())
            if try filePresent(file) {
                try checkFile(file)
                let saved = try JSONDecoder().decode(FileContents.self, from: Data(contentsOf: file))
                guard saved.format == 1, saved.namespace == namespace,
                      saved.entries.keys.allSatisfy({ $0.hasPrefix(namespace + "/") }) else { throw Failure.invalid }
                drafts = saved.entries
            }
        } catch {
            // A damaged/unknown file is not an empty store. Preserve its bytes.
            storageBlocked = true
            storageError = "未发送草稿读取失败，原文件未覆盖：" + error.localizedDescription
        }
    }
    func draft(for key: String) -> Draft { drafts[key]?.draft ?? Draft() }
    func snapshot(for key: String) -> Snapshot {
        Snapshot(key: key, draft: draft(for: key), revision: drafts[key]?.revision)
    }
    @discardableResult func save(_ draft: Draft, for key: String, crew: String? = nil, channel: String? = nil) -> Bool {
        if let old = drafts[key], old.draft == draft, old.crew == crew, old.channel == channel { return storageError == nil }
        if draft.isEmpty { drafts.removeValue(forKey: key) }
        else { drafts[key] = Entry(draft: draft, revision: UUID(), crew: crew, channel: channel) }
        return persist()
    }
    @discardableResult func clear(_ key: String) -> Bool {
        drafts.removeValue(forKey: key)
        return persist()
    }
    /// Only the successful submission's unchanged input may be removed.
    /// The UUID also protects edit-away-and-back (ABA), not just equal text.
    @discardableResult func clearIfUnchanged(_ snapshot: Snapshot) -> Bool {
        guard let old = drafts[snapshot.key], old.revision == snapshot.revision,
              old.draft == snapshot.draft else { return false }
        drafts.removeValue(forKey: snapshot.key)
        if persist() { return true }
        drafts[snapshot.key] = old
        return false
    }
    private enum Failure: LocalizedError {
        case invalid
        var errorDescription: String? { "草稿文件的身份、版本、权限或原子写入不符。" }
    }
    private func checkDirectory(_ url: URL) throws {
        let attrs = try FileManager.default.attributesOfItem(atPath: url.path)
        guard url.standardizedFileURL.path == url.standardizedFileURL.resolvingSymlinksInPath().path,
              attrs[.type] as? FileAttributeType == .typeDirectory,
              (attrs[.ownerAccountID] as? NSNumber)?.uint32Value == geteuid(),
              (attrs[.posixPermissions] as? NSNumber)?.intValue == 0o700 else { throw Failure.invalid }
    }
    private func filePresent(_ url: URL) throws -> Bool {
        var info = stat()
        if lstat(url.path, &info) == 0 { return true }
        guard errno == ENOENT else { throw Failure.invalid }
        return false
    }
    private func checkFile(_ url: URL) throws {
        let attrs = try FileManager.default.attributesOfItem(atPath: url.path)
        guard attrs[.type] as? FileAttributeType == .typeRegular,
              (attrs[.ownerAccountID] as? NSNumber)?.uint32Value == geteuid(),
              (attrs[.posixPermissions] as? NSNumber)?.intValue == 0o600 else { throw Failure.invalid }
    }
    private func persist() -> Bool {
        guard let file else { return true }
        guard !storageBlocked else { return false }
        let parent = file.deletingLastPathComponent()
        let temporary = parent.appendingPathComponent(".composer-draft-" + UUID().uuidString)
        var fd: Int32 = -1
        defer { if fd >= 0 { close(fd) }; try? FileManager.default.removeItem(at: temporary) }
        do {
            try checkDirectory(parent)
            if try filePresent(file) { try checkFile(file) }
            guard drafts.keys.allSatisfy({ $0.hasPrefix(namespace + "/") }) else { throw Failure.invalid }
            let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys]
            let bytes = try encoder.encode(FileContents(format: 1, namespace: namespace, entries: drafts))
            fd = open(temporary.path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, mode_t(0o600))
            guard fd >= 0 else { throw Failure.invalid }
            try bytes.withUnsafeBytes { buffer in
                var offset = 0
                while offset < buffer.count {
                    let n = Darwin.write(fd, buffer.baseAddress!.advanced(by: offset), buffer.count - offset)
                    if n < 0 && errno == EINTR { continue }
                    guard n > 0 else { throw Failure.invalid }
                    offset += n
                }
            }
            guard fsync(fd) == 0, rename(temporary.path, file.path) == 0 else { throw Failure.invalid }
            let dir = open(parent.path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW)
            guard dir >= 0 else { throw Failure.invalid }
            defer { close(dir) }
            guard fsync(dir) == 0 else { throw Failure.invalid }
            storageError = nil
            return true
        } catch {
            storageError = "未发送草稿尚未保存到本机，当前窗口原文仍保留：" + error.localizedDescription
            return false
        }
    }
}
