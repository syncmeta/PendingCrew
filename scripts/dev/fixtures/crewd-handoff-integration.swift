import Foundation
func require(_ ok:@autoclosure()->Bool,_ message:String){precondition(ok(),message)}
@main struct HandoffIntegration {
    @MainActor static func main() async throws {
        let data=CommandLine.arguments[1],captures=URL(fileURLWithPath:CommandLine.arguments[2],isDirectory:true),work=CommandLine.arguments[3]
        let human=CrewdClient();try await human.connect(path:data+"/crewd.sock")
        var methods:[String]=[]
        let store=CrewdWorkStore{method,params in methods.append(method);return try await human.call(method,params)}
        let a=try await human.call("crew.create",.object(["title":.string("Synthetic owner crew"),"workingDirectory":.string(work)]))
        let b=try await human.call("crew.create",.object(["title":.string("Synthetic target crew"),"workingDirectory":.string(work)]))
        func start(_ crew:CrewdValue,_ title:String) async throws->String {
            let row=try await human.call("session.start",.object(["crewId":crew["id"],"role":.string("captain"),"executor":.string("codex"),"brief":.string("synthetic offline identity acceptance"),"title":.string(title),"isolation":.bool(false)]))
            return row["id"].string!
        }
        let owner=try await start(a,"Synthetic owner"),target=try await start(b,"Synthetic target")
        func agent(_ session:String) async throws->CrewdClient {
            let capture=captures.appendingPathComponent(session+".json"),deadline=Date().addingTimeInterval(20)
            while !FileManager.default.fileExists(atPath:capture.path){guard Date()<deadline else{throw CrewdClientError(message:"fake identity capture timeout")};try await Task.sleep(nanoseconds:20_000_000)}
            let info=try JSONDecoder().decode(CrewdValue.self,from:Data(contentsOf:capture))
            require(info["session"] == .string(session),"capture identity comes from actual runner launch")
            let token=try String(contentsOfFile:info["tokenFile"].string!,encoding:.utf8).trimmingCharacters(in:.whitespacesAndNewlines)
            let c=CrewdClient();try await c.connect(path:data+"/crewd.sock")
            _ = try await c.call("agent.attach",.object(["sessionId":.string(session),"token":.string(token)]));return c
        }
        let ownerClient=try await agent(owner),targetClient=try await agent(target)
        func tool(_ c:CrewdClient,_ name:String,_ args:CrewdValue) async throws->CrewdValue {
            let r=try await c.call("agent.call",.object(["tool":.string(name),"args":args]));require(r["isError"] == .bool(false),"actual attached tool succeeded")
            let text=r["text"].string!,begin=text.firstIndex(of:"{")!,end=text.lastIndex(of:"}")!
            return try JSONDecoder().decode(CrewdValue.self,from:Data(text[begin...end].utf8))
        }
        store.selectCrew(a["id"].string!)
        let raw=try await store.createRequirement(requestID:UUID(),text:"original synthetic handoff requirement")
        let source=raw["requirementId"].string!
        var fields=CrewdPlanDrafts.Fields();fields.title="typed handoff plan";fields.owner=owner
        let key=CrewdPlanDrafts.Key(crew:a["id"].string!,object:"",source:source)
        let command=try CrewdPlanCommand.build(key,fields,revision:nil)
        let made=try await store.mutate(command.0,command.1),id=CrewdWorkID(made["id"].string!,kind:.plan)!
        let note="ON_DEMAND_HANDOFF_DETAIL "+String(repeating:"retained original context ",count:40).trimmingCharacters(in:.whitespacesAndNewlines)
        let offer=try await tool(ownerClient,"work_item_transfer_request",.object(["id":.string(id.raw),"expected_revision":made["revision"],"to":.string(target),"note":.string(note)]))
        require(offer["owner"] == .string(owner) && offer["pendingTransfer"]["note"] == .null,"offer is compact and not accepted ownership")
        _ = try await store.detail(id);try await store.refresh()
        require(store.plans[id]!.value["pendingTransfer"]["note"] == .string(note),"actual compact daemon list preserves exact-offer fetched detail")
        let baseline=try await store.ownerHistory(id)
        _ = try await store.withdrawTransfer(id,expectedRevision:store.plans[id]!.revision)
        require(store.plans[id]!.value["pendingTransfer"] == .null && store.plans[id]!.value["owner"] == .string(owner),"Human cancels offer without accepting it")
        let cancelledHistory=try await store.ownerHistory(id)
        require(cancelledHistory["total"] == baseline["total"],"cancel does not invent owner history")
        let second=try await tool(ownerClient,"work_item_transfer_request",.object(["id":.string(id.raw),"expected_revision":.string(store.plans[id]!.revision),"to":.string(target),"note":.string(note)]))
        require(second["pendingTransfer"]["requestSeq"] != offer["pendingTransfer"]["requestSeq"],"reoffer has new actual event identity")
        _ = try await tool(targetClient,"work_item_transfer_accept",.object(["id":.string(id.raw),"expected_revision":second["revision"]]))
        try await store.refresh();_ = try await store.detail(id)
        require(store.plans[id]!.value["owner"] == .string(target) && store.plans[id]!.value["pendingTransfer"] == .null,"real attached target accepted actual transfer")
        let history=try await store.ownerHistory(id)
        require(history["items"].array.first?["how"] == .string("transfer") && history["items"].array.first?["note"] == .string(note),"actual backend history retains accepted original explanation")
        let count=methods.filter{$0=="work.transferWithdraw"}.count
        store.invalidateConnection();human.disconnect();try await human.connect(path:data+"/crewd.sock");try await store.refresh()
        require(methods.filter{$0=="work.transferWithdraw"}.count==count,"reconnect only reads, never replays cancellation")
        for session in [owner,target]{_ = try await human.call("session.stop",.object(["sessionId":.string(session),"reason":.string("synthetic acceptance finished")]))}
        ownerClient.disconnect();targetClient.disconnect();human.disconnect()
        print("PASS: actual daemon + generated fake-runner identity + Swift typed plan/compact detail/Human cancel/target accept/history/read-only reconnect")
    }
}
