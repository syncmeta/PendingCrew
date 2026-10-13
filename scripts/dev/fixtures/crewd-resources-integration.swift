import Foundation
func require(_ ok:@autoclosure()->Bool,_ message:String){precondition(ok(),message)}
@main struct ResourcesIntegration {
    @MainActor static func main() async throws {
        let data=CommandLine.arguments[1],captures=URL(fileURLWithPath:CommandLine.arguments[2],isDirectory:true),work=CommandLine.arguments[3]
        let human=CrewdClient();try await human.connect(path:data+"/crewd.sock")
        var methods:[String]=[]
        var failAfterCreate=false,delayGet=false,held:CheckedContinuation<CrewdValue,Error>?
        let resources=CrewdResources{method,params in
            methods.append(method);let result=try await human.call(method,params)
            if method == "document.create",failAfterCreate {failAfterCreate=false;throw CrewdClientError(message:"synthetic unknown after actual accepted create")}
            if method == "document.get",delayGet {delayGet=false;return try await withCheckedThrowingContinuation{held=$0}}
            return result
        }
        resources.configureCapabilities(human.capabilities)
        human.onEvent={name,value in resources.handleEvent(name,value)}
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
        let crew=a["id"].string!,otherCrew=b["id"].string!
        require(human.capabilities.contains("documents") && human.capabilities.contains("viewFolders"),"normal daemon advertises completed schema10 resource capabilities")
        resources.selectCrew(crew)
        let originalTitle="immutable client document",originalBody="typed UTF8 🌙 original",request=UUID()
        let receipt=try await resources.createDocument(request:request,title:originalTitle,body:originalBody),doc=receipt["document"]["id"].string!
        require(resources.documentIDs.contains(doc),"typed create refreshes real summary list")
        let detail=try await resources.document(doc);require(detail.body == originalBody,"actual full document bytes")
        let current=resources.documents[doc]!
        let updated=try await resources.updateDocument(id:doc,revision:current.revision,title:"collaborative current",body:"edited exact text")
        let collaborative=try await tool(ownerClient,"document_update",.object(["id":.string(doc),"expected_revision":.string(updated.revision),"title":.string("collaborative current"),"body":.string("trusted samecrew actor exact text")]))
        require(collaborative["updatedBy"]["sessionId"] == .string(owner) && collaborative["body"] == .null,"actual trusted samecrew collaboration records provenance with compact receipt")
        do {_ = try await resources.updateDocument(id:doc,revision:current.revision,title:"stale request",body:"must not commit");preconditionFailure("actual stale document CAS accepted")}
        catch let error as CrewdClientError {require(error.code == -32009 && error.data["currentDocument"]["revision"] == collaborative["revision"],"actual structured currentDocument retained")}
        let confirmed=try await resources.createDocument(request:request,title:originalTitle,body:originalBody)
        require(confirmed["reused"] == .bool(true) && confirmed["document"]["id"] == .string(doc) && confirmed["document"]["title"] == .string("collaborative current"),"original UUID confirmation returns current collaborative metadata")
        let plan=try await human.call("work.create",.object(["crewId":.string(crew),"title":.string("one work ledger across scopes")]))
        let beforePlan=try await human.call("work.get",.object(["id":plan["id"]]))
        let dm=try await human.call("channel.create",.object(["crewId":.string(otherCrew),"kind":.string("dm"),"members":.array([.object(["kind":.string("session"),"id":.string(target)])])]))
        for view in CrewdResourceView.allCases {
            let initial=try await resources.refreshFolders(view)
            let parent=try await resources.changeFolder("viewFolder.create",view:view,fields:["requestId":.string(UUID().uuidString.lowercased()),"expectedTreeRevision":.string(initial.revision),"name":.string("typed parent "+view.rawValue)])
            let parentID=parent.folders.first!["id"].string!
            let child=try await resources.changeFolder("viewFolder.create",view:view,fields:["requestId":.string(UUID().uuidString.lowercased()),"expectedTreeRevision":.string(parent.revision),"name":.string("typed child"),"parentId":.string(parentID)])
            let childID=child.folders.first{$0["parentId"] == .string(parentID)}!["id"].string!
            let moved=try await resources.changeFolder("viewFolder.update",view:view,fields:["id":.string(childID),"expectedTreeRevision":.string(child.revision),"name":.string("renamed actual child"),"parentId":.string(parentID),"position":.integer(0)])
            let object:CrewdValue
            switch view {case .messages:object = .object(["kind":.string("channel"),"id":dm["id"]]);case .documents:object = .object(["kind":.string("document"),"id":.string(doc)]);case .todo,.progress:object = .object(["kind":.string("work"),"id":plan["id"]])}
            let assigned=try await resources.changeFolder("viewFolder.assign",view:view,fields:["expectedTreeRevision":.string(moved.revision),"object":object,"folderId":.string(parentID)])
            require(assigned.folder(for:object) == parentID,"actual typed assignment to live object")
            let deleted=try await resources.changeFolder("viewFolder.delete",view:view,fields:["id":.string(parentID),"expectedTreeRevision":.string(assigned.revision)])
            require(deleted.assignments.isEmpty && deleted.folders.first{$0["id"] == .string(childID)}?["parentId"] == .null,"delete preserves child promoted to root and unclassifies direct members")
            let afterPlan=try await human.call("work.get",.object(["id":plan["id"]]));require(afterPlan == beforePlan,"folder operations never create a second work status authority")
        }
        // Real accepted creation followed by a synthetic lost client receipt, not a fake server success.
        var drafts=CrewdResourceDrafts();let key=CrewdResourceDrafts.Key(crew:crew,view:.documents,object:"",operation:"create",domain:.document)
        var fields=CrewdResourceDrafts.Fields();fields.title="uncertain actual creation";fields.body="original unresolved body";drafts.edit(key,fields)
        let unknown=drafts.begin(key)!;failAfterCreate=true
        do {_ = try await resources.createDocument(request:unknown.request,title:unknown.fields.title,body:unknown.fields.body);preconditionFailure("lost receipt test seam did not fire")}catch {drafts.finish(unknown,accepted:false,uncertain:true)}
        fields.body="later edit must not replay original";drafts.edit(key,fields)
        require(drafts.uncertain(key).first?.fields.body == unknown.fields.body,"actual unknown UUID operation preserves complete original payload")
        let recovered=try await resources.createDocument(request:unknown.request,title:unknown.fields.title,body:unknown.fields.body)
        require(recovered["reused"] == .bool(true),"explicit original UUID confirmation does not duplicate actual committed creation")
        // Genuine delivered resource events coalesce only bounded resource reads.
        await resources.activate(.documents,force:true);try await Task.sleep(nanoseconds:300_000_000)
        let eventStart=methods.count
        let rev=resources.documents[doc]!.revision
        let first=try await human.call("document.update",.object(["id":.string(doc),"expectedRevision":.string(rev),"title":.string("event1"),"body":.string("one")]))
        let second=try await human.call("document.update",.object(["id":.string(doc),"expectedRevision":first["revision"],"title":.string("event2"),"body":.string("two")]))
        try await Task.sleep(nanoseconds:350_000_000)
        require(resources.documents[doc]!.revision == second["revision"].string!,"actual socket document events refresh current facts")
        require(Array(methods.dropFirst(eventStart)).sorted() == ["document.list","viewFolder.list"],"actual two events merge one bounded read-only resource refresh")
        resources.handleEvent("events.lagged",.object(["dropped":.integer(2)]));try await Task.sleep(nanoseconds:250_000_000)
        delayGet=true;let pending=Task{try await resources.document(doc)}
        while held == nil {await Task.yield()}
        resources.selectCrew(otherCrew);held!.resume(returning:try await human.call("document.get",.object(["id":.string(doc)])));held=nil
        do {_ = try await pending.value;preconditionFailure("late foreign scope result adopted")}catch is CancellationError{}
        resources.selectCrew(crew);_ = try await resources.document(doc)
        _ = try await resources.deleteDocument(id:doc,revision:resources.documents[doc]!.revision)
        require(!resources.documentIDs.contains(doc) && resources.documents[doc]!.deleted,"actual soft delete removes list membership and keeps terminal metadata")
        let writes=methods.filter{!["document.list","document.get","viewFolder.list"].contains($0)}
        resources.invalidateConnection();human.disconnect();try await human.connect(path:data+"/crewd.sock");resources.configureCapabilities(human.capabilities);await resources.activate(.documents)
        require(methods.filter{!["document.list","document.get","viewFolder.list"].contains($0)} == writes,"actual reconnect performs reads without replaying writes")
        for session in [owner,target]{_ = try await human.call("session.stop",.object(["sessionId":.string(session),"reason":.string("synthetic resource acceptance finished")]))}
        ownerClient.disconnect();targetClient.disconnect();human.disconnect()
        print("PASS: actual daemon typed documents/CAS/original UUID confirmation/four folder scopes/current events/read-only reconnect; isolated fake actors only")
    }
}
