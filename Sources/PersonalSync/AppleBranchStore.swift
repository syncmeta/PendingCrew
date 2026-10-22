import Foundation
import CryptoKit
import Darwin

enum AppleSyncEnvironment: String, Codable, Sendable { case development, production }
struct AppleSyncAccount: Codable, Hashable, Sendable {
    let container: String
    let environment: AppleSyncEnvironment
    let userRecordID: String
}
struct AppleDraftKey: Codable, Hashable, Sendable {
    enum Destination: Codable, Hashable, Sendable { case channel(String), originalSession(String) }
    let authorityID: String
    let databaseID: String
    let crewID: String
    let destination: Destination
    let kind: String
}
enum AppleSyncFailure: Error { case invalid, capacity, corrupt, accountChanged, collision, storage }
struct AppleBranch: Codable, Equatable, Sendable {
    let id: UUID
    let key: AppleDraftKey
    let deviceID: String
    var revision: UInt64
    var payload: Data?
    var retired: Bool
    static func merge(_ a: Self, _ b: Self) throws -> Self {
        guard a.id == b.id, a.key == b.key, a.deviceID == b.deviceID else { throw AppleSyncFailure.collision }
        if a.retired { return a }; if b.retired { return b }
        if a.revision == b.revision, a != b { throw AppleSyncFailure.collision }
        return a.revision >= b.revision ? a : b
    }
}
protocol AppleBranchTransport: Sendable {
    func exchange(account: AppleSyncAccount, collection: String, branches: [AppleBranch]) async throws -> [AppleBranch]
    func cancel() async
}
func appleSyncDigest(_ data: Data) -> String { SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() }
func appleSyncEncode<T: Encodable>(_ value: T) throws -> Data {
    let e = JSONEncoder(); e.outputFormatting = [.sortedKeys]; return try e.encode(value)
}

actor AppleBranchStore<Payload: Codable & Equatable & Sendable> {
    struct Value: Sendable { let branchID: UUID; let deviceID: String; let payload: Payload }
    enum Status: Equatable, Sendable { case localOnly, pending, synced, failed(String) }
    private struct Disk: Codable {
        let format: Int
        let account: AppleSyncAccount?
        let collection: String
        var branches: [UUID: AppleBranch]
        var recovery: [UUID: Data]
    }
    private let root: URL
    private let deviceID: String
    private let collection: String
    private let byteLimit: Int
    private var account: AppleSyncAccount?
    private var branches: [UUID: AppleBranch] = [:]
    private var recovery: [UUID: Data] = [:]
    private var blocked = false
    private var epoch: UInt64 = 0
    private var localRevision: UInt64 = 0
    private var running: (any AppleBranchTransport)?
    private(set) var status: Status = .localOnly
    private(set) var storageError: String?
    init(root: URL, deviceID: String, account: AppleSyncAccount? = nil, collection: String = "drafts", byteLimit: Int = 8 * 1024 * 1024) throws {
        guard !deviceID.isEmpty, !collection.isEmpty, byteLimit > 0 else { throw AppleSyncFailure.invalid }
        self.root = root; self.deviceID = deviceID; self.account = account; self.collection = collection; self.byteLimit = byteLimit
        try Self.directory(root)
        let result = Self.load(root: root, account: account, collection: collection, byteLimit: byteLimit)
        branches = result.0; blocked = result.1; storageError = result.2; recovery = result.3
        status = result.1 ? .failed(result.2!) : (account == nil ? .localOnly : .pending)
    }
    private static func directory(_ root: URL) throws {
        guard root.isFileURL else { throw AppleSyncFailure.storage }
        if !FileManager.default.fileExists(atPath: root.path) { try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700]) }
        guard let canonical = realpath(root.path, nil) else { throw AppleSyncFailure.storage }
        defer { free(canonical) }
        guard String(cString: canonical) == root.path else { throw AppleSyncFailure.storage }
        let a = try FileManager.default.attributesOfItem(atPath: root.path)
        guard a[.type] as? FileAttributeType == .typeDirectory, (a[.ownerAccountID] as? NSNumber)?.uint32Value == geteuid(), (a[.posixPermissions] as? NSNumber)?.intValue == 0o700 else { throw AppleSyncFailure.storage }
    }
    private static func file(root: URL, account: AppleSyncAccount?, collection: String) -> URL {
        let identity = (try? appleSyncEncode(account)) ?? Data("null".utf8)
        return root.appendingPathComponent(appleSyncDigest(identity + Data(collection.utf8)) + ".json")
    }
    private static func load(root: URL, account: AppleSyncAccount?, collection: String, byteLimit: Int) -> ([UUID: AppleBranch], Bool, String?, [UUID: Data]) {
        let file = file(root: root, account: account, collection: collection)
        var s = stat()
        guard lstat(file.path, &s) == 0 else { return errno == ENOENT ? ([:], false, nil, [:]) : ([:], true, "Local file unavailable; preserved", [:]) }
        do {
            guard s.st_mode & S_IFMT == S_IFREG, s.st_uid == geteuid(), s.st_mode & 0o777 == 0o600, s.st_size <= byteLimit else { throw AppleSyncFailure.corrupt }
            let fd = open(file.path, O_RDONLY | O_NOFOLLOW)
            guard fd >= 0 else { throw AppleSyncFailure.storage }; defer { close(fd) }
            var info = stat(); guard fstat(fd, &info) == 0, info.st_ino == s.st_ino, info.st_dev == s.st_dev else { throw AppleSyncFailure.storage }
            let bytes = try FileHandle(fileDescriptor: fd, closeOnDealloc: false).readToEnd() ?? Data()
            guard bytes.count <= byteLimit else { throw AppleSyncFailure.capacity }
            let saved = try JSONDecoder().decode(Disk.self, from: bytes)
            guard saved.format == 1, saved.account == account, saved.collection == collection, saved.branches.allSatisfy({ $0.key == $0.value.id }) else { throw AppleSyncFailure.corrupt }
            for row in saved.branches.values where !row.retired { guard let data = row.payload else { throw AppleSyncFailure.corrupt }; _ = try JSONDecoder().decode(Payload.self, from: data) }
            return (saved.branches, false, nil, saved.recovery)
        } catch { return ([:], true, "Local data corrupt or unavailable; original file preserved", [:]) }
    }
    /// Read-only discovery for directory/offline version UI; never grants access.
    func keys()->Set<AppleDraftKey>{Set(branches.values.map(\.key))}
    func active(key: AppleDraftKey) -> [Value] {
        branches.values.filter { $0.key == key && !$0.retired }.sorted { $0.id.uuidString < $1.id.uuidString }.compactMap { row in
            guard let bytes = row.payload, let p = try? JSONDecoder().decode(Payload.self, from: bytes) else { return nil }
            return Value(branchID: row.id, deviceID: row.deviceID, payload: p)
        }
    }
    /// A deliberate checkpoint, normally debounced by the UI. Capacity failures retain memory.
    @discardableResult func checkpoint(key: AppleDraftKey, payload: Payload) throws -> UUID {
        guard !key.authorityID.isEmpty, !key.databaseID.isEmpty, !key.crewID.isEmpty, !key.kind.isEmpty else { throw AppleSyncFailure.invalid }
        let bytes = try appleSyncEncode(payload)
        let own = branches.values.filter { $0.key == key && $0.deviceID == deviceID && !$0.retired }
        guard own.count <= 1 else { throw AppleSyncFailure.collision }
        let id = own.first?.id ?? UUID()
        if let previous = own.first, previous.payload == bytes { if storageError != nil { try persist() }; return id }
        let revision = (own.first?.revision ?? 0)
        guard revision < UInt64.max else { throw AppleSyncFailure.capacity }
        branches[id] = AppleBranch(id: id, key: key, deviceID: deviceID, revision: revision + 1, payload: bytes, retired: false)
        localRevision &+= 1
        try persist(); return id
    }
    /// Explicit selection/merge. Retired branches can never be reused by checkpoint.
    @discardableResult func resolve(key: AppleDraftKey, payload: Payload?, retiring ids: Set<UUID>) throws -> UUID? {
        guard ids.allSatisfy({ branches[$0]?.key == key }) else { throw AppleSyncFailure.invalid }
        if payload != nil, branches.values.contains(where: { $0.key == key && $0.deviceID == deviceID && !$0.retired && !ids.contains($0.id) }) { throw AppleSyncFailure.invalid }
        let encoded = try payload.map(appleSyncEncode)
        for id in ids { branches[id]!.retired = true; branches[id]!.payload = nil }
        var result: UUID?
        if let encoded {
            // Preserve an unselected local branch rather than overwrite it.
            let id = UUID(); result = id
            branches[id] = AppleBranch(id: id, key: key, deviceID: deviceID, revision: 1, payload: encoded, retired: false)
        }
        localRevision &+= 1; try persist(); return result
    }
    /// Local-only originals discovered behind a remote tombstone; explicit export/restore only.
    func recoveredOriginals() -> [UUID: Payload] { recovery.compactMapValues { try? JSONDecoder().decode(Payload.self, from: $0) } }
    func retryLocalSave() throws { try persist() }
    func selectAccount(_ next: AppleSyncAccount?) async throws {
        epoch &+= 1
        let generation = epoch
        let oldTransport = running; running = nil
        await oldTransport?.cancel()
        guard epoch == generation else { throw AppleSyncFailure.accountChanged }
        // Never silently discard unsaved input on account switch.
        guard storageError == nil else { status = .failed("Save/export current local input before switching account"); throw AppleSyncFailure.storage }
        account = next
        let loaded = Self.load(root: root, account: next, collection: collection, byteLimit: byteLimit)
        branches = loaded.0; blocked = loaded.1; storageError = loaded.2; recovery = loaded.3
        status = blocked ? .failed(loaded.2!) : (next == nil ? .localOnly : .pending)
    }
    func synchronize(using transport: any AppleBranchTransport) async throws {
        guard let account else { status = .localOnly; return }
        guard !blocked, storageError == nil, running == nil else { throw AppleSyncFailure.storage }
        let generation = epoch, revision = localRevision
        running = transport; status = .pending
        do {
            let remote = try await transport.exchange(account: account, collection: collection, branches: Array(branches.values))
            guard epoch == generation, self.account == account else { throw AppleSyncFailure.accountChanged }
            var merged = branches
            for row in remote {
                if !row.retired { guard let data = row.payload else { throw AppleSyncFailure.invalid }; _ = try JSONDecoder().decode(Payload.self, from: data) }
                if row.retired, let old = merged[row.id], !old.retired, let bytes = old.payload { recovery[row.id] = bytes }
                merged[row.id] = try merged[row.id].map { try AppleBranch.merge($0, row) } ?? row
            }
            branches = merged; try persist()
            status = revision == localRevision ? .synced : .pending
            running = nil
        } catch {
            if epoch == generation { running = nil; status = .failed(String(describing: error)) }
            throw error
        }
    }
    private func persist() throws {
        status = account == nil ? .localOnly : .pending
        do {
            guard !blocked else { throw AppleSyncFailure.corrupt }
            try Self.directory(root)
            let bytes = try appleSyncEncode(Disk(format: 1, account: account, collection: collection, branches: branches, recovery: recovery))
            guard bytes.count <= byteLimit else { throw AppleSyncFailure.capacity }
            let target = Self.file(root: root, account: account, collection: collection)
            let temp = root.appendingPathComponent(".pending-" + UUID().uuidString)
            let fd = open(temp.path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0o600)
            guard fd >= 0 else { throw AppleSyncFailure.storage }
            defer { close(fd); try? FileManager.default.removeItem(at: temp) }
            try bytes.withUnsafeBytes { buffer in
                var offset = 0
                while offset < buffer.count {
                    let n = Darwin.write(fd, buffer.baseAddress!.advanced(by: offset), buffer.count - offset)
                    if n < 0 && errno == EINTR { continue }
                    guard n > 0 else { throw AppleSyncFailure.storage }; offset += n
                }
            }
            guard fsync(fd) == 0, rename(temp.path, target.path) == 0 else { throw AppleSyncFailure.storage }
            let dir = open(root.path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW)
            guard dir >= 0 else { throw AppleSyncFailure.storage }; defer { close(dir) }
            guard fsync(dir) == 0 else { throw AppleSyncFailure.storage }
            storageError = nil
        } catch { storageError = "Latest input retained in memory; local save failed: \(error)"; status = .failed(storageError!); throw error }
    }
}
