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
    var readError: CrewdClientError?
    var emptyPlans = false
    var sourceDetails: CrewdValue?
    var askRows: CrewdValue?
    var delayRequirementDetail = false
    func call(_ method: String, _ params: CrewdValue) async throws -> CrewdValue {
        calls.append((method,params))
        if delayNext && method == "work.list" { delayNext = false; return try await withCheckedThrowingContinuation { held = $0 } }
        if delayRequirementDetail && method == "todo.get" { delayRequirementDetail = false; return try await withCheckedThrowingContinuation { held = $0 } }
        let crew = params["crewId"].string ?? "a"
        switch method {
        case "work.list":
            if let readError { throw readError }
            if emptyPlans { return .object(["items":.array([]),"nextAfter":.null]) }
            if params["after"] == .null { return .object(["items":.array([plan(crew:crew, revision: revisionAdvance ? "9007199254740995" : changed ? "9007199254740994" : "9007199254740993",status:changed ? "done" : "future_status")]),"nextAfter":.string("W9007199254740993")]) }
            return .object(["items":.array([plan("W9007199254740994",crew:crew)]),"nextAfter":.null])
        case "todo.list":
            var source: [String:CrewdValue] = ["id":.string("raw-original"),"number":.integer(1),"crewId":.string(crew),"text":.string("original text"),"status":.string(changed ? "done":"in_progress")]
            if !missingSource { source["requirementId"] = .string("#9007199254740995") }
            return .array([.object(source)])
        case "todo.create", "todo.get", "todo.close":
            if let error { throw error }
            if method == "todo.get", let sourceDetails { return sourceDetails }
            return .object(["id":.string("raw-created"),"requirementId":.string("#42"),"crewId":.string("a"),"text":.string("  exact original\n"),"status":.string(method == "todo.close" ? "closed":"unclaimed"),"closure":method == "todo.close" ? .object(["note":.string("reason"),"at":.string("2026-10-13T00:00:00Z")]):.null])
        case "ask.list": return askRows ?? value("[{\"ask\":{\"ask\":\"H9007199254740996\",\"crewId\":\"b\",\"question\":\"global question\",\"item\":\"W9007199254740993\"},\"items\":[\"W9007199254740993\"],\"tier\":\"future_tier\"}]")
        case "work.get":
            var full = plan(params["id"].string!,revision:revisionAdvance ? "9007199254740995" : changed ? "9007199254740994":"9007199254740993",full:true)
            if frozen, case .object(var fields) = full { fields["frozen"] = .bool(true); full = .object(fields) }
            return full
        case "work.update", "work.assign", "work.create":
            if let error { throw error }
            if method == "work.create", let channel=params["channelId"].string,
               case .object(var fields) = plan(revision:"9007199254740994",full:true) {
                fields["channelAssociation"] = .object(["channelId":.string(channel),"linked":.bool(false),"linkRevision":.string("1")])
                return .object(fields)
            }
            changed = true; return plan(revision:"9007199254740994",status:"done",full:true)
        case "work.block":
            if let error { throw error }
            return .object(["item":plan(revision:"9007199254740994",status:"blocked",full:true),"humanTasks":.array([.string("H9007199254740996")]),"split":.array([.string("W9007199254740994")])])
        default: return .null
        }
    }
}
@main struct Test {
    @MainActor static func checkHumanWaits() async throws {
        let source = Fixture()
        let store = CrewdWorkStore(call: { try await source.call($0, $1) })
        store.selectCrew("a")
        let pinned = value(#"{"ask":{"ask":"H9","crewId":"a","question":"original"},"waitedMs":86400000,"tier":"pinned"}"#)
        let otherCrew = value(#"{"ask":{"ask":"H2","crewId":"b"},"waitedMs":7200000,"tier":"future"}"#)
        let fresh = value(#"{"ask":{"ask":"H10","crewId":"a"},"waitedMs":3540000,"tier":"fresh"}"#)
        let olderBackend = value(#"{"ask":{"ask":"H11","crewId":"a"}}"#)
        source.askRows = .array([pinned, otherCrew, fresh, olderBackend])
        try await store.refresh()
        precondition(store.humanTaskIDs.map(\.raw) == ["H9", "H2", "H10", "H11"])
        let filtered = store.humanTaskIDs.filter { store.humanTasks[$0]?["ask"]["crewId"].string == store.selectedCrew }
        precondition(filtered.map(\.raw) == ["H9", "H10", "H11"], "crew filtering preserves authoritative wait order")
        precondition(CrewdHumanWaitPresentation.summary(pinned) == "已等待 1 天 · 已置顶")
        precondition(CrewdHumanWaitPresentation.summary(fresh) == "已等待 59 分钟")
        precondition(CrewdHumanWaitPresentation.summary(otherCrew) == "已等待 2 小时")
        precondition(CrewdHumanWaitPresentation.summary(olderBackend) == nil)
        precondition(CrewdHumanWaitPresentation.summary(value(#"{"waitedMs":-1,"tier":"future"}"#)) == nil)
        let escalated = value(#"{"ask":{"ask":"H9","crewId":"a","question":"original"},"waitedMs":259200000,"tier":"still_wanted"}"#)
        source.askRows = .array([escalated, fresh])
        try await store.refresh()
        let updated = store.humanTasks[filtered[0]]!
        precondition(updated["ask"] == pinned["ask"], "wait escalation leaves the original question untouched")
        precondition(CrewdHumanWaitPresentation.summary(updated) == "已等待 3 天 · 还要吗？")
        precondition(source.calls.allSatisfy { ["ask.list", "todo.list", "work.list"].contains($0.0) })
        print("PASS: Human wait order, 24h pin, 72h review of the original H, and optional field compatibility")
    }

    @MainActor static func main() async throws {
        try await checkHumanWaits()
        let reads=Fixture(), ledger=CrewdWorkStore(call:{try await reads.call($0,$1)})
        ledger.selectCrew("a")
        precondition(!ledger.hasLoaded && !ledger.loading && ledger.lastFailure == nil,
                     "a cleared ledger has not established a real empty result")
        await ledger.ensureLoaded()
        precondition(reads.calls.isEmpty,"wait for connection capabilities before reading")
        ledger.configureCapabilities(["work"])
        reads.delayNext=true
        let firstRead=Task {await ledger.ensureLoaded()}
        while reads.held == nil {await Task.yield()}
        let sharingRead=Task {await ledger.ensureLoaded()}
        await Task.yield()
        precondition(reads.calls.count == 1 && ledger.loading && !ledger.hasLoaded,
                     "another panel shares the current scope read instead of duplicating it")
        let originalScope=ledger.readScope
        ledger.selectCrew("b")
        precondition(ledger.readScope != originalScope && !ledger.hasLoaded)
        reads.held!.resume(returning:.object(["items":.array([plan(crew:"a")]),"nextAfter":.null]));reads.held=nil
        await firstRead.value
        await sharingRead.value
        precondition(!ledger.hasLoaded && ledger.planIDs.isEmpty,"late previous crew cannot qualify the new scope")
        await ledger.ensureLoaded()
        precondition(ledger.hasLoaded && ledger.progress.allSatisfy{$0.crew == "b"})
        let loadedCalls=reads.calls.count
        await ledger.ensureLoaded()
        precondition(reads.calls.count == loadedCalls,"switching ledger panels reuses the loaded scope")
        ledger.invalidateConnection();ledger.configureCapabilities(["work"]);reads.delayNext=true
        let disappearingPanel=Task {await ledger.ensureLoaded()}
        while reads.held == nil {await Task.yield()}
        disappearingPanel.cancel()
        reads.held!.resume(returning:.object(["items":.array([]),"nextAfter":.null]));reads.held=nil
        await disappearingPanel.value
        precondition(ledger.hasLoaded,"leaving one panel does not cancel the store's shared scope read")
        let connectedScope=ledger.readScope
        ledger.invalidateConnection()
        precondition(!ledger.hasLoaded && ledger.readScope != connectedScope)
        ledger.configureCapabilities(["work"])
        reads.delayNext=true
        let disconnectedRead=Task {await ledger.ensureLoaded()}
        while reads.held == nil {await Task.yield()}
        ledger.invalidateConnection()
        reads.held!.resume(returning:.object(["items":.array([]),"nextAfter":.null]));reads.held=nil
        await disconnectedRead.value
        precondition(!ledger.hasLoaded,"a reply from the old connection cannot qualify the reconnect")
        ledger.configureCapabilities(["work"]);reads.readError=CrewdClientError(message:"read denied")
        await ledger.ensureLoaded()
        precondition(!ledger.hasLoaded && ledger.lastFailure?.message == "read denied","failed reads remain visibly unqualified")
        reads.readError=nil;reads.emptyPlans=true
        await ledger.ensureLoaded()
        precondition(ledger.hasLoaded && ledger.progress.isEmpty && ledger.lastFailure == nil,
                     "only a complete successful empty read can show the empty plan state")
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
        let pf=Fixture(), ps=CrewdWorkStore(call:{try await pf.call($0,$1)})
        ps.selectCrew("a")
        let createPlan=value(##"{"crewId":"a","sources":["#42"],"title":"follow-up"}"##)
        let manualDone=value(##"{"requirementId":"#42","crewId":"a","text":"original","status":"done","derivedStatus":"in_progress","todo":{"manualState":"done"}}"##)
        let derivedDone=value(##"{"requirementId":"#42","crewId":"a","text":"original","status":"done","derivedStatus":"done","todo":{}}"##)
        pf.sourceDetails=manualDone
        do {
            _ = try await ps.mutate("work.create",createPlan)
            preconditionFailure("human-resolved source submitted a work.create")
        } catch let error as CrewdClientError { precondition(error.kind == .notSubmitted) }
        precondition(pf.calls.map(\.0) == ["todo.get"],"resolved source must be checked before any write")
        pf.sourceDetails=derivedDone
        _ = try await ps.mutate("work.create",createPlan)
        precondition(pf.calls.filter{$0.0 == "work.create"}.count == 1,"derived done may create follow-up")
        pf.sourceDetails=value(##"{"requirementId":"#42","crewId":"a","text":"original","status":"closed","derivedStatus":"closed","todo":{},"closure":{"note":"final","at":"2026-10-13T00:00:00Z"}}"##)
        do {
            _ = try await ps.mutate("work.create",createPlan)
            preconditionFailure("closed source submitted a work.create")
        } catch let error as CrewdClientError { precondition(error.kind == .notSubmitted) }
        precondition(pf.calls.filter{$0.0 == "work.create"}.count == 1)
        ps.configureCapabilities(["channelWork"])
        let originalRequest=UUID().uuidString.lowercased()
        let retry=CrewdValue.object(["crewId":.string("a"),"sources":.array([.string("#42")]),"title":.string("original plan"),"channelId":.string("one"),"requestId":.string(originalRequest)])
        let beforeRetry=pf.calls.count
        _ = try await ps.mutate("work.create",retry,confirmingOriginalPlan:true)
        precondition(pf.calls[beforeRetry].0 == "work.create" && pf.calls[beforeRetry].1 == retry,
                     "explicit original receipt confirmation keeps its channel and request after the source closes")
        ps.invalidateConnection()
        pf.sourceDetails=derivedDone
        _ = try await ps.mutate("work.create",createPlan)
        precondition(ps.requirements[rid]?["status"] != .string("closed"),
                     "an old authority's closed cache must not block the new authority's source")
        let xf=Fixture(), xs=CrewdWorkStore(call:{try await xf.call($0,$1)})
        xs.selectCrew("a")
        xf.sourceDetails=derivedDone
        do {
            _ = try await xs.mutate("work.create",createPlan,planSourceCrew:"b")
            preconditionFailure("a different source crew qualified plan creation")
        } catch let error as CrewdClientError { precondition(error.kind == .notSubmitted) }
        precondition(xf.calls.map(\.0) == ["todo.get"] && xs.requirements[rid] == nil)
        xf.sourceDetails=value(##"{"requirementId":"#42","crewId":"b","text":"cross-crew original","status":"open","derivedStatus":"done","todo":{"manualState":"open"}}"##)
        _ = try await xs.mutate("work.create",createPlan,planSourceCrew:"b")
        precondition(xf.calls.filter{$0.0 == "work.create"}.count == 1,
                     "a verified reopened source may serve a plan in another crew")
        let lf=Fixture(), ls=CrewdWorkStore(call:{try await lf.call($0,$1)})
        ls.selectCrew("a"); lf.delayRequirementDetail=true
        let late=Task {try await ls.mutate("work.create",createPlan)}
        while lf.held == nil {await Task.yield()}
        ls.selectCrew("b"); ls.selectCrew("a")
        lf.held!.resume(returning:derivedDone); lf.held=nil
        do {_ = try await late.value; preconditionFailure("old scope qualified a new write")} catch {}
        precondition(lf.calls.map(\.0) == ["todo.get"])
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
        print("PASS: scoped initial read/reuse/cancellation/reconnect/empty qualification, organization projection, pagination, global identities, summary/detail, CAS wire, no replay and block contract")
    }
}
