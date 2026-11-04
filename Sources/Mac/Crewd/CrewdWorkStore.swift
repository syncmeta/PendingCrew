import Foundation
import Combine

struct CrewdCustodyIntent:Identifiable {
    enum Status:String {case submitting,unknown,recorded,refused,notFound}
    let id:String
    let crew:String
    let caseID:String
    let expectedRevision:String
    let reason:String
    var status:Status
    var record:CrewdValue = .null
    var message:String?
}

struct CrewdLegacyIntent:Identifiable {
    let id:String,crew:String,messageID:String,sessionID:String,revision:String,note:String
    var status:CrewdCustodyIntent.Status
    var record:CrewdValue = .null

    init(id:String,crew:String,messageID:String,sessionID:String,revision:String,note:String,status:CrewdCustodyIntent.Status) {
        self.id=id;self.crew=crew;self.messageID=messageID;self.sessionID=sessionID
        self.revision=revision;self.note=note;self.status=status
    }

    init(prepared params:CrewdValue) throws {
        guard let id=params["requestId"].string,UUID(uuidString:id) != nil,
              let crew=params["crewId"].string,!crew.isEmpty,
              let message=params["messageId"].string,!message.isEmpty,
              let session=params["sessionId"].string,!session.isEmpty,
              let revision=params["expectedPreviewRevision"].string,!revision.isEmpty,
              let note=params["note"].string,!note.isEmpty else {
            throw CrewdClientError(message:"原历史核验请求的身份或内容不完整。")
        }
        self.init(id:id,crew:crew,messageID:message,sessionID:session,revision:revision,note:note,status:.unknown)
    }

    func validate(_ value:CrewdValue) throws {
        guard value["requestId"] == .string(id),value["messageId"] == .string(messageID),
              value["crewId"] == .string(crew),value["sessionId"] == .string(sessionID),
              value["previewRevision"] == .string(revision),value["note"] == .string(note),
              value["caseId"].string?.isEmpty == false,value["confirmedAt"].int != nil,
              case .bool=value["created"] else {
            throw CrewdClientError(message:"历史核验回执与原请求不符。")
        }
    }
}

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

/// Human corrections are separate from the model's judgment and effective link.
struct CrewdMessageAttributionHistoryPage: Equatable {
    static let method = "message.attribution.history"
    static let limit = 256
    static let cursorLimit = 70
    struct Source: Equatable {
        let crew: String
        let channel: String
        let message: String
    }
    enum Action: String { case set, clear, reset }
    struct Override: Equatable {
        struct Target: Equatable {
            enum Kind: String { case message, requirement, work }
            let kind: Kind
            let id: String
        }
        let action: Action
        let target: Target?
        let targetUnavailable: Bool
    }
    struct Item: Equatable {
        let action: Action
        let atMs: Int64
        let historical: Bool
        let before: Override?
        let after: Override?
    }
    let items: [Item]
    let nextCursor: String?
    let olderUnavailable: Bool

    init(_ value: CrewdValue, source: Source, cursor: String?) throws {
        let invalid = CrewdClientError(message:"人工纠正历史不完整或与当前消息不一致。")
        func parseOverride(_ value: CrewdValue?) throws -> Override? {
            guard let value else { return nil }
            guard case .object(let fields) = value, let raw = fields["action"]?.string,
                  let action = Action(rawValue:raw), action != .reset,
                  case .bool(let unavailable)? = fields["targetUnavailable"] else { throw invalid }
            let target: Override.Target?
            if let wire = fields["target"] {
                guard case .object = wire, let kindName = wire["kind"].string,
                      let kind = Override.Target.Kind(rawValue:kindName),
                      let id = wire["id"].string, !id.isEmpty, id.utf8.count <= 256 else { throw invalid }
                target = .init(kind:kind,id:id)
            } else { target = nil }
            guard action == .set ? (target != nil) != unavailable : target == nil && !unavailable else { throw invalid }
            return .init(action:action,target:target,targetUnavailable:unavailable)
        }
        func matches(_ wire: CrewdValue?, _ expected: String) -> Bool {
            wire?.string?.utf8.elementsEqual(expected.utf8) == true
        }
        guard case .object(let fields) = value,
              matches(fields["crewId"],source.crew), matches(fields["channelId"],source.channel),
              matches(fields["messageId"],source.message), case .array(let rows)? = fields["items"], rows.count <= 20,
              cursor == nil || !rows.isEmpty,
              case .bool(let olderUnavailable)? = fields["olderUnavailable"],
              !olderUnavailable || (cursor != nil && !rows.isEmpty) else { throw invalid }
        if let next = fields["nextCursor"] {
            guard let token = next.string, !token.isEmpty, token.utf8.count <= Self.cursorLimit, token != cursor,
                  rows.count == 20, !olderUnavailable else { throw invalid }
            nextCursor = token
        } else { nextCursor = nil }
        self.olderUnavailable = olderUnavailable
        items = try rows.map { row in
            guard case .object(let fields) = row, let raw = fields["action"]?.string,
                  let action = Action(rawValue:raw), case .integer(let atMs)? = fields["atMs"],
                  case .bool(let historical)? = fields["historical"] else { throw invalid }
            let before = try parseOverride(fields["before"])
            let after = try parseOverride(fields["after"])
            guard action == .reset ? after == nil : after?.action == action else { throw invalid }
            return .init(action:action,atMs:atMs,historical:historical,before:before,after:after)
        }
    }
}

/// Present the server's wait tiers without inferring them from the local clock.
enum CrewdHumanWaitPresentation {
    static func summary(_ row: CrewdValue) -> String? {
        let elapsed = row["waitedMs"].int.flatMap { $0 >= 0 ? elapsedLabel(minutes: $0 / 60_000) : nil }
        let tier: String?
        switch row["tier"].string {
        case "pinned": tier = "已置顶"
        case "still_wanted": tier = "还要吗？"
        default: tier = nil
        }
        let parts = [elapsed, tier].compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    private static func elapsedLabel(minutes: Int) -> String {
        let days = minutes / 1_440, hours = (minutes / 60) % 24
        let remainingMinutes = minutes % 60
        if days > 0 { return hours == 0 ? "已等待 \(days) 天" : "已等待 \(days) 天 \(hours) 小时" }
        if hours > 0 { return remainingMinutes == 0 ? "已等待 \(hours) 小时" : "已等待 \(hours) 小时 \(remainingMinutes) 分钟" }
        if minutes > 0 { return "已等待 \(minutes) 分钟" }
        return "已等待不到 1 分钟"
    }
}

/// User-facing vocabulary only. Raw backend states stay untouched for CAS and actions.
enum CrewdWorkStatusLabel {
    static func display(_ value:String)->String {
        switch value {
        case "unclaimed":return "待认领"
        case "open":return "待推进"
        case "active","in_progress":return "进行中"
        case "done":return "已完成"
        case "blocked":return "受阻"
        case "dropped":return "已放弃"
        case "needs_ruling":return "待裁定"
        case "closed":return "已关闭"
        case "answered":return "已答复"
        case "withdrawn":return "已撤回"
        default:return "状态待确认"
        }
    }
}
struct CrewdPlan: Equatable, Sendable {
    let id: CrewdWorkID
    var summary: CrewdValue
    var detail: CrewdValue?
    var value: CrewdValue {
        guard case .object(var fields) = detail, case .object(let latest) = summary else { return summary }
        // Summary members are authoritative too, including explicit null clears.
        var projection = latest
        // Pending summaries omit the long handoff note. Preserve only detail belonging
        // to this exact version and offer; never carry it into a different request.
        if fields["revision"] == latest["revision"],
           case .object(var full) = fields["pendingTransfer"],
           case .object(let compact) = latest["pendingTransfer"],
           full["to"] == compact["to"], full["requestSeq"] == compact["requestSeq"] {
            full.merge(compact) { _, new in new }; projection["pendingTransfer"] = .object(full)
        }
        fields.merge(projection) { _, new in new }; return .object(fields)
    }
    var revision: String { summary["revision"].string ?? "" }
    var status: String { summary["status"].string ?? "" }
    var crew: String { summary["crewId"].string ?? "" }
    var frozen: Bool { summary["frozen"] == .bool(true) }
}

struct CrewdChannelResponsibilityPage {
    static let method = "work.channelResponsibility"

    static func ownsActiveTopic(_ channels: [CrewdValue], crew: String, channel: String) -> Bool {
        guard !crew.isEmpty, !channel.isEmpty else { return false }
        return channels.contains {
            $0["id"].string?.utf8.elementsEqual(channel.utf8) == true &&
                $0["crewId"].string?.utf8.elementsEqual(crew.utf8) == true &&
                $0["kind"] == .string("topic") && $0["archivedAt"] == .null
        }
    }

    struct Episode: Equatable, Identifiable {
        enum Phase: String { case open, handled, ended, stale }
        enum Notice: String { case notQueued = "not_queued", queued, inflight, delivered, dropped, unknown }
        struct Ownership: Equatable {
            let work: CrewdWorkID
            let owner: String
            let event: String
            let at: Int
        }
        let channel: String
        let id: UUID
        let phase: Phase
        let notice: Notice
        let observedAt: Int
        let updatedAt: Int
        let work: [CrewdWorkID]
        let captain: String?
        let wake: Int?
        let ownership: Ownership?

        init(_ value: CrewdValue) throws {
            guard case .object(let fields) = value,
                  Set(fields.keys).isSubset(of:["channelId","episodeId","phase","noticePhase","observedAtMs","updatedAtMs",
                    "workIds","captainSessionId","wakeId","handled"]),
                  let channel = value["channelId"].string, token(channel),
                  let episode = value["episodeId"].string, let id = UUID(uuidString:episode), episode == id.uuidString.lowercased(),
                  let phase = value["phase"].string.flatMap(Phase.init(rawValue:)),
                  let notice = value["noticePhase"].string.flatMap(Notice.init(rawValue:)),
                  let observed = value["observedAtMs"].int, observed >= 0,
                  let updated = value["updatedAtMs"].int, updated >= observed,
                  case .array(let rows) = value["workIds"], (1...32).contains(rows.count) else { throw invalid() }
            let work = rows.compactMap { $0.string.flatMap(workID) }
            let numbers = work.compactMap { Int64($0.raw.dropFirst()) }
            guard work.count == rows.count, Set(work).count == work.count,
                  numbers.count == work.count, numbers == numbers.sorted() else { throw invalid() }
            let captain = value["captainSessionId"].string
            let wake = value["wakeId"].int
            if notice == .notQueued {
                guard fields["captainSessionId"] == nil, fields["wakeId"] == nil else { throw invalid() }
            } else {
                guard let captain, token(captain), let wake, wake > 0 else { throw invalid() }
            }
            let ownership: Ownership?
            if let handled = fields["handled"] {
                guard phase == .handled, case .object(let row) = handled,
                      Set(row.keys) == ["workId","ownerSessionId","eventSeq","atMs"],
                      let workID = handled["workId"].string.flatMap(CrewdChannelResponsibilityPage.workID), work.contains(workID),
                      let owner = handled["ownerSessionId"].string, token(owner),
                      let event = handled["eventSeq"].string, let seq = UInt64(event), seq > 0, String(seq) == event,
                      let at = handled["atMs"].int, at >= 0 else { throw invalid() }
                ownership = .init(work:workID,owner:owner,event:event,at:at)
            } else {
                guard phase != .handled else { throw invalid() }
                ownership = nil
            }
            self.channel = channel; self.id = id; self.phase = phase; self.notice = notice
            observedAt = observed; updatedAt = updated; self.work = work
            self.captain = captain; self.wake = wake; self.ownership = ownership
        }
    }

    let items: [Episode]
    let next: String?

    init(_ value: CrewdValue, crew: String, channel: String?, after: String?) throws {
        guard case .object(let fields) = value, Set(fields.keys).isSubset(of:["crewId","items","nextAfter"]),
              value["crewId"].string?.utf8.elementsEqual(crew.utf8) == true, Self.token(crew),
              case .array(let rows) = value["items"], rows.count <= 32 else { throw Self.invalid() }
        let episodes = try rows.map(Episode.init)
        let ids = episodes.map(\.channel)
        let next = value["nextAfter"].string
        if let channel {
            guard after == nil, Self.token(channel), fields["nextAfter"] == nil, rows.count <= 1,
                  ids.allSatisfy({ $0.utf8.elementsEqual(channel.utf8) }) else { throw Self.invalid() }
        } else {
            guard Set(ids).count == ids.count, Set(episodes.map(\.id)).count == episodes.count,
                  ids == ids.sorted(by:Self.before),
                  after.map({ after in Self.token(after) && ids.allSatisfy { Self.before(after,$0) } }) ?? true,
                  fields["nextAfter"] == nil || (next != nil && next == ids.last && rows.count == 32) else { throw Self.invalid() }
        }
        items = episodes; self.next = next
    }

    private static func before(_ left: String, _ right: String) -> Bool {
        left.utf8.lexicographicallyPrecedes(right.utf8)
    }
    private static func workID(_ raw: String) -> CrewdWorkID? {
        guard let number = Int64(raw.dropFirst()), number > 0, raw == "W\(number)" else { return nil }
        return CrewdWorkID(raw,kind:.plan)
    }
    private static func token(_ value: String) -> Bool { !value.isEmpty && value.utf8.count <= 512 }
    static func invalid() -> CrewdClientError { .init(message:"群工作归属的原始证据不完整或不属于当前机组。") }
}

struct CrewdCoordinationStatus {
    struct Assignment: Equatable {
        let decisionID: String
        let captain: String
        let previousOwner: String
        let owner: String
        let previousRevision: String
        let revision: String
        let eventSequence: String
        let assignedAtMs: Int64

        init(_ value: CrewdValue, crew: String, id: String) throws {
            let invalid = CrewdClientError(message:"机长分派回执的计划、人员或事件不符。")
            guard case .object = value, value["crewId"] == .string(crew), value["id"] == .string(id),
                  let decision = value["decisionId"].string, UUID(uuidString:decision)?.uuidString.lowercased() == decision,
                  let captain = session(value["captain"]), let previousOwner = session(value["previousOwner"]),
                  let owner = session(value["owner"]), owner != previousOwner,
                  let previous = value["previousRevision"].string, let current = value["revision"].string,
                  let before = UInt64(previous), before > 0, String(before) == previous, before < UInt64.max,
                  current == String(before + 1),
                  let event = value["eventSeq"].string, let sequence = Int64(event), sequence > 0, String(sequence) == event else {
                throw invalid
            }
            guard let milliseconds = milliseconds(value["assignedAtMs"]) else { throw invalid }
            decisionID = decision; self.captain = captain; self.previousOwner = previousOwner; self.owner = owner
            previousRevision = previous; revision = current; eventSequence = event; assignedAtMs = milliseconds
        }
    }

    struct Split: Equatable {
        struct Child: Equatable, Identifiable {
            let id: CrewdWorkID
            let owner: String?
            let eventSequence: String
        }
        let decisionID: String
        let captain: String
        let parentRevision: String
        let splitAtMs: Int64
        let children: [Child]

        init(_ value: CrewdValue, crew: String, id: String) throws {
            let invalid = CrewdClientError(message:"机长拆分回执的计划、人员或子工作不符。")
            guard case .object = value, value["crewId"] == .string(crew), value["id"] == .string(id),
                  let decision = value["decisionId"].string, UUID(uuidString:decision)?.uuidString.lowercased() == decision,
                  let captain = session(value["captain"]), let revision = value["parentRevision"].string,
                  let number = UInt64(revision), number > 0, String(number) == revision,
                  let time = milliseconds(value["splitAtMs"]),
                  case .array(let rows) = value["children"], (2...8).contains(rows.count) else { throw invalid }
            var children: [Child] = []
            var ids = Set<CrewdWorkID>()
            var previous: Int64 = 0
            for row in rows {
                guard case .object(let fields) = row, let raw = row["id"].string, raw != id,
                      let child = CrewdWorkID(raw,kind:.plan), Int64(raw.dropFirst()) != nil, ids.insert(child).inserted,
                      let owner = fields["owner"], owner == .null || session(owner) != nil,
                      row["revision"] == .string("1"),
                      let event = row["eventSeq"].string, let sequence = Int64(event), sequence > previous,
                      String(sequence) == event else { throw invalid }
                children.append(.init(id:child,owner:owner.string,eventSequence:event))
                previous = sequence
            }
            decisionID = decision; self.captain = captain; parentRevision = revision; splitAtMs = time
            self.children = children
        }
    }

    enum Outcome {
        case assignment(Assignment)
        case split(Split)
    }

    let record: CrewdValue
    let lastAssignment: Assignment?
    let requestedAssignment: Assignment?
    let lastSplit: Split?
    let requestedSplit: Split?

    var requestedOutcome: Outcome? {
        if let requestedAssignment { return .assignment(requestedAssignment) }
        if let requestedSplit { return .split(requestedSplit) }
        return nil
    }

    private static func session(_ value: CrewdValue) -> String? {
        guard let id = value.string, !id.isEmpty, id.trimmingCharacters(in:.whitespacesAndNewlines) == id else { return nil }
        return id
    }
    private static func milliseconds(_ value: CrewdValue) -> Int64? {
        switch value {
        case .integer(let number): return number >= 0 ? number : nil
        case .unsigned(let number): return Int64(exactly:number)
        default: return nil
        }
    }
}

/// One backend-owned organization snapshot. Todo and progress contain references to these
/// same plans; they cannot independently mutate a status or synthesize a completion ratio.
@MainActor
final class CrewdWorkStore: ObservableObject {
    typealias Call = @MainActor (String, CrewdValue) async throws -> CrewdValue
    @Published private(set) var plans: [CrewdWorkID: CrewdPlan] = [:]
    @Published private(set) var requirements: [CrewdWorkID: CrewdValue] = [:]
    @Published private var humanInbox: (ids: [CrewdWorkID], tasks: [CrewdWorkID: CrewdValue]) = ([], [:])
    @Published private(set) var selectedCrew = ""
    @Published private(set) var planIDs: [CrewdWorkID] = []
    @Published private(set) var requirementIDs: [CrewdWorkID] = []
    @Published private(set) var lastFailure: CrewdClientError?
    @Published private(set) var loading = false
    @Published private(set) var hasLoaded = false
    @Published private(set) var readScope = UUID()
    @Published private(set) var capabilities:Set<String>=[]
    @Published private(set) var custodyIntents:[CrewdCustodyIntent]=[]
    @Published private(set) var legacyIntents:[CrewdLegacyIntent]=[]
    private var legacyPreviewRead:CrewdValue?
    private var custodyReads:[String:CrewdValue]=[:]
    private let call: Call
    private var generation = UUID()
    private var refreshID = UUID()
    private var requirementEpoch = UUID()
    private var activeMutations = 0
    private var pendingDetails: [CrewdWorkID: UUID] = [:]
    private var ensureTask: Task<Void,Never>?

    init(call: @escaping Call) { self.call = call }
    func configureCapabilities(_ value:Set<String>) {capabilities=value}
    var progress: [CrewdPlan] { planIDs.compactMap { plans[$0] } }
    var todoPlans: [CrewdPlan] { progress }
    var humanTaskIDs: [CrewdWorkID] { humanInbox.ids }
    var humanTasks: [CrewdWorkID: CrewdValue] { humanInbox.tasks }
    func plans(from requirement: CrewdWorkID) -> [CrewdPlan] {
        plans.values.filter { $0.value["sources"].array.contains(.string(requirement.raw)) }
            .sorted { $0.id.raw < $1.id.raw }
    }
    func selectCrew(_ crew: String) {
        guard selectedCrew != crew else { return }
        ensureTask?.cancel(); ensureTask = nil
        invalidateCustodyReads()
        generation = UUID(); refreshID = UUID(); pendingDetails.removeAll(); loading = false
        selectedCrew = crew; planIDs = []; requirementIDs = []; lastFailure = nil
        hasLoaded = false; readScope = UUID()
    }
    /// Stops old reads. Reconnect reconciles by reading; never resends a mutation.
    func invalidateConnection() {
        ensureTask?.cancel(); ensureTask = nil
        capabilities=[]
        invalidateCustodyReads()
        legacyIntents=[]
        requirements.removeAll()
        generation = UUID(); refreshID = UUID(); pendingDetails.removeAll(); loading = false
        hasLoaded = false; readScope = UUID()
    }
    /// A mounted ledger reads once per crew/connection, sharing any read already
    /// in flight. Explicit refresh and successful mutations still reconcile normally.
    func ensureLoaded() async {
        guard !selectedCrew.isEmpty, !capabilities.isEmpty, !hasLoaded else { return }
        if let ensureTask { await ensureTask.value; return }
        guard !loading else { return }
        let scope = readScope
        // The shared read belongs to the store, not to a panel that may disappear
        // while another panel for this same scope is entering.
        let task = Task { [weak self] in
            guard let self else { return }
            do { try await self.refresh() } catch { /* refresh retains current-scope failures. */ }
            if self.readScope == scope { self.ensureTask = nil }
        }
        ensureTask = task
        await task.value
    }
    var supportsChannelResponsibility: Bool { capabilities.contains(CrewdChannelResponsibilityPage.method) }

    func channelResponsibility(crew: String, channel: String? = nil, after: String? = nil) async throws -> CrewdChannelResponsibilityPage {
        guard supportsChannelResponsibility, !crew.isEmpty, crew == selectedCrew,
              channel == nil || after == nil else { throw CrewdChannelResponsibilityPage.invalid() }
        let token = generation
        var params: [String:CrewdValue] = ["crewId":.string(crew)]
        if let channel { params["channelId"] = .string(channel) }
        if let after { params["after"] = .string(after) }
        let value = try await call(CrewdChannelResponsibilityPage.method,.object(params))
        try check(token)
        return try .init(value,crew:crew,channel:channel,after:after)
    }

    func channelResponsibilityWork(_ id: CrewdWorkID, episode: CrewdChannelResponsibilityPage.Episode,
                                   crew: String) async throws -> CrewdPlan {
        guard episode.phase == .open, episode.work.contains(id) else { throw CrewdChannelResponsibilityPage.invalid() }
        let token = generation
        let current = try await channelResponsibility(crew:crew,channel:episode.channel)
        guard let row = current.items.first, row.id == episode.id, row.phase == .open, row.work.contains(id) else {
            throw CrewdClientError(message:"该群的待指派工作已变化，请重新读取。",kind:.notSubmitted)
        }
        let value = try await call("work.get",.object(["id":.string(id.raw)]))
        try check(token)
        let plan = try parsePlan(value,full:true)
        guard plan.id == id, plan.crew == crew, plan.status == "open",
              case .object(let fields) = value, fields["owner"] == .null, fields["frozen"] == .bool(false),
              let revision = UInt64(plan.revision), revision > 0,
              plans[id].flatMap({ UInt64($0.revision) }).map({ $0 <= revision }) ?? true else {
            throw CrewdClientError(message:"该计划已不处于当前机组的待指派状态，请重新读取。",kind:.notSubmitted)
        }
        adopt(plan)
        return plan
    }

    func coordinationStatus(crew: String, id: String, decisionID: String? = nil) async throws -> CrewdCoordinationStatus {
        let token = generation
        guard !crew.isEmpty, CrewdWorkID(id,kind:.plan) != nil else { throw CrewdClientError(message:"计划身份无效。") }
        var params: [String:CrewdValue] = ["crewId":.string(crew),"id":.string(id)]
        if let decisionID {
            guard UUID(uuidString:decisionID)?.uuidString.lowercased() == decisionID else {
                throw CrewdClientError(message:"请输入完整的判断编号。",kind:.notSubmitted)
            }
            params["decisionId"] = .string(decisionID)
        }
        let result = try await call("work.coordinationStatus",.object(params))
        try check(token)
        guard case .object(let fields) = result, fields["crewId"]?.string == crew, fields["id"]?.string == id,
              let record = fields["record"] else { throw CrewdClientError(message:"协调记录不完整或不属于当前计划。") }
        if record != .null {
            guard case .object(let row) = record,
                  row["decisionId"]?.string?.isEmpty == false,
                  row["phase"]?.string != nil, row["reason"]?.string != nil,
                  case .bool? = row["adopted"] else { throw CrewdClientError(message:"协调记录格式不完整。") }
            for key in ["meaningfulProgress","needsCoordination"] {
                guard let value = row[key] else { throw CrewdClientError(message:"协调判断数值缺失。") }
                if value != .null {
                    let number: Double?
                    switch value {case .number(let n):number=n;case .integer(let n):number=Double(n);case .unsigned(let n):number=Double(n);default:number=nil}
                    guard let number, number.isFinite, (0...1).contains(number) else {throw CrewdClientError(message:"协调判断数值无效。")}
                }
            }
            for key in ["recipient","wakeState"] {
                guard let value=row[key], value == .null || value.string != nil else {throw CrewdClientError(message:"协调通知记录不完整。")}
            }
            guard let wake=row["wakeId"], wake == .null || wake.int != nil else {throw CrewdClientError(message:"协调通知身份无效。")}
        }
        func assignment(_ key: String) throws -> CrewdCoordinationStatus.Assignment? {
            guard let value = fields[key], value != .null else { return nil }
            return try .init(value,crew:crew,id:id)
        }
        func split(_ key: String) throws -> CrewdCoordinationStatus.Split? {
            guard let value = fields[key], value != .null else { return nil }
            return try .init(value,crew:crew,id:id)
        }
        let lastAssignment = try assignment("lastAssignment"), requestedAssignment = try assignment("requestedAssignment")
        let lastSplit = try split("lastSplit"), requestedSplit = try split("requestedSplit")
        let assignments = [lastAssignment,requestedAssignment].compactMap { $0 }
        let splits = [lastSplit,requestedSplit].compactMap { $0 }
        let assignmentIDs = Set(assignments.map(\.decisionID))
        guard requestedAssignment.map({ $0.decisionID == decisionID }) ?? true,
              requestedSplit.map({ $0.decisionID == decisionID }) ?? true,
              assignmentIDs.isDisjoint(with:splits.map(\.decisionID)),
              lastAssignment?.decisionID != requestedAssignment?.decisionID || lastAssignment == requestedAssignment,
              lastSplit?.decisionID != requestedSplit?.decisionID || lastSplit == requestedSplit else {
            throw CrewdClientError(message:"协调回执与原判断或查询范围不符。")
        }
        return .init(record:record,lastAssignment:lastAssignment,requestedAssignment:requestedAssignment,
                     lastSplit:lastSplit,requestedSplit:requestedSplit)
    }

    func coordinationChildDetail(_ child: CrewdWorkID, crew: String, parent: String) async throws -> CrewdPlan {
        let token = generation
        guard child.kind == .plan, child.raw != parent, CrewdWorkID(parent,kind:.plan) != nil, !crew.isEmpty else {
            throw CrewdClientError(message:"子工作身份无效。",kind:.notSubmitted)
        }
        let value = try await call("work.get",.object(["id":.string(child.raw)]))
        try check(token)
        let plan = try parsePlan(value,full:true)
        guard plan.id == child, plan.crew == crew, value["parent"] == .string(parent),
              UInt64(plan.revision) != nil, value["title"].string?.isEmpty == false else {
            throw CrewdClientError(message:"子工作详情的机组或上级计划与原回执不符。")
        }
        return plan
    }
    func reassignmentStatus(crew: String, id: String) async throws -> CrewdValue {
        let token = generation
        guard !crew.isEmpty, CrewdWorkID(id,kind:.plan) != nil else { throw CrewdClientError(message:"计划身份无效。") }
        let result = try await call("work.reassignmentStatus",.object(["crewId":.string(crew),"id":.string(id)]))
        try check(token)
        guard case .object(let fields) = result, fields["crewId"]?.string == crew, fields["id"]?.string == id,
              let record = fields["record"] else { throw CrewdClientError(message:"重派记录不完整或不属于当前计划。") }
        if record != .null {
            guard case .object(let row) = record,
                  row["decisionId"]?.string?.isEmpty == false,
                  row["phase"]?.string != nil, row["reason"]?.string != nil, row["oldOwner"]?.string?.isEmpty == false,
                  case .bool? = row["adopted"] else { throw CrewdClientError(message:"重派记录格式不完整。") }
            for key in ["confidence"] {
                guard let value = row[key] else { throw CrewdClientError(message:"重派判断数值缺失。") }
                if value != .null {
                    let number: Double?
                    switch value {case .number(let n):number=n;case .integer(let n):number=Double(n);case .unsigned(let n):number=Double(n);default:number=nil}
                    guard let number, number.isFinite, (0...1).contains(number) else {throw CrewdClientError(message:"重派判断数值无效。")}
                }
            }
            for key in ["newOwner","ownerWakeState","captainWakeState"] {
                guard let value=row[key], value == .null || value.string != nil else {throw CrewdClientError(message:"重派通知记录不完整。")}
            }
            for key in ["ownerWakeId","captainWakeId"] {
                guard let wake=row[key], wake == .null || wake.int != nil else {throw CrewdClientError(message:"重派通知身份无效。")}
            }
        }
        return record
    }
    func patrolStatus() async throws -> CrewdValue {
        let token = generation, crew = selectedCrew
        guard !crew.isEmpty else { throw CrewdClientError(message: "请先选择机组。") }
        let value = try await call("work.patrolStatus", .object(["crewId":.string(crew)]))
        try check(token)
        guard case .object(let fields) = value, fields["crewId"]?.string == crew,
              let run = fields["run"], let error = fields["error"],
              error == .null || error.string != nil else { throw CrewdClientError(message:"后台巡检状态不完整或范围不符。") }
        if run != .null {
            try validatePatrolRun(run)
        }
        if let manual = fields["manualRun"], manual != .null {
            guard manual["runId"].string.flatMap(UUID.init(uuidString:)) != nil,
                  case .bool(let active)? = fields["manualRunActive"],
                  !active || manual["completedAtMs"] == .null else {
                throw CrewdClientError(message: "手动巡检轮次身份或活动状态无效。")
            }
            try validatePatrolRun(manual)
        } else if let active = fields["manualRunActive"], active != .null {
            throw CrewdClientError(message: "手动巡检活动状态缺少对应轮次。")
        }
        if let failure = fields["manualError"], failure != .null, failure.string == nil {
            throw CrewdClientError(message: "手动巡检状态不完整。")
        }
        if let brief = fields["captainBrief"], brief != .null {
            guard let runId = run["runId"].string, !runId.isEmpty,
                  run["completedAtMs"].int != nil,
                  brief["runId"].string == runId,
                  brief["captainSessionId"].string?.isEmpty == false,
                  (brief["wakeId"].int ?? 0) > 0,
                  let phase = brief["phase"].string,
                  CrewdCaptainBriefPhase(rawValue: phase) != nil else {
                throw CrewdClientError(message: "机长简报的投递记录不完整或巡检轮次不符。")
            }
        }
        return value
    }
    private func validatePatrolRun(_ run: CrewdValue) throws {
        guard case .object(let record) = run,
              record["startedAtMs"]?.int != nil,
              let end = record["completedAtMs"], end == .null || end.int != nil,
              case .bool? = record["truncated"],
              case .object(let scanned)? = record["scannedCounts"],
              case .object(let counts)? = record["counts"],
              scanned.values.allSatisfy({ ($0.int ?? -1) >= 0 }),
              counts.values.allSatisfy({ ($0.int ?? -1) >= 0 }) else {
            throw CrewdClientError(message: "后台巡检记录不完整。")
        }
    }
    func similarRequirementPage(after: String?) async throws -> CrewdValue {
        let token = generation, crew = selectedCrew
        guard !crew.isEmpty else { throw CrewdClientError(message: "请先选择机组。") }
        var params: [String: CrewdValue] = ["crewId": .string(crew), "limit": .integer(20)]
        if let after { params["after"] = .string(after) }
        let value = try await call("adminCase.similarRequirements", .object(params))
        try check(token)
        guard value["crewId"] == .string(crew), case .bool = value["enabled"],
              case .array(let rows) = value["items"], rows.count <= 20,
              case .object(let fields) = value, let next = fields["nextAfter"],
              next == .null || (next.string?.isEmpty == false && next.string != after),
              value["enabled"] == .bool(true) || (rows.isEmpty && next == .null) else {
            throw CrewdClientError(message: "相近需求结果不完整或范围不符。")
        }
        var pairs = Set<[String]>()
        for row in rows {
            let score: Double?
            switch row["probability"] {
            case .number(let value): score = value
            case .integer(1), .unsigned(1): score = 1
            default: score = nil
            }
            guard let first = row["firstId"].string, let second = row["secondId"].string,
                  first != second, CrewdWorkID(first, kind: .requirement) != nil,
                  CrewdWorkID(second, kind: .requirement) != nil,
                  let score, score.isFinite,
                  (0.8...1).contains(score), let observed = row["observedAtMs"].int, observed >= 0 else {
                throw CrewdClientError(message: "相近需求建议的编号或证据不完整。")
            }
            guard pairs.insert([first, second]).inserted else {
                throw CrewdClientError(message: "相近需求建议重复。")
            }
            if capabilities.contains("adminCase.requirementReviews") {
                guard validReviewPair(row), row["channelId"].string?.isEmpty == false,
                      validReviewRevision(row["reviewRevision"]), validCandidateRevision(row["candidateRevision"]) else {
                    throw CrewdClientError(message: "相近需求建议缺少可核对的版本。")
                }
            }
        }
        return value
    }

    func requirementReviewPage(after: String? = nil) async throws -> CrewdValue {
        let token = generation, crew = selectedCrew
        let invalid = CrewdClientError(message: "相近需求审阅记录不完整或范围不符。")
        guard !crew.isEmpty, capabilities.contains("adminCase.requirementReviews") else { throw invalid }
        var params: [String: CrewdValue] = ["crewId": .string(crew), "limit": .integer(20)]
        if let after { params["after"] = .string(after) }
        let value = try await call("adminCase.requirementReviews", .object(params))
        try check(token)
        guard value["crewId"] == .string(crew), case .array(let rows) = value["items"], rows.count <= 20,
              value["nextAfter"] == .null ||
                (!rows.isEmpty && value["nextAfter"].string?.isEmpty == false && value["nextAfter"].string != after) else { throw invalid }
        var pairs = Set<[String]>()
        for row in rows {
            let review = row["review"]
            guard review["crewId"] == .string(crew), validReviewPair(review),
                  let first = review["firstId"].string, let second = review["secondId"].string,
                  pairs.insert([first, second]).inserted, review["channelId"].string?.isEmpty == false,
                  validReviewRevision(review["revision"]), review["revision"] != .string("0"),
                  (review["changedAtMs"].int ?? -1) >= 0, case .bool(let effective) = row["effective"],
                  let disposition = review["disposition"].string, ["group", "dismiss", "clear"].contains(disposition),
                  let note = review["note"].string, !note.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                  note.utf8.count <= 4_096,
                  disposition == "clear" ? (!effective && review["candidateRevision"] == .null) : validCandidateRevision(review["candidateRevision"]) else { throw invalid }
        }
        return value
    }

    private func validReviewPair(_ value: CrewdValue) -> Bool {
        func number(_ key: String) -> UInt64? {
            guard let text = value[key].string, CrewdWorkID(text, kind: .requirement) != nil else { return nil }
            return UInt64(text.dropFirst())
        }
        guard let first = number("firstId"), let second = number("secondId") else { return false }
        return first < second
    }

    private func validReviewRevision(_ value: CrewdValue) -> Bool {
        guard let raw = value.string, let number = UInt64(raw) else { return false }
        return String(number) == raw
    }

    private func validCandidateRevision(_ value: CrewdValue) -> Bool {
        guard let raw = value.string, raw.utf8.count == 64 else { return false }
        return raw.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
    }

    func patrolPage(scan: String, after: String?) async throws -> CrewdValue {
        let token = generation, crew = selectedCrew
        guard !crew.isEmpty, ["work", "entries", "channels"].contains(scan) else { throw CrewdClientError(message: "请先选择机组。") }
        var params: [String:CrewdValue] = ["crewId": .string(crew), "scan": .string(scan), "limit": .number(50)]
        if let after { params["after"] = .string(after) }
        let page = try await call("work.patrol", .object(params))
        try check(token)
        let field = scan == "work" ? "items" : scan
        func validRow(_ row: CrewdValue) -> Bool {
            guard case .object(let fields) = row,
                  let id = fields["id"]?.string, !id.isEmpty,
                  fields["crewId"]?.string == crew else { return false }
            func boolean(_ key: String) -> Bool { if case .bool? = fields[key] { return true }; return false }
            switch scan {
            case "entries": return boolean("unclaimed") && boolean("needsRuling")
            case "channels":
                guard boolean("archiveCandidate"), case .array(let reasons)? = fields["blockedBy"],
                      reasons.allSatisfy({ $0.string?.isEmpty == false }) else { return false }
                return fields["archiveCandidate"] != .bool(true) || reasons.isEmpty
            default:
                guard boolean("unowned"), boolean("stopEscalated"), boolean("longWaitsTruncated"),
                      fields["revision"]?.string?.isEmpty == false,
                      case .array(let waits)? = fields["longWaits"],
                      waits.allSatisfy({ $0["id"].string?.isEmpty == false && $0["crewId"].string?.isEmpty == false && $0["askId"].string?.isEmpty == false }) else { return false }
                if let lost = fields["lost"], lost != .null {
                    guard case .object = lost, lost["cause"].string?.isEmpty == false else { return false }
                }
                return true
            }
        }
        guard case .object(let fields) = page, let next = fields["nextAfter"],
              next == .null || (next.string?.isEmpty == false && next.string != after),
              case .array(let rows) = page[field], page["scan"].string == scan,
              rows.allSatisfy(validRow) else {
            throw CrewdClientError(message: "巡检结果不完整或与当前范围不一致，请重新读取。")
        }
        return page
    }
    /// Read only; changing the organization invalidates an outstanding inspection.
    private func validateAttribution(_ value:CrewdValue,crew:String,message:String)throws -> CrewdValue {
        func validLink(_ v:CrewdValue)->Bool {
            ["message","requirement","work"].contains(v["kind"].string ?? "") && v["id"].string?.isEmpty == false && v["revision"].string?.isEmpty == false
        }
        guard case .object(let row)=value,row["crewId"] == .string(crew),row["messageId"] == .string(message),row["revision"]?.string?.isEmpty == false,
              ["model","human","none","stale"].contains(row["source"]?.string ?? ""),let link=row["link"],link == .null || validLink(link),
              let override=row["override"],case .array(let candidates)?=row["candidates"],case .bool?=row["candidatesTruncated"],row["sampleOnly"] == .bool(true) else {throw CrewdClientError(message:"消息归属记录不完整或不属于当前消息。")}
        if override != .null {
            guard ["set","clear"].contains(override["action"].string ?? ""),override["actor"] == .string("human"),override["atMs"].int != nil,
                  (override["action"] == .string("set") ? validLink(override["link"]) : override["link"] == .null) else {throw CrewdClientError(message:"人工归属记录无效。")}
        }
        for candidate in candidates {guard validLink(candidate["link"]),candidate["summary"].string != nil else {throw CrewdClientError(message:"归属候选无效。")}}
        guard let resolved=row["resolvedCandidate"],resolved == .null || (validLink(resolved["link"]) && resolved["summary"].string != nil) else {throw CrewdClientError(message:"精确查找结果无效。")}
        return value
    }
    var supportsMessageAttributionHistory: Bool { capabilities.contains(CrewdMessageAttributionHistoryPage.method) }
    func messageAttributionHistory(source: CrewdMessageAttributionHistoryPage.Source, cursor: String? = nil) async throws -> CrewdMessageAttributionHistoryPage {
        let validCursor = cursor.map { !$0.isEmpty && $0.utf8.count <= CrewdMessageAttributionHistoryPage.cursorLimit } ?? true
        guard supportsMessageAttributionHistory, selectedCrew == source.crew,
              !source.crew.isEmpty, !source.channel.isEmpty, !source.message.isEmpty, validCursor else {
            throw CrewdClientError(message:"当前连接无法读取这条消息的人工纠正历史。",kind:.notSubmitted)
        }
        let token = generation
        var params: [String:CrewdValue] = ["crewId":.string(source.crew),"channelId":.string(source.channel),"messageId":.string(source.message)]
        if let cursor { params["cursor"] = .string(cursor) }
        let page = try await call(CrewdMessageAttributionHistoryPage.method,.object(params))
        try check(token)
        guard supportsMessageAttributionHistory, selectedCrew == source.crew else { throw CancellationError() }
        return try .init(page,source:source,cursor:cursor)
    }
    func messageAttribution(crew:String,message:String,target:CrewdValue?=nil) async throws -> CrewdValue {
        let token=generation
        var params:[String:CrewdValue]=["crewId":.string(crew),"messageId":.string(message)]
        if let target {params["target"]=target}
        let value=try await call("message.attribution.get",.object(params))
        try check(token);return try validateAttribution(value,crew:crew,message:message)
    }
    func messageAttributionWithSummary(crew:String,message:String) async throws -> CrewdValue {
        let original=try await messageAttribution(crew:crew,message:message)
        let token=generation,link=original["link"]
        guard link != .null,!original["candidates"].array.contains(where:{$0["link"] == link}) else {return original}
        do {
            return try await messageAttribution(crew:crew,message:message,target:.object(["kind":link["kind"],"id":link["id"]]))
        } catch is CancellationError {throw CancellationError()}
        catch {try check(token);return original}
    }
    func setMessageAttribution(crew:String,message:String,revision:String,action:String,link:CrewdValue?=nil) async throws -> CrewdValue {
        guard ["set","clear","reset"].contains(action),!revision.isEmpty,(action == "set") == (link != nil),activeMutations == 0 else {throw CrewdClientError(message:"归属操作无效或已有提交进行中。",kind:.notSubmitted)}
        let token=generation;activeMutations += 1
        defer {if token == generation {activeMutations=0}}
        var params:[String:CrewdValue]=["crewId":.string(crew),"messageId":.string(message),"expectedRevision":.string(revision),"action":.string(action)]
        if let link {params["link"]=link}
        let value=try await call("message.attribution.set",.object(params));try check(token)
        let checked=try validateAttribution(value,crew:crew,message:message)
        guard action == "reset" ? checked["override"] == .null : (checked["override"]["action"] == .string(action) && checked["override"]["link"] == (link ?? .null)) else {throw CrewdClientError(message:"归属回执与请求不符，请刷新核对，不会自动重发。")}
        return checked
    }
    private func validateArchiveCheck(_ value:CrewdValue,channel:String,crew:String)throws -> CrewdValue {
        guard value["channel"]["id"] == .string(channel),value["channel"]["crewId"] == .string(crew),value["revision"].string?.isEmpty == false,
              case .bool = value["canArchive"],case .bool = value["canRestore"],case .array(let blockers)=value["blockers"],blockers.allSatisfy({$0.string != nil}),case .bool = value["facts"]["incomplete"] else {throw CrewdClientError(message:"归档核查结果身份或格式不符。")}
        for key in ["linkedWork","liveWork","linkedRequirements","openRequirements","openAsks","pendingWakes","staleWakes","inflightWakes","activeDeliveries","pendingJudgments"] {guard let count=value["facts"][key].int,count>=0 else {throw CrewdClientError(message:"归档事实计数无效。")}}
        return value
    }
    func archiveCheck(channel:String,crew:String) async throws -> CrewdValue {
        let token=generation
        let value=try await call("channel.archiveCheck",.object(["channelId":.string(channel)]));try check(token)
        return try validateArchiveCheck(value,channel:channel,crew:crew)
    }
    func archivedChannels() async throws -> [CrewdValue] {
        let token=generation,crew=selectedCrew
        let value=try await call("channel.list",.object(["crewId":.string(crew)]));try check(token)
        guard case .array(let channels)=value,channels.allSatisfy({row in
            guard case .object(let fields)=row,fields["id"]?.string?.isEmpty == false,fields["crewId"]?.string != nil,let archived=fields["archivedAt"] else{return false}
            return archived == .null || archived.string != nil
        }) else {throw CrewdClientError(message:"群列表格式无效。")}
        return channels.filter {$0["crewId"] == .string(crew) && $0["archivedAt"] != .null}
    }
    func verifyArchive(channel:String,crew:String,revision:String,restore:Bool,evidence:String,note:String) async throws -> CrewdValue {
        guard activeMutations == 0,!revision.isEmpty,!note.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty,restore || !evidence.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty else {throw CrewdClientError(message:"请填写核查说明及归档凭据。",kind:.notSubmitted)}
        let token=generation;activeMutations += 1
        defer {if token == generation {activeMutations=0}}
        var params:[String:CrewdValue]=["channelId":.string(channel),"expectedRevision":.string(revision),"note":.string(note)]
        if !restore {params["evidence"] = .string(evidence)}
        let value=try await call(restore ? "channel.restoreVerified":"channel.archiveVerified",.object(params));try check(token)
        let result=try validateArchiveCheck(value,channel:channel,crew:crew)
        guard (result["channel"]["archivedAt"] == .null) == restore else {throw CrewdClientError(message:"群归档回执状态不符，请重新核对，不会重发。")}
        return result
    }
    private func validateAdminCase(_ row:CrewdValue,crew:String)throws {
        guard row["crewId"] == .string(crew),row["id"].string?.isEmpty == false,row["revision"].string?.isEmpty == false,row["createdAt"].int != nil,
              row["source"]["sessionId"].string?.isEmpty == false,row["source"]["budgetEpoch"].int != nil,row["source"]["restoreEpoch"].string?.isEmpty == false,row["source"]["announcementId"].string?.isEmpty == false,
              ["observed","prepared","dispatched","answered","unknown","unconfigured","waiting_admission","stale"].contains(row["decision"].string ?? ""),["not_started","registered","requested","running","unknown","stale"].contains(row["execution"].string ?? ""),["not_started","prepared","dispatched","decision_ready","exit_requested","parked","unknown","stale","suppressed"].contains(row["retirement"].string ?? ""),case .array(let targets)=row["targets"] else {throw CrewdClientError(message:"行政记录身份、来源或状态不完整。")}
        let provenance=row["source"]["provenance"]
        guard let budget=row["source"]["budgetEpoch"].int,budget>=0,budget>0 || provenance != .null else {throw CrewdClientError(message:"原生来源不能使用迁移零预算。")}
        if provenance != .null {
            guard provenance["kind"] == .string("human_revalidation"),provenance["requestId"].string.flatMap(UUID.init(uuidString:)) != nil,
                  provenance["legacyMessageId"] == row["source"]["announcementId"],provenance["announcementDigest"].string?.isEmpty == false,provenance["previewRevision"].string?.isEmpty == false,
                  provenance["note"].string?.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty == false,provenance["revalidatedAt"] == row["createdAt"] else {throw CrewdClientError(message:"人类重新核验来源证据不完整。")}
        }
        guard case .object(let fields)=row,let record=fields["decisionRecord"] else {throw CrewdClientError(message:"行政判断记录字段缺失。")}
        if record == .null {
            guard row["decision"] == .string("observed") else {throw CrewdClientError(message:"行政判断阶段缺少记录。")}
        } else {
            guard record["decisionId"].string?.isEmpty == false,record["phase"] == row["decision"],record["snapshotWitness"].string?.isEmpty == false,
                  ["prepared","native_answered","generated","provider_unavailable","restart_unknown","unconfigured","no_admission","state_changed","stopped","no_in_scope_targets"].contains(record["reason"].string ?? ""),
                  case .bool = record["adopted"],case .object(let decisionFields)=record,let probability=decisionFields["probability"] else {throw CrewdClientError(message:"行政判断记录格式无效。")}
            if probability != .null {
                let number:Double?
                switch probability {case .number(let n):number=n;case .integer(let n):number=Double(n);case .unsigned(let n):number=Double(n);default:number=nil}
                guard let number,number.isFinite,(0...1).contains(number) else {throw CrewdClientError(message:"行政判断概率无效。")}
            }
        }
        guard let execution=fields["executionRecord"] else {throw CrewdClientError(message:"行政执行记录字段缺失。")}
        if execution == .null {
            guard row["execution"] == .string("not_started") else {throw CrewdClientError(message:"执行阶段缺少记录。")}
        } else {
            guard execution["phase"] == row["execution"],execution["sessionId"].string?.isEmpty == false,execution["attemptId"].string?.isEmpty == false,
                  ["registered","launch_requested","started","launch_unknown","restart_unknown","state_changed"].contains(execution["reason"].string ?? ""),
                  let count=execution["effectsCount"].int,count>=0,case .object(let executionFields)=execution,let generation=executionFields["generation"] else {throw CrewdClientError(message:"行政执行记录格式无效。")}
            if generation != .null {guard let raw=generation.string,let n=UInt64(raw),String(n)==raw else {throw CrewdClientError(message:"执行代次无效。")}}
            if row["execution"] == .string("running"),generation == .null {throw CrewdClientError(message:"实际运行确认缺少执行代次。")}
        }
        if let launch=fields["launchRecord"] {
            if launch != .null {
                guard case .object(let launchFields)=launch,
                      ["waiting","dispatching","bound","unknown","stale"].contains(launch["phase"].string ?? ""),
                      ["ready","source_not_running","source_admission_missing","capacity_unavailable","executor_unsupported","native_auth_unavailable","runtime_unavailable","settings_disabled","source_state_changed","workspace_unavailable","shutting_down","claimed","bound","restart_unknown","state_changed"].contains(launch["reason"].string ?? ""),
                      launch["revision"].string?.isEmpty == false,launch["decisionId"].string?.isEmpty == false,launch["updatedAt"].int != nil,
                      let session=launchFields["sessionId"],let attempt=launchFields["attemptId"] else {throw CrewdClientError(message:"行政启动调度证据不完整。")}
                if launch["phase"] == .string("bound") {
                    guard session.string?.isEmpty == false,attempt.string?.isEmpty == false,
                          session == execution["sessionId"],attempt == execution["attemptId"] else {throw CrewdClientError(message:"启动调度绑定与实际执行记录不符。")}
                } else {
                    guard session == .null,attempt == .null else {throw CrewdClientError(message:"未绑定调度含有不明执行身份。")}
                }
            }
        } else if capabilities.contains("adminCase.automaticExecution") {throw CrewdClientError(message:"行政启动调度字段缺失。")}
        guard let retirement=fields["retirementRecord"] else {throw CrewdClientError(message:"行政退场记录字段缺失。")}
        if retirement == .null {
            guard row["retirement"] == .string("not_started") else {throw CrewdClientError(message:"退场阶段缺少实际记录。")}
        } else {
            guard retirement["phase"] == row["retirement"],retirement["decisionId"].string?.isEmpty == false,
                  ["prepared","answered","low_confidence","provider_unavailable","unconfigured","restart_unknown","state_changed","stopped","exit_unknown"].contains(retirement["reason"].string ?? ""),
                  let generation=retirement["generation"].string,let number=UInt64(generation),String(number)==generation,
                  retirement["generation"] == execution["generation"],execution["sessionId"].string?.isEmpty == false,
                  case .bool = retirement["adopted"],case .object(let retirementFields)=retirement,let probability=retirementFields["probability"] else {throw CrewdClientError(message:"行政退场记录格式无效。")}
            if probability != .null {
                let number:Double?
                switch probability {case .number(let n):number=n;case .integer(let n):number=Double(n);case .unsigned(let n):number=Double(n);default:number=nil}
                guard let number,number.isFinite,(0...1).contains(number) else {throw CrewdClientError(message:"退场概率无效。")}
            }
        }
        guard let handoff=fields["handoffRecord"] else {throw CrewdClientError(message:"行政交接记录字段缺失。")}
        if handoff != .null {
            guard handoff["caseId"] == row["id"],handoff["crewId"] == row["crewId"],
                  handoff["sourceSessionId"] == row["source"]["sessionId"],handoff["adminSessionId"].string?.isEmpty == false,
                  handoff["adminSessionId"] == execution["sessionId"],handoff["captainSessionId"].string?.isEmpty == false,
                  handoff["requestId"].string?.isEmpty == false,handoff["revision"].string?.isEmpty == false,handoff["requestReason"].string?.isEmpty == false,
                  ["waiting","delivered","accepted","stale"].contains(handoff["phase"].string ?? ""),
                  ["requested","notification_delivered","captain_accepted","state_changed","stopped","recipient_changed","notification_dropped"].contains(handoff["reason"].string ?? ""),
                  handoff["createdAt"].int != nil,handoff["wakeId"].int != nil,case .array(let handoffTargets)=handoff["targets"],case .object(let handoffFields)=handoff,
                  let delivered=handoffFields["deliveredAt"],delivered == .null || delivered.string?.isEmpty == false,
                  let accepted=handoffFields["acceptedAt"],accepted == .null || accepted.int != nil,
                  let note=handoffFields["acceptanceNote"],note == .null || note.string?.isEmpty == false else {throw CrewdClientError(message:"行政交接身份或记录格式无效。")}
            if handoff["phase"] == .string("accepted"),accepted == .null || note == .null {throw CrewdClientError(message:"机长接手缺少实际接受记录。")}
            if handoff["phase"] == .string("delivered"),delivered == .null {throw CrewdClientError(message:"通知送达缺少回执时间。")}
            for target in handoffTargets {
                guard CrewdWorkID(target["id"].string ?? "",kind:.plan) != nil,target["crewId"].string?.isEmpty == false,target["revision"].string?.isEmpty == false,
                      ["unanswered","overdue","ready_unscheduled"].contains(target["kind"].string ?? ""),["in_scope","out_of_scope"].contains(target["scope"].string ?? "") else {throw CrewdClientError(message:"交接目标快照格式无效。")}
            }
        }
        guard let outcome=fields["outcomeRecord"] else {throw CrewdClientError(message:"行政结果记录字段缺失。")}
        if outcome != .null {
            guard outcome["caseId"] == row["id"],outcome["crewId"] == row["crewId"],outcome["revision"].string?.isEmpty == false,outcome["verifiedAt"].int != nil,
                  ["unresolved","resolved","stale"].contains(outcome["phase"].string ?? ""),
                  ["no_execution","no_verified_admin_effect","targets_unresolved","out_of_scope","verified","authority_changed"].contains(outcome["reason"].string ?? ""),case .array(let outcomes)=outcome["targets"] else {throw CrewdClientError(message:"行政结果身份或状态无效。")}
            for key in ["outOfScope","missing","pending","blocked","resolvedByAdmin","resolvedByOthers"] {
                guard case .array(let ids)=outcome[key],ids.allSatisfy({CrewdWorkID($0.string ?? "",kind:.plan) != nil}) else {throw CrewdClientError(message:"行政结果目标分类无效。")}
            }
            for result in outcomes {
                guard CrewdWorkID(result["id"].string ?? "",kind:.plan) != nil,case .array(let kinds)=result["sourceKinds"],kinds.allSatisfy({["unanswered","overdue","ready_unscheduled"].contains($0.string ?? "")}),
                      ["resolved","pending","blocked","missing"].contains(result["state"].string ?? ""),
                      ["closed_with_evidence","reassigned_and_acknowledged","no_closure_evidence","no_matching_event","original_problem_remains","notification_pending","notification_dropped","owner_unavailable","target_missing","no_verified_admin_receipt"].contains(result["reason"].string ?? ""),case .object(let resultFields)=result else {throw CrewdClientError(message:"逐项行政核验记录无效。")}
                for key in ["actorSessionId","eventSeq"] {guard let v=resultFields[key],v == .null || v.string?.isEmpty == false else {throw CrewdClientError(message:"行政核验来源缺失。")}}
                guard let resolution=resultFields["resolution"],resolution == .null || ["closed","reassigned"].contains(resolution.string ?? ""),
                      let actor=resultFields["resolvedBy"],actor == .null || ["case_admin","other"].contains(actor.string ?? ""),let wake=resultFields["wakeId"],wake == .null || wake.int != nil else {throw CrewdClientError(message:"行政核验归因格式无效。")}
            }
        }
        if let custody=fields["custodyRecord"] {
            if custody != .null {
                try validateCustody(custody,crew:crew,caseID:row["id"].string ?? "")
                guard custodyTargets(custody["targets"]) == custodyTargets(row["targets"]),custody["originalRequestId"] == handoff["requestId"],handoff["acceptedAt"] != .null else {throw CrewdClientError(message:"责任范围与原案件不一致。")}
            }
        } else if capabilities.contains("adminCase.custodyGet") {throw CrewdClientError(message:"后台缺少责任接管记录字段。")}
        for target in targets {
            guard CrewdWorkID(target["id"].string ?? "",kind:.plan) != nil,target["crewId"].string?.isEmpty == false,target["revision"].string?.isEmpty == false,
                  ["unanswered","overdue","ready_unscheduled"].contains(target["kind"].string ?? ""),["in_scope","out_of_scope"].contains(target["scope"].string ?? "") else {throw CrewdClientError(message:"行政记录目标格式无效。")}
        }
    }
    func adminCases(after:String?=nil) async throws -> CrewdValue {
        let crew=selectedCrew,token=generation
        guard !crew.isEmpty else {throw CrewdClientError(message:"请先选择机组。")}
        var params:[String:CrewdValue]=["crewId":.string(crew),"limit":.integer(20)]
        if let after {params["after"] = .string(after)}
        let page=try await call("adminCase.list",.object(params));try check(token)
        guard case .object(let fields)=page,page["crewId"] == .string(crew),case .array(let items)?=fields["items"],let next=fields["nextAfter"],next == .null || next.string?.isEmpty == false else {throw CrewdClientError(message:"行政记录分页身份或格式不符。")}
        guard next == .null || next.string != after else {throw CrewdClientError(message:"行政记录分页未推进。")}
        for item in items {try validateAdminCase(item,crew:crew)}
        return page
    }
    func adminCase(id:String) async throws -> CrewdValue {
        let crew=selectedCrew,token=generation
        let row=try await call("adminCase.get",.object(["crewId":.string(crew),"id":.string(id)]));try check(token)
        try validateAdminCase(row,crew:crew)
        guard row["id"] == .string(id) else {throw CrewdClientError(message:"行政详情返回了其他案件。")}
        return row
    }
    func adminCaseEffects(id: String, after: String? = nil) async throws -> CrewdValue {
        let crew = selectedCrew, token = generation
        let error = CrewdClientError(message: "行政动作记录的案件、分页或字段不符。")
        guard capabilities.contains("adminCase.effects"), !crew.isEmpty, !id.isEmpty else { throw error }
        var params: [String: CrewdValue] = ["crewId": .string(crew), "id": .string(id), "limit": .integer(20)]
        if let after { params["after"] = .string(after) }
        let page = try await call("adminCase.effects", .object(params))
        try check(token)
        guard page["crewId"] == .string(crew), page["id"] == .string(id),
              case .array(let rows) = page["items"], rows.count <= 20,
              page["nextAfter"] == .null ||
                (page["nextAfter"].string?.isEmpty == false && page["nextAfter"].string != after && !rows.isEmpty) else {
            throw error
        }
        func number(_ value: CrewdValue) -> UInt64? {
            guard let text = value.string, let number = UInt64(text), number > 0, String(number) == text else { return nil }
            return number
        }
        var previous: UInt64?
        var previousEvent: UInt64 = 0
        for row in rows {
            guard let ordinal = number(row["ordinal"]),
                  previous.map({ ordinal > $0 && ordinal - $0 == 1 }) ?? (after != nil || ordinal == 1),
                  CrewdWorkID(row["workId"].string ?? "", kind: .plan) != nil,
                  row["actorSessionId"].string?.isEmpty == false, (row["occurredAtMs"].int ?? -1) >= 0,
                  case .bool = row["detailsTruncated"],
                  let first = number(row["firstEventSeq"]), let last = number(row["lastEventSeq"]),
                  first > previousEvent, first <= last,
                  let kind = row["kind"].string, ["closed", "reassigned", "unlinked"].contains(kind) else { throw error }
            for key in ["note", "previousOwner", "newOwner"] {
                guard row[key] == .null || row[key].string?.isEmpty == false else { throw error }
            }
            if row["resultRevision"] != .null, number(row["resultRevision"]) == nil { throw error }
            if row["evidence"] != .null {
                guard ["commit", "url", "text"].contains(row["evidence"]["kind"].string ?? ""),
                      row["evidence"]["value"].string?.isEmpty == false else { throw error }
            }
            if row["wakeId"] != .null, number(row["wakeId"]) == nil { throw error }
            if row["requestId"] != .null, row["requestId"].string.flatMap(UUID.init(uuidString:)) == nil { throw error }
            switch kind {
            case "closed":
                guard ["done", "duplicate", "superseded"].contains(row["closureKind"].string ?? ""),
                      row["evidence"] != .null, row["note"] != .null else { throw error }
            case "reassigned":
                guard row["newOwner"] != .null, row["note"] != .null else { throw error }
            default:
                let proofFields = ["relatedWorkId", "edge", "requestId", "evidence", "note"]
                if proofFields.contains(where: { row[$0] != .null }) {
                    guard proofFields.allSatisfy({ row[$0] != .null }),
                          CrewdWorkID(row["relatedWorkId"].string ?? "", kind: .plan) != nil,
                          row["relatedWorkId"] != row["workId"],
                          ["parent", "dependency"].contains(row["edge"].string ?? "") else { throw error }
                }
            }
            previous = ordinal
            previousEvent = last
        }
        return page
    }
    func humanBrief() async throws -> CrewdHumanBrief {
        let crew = selectedCrew, token = generation
        guard capabilities.contains("adminCase.humanBrief"), !crew.isEmpty else {
            throw CrewdClientError(message: "当前后台未开放行政简报。", kind: .notSubmitted)
        }
        let value = try await call("adminCase.humanBrief", .object(["crewId": .string(crew)]))
        try check(token)
        return try CrewdHumanBrief(value: value, crew: crew)
    }
    private func legacyError()->CrewdClientError {CrewdClientError(message:"历史核验身份、范围或字段不完整。")}
    private func validateLegacy(_ row:CrewdValue)throws {
        guard row["messageId"].string?.isEmpty == false,row["messageSeq"].int != nil,row["historicalText"].string != nil,row["createdAt"].string != nil,row["revision"].string?.isEmpty == false,
              ["observed","linked","invalid"].contains(row["state"].string ?? ""),row["historyComplete"] == .bool(false),
              ["historical_hint_only","invalid_history","invalid_evidence","human_revalidated"].contains(row["reason"].string ?? ""),case .object(let f)=row,let linked=f["linkedCaseId"],let crew=f["linkedCrewId"],
              (linked == .null && crew == .null) || (linked.string?.isEmpty == false && crew.string?.isEmpty == false) else {throw legacyError()}
    }
    func legacyCases(after:String?=nil) async throws -> CrewdValue {
        let token=generation
        var p:[String:CrewdValue]=["limit":.integer(20)];if let after {p["after"] = .string(after)}
        let v=try await call("adminCase.legacyList",.object(p));try check(token)
        guard case .array(let rows)=v["items"],case .object(let fields)=v,let next=fields["nextAfter"],next == .null || next.string?.isEmpty == false,
              ["indexing","scanning","current"].contains(v["scan"]["phase"].string ?? ""),let through=v["scan"]["throughSeq"].int,let upper=v["scan"]["capturedUpper"].int,through>=0,upper>=through,let errors=v["scan"]["unverifiedSources"].int,errors>=0 else {throw legacyError()}
        guard next == .null || next.string != after else {throw legacyError()}
        for row in rows {try validateLegacy(row)}
        return v
    }
    func legacySessions() async throws -> [CrewdValue] {
        let token=generation,crew=selectedCrew
        let v=try await call("session.list",.object(["crewId":.string(crew)]));try check(token)
        guard case .array(let rows)=v,rows.allSatisfy({$0["crewId"] == .string(crew) && $0["id"].string?.isEmpty == false}) else {throw legacyError()}
        return rows
    }
    func legacyPreview(messageID:String,sessionID:String) async throws -> CrewdValue {
        let token=generation,crew=selectedCrew;legacyPreviewRead=nil
        let v=try await call("adminCase.legacyPreview",.object(["messageId":.string(messageID),"crewId":.string(crew),"sessionId":.string(sessionID)]));try check(token)
        guard v["messageId"] == .string(messageID),v["crewId"] == .string(crew),v["sessionId"] == .string(sessionID),v["revision"].string?.isEmpty == false,
              v["restoreEpoch"].string?.isEmpty == false,v["observedAt"].int != nil,case .bool(let can)=v["canConfirm"],case .array(let blockers)=v["blockers"],case .array(let targets)=v["targets"],case .object(let fields)=v,let budget=fields["budgetEpoch"],budget == .null || (budget.int ?? -1)>=0,let existing=fields["existingCaseId"],existing == .null || existing.string?.isEmpty == false,
              can == blockers.isEmpty,blockers.allSatisfy({["invalid_history","source_unavailable","crew_unavailable","stopped","missing_budget","not_escalated","budget_mismatch","no_targets","no_in_scope_targets","index_incomplete","already_linked"].contains($0.string ?? "")}) else {throw legacyError()}
        for t in targets {guard CrewdWorkID(t["id"].string ?? "",kind:.plan) != nil,t["crewId"].string?.isEmpty == false,t["revision"].string?.isEmpty == false,["unanswered","overdue","ready_unscheduled"].contains(t["kind"].string ?? ""),["in_scope","out_of_scope"].contains(t["scope"].string ?? ""),(t["scope"] == .string("in_scope")) == (t["crewId"] == .string(crew)) else {throw legacyError()}}
        if can && (targets.isEmpty || budget == .null) {throw legacyError()}
        legacyPreviewRead=v;return v
    }
    func restoreLegacyIntents(_ requests:[CrewdValue]) throws {
        let restored=try requests.map {try CrewdLegacyIntent(prepared:$0)}
        guard Set(restored.map(\.id)).count == restored.count else {throw legacyError()}
        legacyIntents=restored
    }
    func confirmLegacy(messageID:String,sessionID:String,revision:String,note:String) async throws -> CrewdValue {
        let note=note.trimmingCharacters(in:.whitespacesAndNewlines),crew=selectedCrew,token=generation
        guard activeMutations==0,!note.isEmpty,note.utf8.count<=2000,let p=legacyPreviewRead,p["messageId"] == .string(messageID),p["sessionId"] == .string(sessionID),p["crewId"] == .string(crew),p["revision"] == .string(revision),p["canConfirm"] == .bool(true),!legacyIntents.contains(where:{$0.messageID==messageID && ($0.status == .unknown || $0.status == .submitting)}) else {throw CrewdClientError(message:"请先查证未知提交并重新核验当前完整事实。",kind:.notSubmitted)}
        let intent=CrewdLegacyIntent(id:UUID().uuidString.lowercased(),crew:crew,messageID:messageID,sessionID:sessionID,revision:revision,note:note,status:.submitting)
        legacyIntents.append(intent);legacyPreviewRead=nil;activeMutations += 1
        defer {activeMutations -= 1}
        do {
            let v=try await call("adminCase.legacyConfirm",.object(["requestId":.string(intent.id),"messageId":.string(messageID),"crewId":.string(crew),"sessionId":.string(sessionID),"expectedPreviewRevision":.string(revision),"note":.string(note)]));try check(token);try intent.validate(v)
            if let i=legacyIntents.firstIndex(where:{$0.id==intent.id}) {legacyIntents[i].status = .recorded;legacyIntents[i].record=v};return v
        } catch {
            if let i=legacyIntents.firstIndex(where:{$0.id==intent.id}),legacyIntents[i].status == .submitting {legacyIntents[i].status = CrewdClientError.isUncertainSubmission(error) ? .unknown:.refused}
            throw error
        }
    }
    func legacyReceipt(requestID:String) async throws -> CrewdValue {
        guard let intent=legacyIntents.first(where:{$0.id==requestID}),intent.crew==selectedCrew else {throw legacyError()}
        let token=generation
        do {
            let v=try await call("adminCase.legacyReceipt",.object(["requestId":.string(requestID)]));try check(token);try intent.validate(v)
            if let i=legacyIntents.firstIndex(where:{$0.id==requestID}) {legacyIntents[i].status = .recorded;legacyIntents[i].record=v};legacyPreviewRead=nil;return v
        }catch {
            try check(token)
            if let e=error as? CrewdClientError,e.refusal == "resource.not_found",let i=legacyIntents.firstIndex(where:{$0.id==requestID}),legacyIntents[i].status != .recorded {legacyIntents[i].status = .notFound}
            legacyPreviewRead=nil;throw error
        }
    }
    private func custodyKey(_ crew:String,_ id:String)->String {"\(crew.utf8.count):\(crew)\(id)"}
    private func invalidateCustodyReads() {
        legacyPreviewRead=nil
        for i in legacyIntents.indices where legacyIntents[i].status == .submitting {legacyIntents[i].status = .unknown}

        custodyReads.removeAll()
        for i in custodyIntents.indices where custodyIntents[i].status == .submitting {custodyIntents[i].status = .unknown;custodyIntents[i].message="连接或机组已切换，原请求结果待查证。"}
    }
    func custodyRequests(caseID:String)->[CrewdCustodyIntent] {custodyIntents.filter{$0.crew == selectedCrew && $0.caseID == caseID}}
    private func custodyTargets(_ value:CrewdValue)->[String] {
        value.array.map {[$0["id"].string ?? "",$0["crewId"].string ?? "",$0["kind"].string ?? "",$0["scope"].string ?? ""].joined(separator:"|")}.sorted()
    }
    private func custodyError()->CrewdClientError {CrewdClientError(message:"责任记录身份、范围或核验字段不完整，请重新读取。")}
    private func custodyRequired(_ row:CrewdValue,_ keys:[String])throws {
        guard case .object(let fields)=row,keys.allSatisfy({fields[$0] != nil}) else {throw custodyError()}
    }
    private func validateCustodyTargets(_ value:CrewdValue,crew:String)throws {
        guard case .array(let rows)=value,!rows.isEmpty else {throw custodyError()}
        for t in rows {
            guard CrewdWorkID(t["id"].string ?? "",kind:.plan) != nil,t["crewId"].string?.isEmpty == false,t["revision"].string?.isEmpty == false,
                  ["unanswered","overdue","ready_unscheduled"].contains(t["kind"].string ?? ""),["in_scope","out_of_scope"].contains(t["scope"].string ?? ""),
                  t["scope"] != .string("in_scope") || t["crewId"] == .string(crew) else {throw custodyError()}
        }
    }
    private func validateCustodyResults(_ value:CrewdValue,targets:CrewdValue)throws {
        guard case .array(let rows)=value else {throw custodyError()}
        let expected=Set(targets.array.filter{$0["scope"] == .string("in_scope")}.compactMap{$0["id"].string})
        let actual=rows.compactMap{$0["id"].string}
        guard Set(actual)==expected,actual.count==rows.count,Set(actual).count==actual.count else {throw custodyError()}
        for t in rows {
            try custodyRequired(t,["resolution","resolvedBy","actorSessionId","eventSeq","wakeId"])
            guard ["resolved","pending","blocked","missing"].contains(t["state"].string ?? ""),
                  ["closed_with_evidence","reassigned_and_acknowledged","no_closure_evidence","no_matching_event","original_problem_remains","notification_pending","notification_dropped","owner_unavailable","target_missing","no_verified_admin_receipt"].contains(t["reason"].string ?? ""),
                  case .array(let kinds)=t["sourceKinds"],!kinds.isEmpty,kinds.allSatisfy({["unanswered","overdue","ready_unscheduled"].contains($0.string ?? "")}),
                  t["resolution"] == .null || ["closed","reassigned"].contains(t["resolution"].string ?? ""),
                  t["resolvedBy"] == .null || ["case_admin","other"].contains(t["resolvedBy"].string ?? ""),
                  t["actorSessionId"] == .null || t["actorSessionId"].string?.isEmpty == false,
                  t["wakeId"] == .null || (t["wakeId"].int ?? 0)>0 else {throw custodyError()}
            if t["eventSeq"] != .null {guard let raw=t["eventSeq"].string,let n=UInt64(raw),n>0,String(n)==raw else {throw custodyError()}}
            if t["state"] == .string("resolved") {guard t["resolution"] != .null,t["resolvedBy"] != .null,t["eventSeq"] != .null else {throw custodyError()}}
        }
    }
    private let custodyReasons:Set<String>=["original_custody","proposed","notification_delivered","captain_accepted","state_changed","notification_dropped","superseded","targets_unresolved","out_of_scope","target_missing","evidence_incomplete","verified"]
    private func validateCustodyTransfer(_ row:CrewdValue,crew:String,caseID:String,targets:CrewdValue?=nil)throws {
        try custodyRequired(row,["deliveredAt","acceptedAt","acceptanceNote","supersededBy"])
        guard row["crewId"] == .string(crew),row["caseId"] == .string(caseID),UUID(uuidString:row["requestId"].string ?? "") != nil,
              row["revision"].string?.isEmpty == false,row["predecessorReceiptId"].string?.isEmpty == false,row["fromSessionId"].string?.isEmpty == false,row["toSessionId"].string?.isEmpty == false,
              ["waiting","delivered","accepted","stale"].contains(row["phase"].string ?? ""),custodyReasons.contains(row["reason"].string ?? ""),row["requestReason"].string?.isEmpty == false,
              row["createdAt"].int != nil,(row["wakeId"].int ?? 0)>0,row["deliveredAt"] == .null || row["deliveredAt"].string?.isEmpty == false,
              row["acceptedAt"] == .null || row["acceptedAt"].int != nil,row["acceptanceNote"] == .null || row["acceptanceNote"].string?.isEmpty == false,
              row["supersededBy"] == .null || UUID(uuidString:row["supersededBy"].string ?? "") != nil else {throw custodyError()}
        try validateCustodyTargets(row["targets"],crew:crew)
        if let targets, custodyTargets(row["targets"]) != custodyTargets(targets) {throw custodyError()}
        if row["phase"] == .string("delivered") {guard row["deliveredAt"] != .null else {throw custodyError()}}
        if row["phase"] == .string("accepted") || row["acceptedAt"] != .null {
            guard row["acceptedAt"] != .null,row["acceptanceNote"] != .null,row["deliveredAt"] != .null else {throw custodyError()}
        }
    }
    private func validateCustody(_ row:CrewdValue,crew:String,caseID:String)throws {
        try custodyRequired(row,["pending","currentRecord","discharge","targetResults","outOfScope"])
        guard row["crewId"] == .string(crew),row["caseId"] == .string(caseID),row["revision"].string?.isEmpty == false,
              row["originalRequestId"].string?.isEmpty == false,row["currentCustodianSessionId"].string?.isEmpty == false,row["currentReceiptId"].string?.isEmpty == false,
              ["active","stale","discharged"].contains(row["phase"].string ?? ""),custodyReasons.contains(row["reason"].string ?? "") else {throw custodyError()}
        try validateCustodyTargets(row["targets"],crew:crew);try validateCustodyResults(row["targetResults"],targets:row["targets"])
        guard case .array(let outside)=row["outOfScope"],outside.allSatisfy({CrewdWorkID($0.string ?? "",kind:.plan) != nil}),
              Set(outside.compactMap{$0.string}) == Set(row["targets"].array.filter{$0["scope"] == .string("out_of_scope")}.compactMap{$0["id"].string}) else {throw custodyError()}
        if row["currentRecord"] != .null {
            let r=row["currentRecord"];try validateCustodyTransfer(r,crew:crew,caseID:caseID,targets:row["targets"])
            guard r["requestId"] == row["currentReceiptId"],r["toSessionId"] == row["currentCustodianSessionId"],r["acceptedAt"] != .null else {throw custodyError()}
        } else if row["currentReceiptId"] != row["originalRequestId"] {throw custodyError()}
        if row["pending"] != .null {
            let r=row["pending"];try validateCustodyTransfer(r,crew:crew,caseID:caseID,targets:row["targets"])
            guard r["fromSessionId"] == row["currentCustodianSessionId"],r["predecessorReceiptId"] == row["currentReceiptId"],r["acceptedAt"] == .null else {throw custodyError()}
        }
        if row["phase"] == .string("discharged") {
            let d=row["discharge"]
            guard d["revision"].string?.isEmpty == false,d["verifiedAt"].int != nil,row["currentRecord"] != .null,outside.isEmpty,
                  d["targets"] == row["targetResults"],d["targets"].array.allSatisfy({$0["state"] == .string("resolved")}) else {throw custodyError()}
        } else if row["discharge"] != .null {throw custodyError()}
    }
    func custody(caseID:String) async throws -> CrewdValue {
        let crew=selectedCrew,token=generation
        guard capabilities.contains("adminCase.custodyGet"),!crew.isEmpty else {throw CrewdClientError(message:"当前后台未开放责任接管查询。",kind:.notSubmitted)}
        let row=try await call("adminCase.custodyGet",.object(["crewId":.string(crew),"caseId":.string(caseID)]));try check(token)
        try validateCustody(row,crew:crew,caseID:caseID);custodyReads[custodyKey(crew,caseID)]=row;return row
    }
    func custodyTransfer(requestID:String,caseID:String) async throws -> CrewdValue {
        let crew=selectedCrew,token=generation
        guard capabilities.contains("adminCase.custodyTransferGet"),UUID(uuidString:requestID) != nil else {throw CrewdClientError(message:"无法查询该责任提议。",kind:.notSubmitted)}
        do {
            let row=try await call("adminCase.custodyTransferGet",.object(["requestId":.string(requestID)]));try check(token)
            try validateCustodyTransfer(row,crew:crew,caseID:caseID)
            guard row["requestId"] == .string(requestID) else {throw custodyError()}
            if let i=custodyIntents.firstIndex(where:{$0.id==requestID && $0.crew==crew && $0.caseID==caseID}) {guard row["requestReason"] == .string(custodyIntents[i].reason) else {throw custodyError()};custodyIntents[i].status = .recorded;custodyIntents[i].record=row;custodyIntents[i].message=nil}
            return row
        } catch {
            try check(token)
            if let e=error as? CrewdClientError,e.refusal == "resource.not_found",let i=custodyIntents.firstIndex(where:{$0.id==requestID && $0.crew==crew && $0.caseID==caseID}) {if custodyIntents[i].record == .null {custodyIntents[i].status = .notFound};custodyIntents[i].message="当前未查到原请求；这不代表它永远未提交。原编号和已有回执保留。";custodyReads.removeValue(forKey:custodyKey(crew,caseID))}
            throw error
        }
    }
    func proposeCustody(caseID:String,revision:String,reason:String) async throws -> CrewdValue {
        let crew=selectedCrew,token=generation,key=custodyKey(selectedCrew,caseID),reason=reason.trimmingCharacters(in:.whitespacesAndNewlines)
        guard capabilities.contains("adminCase.custodyPropose"),activeMutations==0,!reason.isEmpty,reason.count<=2000,
              custodyReads[key]?["revision"] == .string(revision),custodyReads[key]?["phase"] == .string("stale"),
              custodyReads[key]?["pending"] == .null || custodyReads[key]?["pending"]["phase"] == .string("stale"),
              !custodyRequests(caseID:caseID).contains(where:{$0.status == .unknown || $0.status == .submitting}) else {throw CrewdClientError(message:"请先查证未确认的原请求并重新读取失效责任；填写接管理由。",kind:.notSubmitted)}
        let request=UUID().uuidString.lowercased()
        custodyIntents.append(CrewdCustodyIntent(id:request,crew:crew,caseID:caseID,expectedRevision:revision,reason:reason,status:.submitting))
        custodyReads.removeValue(forKey:key);activeMutations += 1
        defer {activeMutations -= 1}
        do {
            let row=try await call("adminCase.custodyPropose",.object(["crewId":.string(crew),"caseId":.string(caseID),"requestId":.string(request),"expectedRevision":.string(revision),"reason":.string(reason)]));try check(token)
            try validateCustody(row,crew:crew,caseID:caseID)
            let record=row["pending"]["requestId"] == .string(request) ? row["pending"]:row["currentRecord"]
            guard record["requestId"] == .string(request),record["requestReason"] == .string(reason) else {throw custodyError()}
            if let i=custodyIntents.firstIndex(where:{$0.id==request}) {custodyIntents[i].status = .recorded;custodyIntents[i].record=record}
            return row
        } catch {
            if token==generation,let i=custodyIntents.firstIndex(where:{$0.id==request}) {
                custodyIntents[i].status = CrewdClientError.isUncertainSubmission(error) ? .unknown:.refused;custodyIntents[i].message=error.localizedDescription
            }
            throw error
        }
    }
    func inspectJudgment(crew: String, kind: String, id: String) async throws -> CrewdValue {
        let token = generation
        let source: CrewdValue = .object(["crewId": .string(crew), "kind": .string(kind), "id": .string(id)])
        let value = try await call("judgment.inspect", source)
        try check(token)
        guard case .object(let fields) = value, fields["record"] != nil, value["source"] == source else { throw CrewdClientError(message: "判断记录与当前对象不一致，请刷新。") }
        return value["record"]
    }
    private func check(_ token: UUID, _ refresh: UUID? = nil) throws {
        guard token == generation, !Task.isCancelled, refresh == nil || refresh == refreshID else { throw CancellationError() }
    }
    private func failure(_ error: Error) -> CrewdClientError {
        (error as? CrewdClientError) ?? CrewdClientError(message: error.localizedDescription)
    }
    private func withoutAssociation(_ value:CrewdValue)->CrewdValue {
        guard case .object(var fields)=value else{return value};fields.removeValue(forKey:"channelAssociation");return .object(fields)
    }
    private func validateAssociation(_ value:CrewdValue,_ params:CrewdValue)throws {
        guard let channel=params["channelId"].string else{return}
        let association=value["channelAssociation"]
        guard association["channelId"] == .string(channel),case .bool = association["linked"],let rev=association["linkRevision"].string,!rev.isEmpty,rev.allSatisfy({$0.isASCII && $0.isNumber}),(rev == "0" || rev.first != "0") else{throw CrewdClientError(message:"创建回执缺少当前会话的真实关联状态。")}
    }
    private func parsePlan(_ value: CrewdValue, full: Bool) throws -> CrewdPlan {
        guard let raw = value["id"].string, let id = CrewdWorkID(raw, kind: .plan),
              let revision = value["revision"].string, !revision.isEmpty,
              revision.first != "0", revision.allSatisfy({ $0.isASCII && $0.isNumber }),
              value["crewId"].string != nil, value["status"].string != nil else {
            throw CrewdClientError(message: "工作契约缺少稳定编号、机组或字符串版本。")
        }
        guard case .array(let sources) = value["sources"], sources.allSatisfy({
            guard let raw = $0.string else { return false }
            return CrewdWorkID(raw, kind: .requirement) != nil
        }) else { throw CrewdClientError(message: "工作摘要缺少公共来源编号；不能推断需求与计划关联。") }
        let canonical=withoutAssociation(value)
        return CrewdPlan(id: id, summary: canonical, detail: full ? canonical : nil)
    }
    private func parseRequirement(_ value: CrewdValue) throws -> CrewdWorkID {
        guard let raw = value["requirementId"].string,
              let id = CrewdWorkID(raw, kind: .requirement),
              value["crewId"].string != nil, value["text"].string != nil,
              value["status"].string != nil else {
            throw CrewdClientError(message: "需求契约缺少公共编号、机组、原文或状态。")
        }
        if value["status"] == .string("closed") {
            guard let note = value["closure"]["note"].string, !note.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                  let at = value["closure"]["at"].string, !at.isEmpty else {
                throw CrewdClientError(message: "关闭需求详情缺少裁定理由或时间。")
            }
        }
        return id
    }
    /// The draft owner retains requestID across uncertain replies; the store never retries writes.
    @discardableResult func createRequirement(requestID: UUID, text: String, channel:String="") async throws -> CrewdValue {
        guard !selectedCrew.isEmpty else { throw CrewdClientError(message: "请先选择机组。",kind:.notSubmitted) }
        var params:[String:CrewdValue]=["requestId":.string(requestID.uuidString.lowercased()),"crewId":.string(selectedCrew),"text":.string(text)]
        if !channel.isEmpty {params["channelId"] = .string(channel)}
        return try await mutate("todo.create",.object(params))
    }
    var supportsRequirementCreateLookup: Bool { capabilities.contains(CrewdRequirementCreateWire.lookupMethod) }
    func requirementCreateLookup(_ original: CrewdRequirementCreateWire.Original) async throws -> CrewdRequirementCreateWire.Receipt? {
        guard original.crew == selectedCrew, supportsRequirementCreateLookup else {
            throw CrewdClientError(message:"当前后台或机组尚不能只读核对原需求。",kind:.notSubmitted)
        }
        let token = generation
        let value = try await call(CrewdRequirementCreateWire.lookupMethod,original.lookup)
        try check(token)
        return try CrewdRequirementCreateWire.receipt(value,original:original)
    }
    @discardableResult func closeRequirement(_ id: CrewdWorkID, expectedStatus: String, note: String) async throws -> CrewdValue {
        guard id.kind == .requirement else { throw CrewdClientError(message: "需求必须使用公共 # 编号。") }
        return try await mutate("todo.close", .object([
            "id": .string(id.raw), "expectedStatus": .string(expectedStatus), "note": .string(note)
        ]))
    }
    var offersRequirementRuling: Bool { CrewdRequirementRulingWire.methods.contains(where:capabilities.contains) }
    var supportsRequirementRuling: Bool {
        (CrewdRequirementRulingWire.methods + ["command.receipts"]).allSatisfy(capabilities.contains)
    }
    func requirementRulingCheck(_ id: CrewdWorkID, crew: String,
                                continuing previous: CrewdRequirementRulingWire.Review? = nil) async throws -> CrewdRequirementRulingWire.Review {
        var paging: [String:CrewdValue] = [:]
        if let previous {
            guard previous.crew == crew, previous.id == id.raw, let after = previous.nextAfter,
                  let revision = previous.revision else { throw CrewdRequirementRulingWire.invalid() }
            paging = ["after":.string(after),"expectedRevision":.string(revision)]
        }
        let value = try await readRequirementRuling("todo.rulingCheck",id:id,crew:crew,paging:paging)
        let page = try CrewdRequirementRulingWire.Review(value,crew:crew,id:id.raw,
            after:previous?.nextAfter,expectedRevision:previous?.revision)
        if let previous, page.state == .ready { return try previous.appending(page) }
        return page
    }
    func requirementRulingResult(_ id: CrewdWorkID, crew: String) async throws -> CrewdRequirementRulingWire.Receipt? {
        let value = try await readRequirementRuling("todo.rulingResult",id:id,crew:crew)
        return try CrewdRequirementRulingWire.result(value,crew:crew,id:id.raw)
    }
    private func readRequirementRuling(_ method: String, id: CrewdWorkID, crew: String,
                                       paging: [String:CrewdValue] = [:]) async throws -> CrewdValue {
        guard id.kind == .requirement, !crew.isEmpty, crew == selectedCrew, supportsRequirementRuling else {
            throw CrewdClientError(message:"当前后台或机组不支持需求裁定核对。",kind:.notSubmitted)
        }
        let token = generation
        var params = paging
        params["crewId"] = .string(crew); params["id"] = .string(id.raw)
        let value = try await call(method,.object(params))
        try check(token)
        return value
    }
    @discardableResult func requirementDetail(_ id: CrewdWorkID, expectedCrew: String? = nil) async throws -> CrewdValue {
        guard id.kind == .requirement else { throw CrewdClientError(message: "需求必须使用公共 # 编号。") }
        let token = generation, epoch = requirementEpoch, request = UUID(); pendingDetails[id] = request
        defer { if pendingDetails[id] == request { pendingDetails[id] = nil } }
        let value = try await call("todo.get", .object(["id": .string(id.raw)]))
        try check(token)
        guard pendingDetails[id] == request, epoch == requirementEpoch else { throw CancellationError() }
        guard try parseRequirement(value) == id,
              expectedCrew == nil || value["crewId"].string == expectedCrew else {
            throw CrewdClientError(message: "需求详情返回了其他对象或机组。")
        }
        if requirements[id]?["status"] == .string("closed"), value["status"] != .string("closed") {
            throw CrewdClientError(message: "需求详情过期，已关闭的需求不能恢复为待处理。")
        }
        requirementEpoch = UUID()
        requirements[id] = withoutAssociation(value)
        return value
    }
    nonisolated static func requirementBlocksPlan(_ value: CrewdValue) -> Bool {
        value["status"] == .string("closed") || value["todo"]["manualState"] == .string("done")
    }
    func requirePlanSource(_ id: CrewdWorkID, sourceCrew: String, in crew: String) async throws {
        guard !crew.isEmpty, selectedCrew == crew, !sourceCrew.isEmpty else {
            throw CrewdClientError(message: "请切回发起计划的机组后核对需求。", kind: .notSubmitted)
        }
        let detail: CrewdValue
        do {
            detail = try await requirementDetail(id, expectedCrew: sourceCrew)
        } catch {
            throw CrewdClientError(message: "无法核对原始需求：" + error.localizedDescription, kind: .notSubmitted)
        }
        if Self.requirementBlocksPlan(detail) {
            throw CrewdClientError(message: "原始需求已人工办结或关闭，无法新建计划。", kind: .notSubmitted)
        }
        let manual = detail["todo"]["manualState"]
        guard manual == .null || manual == .string("open"),
              let status = detail["status"].string,
              ["unclaimed", "in_progress", "open", "done", "needs_ruling"].contains(status),
              status != "done" || detail["derivedStatus"] == .string("done") else {
            throw CrewdClientError(message: "后台未能确认当前需求可新建计划。", kind: .notSubmitted)
        }
    }
    @discardableResult func withdrawTransfer(_ id:CrewdWorkID,expectedRevision:String) async throws -> CrewdValue {
        guard id.kind == .plan else {throw CrewdClientError(message:"交接必须使用公共计划编号。",code:-32602)}
        return try await mutate("work.transferWithdraw",.object(["id":.string(id.raw),"expectedRevision":.string(expectedRevision)]))
    }
    @discardableResult func resolveHuman(_ id:CrewdWorkID,crew:String,text:String,withdraw:Bool) async throws -> CrewdValue {
        guard id.kind == .human,!crew.isEmpty,!text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty else {throw CrewdClientError(message:"请填写公共问题编号与答复或撤回理由。",code:-32602)}
        return try await mutate(withdraw ? "ask.withdraw":"ask.answer",.object(["crewId":.string(crew),"ask":.string(id.raw),withdraw ? "reason":"text":.string(text)]))
    }
    var offersAskResolution: Bool {
        capabilities.contains("ask.resolve") || capabilities.contains("ask.resolveCheck")
    }
    var supportsAskResolution: Bool {
        ["ask.resolve","ask.resolveCheck","command.receipts"].allSatisfy(capabilities.contains)
    }
    func humanResolutionCheck(_ id:CrewdWorkID,crew:String) async throws -> CrewdAskResolutionWire.Review {
        guard id.kind == .human, !crew.isEmpty, supportsAskResolution else {
            throw CrewdClientError(message:"当前后台未具备完整的问题复核与原请求回执。",kind:.notSubmitted)
        }
        let token = generation
        let value = try await call("ask.resolveCheck",.object(["crewId":.string(crew),"ask":.string(id.raw)]))
        try check(token)
        return try .init(value,crew:crew,id:id.raw)
    }
    func humanTodoAttributes(_ id:CrewdWorkID)async throws->CrewdValue {
        let token=generation
        let value=try await call("todo.attributesGet",.object(["object":.object(["kind":.string("humanTask"),"id":.string(id.raw)])]))
        try check(token)
        guard value["object"]["id"] == .string(id.raw),value["object"]["kind"] == .string("humanTask") else{throw CrewdClientError(message:"人待办源属性身份不符。")}
        return value
    }
    func humanDetail(_ id:CrewdWorkID,crew:String)async throws->CrewdValue {
        guard id.kind == .human,capabilities.contains("channelWork") else{throw CrewdClientError(message:"配套后台尚未开放公共人类事项详情。",kind:.notSubmitted)}
        let token=generation
        let value=try await call("ask.get",.object(["crewId":.string(crew),"ask":.string(id.raw)]));try check(token)
        guard value["ask"] == .string(id.raw),value["crewId"] == .string(crew),value["question"].string != nil,value["state"].string != nil else{throw CrewdClientError(message:"人类事项详情身份或原问题不符。")}
        return value
    }
    func duplicateWorkPage(after: String? = nil) async throws -> CrewdWorkDuplicateWire.Page {
        let token = generation, crew = selectedCrew, limit = 20
        guard !crew.isEmpty, capabilities.contains("adminCase.workDuplicates") else { throw CrewdWorkDuplicateWire.invalid() }
        var params: [String:CrewdValue] = ["crewId":.string(crew),"limit":.integer(Int64(limit))]
        if let after { params["after"] = .string(after) }
        let value = try await call("adminCase.workDuplicates",.object(params))
        try check(token)
        return try .init(value,crew:crew,after:after,limit:limit)
    }

    func inspectDuplicateWork(_ candidate: CrewdWorkDuplicateWire.Candidate) async throws -> (duplicate: CrewdPlan, survivor: CrewdPlan) {
        let token = generation, crew = selectedCrew
        guard !crew.isEmpty, capabilities.contains("adminCase.workDuplicates") else { throw CrewdWorkDuplicateWire.invalid() }
        func read(_ id: String, revision: String) async throws -> CrewdPlan {
            let raw = try await call("work.get",.object(["id":.string(id)]))
            try check(token)
            let plan = try parsePlan(raw,full:true)
            guard plan.id.raw == id, plan.crew == crew, plan.revision == revision,
                  !plan.frozen, Set(raw["sources"].array.compactMap(\.string)) == Set(candidate.sources) else {
                throw CrewdClientError(message:"工作或来源已经变化，请重新读取候选。")
            }
            return plan
        }
        let duplicate = try await read(candidate.duplicate,revision:candidate.duplicateRevision)
        let survivor = try await read(candidate.survivor,revision:candidate.survivorRevision)
        guard duplicate.status == "open", duplicate.value["owner"] == .null,
              ["open","active"].contains(survivor.status),
              ["title","goal","acceptance","level"].allSatisfy({ key in
                  duplicate.value[key].string?.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty == false &&
                    duplicate.value[key] == survivor.value[key]
              }) else { throw CrewdClientError(message:"两项当前定义已经不同，请重新读取候选。") }
        return (duplicate,survivor)
    }

    func closureHistory(_ id:CrewdWorkID,beforeSeq:String?=nil,duplicateReviewCrew: String? = nil) async throws -> CrewdValue {
        func sequence(_ raw:String?) -> UInt64? { guard let raw,let n=UInt64(raw),String(n)==raw,n>0 else{return nil};return n }
        guard id.kind == .plan, beforeSeq == nil || sequence(beforeSeq) != nil else {throw CrewdClientError(message:"关闭历史查询身份或游标无效。")}
        let token=generation
        var params:[String:CrewdValue]=["id":.string(id.raw),"limit":.integer(20)]
        if let beforeSeq {params["beforeSeq"] = .string(beforeSeq)}
        let reviewCrew = duplicateReviewCrew == selectedCrew && capabilities.contains("adminCase.workDuplicateHistory") ? duplicateReviewCrew : nil
        if let reviewCrew { params["crewId"] = .string(reviewCrew) }
        let value=try await call(reviewCrew == nil ? "work.closureHistory" : "adminCase.workDuplicateHistory",.object(params));try check(token)
        guard case .object(let fields)=value,case .array(let items)?=fields["items"],items.count <= 20,let next=fields["nextBeforeSeq"],next == .null || sequence(next.string) != nil else {throw CrewdClientError(message:"关闭历史分页格式无效。")}
        var previous=beforeSeq.flatMap {sequence($0)} ?? UInt64.max
        for item in items {
            guard let seq=sequence(item["seq"].string),seq<previous,item["at"].string != nil,
                  ["human","system","session"].contains(item["actor"]["kind"].string ?? ""),
                  ["done","duplicate","superseded"].contains(item["closure"]["kind"].string ?? "") else {throw CrewdClientError(message:"关闭历史事件或顺序无效。")}
            if let reviewCrew { try CrewdWorkDuplicateWire.validateHistory(item,crew:reviewCrew,id:id.raw) }
            else if item["duplicateReview"] != .null { throw CrewdWorkDuplicateWire.invalid() }
            previous=seq
        }
        if let next=next.string {guard sequence(next)==items.last.flatMap({sequence($0["seq"].string)}) else {throw CrewdClientError(message:"关闭历史游标不匹配。")}}
        return value
    }
    func ownerHistory(_ id:CrewdWorkID,before:String?=nil) async throws -> CrewdValue {
        guard id.kind == .plan else {throw CrewdClientError(message:"交接记录必须使用公共计划编号。",code:-32602)}
        let token=generation
        var params:[String:CrewdValue]=["id":.string(id.raw),"limit":.integer(20)]
        if let before {params["before"] = .string(before)}
        let value=try await call("work.history",.object(params));try check(token)
        guard value["id"] == .string(id.raw),case .array = value["items"] else {throw CrewdClientError(message:"交接记录返回了其他对象或无效内容。")}
        return value
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
        let token = generation, current = UUID(), crew = selectedCrew, sourceEpoch = requirementEpoch
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
            var humanIDs: [CrewdWorkID] = []
            for value in items {
                guard let raw = value["ask"]["ask"].string, let id = CrewdWorkID(raw, kind: .human),
                      value["ask"]["crewId"].string != nil, humans[id] == nil else {
                    throw CrewdClientError(message: "待人类事项的全局身份无效或重复。")
                }
                humans[id] = value
                humanIDs.append(id)
            }
            try check(token, current)
            guard sourceEpoch == requirementEpoch else { throw CancellationError() }
            for row in rows { adopt(row) }
            requirementEpoch = UUID()
            for (id, value) in sources {
                var next = value
                // Closed rulings are immutable; list rows omit the on-demand closure fields.
                if value["status"] == .string("closed"), requirements[id]?["status"] == .string("closed"),
                   case .object(var fields) = value, let closure = requirements[id]?["closure"] {
                    fields["closure"] = closure; next = .object(fields)
                }
                requirements[id] = next
            }
            planIDs = rows.map(\.id); requirementIDs = sources.map { $0.0 }; humanInbox = (humanIDs, humans)
            hasLoaded = true
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
    @discardableResult func closePlan(_ id: CrewdWorkID, expectedRevision: String, kind: String, evidenceKind: String, evidence: String, note: String) async throws -> CrewdValue {
        guard id.kind == .plan, ["done","duplicate","superseded"].contains(kind), ["commit","url","text"].contains(evidenceKind),
              !evidence.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty, !note.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty,
              let revision = UInt64(expectedRevision), String(revision) == expectedRevision else {
            throw CrewdClientError(message:"关闭计划需要当前版本、明确类型、凭据和说明。",kind:.notSubmitted)
        }
        return try await mutate("work.close",.object(["id":.string(id.raw),"expectedRevision":.string(expectedRevision),"kind":.string(kind),"evidence":.object(["kind":.string(evidenceKind),"value":.string(evidence.trimmingCharacters(in:.whitespacesAndNewlines))]),"note":.string(note.trimmingCharacters(in:.whitespacesAndNewlines))]))
    }
    @discardableResult func mutate(
        _ method: String, _ params: CrewdValue, planSourceCrew: String? = nil,
        confirmingOriginalPlan: Bool = false, scopeIsCurrent: @MainActor () -> Bool = { true }
    ) async throws -> CrewdValue {
        guard scopeIsCurrent(), !Task.isCancelled else {
            throw CrewdClientError(message: "表单的会话或连接已变化。", kind: .notSubmitted)
        }
        guard ["work.close", "work.create", "work.update", "work.assign", "work.block", "work.transferWithdraw", "work.reassignLost", "ask.answer", "ask.withdraw", "todo.create", "todo.close"].contains(method) else {
            throw CrewdClientError(message: "不支持的工作写入操作。",kind:.notSubmitted)
        }
        if ["ask.answer","ask.withdraw"].contains(method), offersAskResolution {
            throw CrewdClientError(message:"请从原问题复核界面使用可核对的提交。",kind:.notSubmitted)
        }
        if method == "work.reassignLost",!capabilities.contains(method) {throw CrewdClientError(message:"配套后台尚未开放核实失联后的重派操作。",kind:.notSubmitted)}
        if params["channelId"] != .null {
            guard capabilities.contains("channelWork"),let request=params["requestId"].string,UUID(uuidString:request) != nil else{throw CrewdClientError(message:"当前后台尚未开放具有稳定请求编号的群内创建。",kind:.notSubmitted)}
        }
        if confirmingOriginalPlan,
           method != "work.create" || params["channelId"].string?.isEmpty != false {
            throw CrewdClientError(message: "只有原始群内创建请求可以核对。", kind: .notSubmitted)
        }
        guard activeMutations == 0 else { throw CrewdClientError(message: "工作操作正在提交，请稍后再试。",kind:.notSubmitted) }
        if method == "work.create", !confirmingOriginalPlan, !params["sources"].array.isEmpty {
            let crew = params["crewId"].string ?? ""
            let scope = generation
            for source in params["sources"].array {
                guard let raw = source.string, let id = CrewdWorkID(raw, kind: .requirement) else {
                    throw CrewdClientError(message: "计划必须关联公共需求编号。", kind: .notSubmitted)
                }
                try await requirePlanSource(id, sourceCrew: planSourceCrew ?? crew, in: crew)
            }
            guard generation == scope, scopeIsCurrent(), !Task.isCancelled, activeMutations == 0 else {
                throw CrewdClientError(message: "会话或操作状态已变化，请重新核对需求。", kind: .notSubmitted)
            }
        }
        let target = params["id"].string.flatMap { CrewdWorkID($0, kind: .plan) }
        if let target, plans[target]?.frozen == true { throw CrewdClientError(message: "已归档工作只读。", code: -32009, data: .object(["refusal": .string("crew.removed")])) }
        generation = UUID(); refreshID = UUID(); loading = false; pendingDetails.removeAll()
        let token = generation; activeMutations += 1; lastFailure = nil
        var ownsMutation = true
        defer { if ownsMutation { activeMutations = 0 } }
        do {
            let value = try await call(method, params)
            try check(token)
            try validateAssociation(value,params)
            if method == "work.close" {
                guard value["id"] == params["id"], value["status"] == .string(params["kind"] == .string("done") ? "done":"dropped"),
                      value["evidence"]["kind"] == params["evidence"]["kind"],
                      value["evidence"]["value"].string == params["evidence"]["value"].string?.trimmingCharacters(in:.whitespacesAndNewlines) else {
                    throw CrewdClientError(message:"关闭回执与原计划、终态或凭据不符，请读取当前详情核对；不会自动重发。")
                }
            }
            if method == "work.block" { adopt(try parsePlan(value["item"], full: true)) }
            else if method.hasPrefix("work.") { adopt(try parsePlan(value, full: true)) }
            else if method.hasPrefix("ask.") {
                guard value["crewId"] == params["crewId"],value["ask"] == params["ask"],value["state"] == .string(method == "ask.answer" ? "answered":"withdrawn") else {throw CrewdClientError(message:"问题操作回执与所选公共问题或结果不符。")}
                let field=method == "ask.answer" ? "answer":"withdrawReason"
                guard value[field].string == params[method == "ask.answer" ? "text":"reason"].string?.trimmingCharacters(in:.whitespacesAndNewlines) else {throw CrewdClientError(message:"问题操作回执没有保留原始答复或撤回理由。")}
            }
            else if method.hasPrefix("todo.") {
                let id = try parseRequirement(value)
                if method == "todo.close", params["id"].string != id.raw {
                    throw CrewdClientError(message: "关闭需求返回了其他对象。")
                }
                if method == "todo.create", (value["crewId"] != params["crewId"] ||
                    params["text"].string.map({ value["text"].string?.utf8.elementsEqual($0.utf8) == true }) != true) {
                    throw CrewdClientError(message: "创建需求回执与原始机组或原文不符。")
                }
                if method == "todo.close" {
                    guard value["status"] == .string("closed"),
                          value["closure"]["note"].string == params["note"].string?.trimmingCharacters(in: .whitespacesAndNewlines) else {
                        throw CrewdClientError(message: "关闭需求回执缺少已关闭状态或原始裁定理由。")
                    }
                }
                requirementEpoch = UUID()
                requirements[id] = withoutAssociation(value)
            }
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

/// The brief is a projection of one completed patrol, with no write or execution authority.
enum CrewdCaptainBriefPhase: String {
    case queued, inflight, delivered, dropped, unknown

    var label: String {
        switch self {
        case .queued: "待投递"
        case .inflight: "投递中"
        case .delivered: "已送达"
        case .dropped: "已取消"
        case .unknown: "结果未知"
        }
    }
}

struct CrewdHumanBrief {
    let observedAt: Int
    let waits: [CrewdValue]
    let waitsTruncated: Bool
    let cases: [CrewdValue]
    let caseCoverage: String
    let patrol: CrewdValue

    init(value: CrewdValue, crew: String) throws {
        let error = CrewdClientError(message: "行政简报的机组或来源记录不完整。")
        guard value["crewId"] == .string(crew), let observedAt = value["observedAtMs"].int, observedAt >= 0,
              case .array(let waits) = value["humanWaits"], waits.count <= 20,
              case .bool(let waitsTruncated) = value["humanWaitsTruncated"],
              case .array(let cases) = value["cases"], cases.count <= 12,
              let coverage = value["caseCoverage"].string,
              ["complete", "recent_sample", "unavailable"].contains(coverage),
              coverage != "unavailable" || cases.isEmpty else { throw error }
        var asks = Set<CrewdWorkID>(), caseIDs = Set<String>()
        for wait in waits {
            guard let id = CrewdWorkID(wait["askId"].string ?? "", kind: .human), asks.insert(id).inserted,
                  let kind = wait["kind"].string, ["question", "human_task"].contains(kind),
                  wait["question"].string?.isEmpty == false,
                  case .bool = wait["questionTruncated"], case .bool = wait["blockersTruncated"],
                  (wait["askedAtMs"].int ?? -1) >= 0,
                  let elapsed = wait["waitedMs"].int, elapsed >= 0,
                  ["fresh", "pinned", "still_wanted"].contains(wait["tier"].string ?? ""),
                  ["fresh", "pinned", "still_wanted"].contains(wait["notifiedTier"].string ?? ""),
                  wait["defaultDueAtMs"] == .null || (wait["defaultDueAtMs"].int ?? -1) >= 0,
                  case .array(let blockers) = wait["blockingWorkIds"], blockers.count <= 12,
                  Set(blockers.compactMap({ CrewdWorkID($0.string ?? "", kind: .plan) })).count == blockers.count else { throw error }
            if kind == "human_task" {
                guard wait["tier"] == .string("fresh"), wait["notifiedTier"] == .string("fresh") else { throw error }
            }
            let channel = wait["sourceChannelId"], message = wait["sourceMessageId"]
            guard (channel == .null && message == .null) ||
                    (channel.string?.isEmpty == false && message.string?.isEmpty == false) else { throw error }
        }
        for item in cases {
            guard let id = item["caseId"].string, id.hasPrefix("AC"), caseIDs.insert(id).inserted,
                  (item["createdAtMs"].int ?? -1) >= 0,
                  item["decision"].string?.isEmpty == false, item["execution"].string?.isEmpty == false,
                  item["outcomePhase"] == .null || ["unresolved", "stale"].contains(item["outcomePhase"].string ?? ""),
                  item["custodyPhase"] == .null || ["active", "stale", "discharged"].contains(item["custodyPhase"].string ?? "") else { throw error }
        }
        let patrol = value["patrol"]
        guard let patrolState = patrol["state"].string,
              case .object(let scanned) = patrol["scannedCounts"], case .object(let findings) = patrol["findingCounts"],
              (Array(scanned.values) + Array(findings.values)).allSatisfy({ ($0.int ?? -1) >= 0 }),
              case .bool = patrol["findingsTruncated"] else { throw error }
        switch patrolState {
        case "complete", "in_progress":
            guard patrol["runId"].string?.isEmpty == false, let started = patrol["startedAtMs"].int,
                  started >= 0, started <= observedAt else { throw error }
            if patrolState == "complete" {
                guard let completed = patrol["completedAtMs"].int, completed >= started, completed <= observedAt else { throw error }
            } else if patrol["completedAtMs"] != .null { throw error }
        case "not_recorded", "unavailable":
            guard patrol["runId"] == .null, patrol["startedAtMs"] == .null, patrol["completedAtMs"] == .null,
                  scanned.isEmpty, findings.isEmpty else { throw error }
        default: throw error
        }
        self.observedAt = observedAt
        self.waits = waits
        self.waitsTruncated = waitsTruncated
        self.cases = cases
        self.caseCoverage = coverage
        self.patrol = patrol
    }
}

struct CrewdPatrolBrief {
    enum Kind: String, CaseIterable {
        case longWaits = "long_waits"
        case needsRuling = "needs_ruling"
        case unclaimed
        case unowned
        case lost
        case stopEscalated = "stop_escalated"
        case archiveCandidates = "archive_candidates"
        case idleArchiveCandidates = "idle_archive_candidates"
        case archiveReviewIncomplete = "archive_review_incomplete"
        case incompleteChannels = "incomplete_channels"

        var title: String {
            switch self {
            case .longWaits: "等待人类决定较久的工作"
            case .needsRuling: "待裁定需求"
            case .unclaimed: "待认领需求"
            case .unowned: "无人负责的工作"
            case .lost: "负责人失联的工作"
            case .stopEscalated: "停工检查已升级的工作"
            case .archiveCandidates: "待核实归档条件的群"
            case .idleArchiveCandidates: "至少七天未发消息的归档候选"
            case .archiveReviewIncomplete: "暂未核实归档条件的群"
            case .incompleteChannels: "未完整检查的群"
            }
        }

        var unit: String {
            switch self {
            case .archiveCandidates, .idleArchiveCandidates, .archiveReviewIncomplete, .incompleteChannels: "个群"
            default: "项"
            }
        }

        func accepts(_ reference: String) -> Bool {
            switch self {
            case .needsRuling, .unclaimed:
                return CrewdWorkID(reference, kind: .requirement) != nil
            case .archiveCandidates, .idleArchiveCandidates, .archiveReviewIncomplete, .incompleteChannels:
                return !reference.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    && reference.rangeOfCharacter(from: .controlCharacters) == nil
            default:
                return CrewdWorkID(reference, kind: .plan) != nil
            }
        }
    }

    struct Group {
        let kind: Kind
        let count: Int
        let references: [String]
    }

    let groups: [Group]
    let hasPartialReferences: Bool
    let hasUnrecognizedFindings: Bool
    var hasIncompleteChannels: Bool { groups.contains { $0.kind == .incompleteChannels } }

    init(run: CrewdValue) throws {
        let invalid = CrewdClientError(message: "本轮行政简报不完整。")
        guard let started = run["startedAtMs"].int, let completed = run["completedAtMs"].int,
              completed >= started, case .bool(let truncated) = run["truncated"],
              case .object(let counts) = run["counts"], case .object(let findings) = run["findings"],
              Set(counts.keys) == Set(findings.keys) else { throw invalid }
        groups = try Kind.allCases.compactMap { kind in
            guard let rawCount = counts[kind.rawValue] else { return nil }
            guard let count = rawCount.int, count > 0,
                  case .array(let samples)? = findings[kind.rawValue],
                  !samples.isEmpty, samples.count <= 50, samples.count <= count,
                  truncated || samples.count == count else { throw invalid }
            let references = try samples.map { sample in
                guard let reference = sample.string, kind.accepts(reference) else { throw invalid }
                return reference
            }
            guard Set(references).count == references.count else { throw invalid }
            return Group(kind: kind, count: count, references: references)
        }
        hasPartialReferences = truncated
        hasUnrecognizedFindings = counts.keys.contains { Kind(rawValue: $0) == nil }
    }
}
