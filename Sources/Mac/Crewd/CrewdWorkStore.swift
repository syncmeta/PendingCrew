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
    @Published private(set) var hasLoaded = false
    @Published private(set) var readScope = UUID()
    @Published private(set) var capabilities:Set<String>=[]
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
    func plans(from requirement: CrewdWorkID) -> [CrewdPlan] {
        plans.values.filter { $0.value["sources"].array.contains(.string(requirement.raw)) }
            .sorted { $0.id.raw < $1.id.raw }
    }
    func selectCrew(_ crew: String) {
        guard selectedCrew != crew else { return }
        ensureTask?.cancel(); ensureTask = nil
        generation = UUID(); refreshID = UUID(); pendingDetails.removeAll(); loading = false
        selectedCrew = crew; planIDs = []; requirementIDs = []; lastFailure = nil
        hasLoaded = false; readScope = UUID()
    }
    /// Stops old reads. Reconnect reconciles by reading; never resends a mutation.
    func invalidateConnection() {
        ensureTask?.cancel(); ensureTask = nil
        capabilities=[]
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
    func coordinationStatus(crew: String, id: String) async throws -> CrewdValue {
        let token = generation
        guard !crew.isEmpty, CrewdWorkID(id,kind:.plan) != nil else { throw CrewdClientError(message:"计划身份无效。") }
        let result = try await call("work.coordinationStatus",.object(["crewId":.string(crew),"id":.string(id)]))
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
        return record
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
            guard case .object(let record) = run,
                  record["startedAtMs"]?.int != nil,
                  let end = record["completedAtMs"], end == .null || end.int != nil,
                  case .bool? = record["truncated"],
                  case .object(let scanned)? = record["scannedCounts"],
                  case .object(let counts)? = record["counts"],
                  scanned.values.allSatisfy({ ($0.int ?? -1) >= 0 }),
                  counts.values.allSatisfy({ ($0.int ?? -1) >= 0 }) else { throw CrewdClientError(message:"后台巡检记录不完整。") }
        }
        return value
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
              ["observed","prepared","dispatched","answered","unknown","unconfigured","waiting_admission","stale"].contains(row["decision"].string ?? ""),["not_started","registered","requested","running","unknown","stale"].contains(row["execution"].string ?? ""),row["retirement"] == .string("not_started"),case .array(let targets)=row["targets"] else {throw CrewdClientError(message:"行政记录身份、来源或状态不完整。")}
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
    @discardableResult func closeRequirement(_ id: CrewdWorkID, expectedStatus: String, note: String) async throws -> CrewdValue {
        guard id.kind == .requirement else { throw CrewdClientError(message: "需求必须使用公共 # 编号。") }
        return try await mutate("todo.close", .object([
            "id": .string(id.raw), "expectedStatus": .string(expectedStatus), "note": .string(note)
        ]))
    }
    @discardableResult func requirementDetail(_ id: CrewdWorkID) async throws -> CrewdValue {
        guard id.kind == .requirement else { throw CrewdClientError(message: "需求必须使用公共 # 编号。") }
        let token = generation, epoch = requirementEpoch, request = UUID(); pendingDetails[id] = request
        defer { if pendingDetails[id] == request { pendingDetails[id] = nil } }
        let value = try await call("todo.get", .object(["id": .string(id.raw)]))
        try check(token)
        guard pendingDetails[id] == request, epoch == requirementEpoch else { throw CancellationError() }
        guard try parseRequirement(value) == id else { throw CrewdClientError(message: "需求详情返回了其他对象。") }
        if requirements[id]?["status"] == .string("closed"), value["status"] != .string("closed") {
            throw CrewdClientError(message: "需求详情过期，已关闭的需求不能恢复为待处理。")
        }
        requirementEpoch = UUID()
        requirements[id] = withoutAssociation(value)
        return value
    }
    @discardableResult func withdrawTransfer(_ id:CrewdWorkID,expectedRevision:String) async throws -> CrewdValue {
        guard id.kind == .plan else {throw CrewdClientError(message:"交接必须使用公共计划编号。",code:-32602)}
        return try await mutate("work.transferWithdraw",.object(["id":.string(id.raw),"expectedRevision":.string(expectedRevision)]))
    }
    @discardableResult func resolveHuman(_ id:CrewdWorkID,crew:String,text:String,withdraw:Bool) async throws -> CrewdValue {
        guard id.kind == .human,!crew.isEmpty,!text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty else {throw CrewdClientError(message:"请填写公共问题编号与答复或撤回理由。",code:-32602)}
        return try await mutate(withdraw ? "ask.withdraw":"ask.answer",.object(["crewId":.string(crew),"ask":.string(id.raw),withdraw ? "reason":"text":.string(text)]))
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
    func closureHistory(_ id:CrewdWorkID,beforeSeq:String?=nil) async throws -> CrewdValue {
        func sequence(_ raw:String?) -> UInt64? { guard let raw,let n=UInt64(raw),String(n)==raw,n>0 else{return nil};return n }
        guard id.kind == .plan, beforeSeq == nil || sequence(beforeSeq) != nil else {throw CrewdClientError(message:"关闭历史查询身份或游标无效。")}
        let token=generation
        var params:[String:CrewdValue]=["id":.string(id.raw),"limit":.integer(20)]
        if let beforeSeq {params["beforeSeq"] = .string(beforeSeq)}
        let value=try await call("work.closureHistory",.object(params));try check(token)
        guard case .object(let fields)=value,case .array(let items)?=fields["items"],let next=fields["nextBeforeSeq"],next == .null || sequence(next.string) != nil else {throw CrewdClientError(message:"关闭历史分页格式无效。")}
        var previous=beforeSeq.flatMap {sequence($0)} ?? UInt64.max
        for item in items {
            guard let seq=sequence(item["seq"].string),seq<previous,item["at"].string != nil,
                  ["human","system","session"].contains(item["actor"]["kind"].string ?? ""),
                  ["done","duplicate","superseded"].contains(item["closure"]["kind"].string ?? "") else {throw CrewdClientError(message:"关闭历史事件或顺序无效。")}
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
            for value in items {
                guard let raw = value["ask"]["ask"].string, let id = CrewdWorkID(raw, kind: .human),
                      value["ask"]["crewId"].string != nil, humans[id] == nil else {
                    throw CrewdClientError(message: "待人类事项的全局身份无效或重复。")
                }
                humans[id] = value
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
            planIDs = rows.map(\.id); requirementIDs = sources.map { $0.0 }; humanTasks = humans
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
    @discardableResult func mutate(_ method: String, _ params: CrewdValue) async throws -> CrewdValue {
        guard ["work.close", "work.create", "work.update", "work.assign", "work.block", "work.transferWithdraw", "work.reassignLost", "ask.answer", "ask.withdraw", "todo.create", "todo.close"].contains(method) else {
            throw CrewdClientError(message: "不支持的工作写入操作。",kind:.notSubmitted)
        }
        if method == "work.reassignLost",!capabilities.contains(method) {throw CrewdClientError(message:"配套后台尚未开放核实失联后的重派操作。",kind:.notSubmitted)}
        if params["channelId"] != .null {
            guard capabilities.contains("channelWork"),let request=params["requestId"].string,UUID(uuidString:request) != nil else{throw CrewdClientError(message:"当前后台尚未开放具有稳定请求编号的群内创建。",kind:.notSubmitted)}
        }
        guard activeMutations == 0 else { throw CrewdClientError(message: "工作操作正在提交，请稍后再试。",kind:.notSubmitted) }
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
                if method == "todo.create", (value["crewId"] != params["crewId"] || value["text"] != params["text"]) {
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
