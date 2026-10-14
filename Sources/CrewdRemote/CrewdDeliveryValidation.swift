import Foundation
import CoreFoundation
import CryptoKit
import Darwin

/// A build-qualified, explicitly reviewed fake-only scope. No credential API
/// is invoked by qualification; source identity is correlation, not trust.
enum CrewdDeliveryValidation {
    enum Platform { case mac, mobile }
    struct Scope {
        let root: URL
        let data: URL
        let service: String
        let account: UUID
        let actions: Set<String>
        let manifestSHA256: String
        let endpoint: CrewdRemoteEndpoint?
        let authority: String?
        let database: String?
        let pin: String?
    }
    static func qualified() throws -> Scope {
        guard Bundle.main.object(forInfoDictionaryKey: "PendingCrewDeliveryValidation") as? Bool == true,
              let ref = Bundle.main.object(forInfoDictionaryKey: "PendingCrewSourceRef") as? String,
              let path = ProcessInfo.processInfo.environment["PENDINGCREW_VALIDATION_MANIFEST"] else { throw CrewdRemoteError.storageFailure }
        #if os(macOS)
        return try qualify(manifest: URL(fileURLWithPath: path), sourceRef: ref, environment: ProcessInfo.processInfo.environment, platform: .mac)
        #else
        // The real sandbox container is supplied by the OS, never an env path.
        return try qualify(manifest: URL(fileURLWithPath: path), sourceRef: ref, environment: ProcessInfo.processInfo.environment, platform: .mobile, container: URL(fileURLWithPath: NSHomeDirectory(), isDirectory: true))
        #endif
    }
    static func privatePath(_ url: URL, root: URL?, directory: Bool) throws {
        guard url.path.hasPrefix("/"), let resolved = realpath(url.path, nil) else { throw CrewdRemoteError.storageFailure }
        defer { free(resolved) }
        guard String(cString: resolved) == url.path else { throw CrewdRemoteError.storageFailure }
        if let root { guard url.path.hasPrefix(root.path + "/") else { throw CrewdRemoteError.storageFailure } }
        var s = stat()
        guard lstat(url.path, &s) == 0, s.st_uid == geteuid(),
              (s.st_mode & S_IFMT) == (directory ? S_IFDIR : S_IFREG),
              (s.st_mode & 0o777) == (directory ? 0o700 : 0o600) else { throw CrewdRemoteError.storageFailure }
    }
    static func qualify(manifest: URL, sourceRef: String, environment: [String: String], platform: Platform = .mac, container: URL? = nil) throws -> Scope {
        guard sourceRef.count == 40, sourceRef.allSatisfy({ "0123456789abcdef".contains($0) }), manifest.lastPathComponent == "scope.json" else { throw CrewdRemoteError.storageFailure }
        let root = manifest.deletingLastPathComponent()
        try privatePath(root, root: nil, directory: true)
        try privatePath(manifest, root: root, directory: false)
        let fd = open(manifest.path, O_RDONLY | O_NOFOLLOW | O_CLOEXEC)
        guard fd >= 0 else { throw CrewdRemoteError.storageFailure }
        defer { close(fd) }
        var first = stat()
        guard fstat(fd, &first) == 0, first.st_uid == geteuid(), (first.st_mode & S_IFMT) == S_IFREG,
              (first.st_mode & 0o777) == 0o600, first.st_size > 0, first.st_size <= 65536 else { throw CrewdRemoteError.storageFailure }
        var bytes = [UInt8](repeating: 0, count: Int(first.st_size))
        guard read(fd, &bytes, bytes.count) == bytes.count else { throw CrewdRemoteError.storageFailure }
        var last = stat()
        guard fstat(fd, &last) == 0, first.st_ino == last.st_ino, first.st_size == last.st_size,
              first.st_mtimespec.tv_sec == last.st_mtimespec.tv_sec, first.st_mtimespec.tv_nsec == last.st_mtimespec.tv_nsec else { throw CrewdRemoteError.storageFailure }
        guard let v = try JSONSerialization.jsonObject(with: Data(bytes)) as? [String: Any],
              let format = v["format"] as? NSNumber, CFGetTypeID(format) != CFBooleanGetTypeID(), format.intValue == 1, v["sourceRef"] as? String == sourceRef,
              v["validationDomain"] as? String == "pendingcrew.fake-only.delivery-validation.v1",
              let scopeID = v["scopeID"] as? String, let uuid = UUID(uuidString: scopeID), scopeID == uuid.uuidString.lowercased(),
              let service = v["service"] as? String, service == "com.pendingname.pendingcrew.delivery-validation." + scopeID,
              let accountText = v["account"] as? String, let account = UUID(uuidString: accountText), accountText == account.uuidString.lowercased(),
              let actions = v["actions"] as? [String], Set(actions).count == actions.count,
              !actions.isEmpty, Set(actions).isSubset(of: ["records", "read", "save", "remove"]) else { throw CrewdRemoteError.storageFailure }
        let common: Set<String> = ["format", "sourceRef", "scopeID", "service", "account", "actions", "mode", "validationDomain", "endpoint", "expectedAuthorityID", "expectedDatabaseID", "expectedServerPin"]
        guard let rawEndpoint = v["endpoint"] as? [String: Any], Set(rawEndpoint.keys) == ["host", "port"],
              let host = rawEndpoint["host"] as? String, let portValue = rawEndpoint["port"] as? NSNumber,
              CFGetTypeID(portValue) != CFBooleanGetTypeID(), portValue.doubleValue == Double(portValue.intValue),
              (1...65535).contains(portValue.intValue),
              let aid = v["expectedAuthorityID"] as? String, CrewdRemoteText.uuid(aid),
              let db = v["expectedDatabaseID"] as? String, CrewdRemoteText.databaseID(db),
              let spki = v["expectedServerPin"] as? String, CrewdRemoteText.pin(spki) else { throw CrewdRemoteError.storageFailure }
        let listed = CrewdRemoteEndpoint(host: host, port: UInt16(portValue.intValue))
        guard listed.valid else { throw CrewdRemoteError.storageFailure }
        var data: URL
        let endpoint: CrewdRemoteEndpoint? = listed, authority: String? = aid, database: String? = db, pin: String? = spki
        switch platform {
        case .mac:
            guard Set(v.keys) == common.union(["dataDirectory", "workDirectory", "codexProgram", "codexSHA256", "claudeProgram", "claudeSHA256"]),
                  v["mode"] as? String == "localMac", root.path == "/private/tmp/pcw-delivery-validation." + scopeID,
                  v["dataDirectory"] as? String == root.appendingPathComponent("data").path,
                  v["workDirectory"] as? String == root.appendingPathComponent("work").path,
                  environment["PENDINGCREW_CREWD_DATA_DIR"] == v["dataDirectory"] as? String,
                  let codex = v["codexProgram"] as? String, codex == root.appendingPathComponent("bin/fake-codex").path,
                  let claude = v["claudeProgram"] as? String, claude == "/usr/bin/false",
                  environment["PENDINGCREW_CODEX_PROGRAM"] == codex, environment["PENDINGCREW_CLAUDE_PROGRAM"] == claude else { throw CrewdRemoteError.storageFailure }
            data = root.appendingPathComponent("data")
            for directory in [data, root.appendingPathComponent("work"), root.appendingPathComponent("bin")] { try privatePath(directory, root: root, directory: true) }
            let fake = URL(fileURLWithPath: codex)
            guard let resolved = realpath(fake.path, nil) else { throw CrewdRemoteError.storageFailure }
            defer { free(resolved) }
            var s = stat()
            guard String(cString: resolved) == fake.path, lstat(fake.path, &s) == 0, s.st_uid == geteuid(),
                  (s.st_mode & S_IFMT) == S_IFREG, (s.st_mode & 0o777) == 0o700 else { throw CrewdRemoteError.storageFailure }
            for (program, key) in [(codex, "codexSHA256"), (claude, "claudeSHA256")] {
                guard let expected = v[key] as? String, expected.count == 64, expected.allSatisfy({ "0123456789abcdef".contains($0) }),
                      SHA256.hash(data: try Data(contentsOf: URL(fileURLWithPath: program))).map({ String(format: "%02x", $0) }).joined() == expected else { throw CrewdRemoteError.storageFailure }
            }
        case .mobile:
            guard let container, let resolved = realpath(container.path, nil) else { throw CrewdRemoteError.storageFailure }
            defer { free(resolved) }
            let actualContainer = URL(fileURLWithPath: String(cString: resolved), isDirectory: true)
            let parent = actualContainer.appendingPathComponent("Library/Application Support/PendingCrewDeliveryValidation")
            // All validation-created container components are owner-private.
            try privatePath(parent, root: actualContainer, directory: true)
            guard Set(v.keys) == common.union(["presentationDirectory"]),
                  v["mode"] as? String == "remoteMobile", root.path == parent.appendingPathComponent(scopeID).path,
                  v["presentationDirectory"] as? String == root.appendingPathComponent("presentation").path,
                  environment["PENDINGCREW_CODEX_PROGRAM"] == nil, environment["PENDINGCREW_CLAUDE_PROGRAM"] == nil,
                  environment["PENDINGCREW_CREWD_DATA_DIR"] == nil else { throw CrewdRemoteError.storageFailure }
            data = root.appendingPathComponent("presentation");try privatePath(data, root: root, directory: true)
        }
        return Scope(root: root, data: data, service: service, account: account, actions: Set(actions),
                     manifestSHA256: SHA256.hash(data: Data(bytes)).map({ String(format: "%02x", $0) }).joined(),
                     endpoint: endpoint, authority: authority, database: database, pin: pin)
    }
    /// This narrows an already cryptographically verified invitation. It never
    /// replaces signed invitation/receipt/SPKI/grant validation or grants trust.
    static func validateInvitation(_ invite: CrewdRemoteInvitation) throws {
        try requireListedInvitation(invite, scope: qualified())
    }
    static func requireListedInvitation(_ invite: CrewdRemoteInvitation, scope: Scope) throws {
        guard invite.endpoint == scope.endpoint, invite.server.authorityId == scope.authority,
              invite.server.dbId == scope.database, invite.server.spkiSha256 == scope.pin else { throw CrewdRemoteError.wrongIdentity }
    }
    static func permits(_ action: String, account: UUID) throws -> Scope {
        let s = try qualified()
        guard s.account == account, s.actions.contains(action) else { throw CrewdRemoteError.storageFailure }
        return s
    }
}

/// Factory failure is a refusal, never a fallback to personal or memory stores.
final class CrewdRefusedCredentials: CrewdRemoteCredentialStore {
    func records() throws -> [UUID] { throw CrewdRemoteError.storageFailure }
    func read(_ id: UUID) throws -> CrewdRemoteStoredPair { throw CrewdRemoteError.storageFailure }
    func save(_ record: CrewdRemoteStoredPair) throws { throw CrewdRemoteError.storageFailure }
    func remove(_ id: UUID) throws { throw CrewdRemoteError.storageFailure }
}
final class CrewdValidationCredentials: CrewdRemoteCredentialStore {
    private let origin: CrewdDeliveryValidation.Scope
    init(scope: CrewdDeliveryValidation.Scope) { origin = scope }
    private func store(_ action: String, _ id: UUID) throws -> CrewdRemoteKeychain {
        let scope = try CrewdDeliveryValidation.permits(action, account: id)
        guard scope.root == origin.root, scope.service == origin.service, scope.account == origin.account,
              scope.manifestSHA256 == origin.manifestSHA256 else { throw CrewdRemoteError.storageFailure }
        return CrewdRemoteKeychain(service: origin.service)
    }
    func records() throws -> [UUID] {
        let s = try CrewdDeliveryValidation.qualified()
        let store = try store("records", s.account)
        return try store.boundedRecords([s.account])
    }
    private func validate(_ record: CrewdRemoteStoredPair) throws {
        let invitation = try CrewdRemotePairing(credentials: CrewdRefusedCredentials()).invitation(record.invitation, requireFresh: false)
        guard invitation.endpoint == origin.endpoint, invitation.server.authorityId == origin.authority,
              invitation.server.dbId == origin.database, invitation.server.spkiSha256 == origin.pin else { throw CrewdRemoteError.wrongIdentity }
    }
    func read(_ id: UUID) throws -> CrewdRemoteStoredPair {
        let record = try store("read", id).read(id)
        _ = try store("read", id);try validate(record)
        return record
    }
    func save(_ record: CrewdRemoteStoredPair) throws {
        let target = try store("save", record.id)
        try validate(record);_ = try store("save", record.id)
        try target.save(record)
    }
    func remove(_ id: UUID) throws { try store("remove", id).remove(id) }
}
