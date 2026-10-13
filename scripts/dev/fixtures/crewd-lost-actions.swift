import Foundation
@MainActor final class LostPeer {
    var calls:[(String,CrewdValue)]=[]
    var error:CrewdClientError?
    var owner="old-owner"
    func plan()->CrewdValue { .object(["id":.string("W9007199254740993"),"crewId":.string("a"),"revision":.string("18446744073709551615"),"status":.string("active"),"title":.string("actual current plan"),"owner":.string(owner),"frozen":.bool(false),"note":.null,"nextCheckAt":.string("2026-10-14T00:00:00Z"),"parent":.null,"blockedOn":.array([]),"sources":.array([]),"dependsOn":.array([]),"goal":.null,"acceptance":.null,"evidence":.null]) }
    func call(_ method:String,_ params:CrewdValue)async throws->CrewdValue {
        calls.append((method,params))
        switch method {
        case "work.get":return plan()
        case "work.list":return .object(["items":.array([plan()]),"nextAfter":.null])
        case "todo.list","ask.list":return .array([])
        case "work.reassignLost":if let error{throw error};owner="new-owner";return plan()
        default:throw CrewdClientError(message:"unexpected fixture method "+method)
        }
    }
}
@main struct LostActionsTest {
    @MainActor static func main()async throws {
        let peer=LostPeer(),store=CrewdWorkStore(call:{try await peer.call($0,$1)})
        store.selectCrew("a")
        let key=CrewdPlanDrafts.Key(crew:"a",object:"W9007199254740993",source:"")
        var fields=CrewdPlanDrafts.Fields();fields.action = .reassignLost;fields.owner="new-owner";fields.note="actual verified loss explanation"
        let command=try CrewdPlanCommand.build(key,fields,revision:"18446744073709551615")
        precondition(command.0 == "work.reassignLost" && command.1 == .object(["id":.string(key.object),"expectedRevision":.string("18446744073709551615"),"owner":.string(fields.owner),"note":.string(fields.note)]),"lost and ordinary assignment share exact params, never coerce CAS number")
        do {_ = try await store.mutate(command.0,command.1);preconditionFailure("missing cap wrote")}catch{precondition(!CrewdClientError.isUncertainSubmission(error))}
        precondition(peer.calls.isEmpty,"missing negotiated capability must issue no read/write")
        store.configureCapabilities(["work.reassignLost"])
        let id=CrewdWorkID(key.object,kind:.plan)!
        _ = try await store.detail(id)
        let current=peer.plan(),facts=CrewdValue.object(["currentWork":current,"unknown":.bool(true)])
        peer.error=CrewdClientError(message:"healthy owner",code:-32602,data:facts)
        do {_ = try await store.mutate(command.0,command.1);preconditionFailure("healthy accepted")}catch let e as CrewdClientError {precondition(e.code == -32602 && e.data == facts && e.currentWork == current && !CrewdClientError.isUncertainSubmission(e))}
        precondition(store.plans[id]!.value["owner"] == .string("old-owner") && store.lastFailure?.data == facts,"backend rejection retains actual current facts and prior owner")
        peer.error=nil;_ = try await store.mutate(command.0,command.1)
        precondition(store.plans[id]!.value["owner"] == .string("new-owner"))
        let writes=peer.calls.filter{$0.0 == "work.reassignLost"}.count
        store.invalidateConnection();try await store.refresh()
        precondition(peer.calls.filter{$0.0 == "work.reassignLost"}.count == writes,"reconnect only reads, never repeats lost reassignment")
        do {_ = try await store.mutate(command.0,command.1);preconditionFailure("stale hello cap survived disconnect")}catch{precondition(!CrewdClientError.isUncertainSubmission(error))}
        precondition(peer.calls.filter{$0.0 == "work.reassignLost"}.count == writes)
        print("PASS: typed lost/CAS wire, missing-cap zero calls, healthy structured rejection, current facts, success and cap invalidation/no replay")
    }
}
