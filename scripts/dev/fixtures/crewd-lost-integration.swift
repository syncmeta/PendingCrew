import Foundation
func require(_ ok:@autoclosure()->Bool,_ message:String){precondition(ok(),message)}
@main struct LostIntegration {
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
        store.configureCapabilities(human.capabilities)
        store.selectCrew(a["id"].string!)
        let raw=try await store.createRequirement(requestID:UUID(),text:"original synthetic handoff requirement")
        let source=raw["requirementId"].string!
        var fields=CrewdPlanDrafts.Fields();fields.title="typed handoff plan";fields.owner=owner
        let key=CrewdPlanDrafts.Key(crew:a["id"].string!,object:"",source:source)
        let command=try CrewdPlanCommand.build(key,fields,revision:nil)
        let made=try await store.mutate(command.0,command.1),id=CrewdWorkID(made["id"].string!,kind:.plan)!
        fields.action = .reassignLost;fields.owner=target;fields.note="ACTUAL_LOST_EXPLANATION_ON_DEMAND"
        let lost=try CrewdPlanCommand.build(CrewdPlanDrafts.Key(crew:a["id"].string!,object:id.raw,source:""),fields,revision:made["revision"].string!)
        do {_ = try await store.mutate(lost.0,lost.1);preconditionFailure("healthy owner accepted as lost")}catch let e as CrewdClientError{require(e.code == -32602,"actual healthy backend rejection")}
        _ = try await human.call("session.stop",.object(["sessionId":.string(owner),"reason":.string("synthetic stop before explicit failed resume")]))
        let stoppedDeadline=Date().addingTimeInterval(20)
        while true {
            let view=try await human.call("session.inspect",.object(["sessionId":.string(owner)]))
            if view["live"]["running"] == .bool(false) {break}
            guard Date()<stoppedDeadline else{throw CrewdClientError(message:"actual stopped process did not finish")}
            try await Task.sleep(nanoseconds:20_000_000)
        }
        try Data("synthetic only".utf8).write(to:captures.appendingPathComponent("fail-launch-"+owner))
        _ = try await human.call("session.resume",.object(["sessionId":.string(owner)]))
        let deadline=Date().addingTimeInterval(20)
        while true {
            let row=try await human.call("session.get",.object(["sessionId":.string(owner)]))
            if row["resumeFailedAt"].string != nil {break}
            guard Date()<deadline else{throw CrewdClientError(message:"actual failed resume fact missing")}
            try await Task.sleep(nanoseconds:20_000_000)
        }
        let assigned=try await store.mutate(lost.0,lost.1)
        require(assigned["owner"] == .string(target),"actual persisted loss authorizes typed reassignment")
        let history=try await store.ownerHistory(id)
        require(history["items"].array.first?["how"] == .string("lost") && history["items"].array.first?["note"] == .string(fields.note),"actual lost history differs from ordinary assign")
        let count=methods.filter{$0=="work.reassignLost"}.count
        require(count == 2,"one rejected healthy attempt and one successful verified attempt")
        store.invalidateConnection();human.disconnect();try await human.connect(path:data+"/crewd.sock");store.configureCapabilities(human.capabilities);try await store.refresh()
        require(methods.filter{$0=="work.reassignLost"}.count==count,"reconnect never repeats verified reassignment")
        for session in [owner,target]{_ = try await human.call("session.stop",.object(["sessionId":.string(session),"reason":.string("synthetic acceptance finished")]))}
        ownerClient.disconnect();targetClient.disconnect();human.disconnect()
        print("PASS: actual daemon + real fake-runner handshake failure + Swift typed healthy rejection/verified loss/history/read-only reconnect")
    }
}
