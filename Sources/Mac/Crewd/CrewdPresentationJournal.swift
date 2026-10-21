import Foundation
import CryptoKit
import Darwin

/// Durable frontend observations, never an authority receipt or replay queue.
@MainActor
final class CrewdPresentationJournal {
    struct Scope: Codable, Equatable {
        let namespace: UUID
        let localDirectory: String?
        let authorityID: String?
        let databaseID: String?
        let grantID: String?
        let grantRevision: String?
        var authenticatedDatabaseKnown: Bool { authorityID != nil && databaseID != nil }
    }
    struct Prepared: Codable, Identifiable {
        let format: Int
        let id: UUID // local observation identity only
        let scope: Scope
        let method: String
        let params: CrewdValue
        let crew: String
        let preparedAt: Date
    }
    struct Outcome: Codable {
        let format: Int
        let observationID: UUID
        let preparedSHA256: String
        let state: String // replyObserved/refused/unknown; not executed/consumed
        let observedAt: Date
        let detail: String?
    }
    let scope: Scope
    func composerDraftFile(namespace: String) -> URL {
        directory.appendingPathComponent("composer-drafts-" + Self.hash(Data(namespace.utf8)) + ".json")
    }
    private let directory: URL
    private let encoder: JSONEncoder
    private let decoder: JSONDecoder
    private enum StorageError: LocalizedError {
        case refused(String)
        var errorDescription: String? { if case .refused(let reason) = self { return reason }; return nil }
    }
    init(root: URL, scope: Scope) throws {
        self.scope = scope
        encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys]
        encoder.dateEncodingStrategy = .iso8601
        decoder = JSONDecoder(); decoder.dateDecodingStrategy = .iso8601
        try Self.privateDirectory(root)
        directory = root.appendingPathComponent(scope.namespace.uuidString.lowercased())
        try Self.privateDirectory(directory)
        try Self.privateDirectory(directory.appendingPathComponent("prepared"))
        try Self.privateDirectory(directory.appendingPathComponent("outcomes"))
        try Self.privateDirectory(directory.appendingPathComponent("reconciled"))
    }
    static func localNamespace(directory: URL) throws -> UUID {
        let canonical = directory.standardizedFileURL.resolvingSymlinksInPath()
        guard canonical.path == directory.standardizedFileURL.path else { throw StorageError.refused("前端命名空间拒绝别名或符号链接目录。") }
        if let old = ProcessInfo.processInfo.environment["PENDINGCREW_LEGACY_DATA_DIR"] {
            let legacy = URL(fileURLWithPath: old).standardizedFileURL.resolvingSymlinksInPath().path
            guard canonical.path != legacy, !canonical.path.hasPrefix(legacy + "/") else { throw StorageError.refused("候选前端不能写入旧版日常数据目录。") }
        }
        try privateDirectory(canonical)
        let front = canonical.appendingPathComponent("frontend-presentation")
        try privateDirectory(front)
        let file = front.appendingPathComponent("namespace.json")
        if FileManager.default.fileExists(atPath: file.path) {
            try privateFile(file)
            guard let value = UUID(uuidString: String(decoding: try Data(contentsOf: file), as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)) else { throw StorageError.refused("前端命名空间记录损坏；未覆盖原记录。") }
            return value
        }
        let id = UUID()
        do { try immutableWrite(Data((id.uuidString.lowercased() + "\n").utf8), to: file) }
        catch {
            // Another GUI may have created the immutable namespace meanwhile.
            if FileManager.default.fileExists(atPath: file.path) { return try localNamespace(directory: directory) }
            throw error
        }
        return id
    }
    /// Prepare before any original frontend singleton can create a cache directory.
    /// This is presentation storage, separate from both crewd and unknown observations.
    static func prepareUICache(directory: URL) throws -> URL {
        _ = try localNamespace(directory: directory)
        let cache = directory.appendingPathComponent("frontend-presentation/ui-cache")
        try privateDirectory(cache)
        return cache
    }
    private static func privateDirectory(_ directory: URL) throws {
        let fm = FileManager.default
        if !fm.fileExists(atPath: directory.path) {
            try fm.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        }
        let attrs = try fm.attributesOfItem(atPath: directory.path)
        guard attrs[.type] as? FileAttributeType == .typeDirectory,
              (attrs[.ownerAccountID] as? NSNumber)?.uint32Value == geteuid(),
              (attrs[.posixPermissions] as? NSNumber)?.intValue == 0o700,
              directory.standardizedFileURL.path == directory.standardizedFileURL.resolvingSymlinksInPath().path else {
            throw StorageError.refused("前端历史目录须是当前用户的独立 0700 实体目录。")
        }
    }
    private static func privateFile(_ file: URL) throws {
        let attrs = try FileManager.default.attributesOfItem(atPath: file.path)
        guard attrs[.type] as? FileAttributeType == .typeRegular,
              (attrs[.ownerAccountID] as? NSNumber)?.uint32Value == geteuid(),
              (attrs[.posixPermissions] as? NSNumber)?.intValue == 0o600 else {
            throw StorageError.refused("前端历史文件须是当前用户的 0600 普通文件。")
        }
    }
    private static func immutableWrite(_ bytes: Data, to file: URL) throws {
        let fd = open(file.path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, mode_t(0o600))
        guard fd >= 0 else { throw StorageError.refused("无法创建独立前端历史记录；未提交后台操作。") }
        defer { close(fd) }
        try bytes.withUnsafeBytes { buffer in
            var offset = 0
            while offset < buffer.count {
                let written = Darwin.write(fd, buffer.baseAddress!.advanced(by: offset), buffer.count - offset)
                if written < 0 && errno == EINTR { continue }
                guard written > 0 else { throw StorageError.refused("前端历史未完整写入；保留原文件，不提交后台操作。") }
                offset += written
            }
        }
        guard fsync(fd) == 0 else { throw StorageError.refused("前端历史无法同步到磁盘；未提交后台操作。") }
        let parent = open(file.deletingLastPathComponent().path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW)
        guard parent >= 0 else { throw StorageError.refused("前端历史目录同步失败；未提交后台操作。") }
        defer { close(parent) }
        guard fsync(parent) == 0 else { throw StorageError.refused("前端历史目录同步失败；未提交后台操作。") }
    }
    private static func hash(_ bytes: Data) -> String { CryptoKit.SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined() }
    func prepare(method: String, params: CrewdValue, crew: String) throws -> Prepared {
        let record = Prepared(format: 1, id: UUID(), scope: scope, method: method, params: params, crew: crew, preparedAt: Date())
        try Self.immutableWrite(encoder.encode(record), to: directory.appendingPathComponent("prepared").appendingPathComponent(record.id.uuidString.lowercased() + ".json"))
        return record
    }
    func finish(_ record: Prepared, state: String, detail: String? = nil) throws {
        guard record.scope == scope, ["replyObserved", "refused", "unknown"].contains(state) else { throw StorageError.refused("前端历史作用域或状态不符。") }
        let prepared = directory.appendingPathComponent("prepared").appendingPathComponent(record.id.uuidString.lowercased() + ".json")
        try Self.privateFile(prepared)
        let result = Outcome(format: 1, observationID: record.id, preparedSHA256: Self.hash(try Data(contentsOf: prepared)), state: state, observedAt: Date(), detail: detail)
        try Self.immutableWrite(encoder.encode(result), to: directory.appendingPathComponent("outcomes").appendingPathComponent(record.id.uuidString.lowercased() + ".json"))
    }
    /// Append a verified observation without rewriting the immutable Unknown.
    func acknowledge(_ record: Prepared, detail: String) throws {
        guard record.scope == scope else { throw StorageError.refused("核对作用域不符。") }
        let name = record.id.uuidString.lowercased() + ".json"
        let prepared = directory.appendingPathComponent("prepared").appendingPathComponent(name)
        try Self.privateFile(prepared)
        let bytes = try Data(contentsOf: prepared)
        let saved = try decoder.decode(Prepared.self, from: bytes)
        guard saved.id == record.id, saved.scope == record.scope, saved.params == record.params,
              saved.method == record.method, saved.crew == record.crew else { throw StorageError.refused("核对原记录不符。") }
        let hash = Self.hash(bytes)
        let destination = directory.appendingPathComponent("reconciled").appendingPathComponent(name)
        if FileManager.default.fileExists(atPath: destination.path) {
            try Self.privateFile(destination)
            let old = try decoder.decode(Outcome.self, from: Data(contentsOf: destination))
            guard old.format == 1, old.observationID == record.id, old.preparedSHA256 == hash, old.state == "replyObserved" else { throw StorageError.refused("核对观察与原记录不符。") }
            return
        }
        let result = Outcome(format: 1, observationID: record.id, preparedSHA256: hash, state: "replyObserved", observedAt: Date(), detail: detail)
        try Self.immutableWrite(encoder.encode(result), to: destination)
    }
    func unknownRecords() throws -> [Prepared] { try records(includeObservedFirstMessages: false) }
    /// Original successful-observation snapshots remain available for lookup;
    /// their existence is not a durable server success or a replay permission.
    func firstMessageRecords() throws -> [Prepared] {
        Array(try records(includeObservedFirstMessages: true)
            .filter { $0.method == "command.submit" && $0.params["command"]["operation"].string == "crew.createFirstMessage" }
            .sorted { $0.preparedAt > $1.preparedAt }.prefix(64))
    }
    private func records(includeObservedFirstMessages: Bool) throws -> [Prepared] {
        var rows: [Prepared] = []
        for file in try FileManager.default.contentsOfDirectory(at: directory.appendingPathComponent("prepared"), includingPropertiesForKeys: nil).sorted(by: { $0.path < $1.path }) {
            guard file.pathExtension == "json" else { throw StorageError.refused("前端历史目录包含未知对象；保留原资料。") }
            try Self.privateFile(file)
            let bytes = try Data(contentsOf: file), record = try decoder.decode(Prepared.self, from: bytes)
            guard record.format == 1, record.scope.namespace == scope.namespace, file.lastPathComponent == record.id.uuidString.lowercased() + ".json" else { throw StorageError.refused("原前端历史身份或来源不符。") }
            let result = directory.appendingPathComponent("outcomes").appendingPathComponent(file.lastPathComponent)
            var observed: Outcome?
            if FileManager.default.fileExists(atPath: result.path) {
                try Self.privateFile(result)
                let outcome = try decoder.decode(Outcome.self, from: Data(contentsOf: result))
                guard outcome.format == 1, outcome.observationID == record.id, outcome.preparedSHA256 == Self.hash(bytes),
                      ["unknown", "replyObserved", "refused"].contains(outcome.state) else { throw StorageError.refused("原前端历史结果与提交快照不符。") }
                observed = outcome
            }
            let reconciled = directory.appendingPathComponent("reconciled").appendingPathComponent(file.lastPathComponent)
            if FileManager.default.fileExists(atPath: reconciled.path) {
                try Self.privateFile(reconciled)
                let observation = try decoder.decode(Outcome.self, from: Data(contentsOf: reconciled))
                guard observation.format == 1, observation.observationID == record.id,
                      observation.preparedSHA256 == Self.hash(bytes), observation.state == "replyObserved" else { throw StorageError.refused("终态核对与原记录不符。") }
                observed = observation
            }
            guard let outcome = observed else { rows.append(record); continue }
            if outcome.state == "unknown" { rows.append(record) }
            else if includeObservedFirstMessages, outcome.state == "replyObserved",
                    record.method == "command.submit", record.params["command"]["operation"].string == "crew.createFirstMessage" { rows.append(record) }
            else if !["replyObserved", "refused"].contains(outcome.state) { throw StorageError.refused("原前端历史出现未知状态。") }
        }
        return rows
    }
}
