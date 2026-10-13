import Foundation
func require(_ ok:@autoclosure()->Bool,_ message:String){precondition(ok(),message)}
@main struct ChannelIntegration {
    @MainActor static func main() async throws {
        let data=CommandLine.arguments[1],captures=URL(fileURLWithPath:CommandLine.arguments[2],isDirectory:true),workingDirectory=CommandLine.arguments[3]
        let human=CrewdClient();try await human.connect(path:data+"/crewd.sock")
        var methods:[String]=[]
        var loseNext:String?,holdList=false,held:CheckedContinuation<CrewdValue,Error>?,heldValue:CrewdValue?
        let store=CrewdWorkStore{method,params in
            methods.append(method);let value=try await human.call(method,params)
            if loseNext == method {loseNext=nil;throw CrewdClientError(message:"synthetic receipt lost after actual accepted group creation")}
            return value
        }
        let group=CrewdChannelWorkStore{method,params in
            methods.append(method);let value=try await human.call(method,params)
            if method == "channel.workList",holdList {holdList=false;heldValue=value;return try await withCheckedThrowingContinuation{held=$0}}
            return value
        }
        group.setActive(true);group.configureCapabilities(human.capabilities);store.configureCapabilities(human.capabilities)
        human.onEvent={name,value in group.handleEvent(name,value)}
        let a=try await human.call("crew.create",.object(["title":.string("Synthetic owner crew"),"workingDirectory":.string(workingDirectory)]))
        let b=try await human.call("crew.create",.object(["title":.string("Synthetic target crew"),"workingDirectory":.string(workingDirectory)]))
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
        let crew=a["id"].string!
        require(human.capabilities.contains("channelWork"),"real normal schema10 group capability")
        func channel(_ title:String)async throws->String {try await human.call("channel.create",.object(["crewId":.string(crew),"kind":.string("topic"),"name":.string(title),"members":.array([.object(["kind":.string("session"),"id":.string(owner)])])]))["id"].string!}
        let first=try await channel("typed group A"),second=try await channel("typed group B")
        store.selectCrew(crew);group.selectChannel(crew:crew,channel:first);try await group.refresh()
        var requirements=CrewdRequirementDrafts();requirements.set(crew,channel:first,text:"original accepted but unresolved group requirement")
        let req=requirements.begin(crew,channel:first)!;loseNext="todo.create"
        do {_ = try await store.createRequirement(requestID:req.requestID,text:req.text,channel:req.channel);preconditionFailure("actual accepted requirement receipt seam did not fire")}catch {requirements.finish(req,accepted:false,uncertain:true)}
        try await group.refresh();let requirement=group.items.first{$0["object"]["kind"] == .string("requirement")}!["object"]
        _ = try await group.changeLink(object:requirement,expectedRevision:group.linkRevision!,linked:false)
        requirements.set(crew,channel:first,text:"new draft after original uncertain requirement")
        require(requirements.confirm(req),"explicit original request confirmation retains its UUID")
        let reqReceipt=try await store.createRequirement(requestID:req.requestID,text:req.text,channel:req.channel);requirements.finish(req,accepted:true)
        require(reqReceipt["channelAssociation"]["linked"] == .bool(false) && requirements.text(crew,channel:first) == "new draft after original uncertain requirement","confirmed requirement remains unlinked and does not clear new input")
        try await group.refresh();require(group.items.isEmpty,"requirement confirmation never silently reattaches it")
        var plans=CrewdPlanDrafts();let key=CrewdPlanDrafts.Key(crew:crew,object:"",source:requirement["id"].string!,channel:first)
        var fields=CrewdPlanDrafts.Fields();fields.title="original unresolved group plan";fields.owner=owner;plans.edit(key,fields)
        let original=plans.begin(key)!,command=try CrewdPlanCommand.build(key,original.fields,revision:nil,requestID:original.version)
        loseNext="work.create"
        do {_ = try await store.mutate(command.0,command.1);preconditionFailure("actual accepted plan receipt seam did not fire")}catch {plans.finish(original,accepted:false,uncertain:true)}
        try await group.refresh();let work=group.items.first{$0["object"]["kind"] == .string("work")}!["object"],id=CrewdWorkID(work["id"].string!,kind:.plan)!
        _ = try await group.changeLink(object:work,expectedRevision:group.linkRevision!,linked:false)
        fields.title="edited later group draft";plans.edit(key,fields);require(plans.confirm(original),"confirm the immutable old work request")
        let workReceipt=try await store.mutate(command.0,command.1);plans.finish(original,accepted:true)
        require(workReceipt["channelAssociation"]["linked"] == .bool(false) && plans.fields(key).title == "edited later group draft","actual original UUID confirmation preserves unlink and later input")
        require(store.plans[id]!.value["channelAssociation"] == .null,"creation association never contaminates canonical plan authority")
        try await group.refresh();require(group.items.isEmpty,"plan confirmation does not link or recreate work")
        _ = try await group.changeLink(object:work,expectedRevision:group.linkRevision!,linked:true)
        group.selectChannel(crew:crew,channel:second);try await group.refresh();require(group.items.isEmpty,"other real group starts empty instead of showing all crew work")
        _ = try await group.changeLink(object:work,expectedRevision:group.linkRevision!,linked:true)
        let detail=try await store.detail(id)
        _ = try await tool(ownerClient,"work_item_update",.object(["id":.string(id.raw),"expected_revision":.string(detail.revision),"status":.string("done")]))
        try await Task.sleep(nanoseconds:350_000_000)
        require(group.items.first{$0["object"] == work}?["status"] == .string("done"),"real delivered work event updates current group from one W authority")
        group.selectChannel(crew:crew,channel:first);try await group.refresh();require(group.items.first{$0["object"] == work}?["status"] == .string("done"),"same W in original group stays consistent")
        let reqDetail=try await store.requirementDetail(CrewdWorkID(requirement["id"].string!,kind:.requirement)!)
        require(reqDetail["status"] == .string("done"),"unlinked original requirement still derives completion from its actual source work")
        let blocker=try await human.call("work.create",.object(["crewId":.string(crew),"title":.string("public H blocks actual independent plan"),"owner":.string(owner)]))
        let blocked=try await tool(ownerClient,"work_item_block",.object(["id":blocker["id"],"expected_revision":blocker["revision"],"on":.array([.object(["type":.string("new_human"),"question":.string("actual typed group H question")])]),"can_continue":.object(["type":.string("nothing")])]))
        let hid=CrewdWorkID(blocked["humanTasks"].array[0].string!,kind:.human)!,hObject=CrewdValue.object(["kind":.string("humanTask"),"id":.string(hid.raw)])
        _ = try await group.changeLink(object:hObject,expectedRevision:group.linkRevision!,linked:true)
        group.selectChannel(crew:crew,channel:second);try await group.refresh();_ = try await group.changeLink(object:hObject,expectedRevision:group.linkRevision!,linked:true)
        try await store.refresh();let opened=try await store.humanDetail(hid,crew:crew)
        require(opened["state"] == .string("open") && opened["question"] == .string("actual typed group H question"),"typed full public H read uses real original question")
        _ = try await store.resolveHuman(hid,crew:crew,text:"  actual typed answer  ",withdraw:false)
        try await Task.sleep(nanoseconds:350_000_000)
        require(group.items.first{$0["object"] == hObject}?["status"] == .string("answered"),"real Human answer is projected into the current group")
        group.selectChannel(crew:crew,channel:first);try await group.refresh();require(group.items.first{$0["object"] == hObject}?["status"] == .string("answered"),"same H is answered in both groups")
        let closed=try await store.humanDetail(hid,crew:crew)
        require(closed["answer"] == .string("actual typed answer") && closed["closedAt"].string != nil,"closed H detail retains trimmed original answer after leaving pending inbox")
        holdList=true;let old=Task{try await group.refresh()};while held == nil {await Task.yield()}
        group.selectChannel(crew:crew,channel:second);held!.resume(returning:heldValue!);held=nil;heldValue=nil
        do{try await old.value;preconditionFailure("old channel delayed projection adopted")}catch is CancellationError{}
        require(group.items.isEmpty && !group.loaded,"late old group cannot fill newly selected group")
        let writes=methods.filter{!["channel.workList","work.list","work.get","todo.list","todo.get","ask.list","ask.get"].contains($0)}
        group.invalidateConnection();store.invalidateConnection();human.disconnect();try await human.connect(path:data+"/crewd.sock");group.configureCapabilities(human.capabilities);store.configureCapabilities(human.capabilities);try await group.refresh();try await store.refresh()
        require(methods.filter{!["channel.workList","work.list","work.get","todo.list","todo.get","ask.list","ask.get"].contains($0)} == writes,"reconnect never replays any group create/link/unlink/answer")
        require(methods.filter{$0 == "todo.create"}.count == 2 && methods.filter{$0 == "work.create"}.count == 2,"only one actual creation plus explicit immutable confirmation, no automated create retry")
        for session in [owner,target]{_ = try await human.call("session.stop",.object(["sessionId":.string(session),"reason":.string("synthetic group acceptance finished")]))}
        ownerClient.disconnect();targetClient.disconnect();human.disconnect()
        print("PASS: actual normal daemon typed group/#W H lifecycle/real lost receipt original UUID after unlink/no reattach/current events/closed H detail/late scope/read-only reconnect")
    }
}
