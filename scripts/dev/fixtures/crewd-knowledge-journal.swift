import Foundation
@MainActor final class FakeRPC {
    var context = CrewdDataContext(dbId:"db",authorityId:"db",restoreEpoch:"epoch",crewId:"crew",namespace:"namespace",generation:"1")
    var refused = false
    var committed = false
    var submitted = 0
    var requestID = ""
    func info() -> CrewdDataWorkspaceInfo { .init(context:context,state:"ready",engineVersion:"sqlite",reason:nil) }
    func result(_ id:String, before:String) -> CrewdDataChangeResult {
        .init(requestId:id,phase:.committed,context:context,beforeGeneration:before,afterGeneration:context.generation,snapshotId:nil,created:["document":.object(object:.init(reference:.init(namespace:context.namespace,kind:"view",id:"saved-view"),revision:"1"))],changed:[],conflicts:[],reason:nil,resolvedByRequest:nil)
    }
    func call(_ method:String,_ input:CrewdValue) throws -> CrewdValue {
        switch method {
        case "data.workspace.get": return try CrewdKnowledgeStore.encode(info())
        case "data.object.list": return try CrewdKnowledgeStore.encode(CrewdDataObjectPage(context:context,generation:context.generation,items:[],next:nil))
        case "data.submit":
            submitted += 1; requestID = input["requestId"].string!
            if committed{var response=result(requestID,before:context.generation);if refused{response.phase = .rejected;response.created=[:];response.reason="explicit refusal"};return try CrewdKnowledgeStore.encode(response)}
            throw NSError(domain:"reply lost",code:1)
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
        if CommandLine.arguments.dropFirst().first == "document-refused" {
            rpc.committed=true;rpc.refused=true
            let first=store();await first.refresh()
            _ = await first.submitNativeDocument(title:"title",body:"exact original",channel:"group")
            let reopened=store();await reopened.refresh();let stage=try reopened.nativeDocumentStage(channel:"group")!
            let refused=try reopened.nativeDocumentResult(stage)
            precondition(refused?.phase == .rejected)
            _ = await reopened.submitNativeDocument(title:"newer",body:"newer",channel:"group")
            precondition(rpc.submitted == 1,"known refusal still never retries itself")
            try reopened.finishNativeDocument(stage) // Explicit native edit/new-save action only.
            _ = await reopened.submitNativeDocument(title:"newer",body:"newer",channel:"group")
            precondition(rpc.submitted == 2)
            return
        }
        if CommandLine.arguments.dropFirst().first == "document-unknown" {
            let first=store();await first.refresh()
            _ = await first.submitNativeDocument(title:"  原标题 ",body:"原文🌙\n",channel:"group")
            let reopened=store();await reopened.refresh()
            let stage=try reopened.nativeDocumentStage(channel:"group")!
            _ = await reopened.submitNativeDocument(title:"后来编辑",body:"不能覆盖原文",channel:"group")
            precondition(rpc.submitted == 1 && stage.body == "原文🌙\n" && stage.title == "  原标题 ")
            let result=await reopened.lookupNativeDocument(stage)
            precondition(result?.requestId == stage.id.uuidString.lowercased() && rpc.submitted == 1)
            let again=store();await again.refresh()
            let saved=try again.nativeDocumentResult(stage)
            precondition(saved == result && saved != nil,"original lookup receipt/source survives another reopening")
            _ = await again.submitNativeDocument(title:"later",body:"later",channel:"group")
            precondition(rpc.submitted == 1)
            return
        }
        if CommandLine.arguments.dropFirst().first == "document-stage" {
            rpc.committed=true
            let first=store();await first.refresh()
            _ = await first.submitNativeDocument(title:"原标题",body:"原文🌙",channel:"group")
            let reopened=store();await reopened.refresh()
            _ = await reopened.submitNativeDocument(title:"原标题",body:"原文🌙",channel:"group")
            precondition(rpc.submitted == 1,"reopened saved form must adopt its original source instead of creating a second body/view")
            return
        }
        if CommandLine.arguments.dropFirst().first == "resource-matrix" {
            let object=CrewdValue.object(["kind":.string("requirement"),"id":.string("#1")])
            let owner=CrewdValue.object(["kind":.string("human")])
            let source=CrewdValue.object(["context":try CrewdKnowledgeStore.encode(rpc.context),"reference":.object(["namespace":.string(rpc.context.namespace),"kind":.string("view"),"id":.string("source-view")])])
            for method in CrewdPresentationJournal.resourceMutationMethods.sorted() {
                var fields:[String:CrewdValue]=["requestId":.string(UUID().uuidString.lowercased()),"channelId":.string(method),"expectedRevision":.string("0"),"expectedLinkRevision":.string("0")]
                if method.hasPrefix("channel.knowledge"){fields["source"]=source}
                else if method == "humanTask.create"{fields["crewId"] = .string("crew");fields["text"] = .string("原文 Todo \n")}
                else{fields["object"]=object}
                fields["state"] = .string("done");fields["owner"]=owner;fields["tags"] = .array([.string("前端")])
                let params=CrewdValue.object(fields)
                var sends=0
                do{_ = try await journal.performResourceMutation(method,params:params,crew:"crew",call:{_,_ in sends += 1;throw NSError(domain:"lost",code:1)})}catch{}
                let reopened=try CrewdPresentationJournal(root:root,scope:scope)
                fields["requestId"] = .string(UUID().uuidString.lowercased())
                do{_ = try await reopened.performResourceMutation(method,params:.object(fields),crew:"crew",call:{_,_ in sends += 1;return .null})}catch{}
                precondition(sends == 1,"every new resource method must block a fresh UUID for the same unresolved source after reopen: "+method)
                let original=try reopened.unknownRecords().first{$0.method == method && $0.params == params}!
                var receipt:CrewdValue
                if method.hasPrefix("channel.knowledge") {
                    receipt = .object(["channelId":params["channelId"],"source":source,"linkRevision":.string("1"),"linked":.bool(method == "channel.knowledgeLink")])
                }else if method == "channel.workTagsSet" {
                    receipt = .object(["channelId":params["channelId"],"object":object,"linkRevision":.string("1"),"tags":params["tags"]])
                }else {
                    let human=method == "humanTask.create"
                    let todo=CrewdValue.object(["object":human ? .object(["kind":.string("humanTask"),"id":.string("H2")]):object,"crewId":.string("crew"),"revision":.string("1"),"origin":.string(human ? "humanTask":"requirement"),"owner":owner,"manualState":.string(method == "todo.setState" ? "done":"open")])
                    receipt=todo
                    if human{receipt = .object(["requestId":params["requestId"],"todo":todo,"ask":.object(["ask":.string("H2"),"question":params["text"],"crewId":.string("crew")]),"reused":.bool(false),"channelAssociation":.object(["channelId":params["channelId"],"linkRevision":.string("1"),"linked":.bool(true)])])}
                }
                try CrewdPresentationJournal.validateResourceResult(receipt,method:method,params:params)
                try reopened.acknowledge(original,detail:"Exact original receipt independently validated")
                let afterAck=try reopened.unknownRecords();precondition(!afterAck.contains(where:{$0.id == original.id}),"verified original receipt clears only that unresolved observation")
                fields["channelId"] = .string(method+"-malformed")
                do{_ = try await reopened.performResourceMutation(method,params:.object(fields),crew:"crew",call:{_,_ in .null})}catch{}
                let afterBad=try reopened.unknownRecords();precondition(afterBad.contains(where:{$0.method == method && $0.params["channelId"] == fields["channelId"]}),"every malformed resource result stays Unknown: "+method)
            }
            return
        }
        if let mode=CommandLine.arguments.dropFirst().first,mode.hasPrefix("resource-") {
            let params=CrewdValue.object(["requestId":.string(UUID().uuidString.lowercased()),"channelId":.string("group"),"object":.object(["kind":.string("requirement"),"id":.string("#1")]),"expectedRevision":.string("0"),"state":.string("done")])
            var sends=0
            if mode == "resource-malformed" {
                do{_ = try await journal.performResourceMutation("todo.setState",params:params,crew:"crew",call:{_,_ in sends += 1;return .null})}catch{}
                let unresolved=try journal.unknownRecords();precondition(unresolved.count == 1,"malformed returned result must retain original prepared Unknown before journal terminal")
            }else{
                do{_ = try await journal.performResourceMutation("todo.setState",params:params,crew:"crew",call:{_,_ in sends += 1;throw NSError(domain:"lost",code:1)})}catch{}
                let reopened=try CrewdPresentationJournal(root:root,scope:scope)
                do{_ = try await reopened.performResourceMutation("todo.setState",params:params,crew:"crew",call:{_,_ in sends += 1;throw NSError(domain:"lost",code:1)})}catch{}
                precondition(sends == 1,"reopened original Unknown must block a second resource submission before transport")
            }
            return
        }
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
        var links=CrewdChannelKnowledgeDrafts()
        let source=CrewdChannelKnowledgeReference(context:rpc.context,reference:.init(namespace:rpc.context.namespace,kind:"view",id:"original-view"))
        let link=links.begin(crew:"crew",channel:"group",source:source,revision:"0",linked:true)!
        links.finish(link,accepted:false,uncertain:true)
        precondition(links.begin(crew:"crew",channel:"group",source:source,revision:"1",linked:true) == nil,"Unknown link retains exact original UUID, not a replacement")
        var groupCalls:[String]=[],lookupResult=CrewdValue.object(["requestId":.string(link.id.uuidString.lowercased()),"status":.string("notFound")])
        let group=CrewdChannelKnowledgeStore(call:{method,params in
            groupCalls.append(method)
            if method == "resource.requestLookup"{precondition(params["requestId"] == .string(link.id.uuidString.lowercased()));return lookupResult}
            if method == "channel.knowledgeList"{return try CrewdKnowledgeStore.encode(CrewdChannelKnowledgePage(channelId:"group",linkRevision:"1",sourceRevision:"opaque-source",items:[],next:nil))}
            preconditionFailure("lookup attempted a write or content recreation")
        })
        group.selectChannel(crew:"crew",channel:"group")
        let absent=try await group.lookup(link);precondition(absent == nil && groupCalls == ["resource.requestLookup"])
        let result=CrewdChannelKnowledgeReceipt(channelId:"group",linkRevision:"1",source:source,linked:true)
        lookupResult = .object(["requestId":.string(link.id.uuidString.lowercased()),"status":.string("committed"),"operation":.string("channel.knowledgeLink"),"result":try CrewdKnowledgeStore.encode(result)])
        let found=try await group.lookup(link);precondition(found == result)
        precondition(!groupCalls.contains("data.submit") && !groupCalls.contains("channel.knowledgeLink"),"joining an already saved source must not recreate content or resend Unknown")
        group.selectChannel(crew:"crew",channel:"other")
        let before=groupCalls.count
        do{_ = try await group.lookup(link);preconditionFailure("foreign group lookup accepted")}catch is CancellationError{}
        precondition(groupCalls.count == before)
        print("PASS real journal/store: lost reply retains original UUID across reopening; new writes blocked; lookup never resubmits; restoreEpoch transition observed")
    }
}
