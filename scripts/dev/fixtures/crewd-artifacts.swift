import Foundation

@main struct ArtifactNativeProof {
    typealias Wire=CrewdArtifactWire
    typealias Store=CrewdArtifactStore
    @MainActor static func main() async throws {
        if CommandLine.arguments.count==4,CommandLine.arguments[1]=="--prepare" {
            let root=URL(fileURLWithPath:CommandLine.arguments[2]),namespace=UUID(uuidString:CommandLine.arguments[3])!
            let scope=CrewdPresentationJournal.Scope(namespace:namespace,localDirectory:root.path,authorityID:"db",databaseID:"db",grantID:nil,grantRevision:nil)
            let ledger=try CrewdPresentationJournal(root:root,scope:scope)
            do {_=try ledger.prepareArtifact(.object(["crewId":.string("crew"),"target":.string("exact-reference-and-subject")]),crew:"crew");exit(0)}
            catch {exit(23)}
        }
        let without=Set(["managedObjects.events.v1","data"])
        precondition(!CrewdClient.eventTopics(capabilities:without,includePersonalDraft:false).contains("artifact") &&
            !CrewdClient.eventTopics(capabilities:without,includePersonalDraft:true).contains("artifact"))
        let capable=without.union([Wire.capability])
        precondition(CrewdClient.eventTopics(capabilities:capable,includePersonalDraft:false).filter{$0=="artifact"}.count==1 &&
            CrewdClient.eventTopics(capabilities:capable,includePersonalDraft:true).filter{$0=="artifact"}.count==1)
        let root=FileManager.default.temporaryDirectory.appendingPathComponent("pcw-artifact-native-"+UUID().uuidString)
        defer {try? FileManager.default.removeItem(at:root)}
        let crew="crew",db="db",namespace=UUID()
        let journalScope=CrewdPresentationJournal.Scope(namespace:namespace,localDirectory:root.path,authorityID:db,databaseID:db,grantID:nil,grantRevision:nil)
        var ledger=try CrewdPresentationJournal(root:root,scope:journalScope)
        var active:Store.Origin? = .init(scope:journalScope,observation:UUID(),connection:UUID(),crew:crew)
        var authority=CrewdManagedDirectoryWire.Scope(dbId:db,restoreEpoch:UUID().uuidString.lowercased())
        var current=true,supported=true
        let input=Wire.Input(kind:.test,value:"e\u{301}",repository:"opaque-repo")
        let reference=Wire.Reference(namespace:.init(kind:"repository",id:"opaque-repo"),key:"test:e\u{301}")
        let subject=Wire.Subject(kind:.lesson,id:"L1")
        let manual=Wire.Edge(id:"1",reference:reference,crewId:crew,subject:subject,relation:.evidence,
            origin:.init(kind:"manual",actor:.init(kind:"human",sessionId:nil),eventSeq:nil),createdAt:"2026-11-05T00:00:00Z")
        let auto=Wire.Edge(id:"2",reference:reference,crewId:crew,subject:subject,relation:.evidence,
            origin:.init(kind:"lessonEvidence",actor:nil,eventSeq:nil),createdAt:"2026-11-05T00:00:00Z")
        let artifact=Wire.Artifact(reference:reference,createdAt:"2026-11-05T00:00:00Z",verification:"unverified")
        let change=Wire.Change(kind:"link",input:input,subject:subject,relation:.evidence)
        var reply:CrewdValue = .null
        var lose=true,lookupMode="known",corrupt=false,dismiss=false,badPage=false,loopPage=false
        var submitted=0,lookedUp=0,methods:[String]=[]
        var failure:CrewdClientError?=nil
        func value<T:Encodable>(_ input:T)throws->CrewdValue {try CrewdKnowledgeStore.encode(input)}
        func make()->Store {
            Store(origin:{active},rpc:{method,p in
                methods.append(method)
                switch method {
                case "managedObject.scope":return try value(authority)
                case Wire.preview:return try value(reference)
                case Wire.submit:
                    submitted+=1
                    if let failure {throw failure}
                    let op=p["change"]["kind"].string!
                    let effect:Wire.Effect
                    let requestedReference=(p["change"]["reference"] == .null) ? reference:try CrewdKnowledgeStore.decode(p["change"]["reference"],Wire.Reference.self)
                    let requestedEdge=Wire.Edge(id:"1",reference:requestedReference,crewId:crew,subject:subject,relation:.evidence,origin:manual.origin,createdAt:manual.createdAt)
                    if op=="link" || op=="linkExisting" {effect = .init(kind:"linked",created:op=="link",claimAdded:nil,added:true,edge:requestedEdge,edgeId:nil)}
                    else if op=="register" {effect = .init(kind:"registered",created:false,claimAdded:false,added:nil,edge:nil,edgeId:nil)}
                    else {effect = .init(kind:"unlinked",created:nil,claimAdded:nil,added:nil,edge:nil,edgeId:"1")}
                    let r=Wire.Receipt(scope:try CrewdKnowledgeStore.decode(p["scope"],CrewdManagedDirectoryWire.Scope.self),crewId:crew,
                        requestId:p["requestId"].string!,recordedAt:artifact.createdAt,
                        change:try CrewdKnowledgeStore.decode(p["change"],Wire.Change.self),artifact:.init(reference:requestedReference,createdAt:artifact.createdAt,verification:"unverified"),effect:effect)
                    reply=try value(r)
                    if lose {throw CrewdClientError(message:"original reply lost")}
                    return reply
                case Wire.lookup:
                    lookedUp+=1;precondition(p["requestId"]==reply["requestId"])
                    if dismiss {current=false}
                    if lookupMode=="notFound" {return .object(["status":.string("notFound")])}
                    var returned=reply
                    if corrupt {
                        guard case .object(var r)=returned,case .object(var c)=r["change"],case .object(var i)=c["input"] else {fatalError()}
                        i["value"] = .string("é");c["input"] = .object(i);r["change"] = .object(c);returned = .object(r)
                    }
                    return .object(["status":.string(lookupMode),"receipt":returned])
                case Wire.get:
                    let ref=try CrewdKnowledgeStore.decode(p["reference"],Wire.Reference.self)
                    return try value(Wire.Artifact(reference:ref,createdAt:artifact.createdAt,verification:"unverified"))
                case Wire.list:
                    let rows=Array(repeating:artifact,count:badPage ? 17:1)
                    return .object(["items":try value(rows),"nextAfter":loopPage ? p["after"]:.null])
                case Wire.subject,Wire.backlinks:return .object(["items":try value([manual,auto]),"nextAfter":.null])
                default:preconditionFailure("unexpected transport/write fallback: "+method)
                }
            },journal:{ledger},available:{supported})
        }
        var store=make(),scope=try await make().scope(stillCurrent:{current})
        do {_=try await store.submit(change,scope:scope,stillCurrent:{current});fatalError("lost reply confirmed")}catch{}
        precondition(submitted==1)
        let original=try store.pending(scope).only()
        let uuid=original.witness.requestId
        precondition(original.witness.reference==reference)
        ledger=try CrewdPresentationJournal(root:root,scope:journalScope);store=make()
        do {_=try await store.submit(change,scope:scope,stillCurrent:{current});fatalError("second window resent unresolved action")}catch{}
        precondition(submitted==1)
        _=try await store.get(reference,scope:scope,stillCurrent:{current})
        try require(ledger.unknownRecords().count==1) // current existence is no receipt.
        let pending=try store.pending(scope).only()
        corrupt=true
        do {_=try await store.lookup(pending,scope:scope,stillCurrent:{current});fatalError("NFC/NFD receipt drift accepted")}catch{}
        try require(ledger.unknownRecords().count==1)
        corrupt=false;dismiss=true
        do {_=try await store.lookup(pending,scope:scope,stillCurrent:{current});fatalError("dismissed lookup acknowledged")}catch{}
        try require(ledger.unknownRecords().count==1)
        dismiss=false;current=true;lookupMode="notFound"
        let missing=try await store.lookup(pending,scope:scope,stillCurrent:{current});precondition(missing==nil)
        try require(ledger.unknownRecords().count==1)
        lookupMode="known"
        let recovered=try await store.lookup(pending,scope:scope,stillCurrent:{current})
        precondition(recovered?.receipt.requestId==uuid && recovered?.receipt.effect.edge?.id=="1")
        try require(ledger.unknownRecords().isEmpty)
        let edges=try await store.edges(scope,subject:subject,after:nil,stillCurrent:{current})
        precondition(edges.items.count==2 && edges.items[0].origin.manual && !edges.items[1].origin.manual)
        badPage=true
        do {_=try await store.list(scope,after:nil,stillCurrent:{current});fatalError("oversize page accepted")}catch{}
        badPage=false;loopPage=true
        do {_=try await store.list(scope,after:reference,stillCurrent:{current});fatalError("cursor loop accepted")}catch{}
        loopPage=false
        let row=try await store.list(scope,after:nil,stillCurrent:{current});precondition(row.items.count==1)
        // Same input/ref registration reuses its pointer; no phantom new claim is inferred.
        lose=false
        let reused=try await store.submit(.init(kind:"register",input:input),scope:scope,stillCurrent:{current})
        precondition(reused.receipt.effect.created==false && reused.receipt.effect.claimAdded==false)
        let foreign=Wire.Reference(namespace:.init(kind:"unassigned",crewId:"foreign-crew"),key:"file:src/retained.swift")
        let previewCount=methods.filter{$0==Wire.preview}.count
        let linkedExisting=try await store.submit(.init(kind:"linkExisting",subject:subject,relation:.evidence,reference:foreign),scope:scope,stillCurrent:{current})
        precondition(linkedExisting.receipt.artifact.reference==foreign && linkedExisting.receipt.effect.created==false)
        precondition(methods.filter{$0==Wire.preview}.count==previewCount) // never recreate a foreign unassigned pointer.
        // Exact expected manual edge is the only unlink target carried in the receipt.
        let removed=try await store.submit(.init(kind:"unlink",subject:subject,relation:.evidence,reference:reference,expectedEdgeId:"1"),scope:scope,stillCurrent:{current})
        precondition(removed.receipt.effect.edgeId=="1")
        failure=CrewdClientError(message:"stale manual edge",code:-32009,data:.object(["refusal":.string("artifact.edge_changed")]))
        do {_=try await store.submit(.init(kind:"unlink",subject:subject,relation:.evidence,reference:reference,expectedEdgeId:"1"),scope:scope,stillCurrent:{current});fatalError("stale edge accepted")}catch{}
        try require(ledger.unknownRecords().isEmpty)
        failure=nil
        // Restore changes scope; original immutable result can only be historical.
        lose=true
        do {_=try await store.submit(change,scope:scope,stillCurrent:{current});fatalError("lost link confirmed")}catch{}
        let historicalPending=try store.pending(scope).only()
        authority = .init(dbId:db,restoreEpoch:UUID().uuidString.lowercased());scope=try await store.scope(stillCurrent:{current})
        lookupMode="known"
        do {_=try await store.lookup(historicalPending,scope:scope,stillCurrent:{current});fatalError("old scope accepted as current")}catch{}
        lookupMode="historical"
        let historic=try await store.lookup(historicalPending,scope:scope,stillCurrent:{current});precondition(historic?.historical==true)
        let before=methods.count;supported=false
        do {_=try await store.list(scope,after:nil,stillCurrent:{current});fatalError("old Core requested artifacts")}catch{}
        precondition(methods.count==before)
        supported=true;active = .init(scope:journalScope,observation:UUID(),connection:UUID(),crew:crew)
        do {_=try await store.get(reference,scope:scope,stillCurrent:{current});fatalError("stale connection read accepted")}catch{}
        precondition(reference != Wire.Reference(namespace:reference.namespace,key:"test:é"))
        precondition(!Wire.Edge(id:"3",reference:reference,crewId:crew,subject:subject,relation:.appliesTo,origin:auto.origin,createdAt:auto.createdAt).valid)
        // A generic conflict may be a postcommit SQL failure; retain its UUID.
        active = .init(scope:journalScope,observation:UUID(),connection:UUID(),crew:crew)
        scope=try await store.scope(stillCurrent:{current})
        failure=CrewdClientError(message:"generic SQL ambiguity",code:-32009)
        do {_=try await store.submit(.init(kind:"unlink",subject:subject,relation:.evidence,reference:reference,expectedEdgeId:"1"),scope:scope,stillCurrent:{current});fatalError("generic failure accepted")}catch{}
        try require(ledger.unknownRecords().count==1)
        let raceRoot=root.appendingPathComponent("race"),raceNamespace=UUID()
        _=try CrewdPresentationJournal(root:raceRoot,scope:.init(namespace:raceNamespace,localDirectory:raceRoot.path,authorityID:"db",databaseID:"db",grantID:nil,grantRevision:nil))
        let children=[Process(),Process()]
        for child in children {child.executableURL=URL(fileURLWithPath:CommandLine.arguments[0]);child.arguments=["--prepare",raceRoot.path,raceNamespace.uuidString];try child.run()}
        for child in children {child.waitUntilExit()}
        precondition(children.map(\.terminationStatus).sorted()==[0,23])
        precondition(!methods.contains("managedObject.submit") && !methods.contains("managedObject.request.get"))
        print("PASS artifact original UUID after journal reopen; exact canonical witness/UTF8; no resend; current get never confirms; dismissed/missing retains; immutable historical lookup; manual-only UI origin; bounded/cursor reads; old capability; changed connection; actual two-process prepare winner")
        print("submits=\(submitted) artifactLookups=\(lookedUp)")
    }
}
private func require(_ value:@autoclosure ()throws->Bool)rethrows {let result=try value();precondition(result)}
private extension Array {func only()throws->Element {precondition(count==1);return self[0]}}
