#if os(macOS)
import Foundation
import Combine

/// Public identity, never inferred from a legacy per-crew number or converted to Double.
struct CrewdWorkID: Hashable, Sendable {
    enum Kind: Character, Sendable { case requirement = "#", plan = "W", human = "H" }
    let kind: Kind
    let raw: String
    init?(_ raw: String, kind: Kind) {
        let digits = raw.dropFirst()
        guard raw.first == kind.rawValue, !digits.isEmpty, digits.first != "0",
              digits.allSatisfy({ $0.isASCII && $0.isNumber }) else { return nil }
        self.raw = raw; self.kind = kind
    }
}

struct CrewdPlan: Equatable, Sendable {
    let id: CrewdWorkID
    var summary: CrewdValue
    var detail: CrewdValue?
    var value: CrewdValue {
        guard case .object(var fields) = detail, case .object(let latest) = summary else { return summary }
        // Summary members are authoritative too, including explicit null clears.
        fields.merge(latest) { _, new in new }; return .object(fields)
    }
    var revision: String { summary["revision"].string ?? "" }
    var status: String { summary["status"].string ?? "" }
    var crew: String { summary["crewId"].string ?? "" }
    var frozen: Bool { summary["frozen"] == .bool(true) }
}

/// One backend-owned organization snapshot. Todo and progress contain references to these
/// same plans; they cannot independently mutate a status or synthesize a completion ratio.
@MainActor
final class CrewdWorkStore: ObservableObject {
    typealias Call = @MainActor (String, CrewdValue) async throws -> CrewdValue
    @Published private(set) var plans: [CrewdWorkID: CrewdPlan] = [:]
    @Published private(set) var requirements: [CrewdWorkID: CrewdValue] = [:]
    @Published private(set) var humanTasks: [CrewdWorkID: CrewdValue] = [:]
    @Published private(set) var selectedCrew = ""
    @Published private(set) var planIDs: [CrewdWorkID] = []
    @Published private(set) var requirementIDs: [CrewdWorkID] = []
    @Published private(set) var lastFailure: CrewdClientError?
    @Published private(set) var loading = false
    private let call: Call
    private var generation = UUID()
    private var refreshID = UUID()
    private var activeMutations = 0
    private var pendingDetails: [CrewdWorkID: UUID] = [:]

    init(call: @escaping Call) { self.call = call }
    var progress: [CrewdPlan] { planIDs.compactMap { plans[$0] } }
    var todoPlans: [CrewdPlan] { progress }
    func plans(from requirement: CrewdWorkID) -> [CrewdPlan] {
        plans.values.filter { $0.detail?["sources"].array.contains(.string(requirement.raw)) == true }
            .sorted { $0.id.raw < $1.id.raw }
    }
    func selectCrew(_ crew: String) {
        guard selectedCrew != crew else { return }
        generation = UUID(); refreshID = UUID(); pendingDetails.removeAll(); loading = false
        selectedCrew = crew; planIDs = []; requirementIDs = []; lastFailure = nil
    }
    /// Stops old reads. Reconnect reconciles by reading; never resends a mutation.
    func invalidateConnection() {
        generation = UUID(); refreshID = UUID(); pendingDetails.removeAll(); loading = false
    }
    private func check(_ token: UUID, _ refresh: UUID? = nil) throws {
        guard token == generation, !Task.isCancelled, refresh == nil || refresh == refreshID else { throw CancellationError() }
    }
    private func failure(_ error: Error) -> CrewdClientError {
        (error as? CrewdClientError) ?? CrewdClientError(message: error.localizedDescription)
    }
    private func parsePlan(_ value: CrewdValue, full: Bool) throws -> CrewdPlan {
        guard let raw = value["id"].string, let id = CrewdWorkID(raw, kind: .plan),
              let revision = value["revision"].string, !revision.isEmpty,
              revision.first != "0", revision.allSatisfy({ $0.isASCII && $0.isNumber }),
              value["crewId"].string != nil, value["status"].string != nil else {
            throw CrewdClientError(message: "工作契约缺少稳定编号、机组或字符串版本。")
        }
        return CrewdPlan(id: id, summary: value, detail: full ? value : nil)
    }
    private func adopt(_ incoming: CrewdPlan) {
        if let old = plans[incoming.id] {
            let a = old.revision, b = incoming.revision
            if a.count > b.count || (a.count == b.count && a > b) { return }
            if a == b && incoming.detail == nil {
                var merged = incoming; merged.detail = old.detail; plans[incoming.id] = merged; return
            }
        }
        plans[incoming.id] = incoming
    }
    func refresh() async throws {
        guard activeMutations == 0 else { return }
        let token = generation, current = UUID(), crew = selectedCrew
        refreshID = current; loading = true; lastFailure = nil
        defer { if refreshID == current { loading = false } }
        do {
            var rows: [CrewdPlan] = [], cursor: String? = nil, seen = Set<String>(), planSeen = Set<CrewdWorkID>()
            if !crew.isEmpty {
                repeat {
                    var params: [String: CrewdValue] = ["crewId": .string(crew), "limit": .integer(100)]
                    if let cursor { params["after"] = .string(cursor) }
                    let page = try await call("work.list", .object(params)); try check(token, current)
                    guard case .array(let items) = page["items"] else { throw CrewdClientError(message: "工作列表契约无效。") }
                    for value in items {
                        let row = try parsePlan(value, full: false)
                        guard row.crew == crew, planSeen.insert(row.id).inserted else { throw CrewdClientError(message: "工作分页返回了其他机组或重复对象。") }
                        rows.append(row)
                    }
                    if page["nextAfter"] == .null { cursor = nil }
                    else {
                        guard let next = page["nextAfter"].string, CrewdWorkID(next, kind: .plan) != nil,
                              seen.insert(next).inserted else { throw CrewdClientError(message: "工作分页游标无效或重复。") }
                        cursor = next
                    }
                } while cursor != nil
            }
            var sources: [(CrewdWorkID, CrewdValue)] = []
            if !crew.isEmpty {
                let raw = try await call("todo.list", .object(["crewId": .string(crew), "ledger": .string("agent")]))
                try check(token, current)
                guard case .array(let items) = raw else { throw CrewdClientError(message: "原始需求列表契约无效。") }
                for value in items {
                    guard let rawID = value["requirementId"].string, let id = CrewdWorkID(rawID, kind: .requirement), value["crewId"].string == crew, !sources.contains(where: { $0.0 == id }) else {
                        throw CrewdClientError(message: "后台尚未提供原始需求的全局身份；不能用机组内编号替代。")
                    }
                    sources.append((id, value))
                }
            }
            let waits = try await call("ask.list", .object([:])); try check(token, current)
            guard case .array(let items) = waits else { throw CrewdClientError(message: "待人类事项列表契约无效。") }
            var humans: [CrewdWorkID: CrewdValue] = [:]
            for value in items {
                guard let raw = value["ask"]["ask"].string, let id = CrewdWorkID(raw, kind: .human),
                      value["ask"]["crewId"].string != nil, humans[id] == nil else {
                    throw CrewdClientError(message: "待人类事项的全局身份无效或重复。")
                }
                humans[id] = value
            }
            try check(token, current)
            for row in rows { adopt(row) }
            for (id, value) in sources { requirements[id] = value }
            planIDs = rows.map(\.id); requirementIDs = sources.map { $0.0 }; humanTasks = humans
        } catch {
            if token == generation && refreshID == current && !(error is CancellationError) { lastFailure = failure(error) }
            throw error
        }
    }
    @discardableResult func detail(_ id: CrewdWorkID) async throws -> CrewdPlan {
        guard id.kind == .plan else { throw CrewdClientError(message: "详情必须使用公共 W 编号。") }
        let token = generation, request = UUID(); pendingDetails[id] = request
        defer { if pendingDetails[id] == request { pendingDetails[id] = nil } }
        let value = try await call("work.get", .object(["id": .string(id.raw)]))
        try check(token)
        guard pendingDetails[id] == request else { throw CancellationError() }
        let plan = try parsePlan(value, full: true)
        guard plan.id == id else { throw CrewdClientError(message: "工作详情返回了其他对象。") }
        adopt(plan); return plans[id] ?? plan
    }
    /// A failed mutation retains structured facts for UI and leaves drafts to their owner.
    /// Transport loss is an uncertain commit, never a reason to repeat the command.
    @discardableResult func mutate(_ method: String, _ params: CrewdValue) async throws -> CrewdValue {
        guard ["work.create", "work.update", "work.assign", "work.block", "ask.answer", "ask.withdraw"].contains(method) else {
            throw CrewdClientError(message: "不支持的工作写入操作。")
        }
        guard activeMutations == 0 else { throw CrewdClientError(message: "工作操作正在提交，请稍后再试。") }
        let target = params["id"].string.flatMap { CrewdWorkID($0, kind: .plan) }
        if let target, plans[target]?.frozen == true { throw CrewdClientError(message: "已归档工作只读。", code: -32009, data: .object(["refusal": .string("crew.removed")])) }
        generation = UUID(); refreshID = UUID(); loading = false; pendingDetails.removeAll()
        let token = generation; activeMutations += 1; lastFailure = nil
        var ownsMutation = true
        defer { if ownsMutation { activeMutations = 0 } }
        do {
            let value = try await call(method, params)
            try check(token)
            if method == "work.block" { adopt(try parsePlan(value["item"], full: true)) }
            else if method.hasPrefix("work.") { adopt(try parsePlan(value, full: true)) }
            activeMutations = 0; ownsMutation = false
            // Reconcile derived requirements and global human inbox by reads only.
            do { try await refresh() } catch { /* The write succeeded; read failure is recorded separately. */ }
            return value
        } catch {
            if token == generation && !(error is CancellationError) { lastFailure = failure(error) }
            throw error
        }
    }
}
#endif
