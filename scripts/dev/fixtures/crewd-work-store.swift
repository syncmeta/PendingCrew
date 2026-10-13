import Foundation

func value(_ text: String) -> CrewdValue { try! JSONDecoder().decode(CrewdValue.self, from: Data(text.utf8)) }
func plan(_ id: String = "W9007199254740993", crew: String = "a", revision: String = "9007199254740993", status: String = "open", full: Bool = false) -> CrewdValue {
    var fields: [String: CrewdValue] = ["id":.string(id),"crewId":.string(crew),"revision":.string(revision),"status":.string(status),"title":.string("current"),"owner":.null,"frozen":.bool(false),"note":.null,"nextCheckAt":.null,"parent":.null,"blockedOn":.array([])]
    fields["sources"] = .array(id == "W9007199254740993" ? [.string("#9007199254740995")] : [])
    if full { fields["goal"] = .string("long goal"); fields["dependsOn"] = .array([]); fields["acceptance"] = .string("accept"); fields["evidence"] = .null }
    return .object(fields)
}
@MainActor final class Fixture {
    var calls: [(String,CrewdValue)] = []
    var changed = false
    var revisionAdvance = false
    var error: CrewdClientError?
    var missingSource = false
    var frozen = false
    var delayNext = false
    var held: CheckedContinuation<CrewdValue, Error>?
    func call(_ method: String, _ params: CrewdValue) async throws -> CrewdValue {
        calls.append((method,params))
        if delayNext && method == "work.list" { delayNext = false; return try await withCheckedThrowingContinuation { held = $0 } }
        let crew = params["crewId"].string ?? "a"
        switch method {
        case "work.list":
            if params["after"] == .null { return .object(["items":.array([plan(crew:crew, revision: revisionAdvance ? "9007199254740995" : changed ? "9007199254740994" : "9007199254740993",status:changed ? "done" : "future_status")]),"nextAfter":.string("W9007199254740993")]) }
            return .object(["items":.array([plan("W9007199254740994",crew:crew)]),"nextAfter":.null])
        case "todo.list":
            var source: [String:CrewdValue] = ["id":.string("raw-original"),"number":.integer(1),"crewId":.string(crew),"text":.string("original text"),"status":.string(changed ? "done":"in_progress")]
            if !missingSource { source["requirementId"] = .string("#9007199254740995") }
            return .array([.object(source)])
        case "todo.create", "todo.get", "todo.close":
            if let error { throw error }
            return .object(["id":.string("raw-created"),"requirementId":.string("#42"),"crewId":.string("a"),"text":.string("  exact original\n"),"status":.string(method == "todo.close" ? "closed":"unclaimed"),"closure":method == "todo.close" ? .object(["note":.string("reason"),"at":.string("2026-10-13T00:00:00Z")]):.null])
        case "ask.list": return value("[{\"ask\":{\"ask\":\"H9007199254740996\",\"crewId\":\"b\",\"question\":\"global question\",\"item\":\"W9007199254740993\"},\"items\":[\"W9007199254740993\"],\"tier\":\"future_tier\"}]")
        case "work.get":
            var full = plan(params["id"].string!,revision:revisionAdvance ? "9007199254740995" : changed ? "9007199254740994":"9007199254740993",full:true)
            if frozen, case .object(var fields) = full { fields["frozen"] = .bool(true); full = .object(fields) }
            return full
        case "work.update", "work.assign", "work.create":
            if let error { throw error }
            changed = true; return plan(revision:"9007199254740994",status:"done",full:true)
        case "work.block":
            if let error { throw error }
            return .object(["item":plan(revision:"9007199254740994",status:"blocked",full:true),"humanTasks":.array([.string("H9007199254740996")]),"split":.array([.string("W9007199254740994")])])
        default: return .null
        }
    }
}
@main struct Test {
    @MainActor static func main() async throws {
        let transferID=CrewdWorkID("W42",kind:.plan)!
        guard case .object(var full)=plan("W42",full:true), case .object(var short)=plan("W42") else {preconditionFailure()}
        full["pendingTransfer"] = .object(["to":.string("target"),"requestSeq":.string("99"),"note":.string("on-demand full handoff"),"requestedBy":.string("owner")])
        short["pendingTransfer"] = .object(["to":.string("target"),"requestSeq":.string("99")])
        var pending=CrewdPlan(id:transferID,summary:.object(short),detail:.object(full))
        precondition(pending.value["pendingTransfer"]["note"] == .string("on-demand full handoff"),"bounded summary must preserve same-offer detail after list refresh")
        short["pendingTransfer"] = .object(["to":.string("target"),"requestSeq":.string("100")]); pending.summary = .object(short)
        precondition(pending.value["pendingTransfer"]["note"] == .null,"different offer must not inherit old private handoff note")
        short["pendingTransfer"] = .object(["to":.string("another-target"),"requestSeq":.string("99")]); pending.summary = .object(short)
        precondition(pending.value["pendingTransfer"]["note"] == .null,"same sequence for different target must not inherit old note")
        short["pendingTransfer"] = .object(["to":.string("target"),"requestSeq":.string("99")]); short["revision"] = .string("9007199254740994"); pending.summary = .object(short)
        precondition(pending.value["pendingTransfer"]["note"] == .null,"different work revision must not inherit stale detail")
        short["pendingTransfer"] = .null; pending.summary = .object(short)
        precondition(pending.value["pendingTransfer"] == .null,"explicit clear must not resurrect pending offer")
        let rf=Fixture(), rs=CrewdWorkStore(call: {try await rf.call($0,$1)})
        rs.selectCrew("a")
        let request=UUID(), created=try await rs.createRequirement(requestID:request, text:"  exact original\n")
        let rid=CrewdWorkID("#42",kind:.requirement)!
        precondition(created["text"] == .string("  exact original\n"))
        precondition(rf.calls.first!.1["requestId"] == .string(request.uuidString.lowercased()))
        precondition(rf.calls.first!.1["text"] == .string("  exact original\n"))
        _ = try await rs.requirementDetail(rid)
        _ = try await rs.closeRequirement(rid, expectedStatus:"unclaimed", note:"reason")
        precondition(rs.requirements[rid]?["closure"]["note"] == .string("reason"))
        rf.error=CrewdClientError(message:"lost reply")
        do {_ = try await rs.createRequirement(requestID:request,text:"  exact original\n"); preconditionFailure()} catch {}
        let writeCount=rf.calls.filter {$0.0 == "todo.create"}.count
        rs.invalidateConnection(); rf.error=nil; try await rs.refresh()
        precondition(rf.calls.filter {$0.0 == "todo.create"}.count == writeCount, "uncertain requirement write never automatically replayed")
        let f=Fixture(), s=CrewdWorkStore(call: {try await f.call($0,$1)})
        s.selectCrew("a"); try await s.refresh()
        let id=CrewdWorkID("W9007199254740993",kind:.plan)!, source=CrewdWorkID("#9007199254740995",kind:.requirement)!
        precondition(s.planIDs.count == 2 && s.progress == s.todoPlans)
        precondition(s.plans[id]!.revision == "9007199254740993" && s.plans[id]!.status == "future_status")
        precondition(s.requirements[source]!["number"] == .integer(1))
        precondition(s.plans(from: source).map(\.id) == [id], "initial requirement linkage must not require opening detail")
        precondition(s.humanTasks.count == 1 && s.humanTasks.keys.first!.raw == "H9007199254740996")
        _ = try await s.detail(id); try await s.refresh()
        precondition(s.plans[id]!.detail?["goal"] == .string("long goal") && s.plans(from:source).count == 1)
        _ = try await s.mutate("work.update", .object(["id":.string(id.raw),"expectedRevision":.string(s.plans[id]!.revision),"status":.string("done")]))
        precondition(s.progress.first!.status == "done" && s.todoPlans.first!.status == "done" && s.requirements[source]!["status"] == .string("done"))
        precondition(s.plans[id]!.value["owner"] == .null && s.plans[id]!.value["note"] == .null)
        f.revisionAdvance = true; try await s.refresh()
        precondition(s.plans[id]!.detail == nil && s.plans[id]!.revision == "9007199254740995", "new summary revision must invalidate obsolete detail")
        precondition(s.plans(from: source).map(\.id) == [id], "revision refresh must retain reliable source linkage")
        let facts=value("{\"refusal\":\"work.revision_conflict\",\"currentWork\":{\"id\":\"W9007199254740993\",\"revision\":\"18446744073709551615\"},\"unknown\":true}")
        f.error=CrewdClientError(message:"conflict",code:-32009,data:facts)
        let count=f.calls.count
        do { _ = try await s.mutate("work.update",.object(["id":.string(id.raw),"expectedRevision":.string("1"),"status":.string("done") ])); preconditionFailure("conflict accepted") } catch let e as CrewdClientError {
            precondition(e.code == -32009 && e.currentWork["revision"] == .string("18446744073709551615") && s.lastFailure?.data == facts)
        }
        precondition(f.calls.count == count+1, "rejected write must not replay")
        f.error=CrewdClientError(message:"lost reply")
        let lost=f.calls.count
        do { _ = try await s.mutate("work.update",.object(["id":.string(id.raw),"expectedRevision":.string("1")])); preconditionFailure() } catch {}
        s.invalidateConnection(); f.error=nil; try await s.refresh()
        precondition(f.calls.filter{$0.0 == "work.update"}.count == 3 && f.calls.count > lost, "reconnect reads only")
        f.delayNext=true
        let old=Task {try await s.refresh()}
        while f.held == nil { await Task.yield() }
        s.selectCrew("b")
        f.held!.resume(returning:.object(["items":.array([plan(crew:"a")]),"nextAfter":.null])); f.held=nil
        do {try await old.value; preconditionFailure("late old scope accepted")} catch is CancellationError {}
        precondition(s.planIDs.isEmpty); try await s.refresh(); precondition(s.progress.allSatisfy{$0.crew == "b"})
        f.missingSource=true
        do {try await s.refresh(); preconditionFailure("guessed #local")} catch {}
        precondition(s.lastFailure != nil && s.requirementIDs == [source],"unsupported association preserves last good snapshot")
        f.missingSource=false
        let blockParams=value("{\"id\":\"W9007199254740993\",\"expectedRevision\":\"9007199254740994\",\"on\":[{\"type\":\"new_human\",\"question\":\"decision\",\"default\":{\"text\":\"option\",\"afterMinutes\":15}}],\"canContinue\":{\"type\":\"split\",\"items\":[{\"crewId\":\"b\",\"title\":\"independent\"}]}}")
        let result=try await s.mutate("work.block",blockParams)
        precondition(result["humanTasks"].array == [.string("H9007199254740996")])
        precondition(f.calls.first(where:{$0.0 == "work.block"})!.1 == blockParams)
        f.error=CrewdClientError(message:"permission",code:-32003,data:.object(["refusal":.string("work.actor_forbidden")]))
        let humans=s.humanTasks, plansBefore=s.plans, n=f.calls.count
        do {_ = try await s.mutate("work.block",blockParams); preconditionFailure("unauthorized block") } catch let e as CrewdClientError { precondition(e.code == -32003) }
        precondition(f.calls.count == n+1 && s.humanTasks == humans && s.plans == plansBefore,"no phantom H or split")
        f.error=nil; f.frozen=true; _ = try await s.detail(id)
        let frozenCount=f.calls.count
        do {_ = try await s.mutate("work.update",.object(["id":.string(id.raw),"expectedRevision":.string("1")])); preconditionFailure("frozen write") } catch {}
        precondition(f.calls.count == frozenCount,"known frozen write never submitted")
        precondition(CrewdWorkID("W01",kind:.plan) == nil && CrewdWorkID("H1",kind:.plan) == nil)
        if CommandLine.arguments.count > 1 {
            let c=CrewdClient(); try await c.connect(path:CommandLine.arguments[1])
            do {_ = try await c.call("work.update",.object([:])); preconditionFailure("wire failure accepted")}
            catch let e as CrewdClientError {precondition(e.code == -32009 && e.refusal == "work.revision_conflict" && e.currentWork["revision"] == .string("18446744073709551615") && e.data["unknown"] == .bool(true))}
            c.disconnect()
        }
        print("PASS: organization projection, pagination, global identities, summary/detail, CAS wire, no replay, scoped refresh and block contract")
    }
}
