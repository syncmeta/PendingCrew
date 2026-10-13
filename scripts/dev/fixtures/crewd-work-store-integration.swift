import Foundation

func require(_ yes: @autoclosure () -> Bool, _ message: String) { precondition(yes(), message) }
@main struct WorkIntegration {
    @MainActor static func main() async throws {
        let data=CommandLine.arguments[1], crew="synthetic-source-crew", owner="worker-synthetic-source"
        let client=CrewdClient(); try await client.connect(path:data + "/crewd.sock")
        var calls: [String] = []
        let store=CrewdWorkStore { method, params in calls.append(method); return try await client.call(method,params) }
        store.selectCrew(crew); try await store.refresh()
        require(store.requirementIDs.count == 1,"CLI imported source must have one global identity")
        let source=store.requirementIDs[0]
        require(store.requirements[source]!["text"] == .string("synthetic original demand"),"raw source is original text")
        let request = UUID(), original = "  Swift public requirement\n"
        let created = try await store.createRequirement(requestID:request, text:original)
        let publicSource = CrewdWorkID(created["requirementId"].string!,kind:.requirement)!
        let repeatResult = try await store.createRequirement(requestID:request,text:original)
        require(repeatResult["requirementId"] == .string(publicSource.raw),"same draft UUID returns same public identity")
        require(store.requirementIDs.count == 2,"idempotent creation allocates only one source")
        let raw = try await store.requirementDetail(publicSource)
        require(raw["text"] == .string(original),"public get retains original bytes")
        do {
            _ = try await store.createRequirement(requestID:request,text:"changed payload")
            preconditionFailure("same request with changed payload accepted")
        } catch let e as CrewdClientError { require(e.code == -32009,"payload conflict is explicit") }
        let closed = try await store.closeRequirement(publicSource,expectedStatus:"unclaimed",note:"synthetic deliberate ruling")
        require(closed["status"] == .string("closed") && closed["closure"]["note"] == .string("synthetic deliberate ruling"),"human ruling comes from actual backend")
        require(store.requirements[publicSource]?["closure"]["note"] == .string("synthetic deliberate ruling"),"list reconciliation retains immutable ruling detail")
        var creation=CrewdPlanDrafts.Fields();creation.title="Swift organization parent";creation.owner=owner
        let createCommand=try CrewdPlanCommand.build(.init(crew:crew,object:"",source:source.raw),creation,revision:nil)
        let made=try await store.mutate(createCommand.0,createCommand.1)
        let id=CrewdWorkID(made["id"].string!,kind:.plan)!, initial=made["revision"].string!
        require(store.plans(from:source).map(\.id) == [id],"real list must associate source before detail")
        require(store.todoPlans == store.progress,"Todo and progress share canonical plans")
        _ = try await store.detail(id)
        let planKey=CrewdPlanDrafts.Key(crew:crew,object:id.raw,source:"")
        var report=CrewdPlanDrafts.Fields();report.action = .report;report.note="typed form progress"
        let reportCommand=try CrewdPlanCommand.build(planKey,report,revision:initial)
        _ = try await store.mutate(reportCommand.0,reportCommand.1)
        require(store.plans[id]!.status == "active" && store.plans[id]!.value["nextCheckAt"].string != nil,"typed report persists actual checkpoint")
        var block=CrewdPlanDrafts.Fields();block.action = .block;block.question="synthetic decision";block.split=true;block.splitTitle="independent portion"
        let blockCommand=try CrewdPlanCommand.build(planKey,block,revision:store.plans[id]!.revision)
        let blocked=try await store.mutate(blockCommand.0,blockCommand.1)
        require(blocked["humanTasks"].array.count == 1 && blocked["split"].array.count == 1,"backend issued H and child atomically")
        let h=blocked["humanTasks"].array[0].string!, child=CrewdWorkID(blocked["split"].array[0].string!,kind:.plan)!
        require(store.humanTasks[CrewdWorkID(h,kind:.human)!]?["ask"]["item"] == .string(id.raw),"global H links same W")
        require(store.plans[id]!.status == "blocked","real block state reflected")
        _ = try await store.mutate("ask.answer",.object(["crewId":.string(crew),"ask":.string(h),"text":.string("synthetic answer")]))
        require(store.humanTasks[CrewdWorkID(h,kind:.human)!] == nil,"answered H disappears from same organization inbox")
        require(store.plans[id]!.status == "active", "answer resumes unblocked work with satisfied dependencies")
        require(store.plans[id]!.value["blockedOn"] == .array([]), "last human blocker really cleared")
        require(store.plans[id]!.value["nextCheckAt"].string != nil, "resumed active work retains mandatory checkpoint")
        let latest=store.plans[id]!.revision, count=calls.count
        do {
            _ = try await store.mutate("work.update",.object(["id":.string(id.raw),"expectedRevision":.string(initial),"status":.string("done")]))
            preconditionFailure("stale CAS unexpectedly accepted")
        } catch let e as CrewdClientError {
            require(e.code == -32009 && e.refusal == "work.revision_conflict","actual RPC structured conflict")
            require(e.currentWork["id"] == .string(id.raw) && e.currentWork["revision"] == .string(latest),"actual bounded current facts retained")
        }
        require(calls.count == count+1,"CAS failure never replays or auto writes")
        let part=try await store.detail(child)
        var completed=CrewdPlanDrafts.Fields();completed.action = .complete
        let childCommand=try CrewdPlanCommand.build(.init(crew:crew,object:child.raw,source:""),completed,revision:part.revision)
        _ = try await store.mutate(childCommand.0,childCommand.1)
        let doneCommand=try CrewdPlanCommand.build(planKey,completed,revision:store.plans[id]!.revision)
        _ = try await store.mutate(doneCommand.0,doneCommand.1)
        require(store.requirements[source]!["status"] == .string("done"),"requirement status derives from same real plans")
        require(store.plans(from:source).map(\.id) == [id],"new revision summary still links source")
        let revision=store.plans[id]!.revision, beforeReconnect=calls.count
        client.disconnect(); store.invalidateConnection()
        try await client.connect(path:data + "/crewd.sock"); try await store.refresh()
        require(store.plans[id]!.revision == revision && store.plans[id]!.status == "done","read-only reconnect preserves actual state")
        require(calls.dropFirst(beforeReconnect).allSatisfy{["work.list","todo.list","ask.list"].contains($0)},"reconnect has no write replay")
        let record=try JSONEncoder().encode(calls)
        try record.write(to:URL(fileURLWithPath:data).deletingLastPathComponent().appendingPathComponent("swift-methods.json"))
        client.disconnect()
        print("PASS: real CLI source import -> daemon -> typed plan forms/create/report/block H/split -> answer -> completion/CAS -> read-only reconnect")
    }
}
