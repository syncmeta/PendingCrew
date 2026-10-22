import Foundation
import Darwin
import CryptoKit

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


    struct Entry: Codable {
        let draft: Draft
        let revision: UUID
        let crew: String?
        let channel: String?
        let sourceIdentity: Data?
        init(draft:Draft,revision:UUID,crew:String?,channel:String?,sourceIdentity:Data?=nil){
            self.draft=draft;self.revision=revision;self.crew=crew;self.channel=channel;self.sourceIdentity=sourceIdentity
        }
    }
    struct Snapshot {
        let key: String
        let draft: Draft
        fileprivate let revision: UUID?
    }
    fileprivate struct FileContents: Codable {
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
    @discardableResult func save(_ draft: Draft, for key: String, crew: String? = nil, channel: String? = nil, retainEmpty: Bool = false, sourceIdentity: Data? = nil) -> Bool {
        if let old = drafts[key], old.draft == draft, old.crew == crew, old.channel == channel, old.sourceIdentity == sourceIdentity { return storageError == nil }
        if draft.isEmpty && !retainEmpty { drafts.removeValue(forKey: key) }
        else { drafts[key] = Entry(draft: draft, revision: UUID(), crew: crew, channel: channel, sourceIdentity: sourceIdentity) }
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

/// A fresh read of this application's existing presentation files. These values
/// are historical local input, never credentials, live workspaces or receipts.
enum CrewOfflineComposerDrafts {
    static let fileLimit = 8 * 1024 * 1024
    static let totalLimit = 32 * 1024 * 1024
    static let recordLimit = 256
    static let matchingFileLimit = 256
    struct Item: Identifiable, Sendable {
        let id: String
        let record: String
        let namespace: String
        let key: String
        let crew: String?
        let channel: String?
        let text: String
        let mentionTokens: [String]
        let reply: String?
    }
    struct Readback: Sendable {
        var items: [Item] = []
        var issues: [String] = []
        var missing = false
        var complete: Bool { issues.isEmpty }
    }
    private enum Refusal: Error { case unsafe, invalid, tooLarge, changed, missing }
    private static func validFileName(_ name: String) -> Bool {
        guard name.hasPrefix("composer-drafts-"), name.hasSuffix(".json") else { return false }
        let hash = name.dropFirst("composer-drafts-".count).dropLast(5)
        return hash.count == 64 && hash.allSatisfy { "0123456789abcdef".contains($0) }
    }
    private static func directory(_ fd: Int32) throws {
        var info = stat()
        guard fstat(fd, &info) == 0, info.st_mode & S_IFMT == S_IFDIR,
              info.st_uid == geteuid(), info.st_mode & 0o7777 == 0o700 else { throw Refusal.unsafe }
    }
    /// Bind every configured path component before walking the two fixed
    /// private journal components. A pathname check followed by open(path)
    /// would leave intermediate ancestors vulnerable to symlink replacement.
    private static func journalDirectory(_ dataDirectory: URL) throws -> Int32 {
        guard dataDirectory.isFileURL, dataDirectory.path.hasPrefix("/") else { throw Refusal.unsafe }
        var current = open("/", O_RDONLY | O_DIRECTORY | O_NOFOLLOW)
        guard current >= 0 else { throw Refusal.unsafe }
        var returned = false
        defer { if !returned { close(current) } }
        func descend(_ component: String) throws {
            guard !component.isEmpty, component != ".", component != "..", !component.contains("/") else { throw Refusal.unsafe }
            let next = openat(current, component, O_RDONLY | O_DIRECTORY | O_NOFOLLOW)
            guard next >= 0 else {
                if errno == ENOENT { throw Refusal.missing }
                throw Refusal.unsafe
            }
            close(current); current = next
        }
        for component in dataDirectory.pathComponents where component != "/" { try descend(component) }
        try directory(current)
        for component in ["frontend-presentation", "unknown-submissions"] {
            try descend(component)
            try directory(current)
        }
        returned = true
        return current
    }
    /// Enumerate the already opened directory; never follow a path discovered
    /// in a file. Only matching immediate names count toward the bounded list.
    private static func names(_ fd: Int32, limit: Int, matching: (String) -> Bool) throws -> (values: [String], limited: Bool) {
        let copy = dup(fd)
        guard copy >= 0 else { throw Refusal.unsafe }
        guard let stream = fdopendir(copy) else { close(copy); throw Refusal.unsafe }
        defer { closedir(stream) }
        var values: [String] = []
        while true {
            errno = 0
            guard let entry = readdir(stream) else {
                guard errno == 0 else { throw Refusal.unsafe }
                return (values.sorted(), false)
            }
            let name = withUnsafePointer(to: entry.pointee.d_name) {
                $0.withMemoryRebound(to: CChar.self, capacity: Int(entry.pointee.d_namlen) + 1) { String(cString: $0) }
            }
            guard matching(name) else { continue }
            if values.count == limit { return (values.sorted(), true) }
            values.append(name)
        }
    }
    private static func decode(_ data: Data, record: String, file: String) throws -> [Item] {
        let saved = try JSONDecoder().decode(CrewComposerDraftStore.FileContents.self, from: data)
        let namespaceRecord = String(saved.namespace.prefix(36))
        let hash = SHA256.hash(data: Data(saved.namespace.utf8)).map { String(format: "%02x", $0) }.joined()
        guard saved.format == 1, UUID(uuidString: namespaceRecord) == UUID(uuidString: record),
              saved.namespace.dropFirst(36).hasPrefix("/"), saved.namespace.utf8.count > 37,
              file == "composer-drafts-" + hash + ".json",
              saved.entries.keys.allSatisfy({ $0.hasPrefix(saved.namespace + "/") }) else { throw Refusal.invalid }
        // dbID is opaque (it may contain '/'); do not split or reinterpret the
        // namespace as a current authorization or a destination to reconnect.
        return saved.entries.sorted { $0.key < $1.key }.map { key, value in
            let reply = value.draft.replyTarget.map { $0.quotedSender + " · " + $0.replyToId + "\n" + $0.quotedSnippet }
            return Item(id: record + "/" + file + "/" + key, record: record, namespace: saved.namespace,
                key: key, crew: value.crew, channel: value.channel, text: value.draft.text,
                mentionTokens: value.draft.stagedMentions.map(\.token), reply: reply)
        }
    }
    private static func readFile(_ parent: Int32, name: String, record: String, bytesRead: inout Int) throws -> [Item] {
        guard validFileName(name) else { throw Refusal.invalid }
        let fd = openat(parent, name, O_RDONLY | O_NOFOLLOW | O_NONBLOCK)
        guard fd >= 0 else { throw Refusal.unsafe }
        defer { close(fd) }
        var before = stat()
        guard fstat(fd, &before) == 0, before.st_mode & S_IFMT == S_IFREG,
              before.st_uid == geteuid(), before.st_mode & 0o7777 == 0o600, before.st_nlink == 1, before.st_size >= 0 else { throw Refusal.unsafe }
        guard before.st_size <= fileLimit else { throw Refusal.tooLarge }
        guard before.st_size <= totalLimit - bytesRead else { throw Refusal.tooLarge }
        var bytes = Data()
        var buffer = [UInt8](repeating: 0, count: 16384)
        while bytes.count < Int(before.st_size) {
            let remaining = min(buffer.count, Int(before.st_size) - bytes.count)
            let count = buffer.withUnsafeMutableBytes { Darwin.read(fd, $0.baseAddress, remaining) }
            if count < 0 && errno == EINTR { continue }
            guard count > 0 else { throw Refusal.changed }
            bytesRead += count
            bytes.append(contentsOf: buffer.prefix(count))
        }
        var after = stat()
        guard fstat(fd, &after) == 0, after.st_size == before.st_size,
              after.st_uid == before.st_uid, after.st_mode == before.st_mode, after.st_nlink == before.st_nlink,
              after.st_mtimespec.tv_sec == before.st_mtimespec.tv_sec,
              after.st_mtimespec.tv_nsec == before.st_mtimespec.tv_nsec,
              after.st_ctimespec.tv_sec == before.st_ctimespec.tv_sec,
              after.st_ctimespec.tv_nsec == before.st_ctimespec.tv_nsec else { throw Refusal.changed }
        return try decode(bytes, record: record, file: name)
    }
    /// The caller supplies only its configured app data root; there is no
    /// directory picker, recursive search, initializer or write path here.
    static func read(dataDirectory: URL) -> Readback {
        var result = Readback()
        func issue(_ detail: String) { result.issues.append("未完整读取，原文件保留：" + detail) }
        let fd: Int32
        do { fd = try journalDirectory(dataDirectory) }
        catch Refusal.missing { result.missing = true; return result }
        catch { issue("本机草稿目录或固定祖先身份不符，无法安全读取。"); return result }
        defer { close(fd) }
        do {
            try directory(fd)
            let records = try names(fd, limit: recordLimit) { UUID(uuidString: $0)?.uuidString.lowercased() == $0 }
            if records.limited { issue("记录目录超过单次 256 项上限。") }
            var fileCount = 0, bytesRead = 0
            for record in records.values {
                let parent = openat(fd, record, O_RDONLY | O_DIRECTORY | O_NOFOLLOW)
                guard parent >= 0 else { issue("记录 " + record + " 的目录不可安全读取。"); continue }
                do {
                    try directory(parent)
                    let files = try names(parent, limit: matchingFileLimit - fileCount) { $0.hasPrefix("composer-drafts-") && $0.hasSuffix(".json") }
                    fileCount += files.values.count
                    if files.limited { issue("草稿文件超过单次 256 项上限。") }
                    for file in files.values {
                        do { result.items += try readFile(parent, name: file, record: record, bytesRead: &bytesRead) }
                        catch Refusal.tooLarge { issue("记录 " + record + " 的文件超出单文件 8MiB 或单次累计 32MiB 上限。") }
                        catch { issue("记录 " + record + " 的草稿权限、身份、格式或读取快照不符。") }
                    }
                } catch { issue("记录 " + record + " 的目录权限或枚举失败。") }
                close(parent)
            }
        } catch { issue("本机草稿根目录权限或枚举失败。") }
        return result
    }
}
