import Foundation
func row(_ kind:String,_ id:String)->CrewdValue {.object(["object":.object(["kind":.string(kind),"id":.string(id)]),"crewId":.string("a"),"label":.string("actual bounded label"),"labelTruncated":.bool(false),"status":.string(kind == "humanTask" ? "open":"ready"),"revision":.string("1")])}
func page(_ items:[CrewdValue],channel:String="group-a",links:String="1",source:String="9",next:String?=nil)->CrewdValue {.object(["channelId":.string(channel),"linkRevision":.string(links),"sourceRevision":.string(source),"items":.array(items),"next":next.map(CrewdValue.string) ?? .null])}
@MainActor final class GroupPeer {
    var calls:[(String,CrewdValue)]=[],pages:[CrewdValue]=[],receipt:CrewdValue = .null,error:CrewdClientError?
    var hold=false,held:CheckedContinuation<CrewdValue,Error>?
    func call(_ method:String,_ params:CrewdValue)async throws->CrewdValue {
        calls.append((method,params));if let error{throw error}
        if hold {hold=false;return try await withCheckedThrowingContinuation{held=$0}}
        if method == "channel.workList" {guard !pages.isEmpty else{throw CrewdClientError(message:"unexpected page read")};return pages.removeFirst()}
        return receipt
    }
}
@MainActor func rejects(_ f:()async throws->Void)async {do{try await f();preconditionFailure("malformed channel projection accepted")}catch{}}
@main struct ChannelWorkQA {
    @MainActor static func main()async throws {
        let peer=GroupPeer(),store=CrewdChannelWorkStore(call:{try await peer.call($0,$1)})
        store.selectChannel(crew:"a",channel:"group-a")
        await rejects{try await store.refresh()};await rejects{_ = try await store.changeLink(object:.object(["kind":.string("work"),"id":.string("W2")]),expectedRevision:"0",linked:true)}
        precondition(peer.calls.isEmpty,"cap absent must produce zero reads or writes")
        store.configureCapabilities(["channelWork"])
        let hiddenStart=peer.calls.count
        store.handleEvent("work.changed",.object(["crewId":.string("a")]))
        try await Task.sleep(nanoseconds:250_000_000)
        precondition(peer.calls.count == hiddenStart,"initial hidden group view must not issue an event refresh before any view activates")
        store.setActive(true)
        peer.pages=[page([row("requirement","#1")],next:"opaque-next"),page([row("work","W2"),row("humanTask","H3")])]
        try await store.refresh();precondition(store.items.count == 3 && store.linkRevision == "1" && store.sourceRevision == "9")
        let authority=store.items
        for bad in [page([row("work","W2")],source:"10"),page([row("work","W2")],links:"2"),page([row("work","W2")],channel:"group-b"),page([row("requirement","#1")]),page([row("work","W2")],next:"opaque-next")] {
            peer.pages=[page([row("requirement","#1")],next:"opaque-next"),bad]
            await rejects{try await store.refresh()};precondition(store.items == authority,"invalid/stale/duplicate/cyclic pages must preserve the complete prior snapshot")
        }
        var long=row("work","W2");if case .object(var fields)=long {fields["label"] = .string(String(repeating:"x",count:121));long = .object(fields)}
        peer.pages=[page([long])];await rejects{try await store.refresh()};precondition(store.items == authority)
        peer.pages=[page([row("work","W2")],links:"01")];await rejects{try await store.refresh()}
        peer.hold=true;let old=Task{try await store.refresh()};while peer.held == nil {await Task.yield()}
        store.selectChannel(crew:"a",channel:"group-b");peer.held!.resume(returning:page([row("work","W2")]));peer.held=nil
        do{try await old.value;preconditionFailure("old group response adopted")}catch is CancellationError{}
        precondition(store.items.isEmpty && !store.loaded && !store.loading)
        store.selectChannel(crew:"a",channel:"group-a");peer.pages=[page([row("work","W2")])];try await store.refresh()
        let object=CrewdValue.object(["kind":.string("work"),"id":.string("W2")])
        let facts=CrewdValue.object(["currentLinkRevision":.string("9007199254740993"),"currentSourceRevision":.string("9007199254740994")])
        peer.error=CrewdClientError(message:"real structured conflict",code:-32009,data:facts)
        do{_ = try await store.changeLink(object:object,expectedRevision:"1",linked:false);preconditionFailure()}catch let e as CrewdClientError {precondition(e.data == facts && store.failure?.data == facts)};peer.error=nil
        peer.receipt = .object(["channelId":.string("group-a"),"linkRevision":.string("2"),"object":object,"linked":.bool(false)]);peer.pages=[page([],links:"2")]
        _ = try await store.changeLink(object:object,expectedRevision:"1",linked:false)
        let sent=peer.calls.last{$0.0 == "channel.workUnlink"}!
        precondition(sent.1["channelId"] == .string("group-a") && sent.1["expectedLinkRevision"] == .string("1") && sent.1["object"] == object)
        for bad in [CrewdValue.object(["channelId":.string("group-b"),"linkRevision":.string("3"),"object":object,"linked":.bool(true)]),CrewdValue.object(["channelId":.string("group-a"),"linkRevision":.string("3"),"object":.object(["kind":.string("work"),"id":.string("W99")]),"linked":.bool(true)]),CrewdValue.object(["channelId":.string("group-a"),"linkRevision":.string("3"),"object":object,"linked":.bool(false)])] {
            peer.receipt=bad;await rejects{_ = try await store.changeLink(object:object,expectedRevision:"2",linked:true)}
            precondition(store.items.isEmpty && store.linkRevision == "2","wrong scope/object/direction receipt cannot become current channel facts")
        }
        peer.hold=true;let pending=Task{try await store.changeLink(object:object,expectedRevision:"2",linked:true)}
        while peer.held == nil {await Task.yield()}
        let beforeBusy=peer.calls.count
        await rejects{_ = try await store.changeLink(object:object,expectedRevision:"2",linked:true)}
        precondition(peer.calls.count == beforeBusy,"known local busy must reject before transport")
        store.selectChannel(crew:"a",channel:"group-b")
        peer.held!.resume(returning:.object(["channelId":.string("group-a"),"linkRevision":.string("3"),"object":object,"linked":.bool(true)]));peer.held=nil
        do{_ = try await pending.value;preconditionFailure("late link response changed new group")}catch is CancellationError{}
        precondition(store.items.isEmpty && !store.writing)
        var links=CrewdChannelLinkDrafts();let ka=CrewdChannelLinkDrafts.Key(crew:"a",channel:"group-a"),kb=CrewdChannelLinkDrafts.Key(crew:"a",channel:"group-b")
        let sa=links.begin(ka,object:object,expectedRevision:"2",linked:true)!;precondition(links.begin(ka,object:object,expectedRevision:"2",linked:true) == nil)
        let sb=links.begin(kb,object:.object(["kind":.string("requirement"),"id":.string("#1")]),expectedRevision:"7",linked:false)!
        links.finish(sa,accepted:false,uncertain:true);precondition(links.uncertain(ka) == [sa] && links.busy(kb) && links.uncertain(kb).isEmpty,"late old group link must keep old exact object/direction/revision while leaving new group untouched")
        links.acknowledge(kb);precondition(links.busy(kb));links.finish(sb,accepted:true);precondition(links.uncertain(ka) == [sa])
        var requirements=CrewdRequirementDrafts();requirements.set("a",channel:"group-a",text:"original request A");requirements.set("a",channel:"group-b",text:"request B")
        let ra=requirements.begin("a",channel:"group-a")!,rb=requirements.begin("a",channel:"group-b")!;precondition(ra.requestID != rb.requestID)
        requirements.finish(ra,accepted:false,uncertain:true);requirements.set("a",channel:"group-a",text:"new edited A")
        precondition(requirements.confirm(ra));requirements.finish(ra,accepted:true)
        precondition(requirements.text("a",channel:"group-a") == "new edited A" && requirements.busy("a",channel:"group-b"),"original UUID confirmation must not clear a later edit or another group")
        var plans=CrewdPlanDrafts();let pa=CrewdPlanDrafts.Key(crew:"a",object:"",source:"#1",channel:"group-a"),pb=CrewdPlanDrafts.Key(crew:"a",object:"",source:"#1",channel:"group-b")
        var fields=CrewdPlanDrafts.Fields();fields.title="original grouped plan";plans.edit(pa,fields);let original=plans.begin(pa)!
        plans.finish(original,accepted:false,uncertain:true);fields.title="edited later grouped plan";plans.edit(pa,fields);plans.edit(pb,fields);let newer=plans.begin(pb)!
        precondition(plans.confirm(original));let command=try CrewdPlanCommand.build(pa,original.fields,revision:nil,requestID:original.version)
        precondition(command.1["requestId"] == .string(original.version.uuidString.lowercased()) && command.1["channelId"] == .string("group-a") && command.1["title"] == .string("original grouped plan"))
        plans.finish(original,accepted:true);precondition(plans.fields(pa).title == "edited later grouped plan" && plans.busy(pb));plans.finish(newer,accepted:true)
        var transport:[(String,CrewdValue)]=[]
        let association=CrewdValue.object(["channelId":.string("group-a"),"linkRevision":.string("4"),"linked":.bool(false)])
        let workStore=CrewdWorkStore {method,params in
            transport.append((method,params))
            switch method {
            case "work.create":return .object(["id":.string("W12"),"crewId":.string("a"),"revision":.string("1"),"status":.string("ready"),"sources":.array([.string("#1")]),"channelAssociation":association])
            case "todo.create":return .object(["requirementId":.string("#11"),"crewId":.string("a"),"text":params["text"],"status":.string("unclaimed"),"closure":.null,"channelAssociation":association])
            case "work.list":return .object(["items":.array([]),"next":.null])
            case "todo.list","ask.list":return .array([])
            default:throw CrewdClientError(message:"unexpected canonical WorkStore read "+method)
            }
        }
        workStore.selectCrew("a")
        await rejects{_ = try await workStore.mutate(command.0,command.1)}
        precondition(transport.isEmpty,"group UUID create must reject before transport when capability is absent")
        workStore.configureCapabilities(["channelWork"])
        let made=try await workStore.mutate(command.0,command.1)
        precondition(made["channelAssociation"]["linked"] == .bool(false) && workStore.plans[CrewdWorkID("W12",kind:.plan)!]!.value["channelAssociation"] == .null,"unlinked confirmation remains a transient creation receipt, never canonical lifecycle/cache authority")
        let confirmed=try await workStore.createRequirement(requestID:ra.requestID,text:ra.text,channel:ra.channel)
        precondition(confirmed["channelAssociation"]["linked"] == .bool(false) && workStore.requirements[CrewdWorkID("#11",kind:.requirement)!]!["channelAssociation"] == .null)
        precondition(transport.filter{$0.0 == "channel.workLink"}.isEmpty,"confirming a removed initial association must not cause a link request")
        #if QA_ACTIVE_OWNER_CONTROLS
        store.setActive(false);store.selectChannel(crew:"a",channel:"group-a")
        let viewA=UUID(),viewB=UUID();store.setActive(true,owner:viewA);store.setActive(true,owner:viewB);store.setActive(false,owner:viewA)
        peer.pages=[page([row("work","W2")])];let oneRemaining=peer.calls.count
        store.handleEvent("work.changed",.object(["crewId":.string("a")]))
        store.handleEvent("work.changed",.object(["crewId":.string("a")]))
        try await Task.sleep(nanoseconds:250_000_000)
        precondition(peer.calls.count == oneRemaining+1 && store.items.count == 1,"closing one of two visible owners retains one coalesced read for the remaining view")
        store.setActive(false,owner:viewB);let allHidden=peer.calls.count
        store.handleEvent("events.lagged",.object(["dropped":.integer(2)]));try await Task.sleep(nanoseconds:250_000_000)
        precondition(peer.calls.count == allHidden,"all views hidden must issue zero event-driven reads")
        #endif
        let beforeReconnect=peer.calls.count;store.invalidateConnection();await rejects{try await store.refresh()};precondition(peer.calls.count == beforeReconnect)
        print("PASS: channel projection full-page authority/two revision bounds/capability/scope+late-link/no replay and exact per-channel uncertain request payloads; offline only")
    }
}
