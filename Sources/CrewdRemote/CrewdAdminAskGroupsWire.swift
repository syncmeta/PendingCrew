import Foundation

/// Read-only suggestions. Original H resolution keeps its own existing receipt protocol.
enum CrewdAdminAskGroupsWire {
    static let capability = "adminAskGroups.v1"
    static let list = "adminAskGroups.list"
    static let changed = "adminAskGroups.changed"
    static let invalidated = "adminAskGroups.invalidated"
    static let topic = "adminAskGroups"
    struct Scope: Codable, Equatable { let dbId: String; let restoreEpoch: String }
    enum Availability: String, Codable { case disabled, unconfigured, ready, archived }
    enum State: String, Codable { case prepared, dispatched, suggested, different, unavailable, unknown, stale }
    struct Question: Codable, Equatable {
        let id: String
        let question: String
        let questionTruncated: Bool
        let resolutionRevision: String
        let sourceChannelId: String
        let sourceMessageId: String
        let askedAtMs: Int64
        let `default`: String?
        let defaultTruncated: Bool
        let defaultDueAtMs: Int64?
    }
    struct Pair: Codable, Equatable, Identifiable {
        let id: String
        let first: Question
        let second: Question
        let state: State
        let probability: Double?
        let observedAtMs: Int64?
        let excerptTruncated: Bool
    }
    struct Page: Codable, Equatable {
        let scope: Scope
        let crewId: String
        let availability: Availability
        let observedAtMs: Int64
        let items: [Pair]
        let nextAfter: String?
        let partialCoverage: Bool
        let historicalBaseline: Bool
    }
    enum Problem: LocalizedError {
        case unavailable, changed, invalid
        var errorDescription: String? {
            switch self {
            case .unavailable: "当前连接不支持同类待答问题整理。"
            case .changed: "来源已变化，请重新读取同类待答问题。"
            case .invalid: "无法核对整理结果；原问题及答复未作更改。"
            }
        }
    }
    static func bytes(_ a: String, _ b: String) -> Bool { Data(a.utf8) == Data(b.utf8) }
    static func hex(_ s: String) -> Bool { s.utf8.count == 64 && s.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) } }
    static func uuid(_ s: String) -> Bool { UUID(uuidString: s)?.uuidString.lowercased() == s }
    static func number(_ s: String) -> Int64? {
        guard s.first == "H", let n = Int64(s.dropFirst()), n > 0, bytes("H" + String(n), s) else { return nil }
        return n
    }
    static func opaque(_ s: String, max: Int = 512) -> Bool { !s.isEmpty && s.utf8.count <= max && !s.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains) }
    private static func closed(_ v: CrewdValue, _ required: Set<String>, _ optional: Set<String> = []) throws {
        guard case .object(let f) = v, required.isSubset(of: Set(f.keys)), Set(f.keys).isSubset(of: required.union(optional)) else { throw Problem.invalid }
    }
    static func decode(_ value: CrewdValue) throws -> Page {
        try closed(value, ["scope", "crewId", "availability", "observedAtMs", "items", "partialCoverage", "historicalBaseline"], ["nextAfter"])
        try closed(value["scope"], ["dbId", "restoreEpoch"])
        guard case .array(let rows) = value["items"], rows.count <= 20 else { throw Problem.invalid }
        for row in rows {
            try closed(row, ["id", "first", "second", "state", "excerptTruncated"], ["probability", "observedAtMs"])
            for key in ["first", "second"] {
                try closed(row[key], ["id", "question", "questionTruncated", "resolutionRevision", "sourceChannelId", "sourceMessageId", "askedAtMs", "defaultTruncated"], ["default", "defaultDueAtMs"])
            }
        }
        let encoder = JSONEncoder(); encoder.outputFormatting = [.withoutEscapingSlashes]
        let raw = try encoder.encode(value)
        // This is a bound on the parsed local representation, not a claim to
        // recover original socket bytes with another language's JSON spelling.
        // Core independently enforces its actual encoded 24-KiB page budget.
        guard raw.count <= 512 * 1024 else { throw Problem.invalid }
        let page = try JSONDecoder().decode(Page.self, from: raw)
        guard opaque(page.crewId), opaque(page.scope.dbId, max: 256), uuid(page.scope.restoreEpoch), page.items.count <= 20,
              (!page.historicalBaseline || page.partialCoverage), page.items.allSatisfy(valid),
              Set(page.items.map { Data($0.id.utf8) }).count == page.items.count,
              page.nextAfter.map({ opaque($0, max: 129) && $0.utf8.count == 129 && $0.split(separator: ":", omittingEmptySubsequences: false).count == 2 && $0.split(separator: ":").allSatisfy { hex(String($0)) } }) ?? true,
              page.availability == .ready || (page.items.isEmpty && page.nextAfter == nil) else { throw Problem.invalid }
        return page
    }
    static func valid(_ q: Question) -> Bool {
        number(q.id) != nil && !q.question.isEmpty && q.question.utf8.count <= 1024 && hex(q.resolutionRevision) &&
        opaque(q.sourceChannelId) && opaque(q.sourceMessageId) &&
        (q.default?.utf8.count ?? 0) <= 512 && (!q.defaultTruncated || q.default != nil) &&
        ((q.default == nil) == (q.defaultDueAtMs == nil))
    }
    static func valid(_ p: Pair) -> Bool {
        guard hex(p.id), valid(p.first), valid(p.second), let a = number(p.first.id), let b = number(p.second.id), a < b,
              p.excerptTruncated == (p.first.questionTruncated || p.first.defaultTruncated || p.second.questionTruncated || p.second.defaultTruncated),
              p.probability.map({ $0.isFinite && (0...1).contains($0) }) ?? true else { return false }
        switch p.state {
        case .prepared, .dispatched: return p.probability == nil && p.observedAtMs == nil
        case .suggested, .different:
            return p.observedAtMs != nil && p.probability.map { ($0 >= 0.8) == (p.state == .suggested) } == true
        case .unknown, .unavailable, .stale: return p.probability == nil
        }
    }
}
