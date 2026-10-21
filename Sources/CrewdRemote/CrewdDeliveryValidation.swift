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
    /// Closed diagnostics contain no raw configuration or credential values.
    enum QualificationStage: String, CaseIterable {
        case entryArguments, bundleIdentifier, validationMarker, sourceStamp, manifestEnvironment
        case canonicalPathUnavailable, canonicalPathMismatch, pathContainment, pathMetadata
        case manifestNameAndRef, manifestOpen, manifestDescriptor, manifestRead, manifestChanged
        case manifestIdentityAndActions, endpointIdentity, endpointShape, macScopeAndEnvironment
        case databaseIDStringCast, databaseIDEmpty, databaseIDUTF8Bound, databaseIDTrimChanged, databaseIDControlScalar
        case providerCanonicalPath, providerMetadata, providerHash, mobileContainer, mobileScopeAndEnvironment
        case credentialAction, workflowLease, unexpected
    }
    enum QualificationMatch: String, CaseIterable {
        case entryArgumentsEmpty, bundleIdentifierMatches, validationMarkerMatches, sourceStampPresent, manifestEnvironmentPresent

    }
    struct QualificationRejection: LocalizedError {
        let stage: QualificationStage
        let matches: [QualificationMatch: Bool]
        init(_ stage: QualificationStage, matches: [QualificationMatch: Bool] = [:]) {
            self.stage = stage; self.matches = matches
        }
        // Preserve the existing generic user-facing storage failure.
        var errorDescription: String? { CrewdRemoteError.storageFailure.errorDescription }
    }
    static func requireEntryArguments(_ arguments: [String]) throws {
        guard arguments.dropFirst().isEmpty else {
            throw QualificationRejection(.entryArguments, matches: [.entryArgumentsEmpty: false])
        }
    }
    static func startupDiagnostic(_ error: Error, entryArgumentsEmpty: Bool) -> String {
        let rejection = error as? QualificationRejection
        var matches = rejection?.matches ?? [:]
        matches[.entryArgumentsEmpty] = entryArgumentsEmpty
        let event: [String: Any] = [
            "kind": "deliveryValidationAdmissionRefused",
            "stage": (rejection?.stage ?? .unexpected).rawValue,
            "matches": Dictionary(uniqueKeysWithValues: matches.map { ($0.key.rawValue, $0.value) })
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: event, options: [.sortedKeys]),
              let text = String(data: data, encoding: .utf8) else {
            return "{\"kind\":\"deliveryValidationAdmissionRefused\",\"matches\":{},\"stage\":\"unexpected\"}\n"
        }
        return text + "\n"
    }
    static func qualified() throws -> Scope {
        // Capture the original application context once; no fallback bundle or
        // environment substitution is permitted.
        let bundle = Bundle.main
        let identifier = bundle.bundleIdentifier
        let marker = bundle.object(forInfoDictionaryKey: "PendingCrewDeliveryValidation") as? Bool
        let ref = bundle.object(forInfoDictionaryKey: "PendingCrewSourceRef") as? String
        let environment = ProcessInfo.processInfo.environment
        #if os(macOS)
        let scope = try qualifyApplication(bundleIdentifier: identifier, validationMarker: marker, sourceRef: ref,
                                           environment: environment, platform: .mac)
        #if PENDINGCREW_DELIVERY_VALIDATION
        try CrewdValidationWorkflowLease.processLifetime.admit(root: scope.root, manifestSHA256: scope.manifestSHA256) {
            let current = try qualifyApplication(bundleIdentifier: identifier, validationMarker: marker, sourceRef: ref,
                                                 environment: environment, platform: .mac)
            guard current.manifestSHA256 == scope.manifestSHA256 else { throw QualificationRejection(.workflowLease) }
        }
        #endif
        return scope
        #else
        // Delayed until the existing outer qualification succeeds, as before.
        return try qualifyApplication(bundleIdentifier: identifier, validationMarker: marker, sourceRef: ref,
                                      environment: environment, platform: .mobile,
                                      container: { URL(fileURLWithPath: NSHomeDirectory(), isDirectory: true) })
        #endif
    }
    static func qualifyApplication(bundleIdentifier: String?, validationMarker: Bool?, sourceRef: String?,
                                   environment: [String: String], platform: Platform,
                                   container: (() -> URL)? = nil) throws -> Scope {
        let matches: [QualificationMatch: Bool] = [
            .bundleIdentifierMatches: bundleIdentifier == "com.pendingname.pendingcrew.native-candidate",
            .validationMarkerMatches: validationMarker == true,
            .sourceStampPresent: sourceRef != nil,
            .manifestEnvironmentPresent: environment["PENDINGCREW_VALIDATION_MANIFEST"] != nil
        ]
        do {
            guard bundleIdentifier == "com.pendingname.pendingcrew.native-candidate" else { throw QualificationRejection(.bundleIdentifier) }
            guard validationMarker == true else { throw QualificationRejection(.validationMarker) }
            guard let ref = sourceRef else { throw QualificationRejection(.sourceStamp) }
            guard let path = environment["PENDINGCREW_VALIDATION_MANIFEST"] else { throw QualificationRejection(.manifestEnvironment) }
            return try qualify(manifest: URL(fileURLWithPath: path), sourceRef: ref, environment: environment,
                               platform: platform, container: container?())
        } catch let failure as QualificationRejection {
            throw QualificationRejection(failure.stage, matches: matches.merging(failure.matches) { _, inner in inner })
        } catch {
            // Do not include localized descriptions, URLs, paths or raw errors.
            throw QualificationRejection(.unexpected, matches: matches)
        }
    }
    static func privatePath(_ url: URL, root: URL?, directory: Bool) throws {
        guard url.path.hasPrefix("/"), let resolved = realpath(url.path, nil) else { throw QualificationRejection(.canonicalPathUnavailable) }
        defer { free(resolved) }
        guard String(cString: resolved) == url.path else { throw QualificationRejection(.canonicalPathMismatch) }
        if let root { guard url.path.hasPrefix(root.path + "/") else { throw QualificationRejection(.pathContainment) } }
        var s = stat()
        guard lstat(url.path, &s) == 0, s.st_uid == geteuid(),
              (s.st_mode & S_IFMT) == (directory ? S_IFDIR : S_IFREG),
              (s.st_mode & 0o777) == (directory ? 0o700 : 0o600) else { throw QualificationRejection(.pathMetadata) }
    }
    static func qualify(manifest: URL, sourceRef: String, environment: [String: String], platform: Platform = .mac, container: URL? = nil) throws -> Scope {
        guard sourceRef.count == 40, sourceRef.allSatisfy({ "0123456789abcdef".contains($0) }), manifest.lastPathComponent == "scope.json" else { throw QualificationRejection(.manifestNameAndRef) }
        let root = manifest.deletingLastPathComponent()
        try privatePath(root, root: nil, directory: true)
        try privatePath(manifest, root: root, directory: false)
        let fd = open(manifest.path, O_RDONLY | O_NOFOLLOW | O_CLOEXEC)
        guard fd >= 0 else { throw QualificationRejection(.manifestOpen) }
        defer { close(fd) }
        var first = stat()
        guard fstat(fd, &first) == 0, first.st_uid == geteuid(), (first.st_mode & S_IFMT) == S_IFREG,
              (first.st_mode & 0o777) == 0o600, first.st_size > 0, first.st_size <= 65536 else { throw QualificationRejection(.manifestDescriptor) }
        var bytes = [UInt8](repeating: 0, count: Int(first.st_size))
        guard read(fd, &bytes, bytes.count) == bytes.count else { throw QualificationRejection(.manifestRead) }
        var last = stat()
        guard fstat(fd, &last) == 0, first.st_ino == last.st_ino, first.st_size == last.st_size,
              first.st_mtimespec.tv_sec == last.st_mtimespec.tv_sec, first.st_mtimespec.tv_nsec == last.st_mtimespec.tv_nsec else { throw QualificationRejection(.manifestChanged) }
        guard let v = try JSONSerialization.jsonObject(with: Data(bytes)) as? [String: Any],
              let format = v["format"] as? NSNumber, CFGetTypeID(format) != CFBooleanGetTypeID(), Set(["c", "s", "i", "l", "q", "C", "S", "I", "L", "Q"]).contains(String(cString: format.objCType)), format.doubleValue == 1.0, format.intValue == 1, v["sourceRef"] as? String == sourceRef,
              v["validationDomain"] as? String == "pendingcrew.fake-only.delivery-validation.v1",
              let scopeID = v["scopeID"] as? String, let uuid = UUID(uuidString: scopeID), scopeID == uuid.uuidString.lowercased(),
              let service = v["service"] as? String, service == "com.pendingname.pendingcrew.delivery-validation." + scopeID,
              let accountText = v["account"] as? String, let account = UUID(uuidString: accountText), accountText == account.uuidString.lowercased(),
              let actions = v["actions"] as? [String], Set(actions).count == actions.count,
              !actions.isEmpty, Set(actions).isSubset(of: ["records", "read", "save", "remove"]) else { throw QualificationRejection(.manifestIdentityAndActions) }
        let common: Set<String> = ["format", "sourceRef", "scopeID", "service", "account", "actions", "mode", "validationDomain", "endpoint", "expectedAuthorityID", "expectedDatabaseID", "expectedServerPin"]
        guard let rawEndpoint = v["endpoint"] as? [String: Any], Set(rawEndpoint.keys) == ["host", "port"],
              let host = rawEndpoint["host"] as? String, let portValue = rawEndpoint["port"] as? NSNumber,
              CFGetTypeID(portValue) != CFBooleanGetTypeID(), portValue.doubleValue == Double(portValue.intValue),
              (1...65535).contains(portValue.intValue),
              let aid = v["expectedAuthorityID"] as? String, CrewdRemoteText.uuid(aid) else { throw QualificationRejection(.endpointIdentity) }
        // Evaluate the cast and each existing database predicate exactly once,
        // at their original position between authority and SPKI checks. Scope
        // receives the accepted value from this same evaluation, not a recheck.
        guard let databaseText = v["expectedDatabaseID"] as? String else { throw QualificationRejection(.databaseIDStringCast) }
        let db: String
        switch CrewdRemoteText.validateDatabaseID(databaseText) {
        case .accepted(let original): db = original
        case .rejected(let reason):
            switch reason {
            case .empty: throw QualificationRejection(.databaseIDEmpty)
            case .utf8Bound: throw QualificationRejection(.databaseIDUTF8Bound)
            case .trimChanged: throw QualificationRejection(.databaseIDTrimChanged)
            case .controlScalar: throw QualificationRejection(.databaseIDControlScalar)
            }
        }
        guard let spki = v["expectedServerPin"] as? String, CrewdRemoteText.pin(spki) else { throw QualificationRejection(.endpointIdentity) }
        let listed = CrewdRemoteEndpoint(host: host, port: UInt16(portValue.intValue))
        guard listed.valid else { throw QualificationRejection(.endpointShape) }
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
                  environment["PENDINGCREW_CODEX_PROGRAM"] == codex, environment["PENDINGCREW_CLAUDE_PROGRAM"] == claude,
                  environment["PENDINGCREW_CREWD_PROGRAM"] == nil else { throw QualificationRejection(.macScopeAndEnvironment) }
            data = root.appendingPathComponent("data")
            for directory in [data, root.appendingPathComponent("work"), root.appendingPathComponent("bin")] { try privatePath(directory, root: root, directory: true) }
            let fake = URL(fileURLWithPath: codex)
            guard let resolved = realpath(fake.path, nil) else { throw QualificationRejection(.providerCanonicalPath) }
            defer { free(resolved) }
            var s = stat()
            guard String(cString: resolved) == fake.path, lstat(fake.path, &s) == 0, s.st_uid == geteuid(),
                  (s.st_mode & S_IFMT) == S_IFREG, (s.st_mode & 0o777) == 0o700 else { throw QualificationRejection(.providerMetadata) }
            for (program, key) in [(codex, "codexSHA256"), (claude, "claudeSHA256")] {
                guard let expected = v[key] as? String, expected.count == 64, expected.allSatisfy({ "0123456789abcdef".contains($0) }),
                      SHA256.hash(data: try Data(contentsOf: URL(fileURLWithPath: program))).map({ String(format: "%02x", $0) }).joined() == expected else { throw QualificationRejection(.providerHash) }
            }
        case .mobile:
            guard let container, let resolved = realpath(container.path, nil) else { throw QualificationRejection(.mobileContainer) }
            defer { free(resolved) }
            let actualContainer = URL(fileURLWithPath: String(cString: resolved), isDirectory: true)
            let parent = actualContainer.appendingPathComponent("Library/Application Support/PendingCrewDeliveryValidation")
            // All validation-created container components are owner-private.
            try privatePath(parent, root: actualContainer, directory: true)
            guard Set(v.keys) == common.union(["presentationDirectory"]),
                  v["mode"] as? String == "remoteMobile", root.path == parent.appendingPathComponent(scopeID).path,
                  v["presentationDirectory"] as? String == root.appendingPathComponent("presentation").path,
                  environment["PENDINGCREW_CODEX_PROGRAM"] == nil, environment["PENDINGCREW_CLAUDE_PROGRAM"] == nil,
                  environment["PENDINGCREW_CREWD_DATA_DIR"] == nil, environment["PENDINGCREW_CREWD_PROGRAM"] == nil else { throw QualificationRejection(.mobileScopeAndEnvironment) }
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
        guard s.account == account, s.actions.contains(action) else { throw QualificationRejection(.credentialAction) }
        return s
    }
}

/// Factory failure is a refusal, never a fallback to personal or memory stores.
@CrewdCredentialActor final class CrewdRefusedCredentials: CrewdRemoteCredentialStore {
    func records() throws -> [UUID] { throw CrewdRemoteError.storageFailure }
    func read(_ id: UUID) throws -> CrewdRemoteStoredPair { throw CrewdRemoteError.storageFailure }
    func save(_ record: CrewdRemoteStoredPair) throws { throw CrewdRemoteError.storageFailure }
    func remove(_ id: UUID) throws { throw CrewdRemoteError.storageFailure }
}
@CrewdCredentialActor final class CrewdValidationCredentials: CrewdRemoteCredentialStore {
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

#if os(macOS) && PENDINGCREW_DELIVERY_VALIDATION
/// This holder belongs to the process, never to a workspace, credentials store,
/// connection, or presentation. Retirement cannot release its shared lease.
/// The lease confers no credential, IPC, or Human authority.
final class CrewdValidationWorkflowLease {
    static let processLifetime = CrewdValidationWorkflowLease()
    private let mutex = NSLock()
    private var held: (fd: Int32, path: String, manifest: String)?

    private func reject() -> CrewdDeliveryValidation.QualificationRejection {
        CrewdDeliveryValidation.QualificationRejection(.workflowLease)
    }
    private func checked(_ fd: Int32, path: String) throws {
        guard let canonical = realpath(path, nil) else { throw reject() }
        defer { free(canonical) }
        guard String(cString: canonical) == path else { throw reject() }
        var descriptor = stat(), named = stat()
        guard fstat(fd, &descriptor) == 0, lstat(path, &named) == 0,
              descriptor.st_dev == named.st_dev, descriptor.st_ino == named.st_ino,
              descriptor.st_uid == geteuid(), named.st_uid == geteuid(),
              descriptor.st_nlink == 1, named.st_nlink == 1,
              descriptor.st_size == 0, named.st_size == 0,
              (descriptor.st_mode & S_IFMT) == S_IFREG, (named.st_mode & S_IFMT) == S_IFREG,
              (descriptor.st_mode & 0o7777) == 0o600, (named.st_mode & 0o7777) == 0o600 else { throw reject() }
    }
    func admit(root: URL, manifestSHA256: String, revalidate: () throws -> Void) throws {
        mutex.lock(); defer { mutex.unlock() }
        try CrewdDeliveryValidation.privatePath(root, root: nil, directory: true)
        let path = root.appendingPathComponent("validation-execution.lease").path
        if let existing = held {
            guard existing.path == path, existing.manifest == manifestSHA256 else { throw reject() }
            try checked(existing.fd, path: path)
            try revalidate()
            try checked(existing.fd, path: path)
            return
        }
        // No creation, replacement, chmod or truncation by the App. Publisher
        // initializes this exact inode in a separately reviewed transaction.
        // Reject special files before open; NONBLOCK also closes a regular ->
        // FIFO race without ever waiting for an unrelated writer.
        var candidate = stat()
        guard lstat(path, &candidate) == 0, candidate.st_uid == geteuid(),
              (candidate.st_mode & S_IFMT) == S_IFREG,
              (candidate.st_mode & 0o7777) == 0o600,
              candidate.st_nlink == 1, candidate.st_size == 0 else { throw reject() }
        let fd = open(path, O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK)
        guard fd >= 0 else { throw reject() }
        var retained = false
        defer { if !retained { close(fd) } }
        try checked(fd, path: path)
        guard flock(fd, LOCK_SH | LOCK_NB) == 0 else { throw reject() }
        try checked(fd, path: path)
        // Close the prequalification -> SH race: publication may have replaced
        // scope.json before this SH acquisition. Validate original predicates
        // again while SH is held, comparing the exact qualified bytes.
        try revalidate()
        try checked(fd, path: path)
        held = (fd, path, manifestSHA256)
        retained = true
    }
    // Tests may own an independent holder; the static processLifetime holder
    // remains strongly rooted until process termination.
    deinit { if let value = held { close(value.fd) } }
}
#endif
