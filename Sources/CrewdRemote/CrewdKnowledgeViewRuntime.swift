import Foundation
import Combine
import CryptoKit

// Client renderer domain, deliberately not a public RPC DTO. The frozen backend
// adapter will construct these values from the actual knowledge authority.
struct CrewdKnowledgeViewOrigin: Equatable, Hashable {
    let authority: String
    let database: String
    let restoreEpoch: String
    let crew: String
    let namespace: String
    let generation: String
    let permissionEpoch: String
}

struct CrewdKnowledgeViewBundle: Equatable {
    struct Asset: Equatable {
        let data: Data
        let mime: String
        var digest: String { SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() }
        var renderable: Bool {
            ["image/png", "image/jpeg", "image/gif", "image/webp", "font/woff2"].contains(mime)
        }
    }
    let id: String
    let revision: String
    let html: String
    let css: String
    let javascript: String
    let definitionDigest: String
    let assets: [String: Asset]
    let declaresKnowledgeWrite: Bool
    let readOnlySnapshot: Bool
    var digest: String {
        var values = [id, revision, html, css, javascript, definitionDigest, declaresKnowledgeWrite ? "write" : "read", readOnlySnapshot ? "snapshot" : "live"]
        values += assets.keys.sorted().flatMap { key in [key, assets[key]!.mime, assets[key]!.digest] }
        let encoded = try! JSONEncoder().encode(values)
        return SHA256.hash(data: encoded).map { String(format: "%02x", $0) }.joined()
    }
    var bounded: Bool {
        !id.isEmpty && !revision.isEmpty && html.utf8.count + css.utf8.count + javascript.utf8.count <= 1_048_576 && assets.count <= 128 && assets.values.reduce(0, { $0 + $1.data.count }) <= 16_777_216 &&
        assets.values.allSatisfy { $0.renderable && $0.data.count <= 8_388_608 } &&
        assets.keys.allSatisfy { !$0.isEmpty && $0.utf8.count <= 128 && $0.utf8.allSatisfy { (48...57).contains($0) || (65...90).contains($0) || (97...122).contains($0) || $0 == 45 || $0 == 95 } }
    }
}

struct CrewdKnowledgeViewContext: Equatable, Hashable {
    let origin: CrewdKnowledgeViewOrigin
    let view: String
    let revision: String
    let bundleDigest: String
    let readContext: CrewdDataContext
    let readGeneration: String
    let instance: UUID
}

struct CrewdKnowledgeViewWrite: Equatable {
    let requestID: UUID
    let callID: UUID
    let context: CrewdKnowledgeViewContext
    let input: CrewdValue
}

enum CrewdKnowledgeViewWriteObservation {
    case committed(CrewdValue)
    case rejected(CrewdValue)
    case conflicted(CrewdValue)
    case unknown
    case notFound
    case historical
}

// No generic method-name/Core bridge. Only the future immutable knowledge
// adapter implements this protocol; it must persist prepare before submission,
// validate the exact backend receipt, and finish the original origin's journal.
@MainActor protocol CrewdKnowledgeViewAuthority: AnyObject {
    func currentOrigin() -> CrewdKnowledgeViewOrigin?
    func hasUnresolvedWrite(in origin: CrewdKnowledgeViewOrigin) -> Bool
    func read(context: CrewdKnowledgeViewContext, input: CrewdValue) async throws -> CrewdValue
    func prepare(_ intent: CrewdKnowledgeViewWrite) throws
    func submit(_ intent: CrewdKnowledgeViewWrite) async throws -> CrewdKnowledgeViewWriteObservation
    func lookup(_ intent: CrewdKnowledgeViewWrite) async throws -> CrewdKnowledgeViewWriteObservation
    func navigate(context: CrewdKnowledgeViewContext, reference: CrewdValue) async throws
}

@MainActor final class CrewdKnowledgeViewRuntime: ObservableObject {
    enum Failure: Error { case unavailable, invalid, readOnly, busy, unresolved, stale }
    enum Operation: String { case read, write, lookup, navigate }
    struct Request {
        let callID: UUID
        let operation: Operation
        let input: CrewdValue
        static func decode(_ text: String) throws -> Self {
            guard text.utf8.count <= 65_536, let bytes = text.data(using: .utf8),
                  case .object(let object) = try JSONDecoder().decode(CrewdValue.self, from: bytes),
                  Set(object.keys) == ["callID", "operation", "input"],
                  let raw = object["callID"]?.string, let id = UUID(uuidString: raw),
                  id.uuidString.lowercased() == raw,
                  let op = object["operation"]?.string.flatMap(Operation.init(rawValue:)),
                  let input = object["input"], case .object = input else { throw Failure.invalid }
            return .init(callID: id, operation: op, input: input)
        }
    }
    struct Reply {
        let callID: UUID
        let value: CrewdValue
    }
    @Published private(set) var context: CrewdKnowledgeViewContext?
    @Published private(set) var bundle: CrewdKnowledgeViewBundle?
    @Published private(set) var writeEnabled = false
    @Published private(set) var pendingWrite: CrewdKnowledgeViewWrite?
    @Published private(set) var failed = false
    @Published private(set) var unsupportedResourcePolicy = false
    // Host handshake readiness, not proof of every WebKit resource policy.
    @Published private(set) var renderQualified = false
    @Published private(set) var executionEnabled = false
    enum ResourceDenial: String, CaseIterable { case filePanel, mediaCapture, geolocation, newWindow, navigation }
    @Published private(set) var resourceDenials: [ResourceDenial: Int] = [:]
    func observedResourceDenial(_ reason: ResourceDenial) {
        // Closed counters only; never URLs, file names, origins or author text.
        guard context != nil, executionEnabled else { return }
        resourceDenials[reason] = min(255, (resourceDenials[reason] ?? 0) + 1)
    }
    private weak var authority: CrewdKnowledgeViewAuthority?
    private var readCalls = Set<UUID>()
    private var writeCalls: [UUID: CrewdKnowledgeViewWrite] = [:]

    init(authority: CrewdKnowledgeViewAuthority) { self.authority = authority }
    func activate(_ value: CrewdKnowledgeViewBundle, origin: CrewdKnowledgeViewOrigin, readContext: CrewdDataContext, readGeneration: String) throws {
        guard value.bounded, authority?.currentOrigin() == origin else { throw Failure.unavailable }
        if let context, context.origin == origin, context.view == value.id,
           context.revision == value.revision, context.bundleDigest == value.digest,
           context.readContext == readContext, context.readGeneration == readGeneration { return }
        revoke()
        bundle = value
        context = .init(origin: origin, view: value.id, revision: value.revision, bundleDigest: value.digest, readContext: readContext, readGeneration: readGeneration, instance: UUID())
    }
    func revoke() {
        // Retained write intents are not replayed or discarded. The adapter's
        // durable journal remains authoritative even after this runtime dies.
        writeEnabled = false; context = nil; bundle = nil; readCalls.removeAll(); failed = false; renderQualified = false; executionEnabled = false; unsupportedResourcePolicy = false; resourceDenials.removeAll()
    }
    func synchronizeAuthority() {
        if let context, authority?.currentOrigin() != context.origin { revoke() }
        if let pendingWrite, authority?.currentOrigin() == pendingWrite.context.origin,
           authority?.hasUnresolvedWrite(in: pendingWrite.context.origin) == false {
            self.pendingWrite = nil
        }
    }
    func renderFailed(resourcePolicyUnsupported: Bool = false) { writeEnabled = false; failed = true; unsupportedResourcePolicy = resourcePolicyUnsupported; renderQualified = false; executionEnabled = false }
    func renderReady() {
        guard let context, authority?.currentOrigin() == context.origin, !failed, executionEnabled else { return }
        renderQualified = true
    }
    /// Only a native host action may enable this captured code revision. This
    /// is separate from the knowledge Apply grant and creates no clipboard API.
    func setExecutionEnabledFromNativeHost(_ enabled: Bool, context expected: CrewdKnowledgeViewContext) throws {
        synchronizeAuthority()
        guard let current = context, current == expected, bundle != nil,
              authority?.currentOrigin() == current.origin, !failed else { throw Failure.stale }
        if enabled { executionEnabled = true; return }
        executionEnabled = false; writeEnabled = false; renderQualified = false
        readCalls.removeAll(); resourceDenials.removeAll()
        // A late response from a stopped WebKit instance cannot re-enter a
        // subsequently enabled instance, even for unchanged code and origin.
        context = .init(origin: current.origin, view: current.view, revision: current.revision,
                        bundleDigest: current.bundleDigest, readContext: current.readContext,
                        readGeneration: current.readGeneration, instance: UUID())
    }
    func setWriteEnabledFromNativeHost(_ enabled: Bool) throws {
        synchronizeAuthority()
        guard let context, let bundle, let authority, authority.currentOrigin() == context.origin, !failed, renderQualified, executionEnabled else { throw Failure.unavailable }
        guard !enabled || (bundle.declaresKnowledgeWrite && !bundle.readOnlySnapshot) else { throw Failure.readOnly }
        guard !enabled || (!blocked(context) && !authority.hasUnresolvedWrite(in: context.origin)) else { throw Failure.unresolved }
        writeEnabled = enabled
    }
    private func current(_ expected: CrewdKnowledgeViewContext) -> Bool {
        context == expected && authority?.currentOrigin() == expected.origin && !failed && renderQualified && executionEnabled
    }
    private func blocked(_ context: CrewdKnowledgeViewContext) -> Bool {
        pendingWrite?.context.origin == context.origin
    }
    private func payload(_ kind: String, _ value: CrewdValue = .null) -> CrewdValue {
        .object(["status": .string(kind), "value": value])
    }
    func handle(_ request: Request, from expected: CrewdKnowledgeViewContext) async throws -> Reply {
        synchronizeAuthority()
        guard current(expected), let authority else { throw Failure.stale }
        switch request.operation {
        case .read, .navigate:
            guard readCalls.count < 4, readCalls.insert(request.callID).inserted else { throw Failure.busy }
            defer { readCalls.remove(request.callID) }
            if request.operation == .navigate {
                try await authority.navigate(context: expected, reference: request.input)
                guard current(expected) else { throw Failure.stale }
                return .init(callID: request.callID, value: payload("opened"))
            }
            let result = try await authority.read(context: expected, input: request.input)
            guard current(expected) else { throw Failure.stale }
            return .init(callID: request.callID, value: payload("read", result))
        case .write:
            guard writeEnabled, bundle?.declaresKnowledgeWrite == true, bundle?.readOnlySnapshot == false else { throw Failure.readOnly }
            // Repeated JS call IDs never become a new backend request. An
            // unknown write blocks new IDs too, until the host queries it.
            guard writeCalls[request.callID] == nil else { throw Failure.unresolved }
            guard !blocked(expected), !authority.hasUnresolvedWrite(in: expected.origin) else { throw Failure.unresolved }
            let intent = CrewdKnowledgeViewWrite(requestID: UUID(), callID: request.callID, context: expected, input: request.input)
            writeCalls[request.callID] = intent; pendingWrite = intent
            let observation: CrewdKnowledgeViewWriteObservation
            do { try authority.prepare(intent); observation = try await authority.submit(intent) }
            catch { observation = .unknown }
            guard current(expected) else { throw Failure.stale }
            return reply(observation, intent: intent)
        case .lookup:
            // JS may only query its own retained request, never supply a new
            // arbitrary backend request UUID or recreate a secret intent.
            guard case .object(let lookupInput) = request.input, Set(lookupInput.keys) == ["writeCallID"],
                  let raw = request.input["writeCallID"].string, let call = UUID(uuidString: raw),
                  let intent = writeCalls[call], intent.context == expected else { throw Failure.invalid }
            let observation = try await authority.lookup(intent)
            guard current(expected) else { throw Failure.stale }
            let result = reply(observation, intent: intent)
            return .init(callID: request.callID, value: result.value)
        }
    }
    func reviewPendingFromNativeHost() async throws -> CrewdKnowledgeViewWriteObservation {
        synchronizeAuthority()
        guard let context, let intent = pendingWrite, let authority,
              context.origin == intent.context.origin,
              authority.currentOrigin() == intent.context.origin else { throw Failure.unavailable }
        let observation = try await authority.lookup(intent)
        guard self.context?.origin == intent.context.origin,
              authority.currentOrigin() == intent.context.origin else { throw Failure.stale }
        switch observation {
        case .committed, .rejected, .conflicted: if pendingWrite == intent { pendingWrite = nil }; writeEnabled = false
        default: break
        }
        return observation
    }
    private func reply(_ observation: CrewdKnowledgeViewWriteObservation, intent: CrewdKnowledgeViewWrite) -> Reply {
        let value: CrewdValue
        switch observation {
        case .committed(let result):
            writeEnabled = false
            if pendingWrite == intent { pendingWrite = nil }
            value = payload("committed", result)
        case .rejected(let result):
            writeEnabled = false
            if pendingWrite == intent { pendingWrite = nil }
            value = payload("rejected", result)
        case .conflicted(let result):
            writeEnabled = false
            if pendingWrite == intent { pendingWrite = nil }
            value = payload("conflicted", result)
        case .unknown: value = payload("unknown")
        case .notFound: value = payload("not_found")
        case .historical: value = payload("historical")
        }
        return .init(callID: intent.callID, value: value)
    }
}
