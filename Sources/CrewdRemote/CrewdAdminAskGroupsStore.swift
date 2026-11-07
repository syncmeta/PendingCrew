import Foundation
import Combine

/// Pure reads with a per-presentation lease. No model calls, answer writes or receipts.
@MainActor final class CrewdAdminAskGroupsStore: ObservableObject {
    typealias Wire = CrewdAdminAskGroupsWire
    typealias Origin = CrewdKnowledgeStore.Origin
    typealias RPC = CrewdKnowledgeStore.RPC
    private let origin: () -> Origin?
    private let rpc: RPC
    private let available: () -> Bool
    @Published private(set) var page: Wire.Page?
    @Published private(set) var items: [Wire.Pair] = []
    @Published private(set) var busy = false
    @Published private(set) var failure: String?
    @Published private(set) var hasUpdates = false
    @Published private(set) var invalidation = UUID()
    struct Invalidation {
        let name: String
        let data: CrewdValue
        let connection: UUID
    }
    @Published private(set) var lastInvalidation: Invalidation?
    private var ticket = UUID()
    private var source: Data?
    private var crew: String?
    private var cursors = Set<Data>()
    init(origin: @escaping () -> Origin?, rpc: @escaping RPC, available: @escaping () -> Bool) {
        self.origin = origin; self.rpc = rpc; self.available = available
    }
    /// Independent cursors/tickets for every actual sheet; the Workspace's one
    /// instance only forwards authorized, content-free invalidation hints.
    func makePresentation() -> CrewdAdminAskGroupsStore {
        .init(origin: origin, rpc: rpc, available: available)
    }
    static func identity(_ o: Origin) -> Data {
        let raw: CrewdValue = .object([
            "presentation": (try? CrewdKnowledgeStore.encode(o.scope)) ?? .null,
            "observation": .string(o.observation.uuidString),
            "connection": .string(o.connection.uuidString), "crew": .string(o.crew),
        ])
        let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys]
        return (try? encoder.encode(raw)) ?? Data()
    }
    var sourceKey: Data { guard available(), let o = origin() else { return Data() }; return Self.identity(o) }
    var supported: Bool { available() }
    var sourceCurrent: Bool { source != nil && available() && source == sourceKey }
    func owns(_ expectedCrew: String) -> Bool { sourceCurrent && crew.map { Wire.bytes($0, expectedCrew) } == true }
    func cancel() {
        ticket = UUID(); busy = false; page = nil; items = []; source = nil; crew = nil; failure = nil; cursors = []; hasUpdates = false
    }
    func handleEvent(_ name: String, _ value: CrewdValue, connection: UUID) {
        guard origin()?.connection == connection else { return }
        if name == Wire.invalidated {
            guard case .object(let fields) = value, fields.isEmpty else { return }
            // Global settings qualification changed; no private source identity.
        } else if name == Wire.changed {
            guard case .object(let f) = value, Set(f.keys) == ["crewId"], let changedCrew = value["crewId"].string,
                  Wire.opaque(changedCrew), crew.map({ Wire.bytes($0, changedCrew) }) ?? true else { return }
            // Advisory records/frontiers changed, not H text or read authority.
            // Keep a readable snapshot; a separate real source change below
            // revokes it immediately. A busy model job must not erase cards.
            hasUpdates = true
            lastInvalidation = .init(name: name, data: value, connection: connection)
            invalidation = UUID()
            return
        } else if !["events.lagged", "crew.changed", "crew.purged", "ask.changed", "work.changed", "channel.changed", "channel.purged", "message.removed", "todo.attributesChanged"].contains(name) { return }
        let sanitized: CrewdValue = .object([:])
        cancel(); lastInvalidation = .init(name: name, data: sanitized, connection: connection); invalidation = UUID()
    }
    func reload(crew requested: String, current: @MainActor () -> Bool) async {
        guard current(), !Task.isCancelled else { return }
        cancel()
        guard supported, Wire.opaque(requested), let o = origin(), current() else { failure = Wire.Problem.unavailable.localizedDescription; return }
        source = Self.identity(o); crew = requested
        await load(after: nil, o: o, current: current)
    }
    func more(current: @MainActor () -> Bool) async {
        guard current(), !busy, sourceCurrent, let o = origin(), let after = page?.nextAfter else { return }
        await load(after: after, o: o, current: current)
    }
    private func live(_ id: UUID, _ o: Origin, _ expectedCrew: String, _ current: @MainActor () -> Bool) -> Bool {
        !Task.isCancelled && current() && ticket == id && available() && source == Self.identity(o) &&
        origin().map(Self.identity) == Self.identity(o) && crew.map { Wire.bytes($0, expectedCrew) } == true
    }
    private func load(after: String?, o: Origin, current: @MainActor () -> Bool) async {
        guard let requested = crew, current(), sourceCurrent else { return }
        let id = UUID(); ticket = id; busy = true; failure = nil
        defer { if ticket == id { busy = false } }
        do {
            var fields: [String: CrewdValue] = ["crewId": .string(requested), "limit": .integer(5)]
            if let after { fields["after"] = .string(after) }
            let raw = try await rpc(Wire.list, .object(fields))
            guard live(id, o, requested, current) else { return }
            let actual = try Wire.decode(raw)
            guard Wire.bytes(actual.crewId, requested), Wire.bytes(actual.scope.dbId, o.scope.databaseID ?? "") else { throw Wire.Problem.invalid }
            if let old = page {
                guard Wire.bytes(old.scope.dbId, actual.scope.dbId), Wire.bytes(old.scope.restoreEpoch, actual.scope.restoreEpoch), actual.availability == old.availability else { throw Wire.Problem.changed }
            }
            if let next = actual.nextAfter {
                guard !cursors.contains(Data(next.utf8)), after.map({ Wire.bytes($0, next) }) != true else { throw Wire.Problem.invalid }
            }
            let previous = Set(items.map { Data($0.id.utf8) })
            guard actual.items.allSatisfy({ !previous.contains(Data($0.id.utf8)) }) else { throw Wire.Problem.invalid }
            guard live(id, o, requested, current) else { return }
            if let next = actual.nextAfter { cursors.insert(Data(next.utf8)) }
            items += actual.items; page = actual
        } catch {
            guard live(id, o, requested, current) else { return }
            page = nil; items = []; failure = error.localizedDescription
        }
    }
}

