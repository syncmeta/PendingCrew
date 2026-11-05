import Foundation

@main struct OrganizationSearchProof {
    typealias Wire=CrewdOrganizationSearchWire
    typealias Store=CrewdOrganizationSearchStore
    typealias V2=CrewdManagedTableV2Wire
    @MainActor static func main() async throws {
        let presentation=CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:nil,authorityID:"db",databaseID:"db",grantID:"grant",grantRevision:"1")
        var origin=Store.Origin(scope:presentation,observation:UUID(),connection:UUID(),crew:"crew")
        let authority=Wire.Scope(dbId:"db",restoreEpoch:UUID().uuidString.lowercased())
        let snapshot:CrewdValue = .object(["sortedAt":.string("2026-11-06T01:00:00Z"),"expiresAt":.string("2026-11-06T01:15:00Z")])
        let wrongSnapshot:CrewdValue = .object(["sortedAt":.string("2026-11-06T01:01:00Z"),"expiresAt":.string("2026-11-06T01:16:00Z")])
        func encode<T:Encodable>(_ value:T)throws->CrewdValue{try CrewdKnowledgeStore.encode(value)}
        func target(_ kind:String,_ id:String)->CrewdValue {.object(["kind":.string(kind),"id":.string(id)])}
        func hit(_ kind:String,_ id:String,status:String="current")->CrewdValue {
            .object(["target":target(kind,id),"kind":.string(kind),"ownerCrewId":.string("crew"),"tableId":kind=="row" ? .string("T3"):.null,"tableTitle":kind=="row" ? .string("真实表名"):.null,"title":.string("actual "+id),"excerpt":.string("bounded actual text"),"matchedOn":.string("text"),"status":.string(status),"changedAt":.string("2026-11-05T01:00:00Z")])
        }
        func page(_ items:[CrewdValue],next:String?=nil,building:Bool=false,cut:CrewdValue?=nil)throws->CrewdValue {
            .object(["scope":try encode(authority),"indexState":.string(building ? "building":"ready"),"snapshot":building ? .null:cut ?? snapshot,"items":.array(items),"nextCursor":next.map(CrewdValue.string) ?? .null])
        }
        var available=true,knowledge=false,mode="first",calls:[(String,CrewdValue)]=[],rebuild=0
        var deferred:[CheckedContinuation<CrewdValue,Error>]=[]
        // Forward declarations are immutable actual wire rows, not fabricated inverse actions.
        func edge(_ id:String,origin:CrewdValue,from:CrewdValue,to:CrewdValue,direction:String="outgoing",neighbor:CrewdValue?=nil)->CrewdValue {
            .object(["direction":.string(direction),"edge":.object(["id":.string(id),"ownerCrewId":.string("crew"),"source":from,"target":to,"summary":.string("real relation"),"origin":origin,"editable":.bool(false)]),"neighbor":.object(["target":neighbor ?? to,"summary":.string("actual neighbor"),"ownerCrewId":.string("crew"),"messageChannelId":.null]),"status":.string("current"),"changedAt":.null])
        }
        let source=target("decision","D1"),work=target("work","W2")
        let expansionRows=[edge("1",origin:.object(["origin":.string("manualRelates")]),from:source,to:work),edge("adapter-about",origin:.object(["origin":.string("adapter"),"adapter":.string("decision.about"),"readonlyEdgeId":.string("real-about")]),from:source,to:work)]
        let selfRow=edge("self",origin:.object(["origin":.string("adapter"),"adapter":.string("decision.about"),"readonlyEdgeId":.string("real-self")]),from:source,to:source)
        let badSelfRow=edge("bad-self",origin:.object(["origin":.string("adapter"),"adapter":.string("decision.about"),"readonlyEdgeId":.string("real-self")]),from:source,to:source,direction:"incoming")
        let privateTarget:CrewdValue = .object(["kind":.string("document"),"source":.null])
        let privateRow=edge("private",origin:.object(["origin":.string("adapter"),"adapter":.string("source.message"),"readonlyEdgeId":.string("private")]),from:source,to:privateTarget)
        let store=Store(origin:{origin},rpc:{method,params in
            calls.append((method,params))
            switch mode {
            case "decisiononly":return try page([hit("decision","D1")])
            case "messageonly":return try page([hit("message","m1")])
            case "rowonly":return try page([hit("row","O2")])
            case "first":return try page([hit("decision","D1"),hit("row","O2")],next:"cursor-a")
            case "empty":return try page([],next:"cursor-b")
            case "last":return try page([hit("decision","D4",status:"historical")])
            case "loop":return try page([],next:"cursor-a")
            case "snapshot":return try page([hit("decision","D5")],cut:wrongSnapshot)
            case "expired":throw CrewdClientError(message:"typed expired",code:-32009,data:.object(["refusal":.string("organizationSearch.cursor_expired")]))
            case "unavailable":throw CrewdClientError(message:"index unavailable",code:-32009,data:.object(["refusal":.string("organizationSearch.unavailable")]))
            case "forbidden":throw CrewdClientError(message:"revoked",code:-32003)
            case "building":rebuild+=1;return try page([],building:true)
            case "rebuilding":rebuild+=1;return try page(rebuild==3 ? [hit("work","W6")]:[],building:rebuild<3)
            case "deferred":return try await withCheckedThrowingContinuation{deferred.append($0)}
            case "expand":return try page(expansionRows,next:"edge-a")
            case "self":return try page([selfRow])
            case "badself":return try page([badSelfRow])
            case "badtarget":return try page([hit("decision","W7")])
            case "private":return try page([privateRow])
            default:preconditionFailure("unexpected mode")
            }
        },available:{available},knowledgeAvailable:{knowledge},pause:{})
        let query=Wire.Query(query:"é")
        available=false
        await store.start(query,current:{true});await store.more(current:{true});await store.expand(.init(kind:.decision,idValue:"D1"),includeSystemDefaults:false,current:{true})
        precondition(calls.isEmpty && store.items.isEmpty)
        available=true
        let invalids=[Wire.Query(query:"   "),Wire.Query(query:"query",domain:.messages,kinds:[.decision]),Wire.Query(query:"query",domain:.objects,channelId:"channel"),Wire.Query(query:"query",tableId:"T3",channelId:"channel")]
        for invalid in invalids {let before=calls.count;await store.start(invalid,current:{true});precondition(calls.count==before && store.items.isEmpty)}
        let mixed:[(Wire.Query,String,Wire.Kind)]=[(.init(query:"query",domain:.messages,kinds:[.message,.decision]),"messageonly",.message),(.init(query:"query",domain:.objects,kinds:[.message,.decision]),"decisiononly",.decision),(.init(query:"query",kinds:[.row,.decision],tableId:"T3"),"rowonly",.row),(.init(query:"query",kinds:[.message,.decision],channelId:"channel"),"messageonly",.message)]
        for (filtered,response,expected) in mixed {precondition(filtered.valid && filtered.effectiveKinds==[expected]);mode=response;await store.start(filtered,current:{true});precondition(store.phase == .ready && store.items.count==1 && store.items[0].kind==expected)}
        mode="first"
        await store.start(query,current:{true});precondition(store.items.map(\.target.idValue)==["D1","O2"] && store.nextCursor=="cursor-a" && store.snapshot?.sortedAt=="2026-11-06T01:00:00Z")
        let initialCalls=calls.count;store.handleEvent("message.posted",connection:origin.connection);precondition(calls.count==initialCalls && store.nextCursor=="cursor-a")
        let originalCut=store.snapshot
        mode="building";await store.more(current:{true});precondition(store.phase == .building && store.items.count==2 && store.snapshot==originalCut && store.nextCursor=="cursor-a" && calls.last!.1["cursor"].string=="cursor-a")
        mode="empty";await store.more(current:{true});precondition(store.items.count==2 && store.nextCursor=="cursor-b" && calls.last!.1["cursor"].string=="cursor-a")
        mode="last";await store.more(current:{true});precondition(store.items.map(\.target.idValue)==["D1","O2","D4"] && store.nextCursor==nil)
        precondition(calls.last!.1["query"].string!.utf8.elementsEqual(query.query.utf8))
        mode="first";await store.start(query,current:{true});mode="loop";await store.more(current:{true});precondition(store.phase == .failed && store.items.count==2 && store.nextCursor=="cursor-a")
        mode="snapshot";await store.more(current:{true});precondition(store.phase == .failed && store.items.count==2)
        mode="expired";await store.more(current:{true});precondition(store.phase == .expired && store.items.isEmpty && store.nextCursor==nil)
        mode="building";rebuild=0;let beforeBuilding=calls.count;await store.start(query,current:{true});precondition(rebuild==3 && calls.count==beforeBuilding+3 && store.phase == .building && !store.busy && store.items.isEmpty && store.snapshot==nil)
        mode="rebuilding";rebuild=0;await store.start(query,current:{true});precondition(rebuild==3 && store.phase == .ready && store.items.first?.target.idValue=="W6")
        mode="unavailable";await store.start(query,current:{true});precondition(store.phase == .unavailable && store.items.isEmpty)
        mode="badtarget";await store.start(query,current:{true});precondition(store.phase == .failed && store.items.isEmpty)
        // Even NFC-equivalent strings have distinct read identity; A→B→A gets three tickets.
        mode="deferred"
        let first=Task{@MainActor in await store.start(.init(query:"é"),current:{true})}
        await wait{deferred.count==1}
        let second=Task{@MainActor in await store.start(.init(query:"e\u{301}"),current:{true})}
        await wait{deferred.count==2}
        let third=Task{@MainActor in await store.start(.init(query:"é"),current:{true})}
        await wait{deferred.count==3}
        deferred[2].resume(returning:try page([hit("decision","D7")]))
        await third.value
        deferred[0].resume(returning:try page([hit("decision","D8")]))
        deferred[1].resume(returning:try page([hit("decision","D9")]))
        await first.value;await second.value
        precondition(store.items.count==1 && store.items[0].target.idValue=="D7")
        precondition(Wire.Query(query:"é").identity != Wire.Query(query:"e\u{301}").identity)
        deferred=[]
        let pending=Task{@MainActor in await store.start(query,current:{true})};await wait{deferred.count==1}
        origin = .init(scope:presentation,observation:UUID(),connection:UUID(),crew:"crew")
        deferred[0].resume(returning:try page([hit("decision","D10")]))
        await pending.value;precondition(!store.sourceCurrent && store.items.isEmpty)
        // Canonically equivalent grant IDs still denote distinct captured sources.
        let oldOrigin=origin
        let grantA=CrewdPresentationJournal.Scope(namespace:presentation.namespace,localDirectory:nil,authorityID:"db",databaseID:"db",grantID:"é",grantRevision:"1")
        let grantB=CrewdPresentationJournal.Scope(namespace:presentation.namespace,localDirectory:nil,authorityID:"db",databaseID:"db",grantID:"e\u{301}",grantRevision:"1")
        let sourceAuthority=UUID(),sourceConnection=UUID(),sourceObservation=UUID()
        let sessionWitness=CrewdSessionDisplayWitness(authority:sourceAuthority,connection:sourceConnection,observation:sourceObservation,scope:CrewdSessionDisplayWitness.scopeBytes(grantA))
        precondition(sessionWitness.matches(authority:sourceAuthority,connection:sourceConnection,observation:sourceObservation,scope:CrewdSessionDisplayWitness.scopeBytes(grantA)))
        precondition(!sessionWitness.matches(authority:sourceAuthority,connection:sourceConnection,observation:sourceObservation,scope:CrewdSessionDisplayWitness.scopeBytes(grantB)))
        precondition(!sessionWitness.matches(authority:UUID(),connection:sourceConnection,observation:sourceObservation,scope:CrewdSessionDisplayWitness.scopeBytes(grantA)))
        precondition(!sessionWitness.matches(authority:sourceAuthority,connection:UUID(),observation:sourceObservation,scope:CrewdSessionDisplayWitness.scopeBytes(grantA)))
        precondition(!sessionWitness.matches(authority:sourceAuthority,connection:sourceConnection,observation:UUID(),scope:CrewdSessionDisplayWitness.scopeBytes(grantA)))
        precondition(!sessionWitness.matches(authority:sourceAuthority,connection:sourceConnection,observation:sourceObservation,scope:nil))
        origin = .init(scope:grantA,observation:oldOrigin.observation,connection:oldOrigin.connection,crew:"crew")
        deferred=[]
        let granted=Task{@MainActor in await store.start(query,current:{true})};await wait{deferred.count==1}
        origin = .init(scope:grantB,observation:oldOrigin.observation,connection:oldOrigin.connection,crew:"crew")
        deferred[0].resume(returning:try page([hit("decision","D11")]))
        await granted.value;precondition(!store.sourceCurrent && store.items.isEmpty)
        origin=oldOrigin;deferred=[]
        let capped=Task{@MainActor in await store.start(query,current:{true})};await wait{deferred.count==1}
        available=false;deferred[0].resume(returning:try page([hit("decision","D12")]))
        await capped.value;precondition(!store.sourceCurrent && store.items.isEmpty)
        available=true
        mode="first";await store.start(query,current:{true});precondition(store.sourceCurrent)
        mode="expand";await store.expand(.init(kind:.decision,idValue:"D1"),includeSystemDefaults:false,current:{true});precondition(store.neighbors.count==2 && store.expansionNext=="edge-a")
        let originalExpansion=store.expansionSnapshot
        mode="building";await store.expandMore(current:{true});precondition(store.expansionPhase == .building && store.neighbors.count==2 && store.expansionSnapshot==originalExpansion && store.expansionNext=="edge-a" && calls.last!.1["cursor"].string=="edge-a")
        mode="self";await store.expandMore(current:{true});precondition(store.neighbors.count==3 && store.expansionNext==nil && store.neighbors.filter{$0.neighbor.target.idValue=="W2"}.count==2)
        mode="badself";await store.expand(.init(kind:.decision,idValue:"D1"),includeSystemDefaults:false,current:{true});precondition(store.expansionPhase == .failed && store.neighbors.isEmpty)
        // A private target cannot turn into a fake successful empty bridge.
        let beforePrivate=calls.count
        let invalidPrivate=V2.Target(kind:.document)
        await store.expand(invalidPrivate,includeSystemDefaults:false,current:{true});precondition(calls.count==beforePrivate && store.expansionPhase == .unavailable)
        mode="first";await store.start(query,current:{true});store.handleEvent("channel.changed",connection:origin.connection)
        precondition(store.items.isEmpty && !store.sourceCurrent && store.phase == .sourceChanged)
        available=false;let beforeOld=calls.count;await store.start(query,current:{true});precondition(calls.count==beforeOld)
        precondition(calls.allSatisfy{[Wire.search,Wire.expand].contains($0.0)})
        print("PASS actual OrganizationSearch Store: cap0RPC; raw UTF8/NFC and A→B→A tickets; source/cap/grant invalidation; fixed cross-page snapshot/order; continuation Building keeps original cursor/expiry in search+expand; filter intersection; immutable session display witness; empty continuation and cursor loop; bounded Building3; explicit expiry; exact target kinds; two relations/one neighbor and outgoing self once; private target no bridge; pure reads only")
        print("RPC=\(calls.count)")
    }
    @MainActor static func wait(_ ready:()->Bool) async {for _ in 0..<1000 {if ready(){return};await Task.yield()};preconditionFailure("deferred fixture did not reach controlled boundary")}
}
