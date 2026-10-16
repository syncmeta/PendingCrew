import Foundation
@MainActor final class FakeRPC {
    var context = CrewdDataContext(dbId:"db",authorityId:"db",restoreEpoch:"epoch",crewId:"crew",namespace:"namespace",generation:"1")
    var submitted = 0
    var requestID = ""
    func info() -> CrewdDataWorkspaceInfo { .init(context:context,state:"ready",engineVersion:"sqlite",reason:nil) }
    func result(_ id:String, before:String) -> CrewdDataChangeResult {
        .init(requestId:id,phase:.committed,context:context,beforeGeneration:before,afterGeneration:context.generation,snapshotId:nil,created:[:],changed:[],conflicts:[],reason:nil,resolvedByRequest:nil)
    }
    func call(_ method:String,_ input:CrewdValue) throws -> CrewdValue {
        switch method {
        case "data.workspace.get": return try CrewdKnowledgeStore.encode(info())
        case "data.object.list": return try CrewdKnowledgeStore.encode(CrewdDataObjectPage(context:context,generation:context.generation,items:[],next:nil))
        case "data.submit":
            submitted += 1; requestID = input["requestId"].string!; throw NSError(domain:"reply lost",code:1)
        case "data.request.get":
            precondition(input["requestId"].string == requestID)
            context.generation = "2"
            return try CrewdKnowledgeStore.encode(CrewdDataRequestGetResult(workspace:info(),lookup:.known(value:result(requestID,before:"1"))))
        case "data.restore":
            let before = context.generation
            context.restoreEpoch = "restored-epoch"; context.generation = "3"
            return try CrewdKnowledgeStore.encode(result(input["requestId"].string!,before:before))
        default: preconditionFailure("unexpected call "+method)
        }
    }
}
@main struct Regression {
    @MainActor static func main() async throws {
        let root = URL(fileURLWithPath:"/private/tmp/pcw-client-store-regression."+UUID().uuidString)
        let scope = CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:root.path,authorityID:"authority",databaseID:"db",grantID:nil,grantRevision:nil)
        let journal = try CrewdPresentationJournal(root:root,scope:scope)
        let origin = CrewdKnowledgeStore.Origin(scope:scope,observation:UUID(),connection:UUID(),crew:"crew")
        let rpc = FakeRPC()
        func store() -> CrewdKnowledgeStore { CrewdKnowledgeStore(origin:{origin},rpc:{ try rpc.call($0,$1) },journal:{journal}) }
        let first = store(); await first.refresh()
        let change = CrewdDataChange.apply(message:"manual",edits:[.sql(statements:[.init(sql:"INSERT INTO example VALUES (1)",params:[])])])
        await first.submitFromNativeHost(change)
        precondition(rpc.submitted == 1 && first.pending.count == 1)
        let original = first.pending[0].id
        await first.submitFromNativeHost(change); precondition(rpc.submitted == 1)
        first.invalidate()
        let reopened = store(); await reopened.refresh()
        precondition(reopened.pending.map(\.id) == [original])
        await reopened.lookup(original)
        precondition(rpc.submitted == 1 && reopened.pending.isEmpty)
        await reopened.restoreSnapshot("snapshot")
        precondition(reopened.pending.isEmpty && reopened.result?.context.restoreEpoch == "restored-epoch")
        print("PASS real journal/store: lost reply retains original UUID across reopening; new writes blocked; lookup never resubmits; restoreEpoch transition observed")
    }
}
