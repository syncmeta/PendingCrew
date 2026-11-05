import Foundation

@main struct ArtifactHistoryNativeProof {
    typealias Store=CrewdArtifactStore
    typealias Wire=CrewdArtifactHistoryWire
    typealias Artifact=CrewdArtifactWire
    @MainActor static func main() async throws {
        if CommandLine.arguments.count==4,CommandLine.arguments[1]=="--prepare" {
            let root=URL(fileURLWithPath:CommandLine.arguments[2]),namespace=UUID(uuidString:CommandLine.arguments[3])!
            let scope=CrewdPresentationJournal.Scope(namespace:namespace,localDirectory:root.path,authorityID:"db",databaseID:"db",grantID:nil,grantRevision:nil)
            let journal=try CrewdPresentationJournal(root:root,scope:scope)
            do {_=try journal.prepareArtifactHistory(.object(["crewId":.string("archived-a"),"target":.string("db:epoch:archived-a")]),crew:"archived-a");exit(0)}catch {exit(23)}
        }
        let root=FileManager.default.temporaryDirectory.appendingPathComponent("pcw-history-native-"+UUID().uuidString)
        defer {try? FileManager.default.removeItem(at:root)}
        let namespace=UUID(),target="archived-a",presentation="live-b",db="db"
        let journalScope=CrewdPresentationJournal.Scope(namespace:namespace,localDirectory:root.path,authorityID:db,databaseID:db,grantID:nil,grantRevision:nil)
        var journal=try CrewdPresentationJournal(root:root,scope:journalScope)
        let origin=Store.Origin(scope:journalScope,observation:UUID(),connection:UUID(),crew:presentation)
        var authority=Wire.Scope(dbId:db,restoreEpoch:UUID().uuidString.lowercased())
        var current=true,historyCap=true,job:Wire.Job?
        let cut=Wire.Cut(legacyEventSeq:"14",legacyItem:"3",legacySnapshotSeq:"16",legacySnapshotDigest:String(repeating:"a",count:64),legacyRetainedEvents:"4",eventSeq:"20",lessonNumber:"8")
        let zero=Wire.Counts(scanned:"0",doneEvents:"0",legacySnapshots:"0",lessons:"0",missingTargets:"0",artifactsAdded:"0",edgesAdded:"0")
        let delta=Wire.Counts(scanned:"1",doneEvents:"1",legacySnapshots:"0",lessons:"0",missingTargets:"0",artifactsAdded:"1",edgesAdded:"1")
        let at="2026-11-05T00:00:00Z"
        var begins=0,steps=0,lookups=0,methods:[String]=[]
        var saved:[String:CrewdValue]=[:]
        var lose=false,corrupt=false,dismiss=false,lookupMissing=false
        var mutateReply:((CrewdValue)->CrewdValue)?
        var mutateStatus:((CrewdValue)->CrewdValue)?
        var stepCursor:Wire.Cursor?
        var stepDelta:Wire.Counts?
        var fail:CrewdClientError?
        let foreignLast=UUID().uuidString.lowercased()
        func value<T:Encodable>(_ x:T)throws->CrewdValue {try CrewdKnowledgeStore.encode(x)}
        func copy(_ old:Wire.Job,revision:String?=nil,cursor:Wire.Cursor?=nil,counts:Wire.Counts?=nil,last:String?=nil)->Wire.Job {
            .init(id:old.id,scope:old.scope,crewId:old.crewId,cut:old.cut,cursor:cursor ?? old.cursor,revision:revision ?? old.revision,counts:counts ?? old.counts,
                createdAt:old.createdAt,updatedAt:at,lastRequestId:last ?? old.lastRequestId)
        }
        func make()->Store {
            Store(origin:{origin},rpc:{method,p in
                methods.append(method)
                switch method {
                case "managedObject.scope":return try value(authority)
                case "crew.list":
                    precondition(p["all"] == .bool(true))
                    return .array([.object(["id":.string(target),"title":.string("Archived target"),"removedAt":.string(at)]),.object(["id":.string(presentation),"title":.string("Current crew")])])
                case Wire.status:
                    precondition(p["crewId"] == .string(target))
                    let raw:CrewdValue = .object(["scope":try value(authority),"crewId":.string(target),"job":try job.map(value) ?? .null,"canContinue":.bool(job.map{$0.scope==authority && $0.cursor.phase != .complete} ?? false)])
                    return mutateStatus?(raw) ?? raw
                case Wire.begin:
                    begins+=1
                    if let fail {throw fail}
                    let created=job==nil || job?.scope != authority
                    if created {job = .init(id:UUID().uuidString.lowercased(),scope:authority,crewId:target,cut:cut,cursor:.init(phase:.legacyEvents,after:"0"),revision:"1",counts:zero,createdAt:at,updatedAt:at,lastRequestId:p["requestId"].string)}
                    else {job=copy(job!,last:foreignLast)} // reused Begin's recent request belongs to someone else.
                    let receipt=Wire.Receipt(scope:authority,crewId:target,requestId:p["requestId"].string!,recordedAt:at,operation:.init(kind:"begin"),job:job!,effect:.init(kind:"begun",created:created,beforeRevision:nil,beforeCursor:nil,delta:nil))
                    let raw=try value(receipt);saved[receipt.requestId]=raw
                    if lose {throw CrewdClientError(message:"reply observed then interrupted")};return mutateReply?(raw) ?? raw
                case Wire.step:
                    steps+=1
                    if let fail {throw fail}
                    let before=job!,expected=try CrewdKnowledgeStore.decode(p["expectedCursor"],Wire.Cursor.self)
                    precondition(before.revision==p["expectedRevision"].string && before.cursor==expected)
                    let revision=String(UInt64(before.revision)!+1),delta=stepDelta ?? delta
                    job=copy(before,revision:revision,cursor:stepCursor ?? .init(phase:before.cursor.phase,after:revision),counts:before.counts.adding(delta)!,last:p["requestId"].string!)
                    let operation=Wire.Operation(kind:"step",jobId:before.id,expectedRevision:before.revision,expectedCursor:before.cursor,limit:p["limit"].int)
                    let receipt=Wire.Receipt(scope:authority,crewId:target,requestId:p["requestId"].string!,recordedAt:at,operation:operation,job:job!,effect:.init(kind:"advanced",created:nil,beforeRevision:before.revision,beforeCursor:before.cursor,delta:delta))
                    let raw=try value(receipt);saved[receipt.requestId]=raw
                    if lose {throw CrewdClientError(message:"reply observed then interrupted")};return mutateReply?(raw) ?? raw
                case Wire.lookup:
                    lookups+=1
                    if dismiss {current=false}
                    let old=try CrewdKnowledgeStore.decode(p["scope"],Wire.Scope.self)
                    let state=old==authority ? "known":"historical"
                    guard !lookupMissing,var raw=saved[p["requestId"].string!] else {return .object(["status":.string(old==authority ? "notFound":"historical")])}
                    if corrupt {
                        guard case .object(var r)=raw,case .object(var e)=r["effect"] else {fatalError()}
                        e["beforeRevision"] = .string("99");r["effect"] = .object(e);raw = .object(r)
                    }
                    return .object(["status":.string(state),"receipt":mutateReply?(raw) ?? raw])
                case Artifact.subject:
                    if !historyCap {precondition(p["includeLegacySources"] == .null);return .object(["items":.array([]),"nextAfter":.null])}
                    precondition(p["includeLegacySources"] == .bool(true))
                    let ref=Artifact.Reference(namespace:.init(kind:"unassigned",crewId:target),key:"commit:abcdef1")
                    let source=Artifact.Subject(kind:.work,id:"W1")
                    let a=Artifact.Edge(id:"1",reference:ref,crewId:target,subject:source,relation:.evidence,
                        origin:.init(kind:"legacyWorkDone",actor:nil,eventSeq:nil,crewId:target,localSeq:"2",legacyWorkId:"1"),createdAt:at)
                    let b=Artifact.Edge(id:"2",reference:ref,crewId:target,subject:source,relation:.evidence,
                        origin:.init(kind:"legacyWorkSnapshot",actor:nil,eventSeq:nil,crewId:target,legacyWorkId:"3",snapshotSeq:"16"),createdAt:at)
                    return .object(["items":try value([a,b]),"nextAfter":.null])
                default:preconditionFailure("unexpected write or fallback: "+method)
                }
            },journal:{journal},available:{true},historyAvailable:{historyCap})
        }
        var store=make(),scope=try await make().scope(stillCurrent:{current})
        // Every contradictory direct reply must stay unknown, and an equally malformed
        // original lookup must not clear it. Only the unmodified original receipt settles it.
        func rejectReceipt(_ name:String,_ mutation:@escaping (CrewdValue)->CrewdValue,step:Bool=false,reuse:Bool=false,beforePhase:Wire.Phase?=nil) async throws {
            job=nil
            if step || reuse {_=try await store.beginHistory(target,scope:scope,stillCurrent:{current})}
            if let beforePhase {job=copy(job!,cursor:.init(phase:beforePhase,after:"0"))}
            mutateReply=mutation
            do {
                if step {_=try await store.stepHistory(job!,limit:1,scope:scope,stillCurrent:{current})}
                else {_=try await store.beginHistory(target,scope:scope,stillCurrent:{current})}
                fatalError("malformed direct receipt accepted: "+name)
            } catch {}
            let pending=try store.historyPending(scope,crew:target).only()
            do {_=try await store.lookupHistory(pending,scope:scope,stillCurrent:{current});fatalError("malformed lookup accepted: "+name)}catch{}
            try require(journal.unknownRecords().count==1)
            mutateReply=nil
            let observed=try await store.lookupHistory(pending,scope:scope,stillCurrent:{current})
            precondition(observed != nil)
            try require(journal.unknownRecords().isEmpty)
        }
        for invalid in ["garbage","01","18446744073709551616","15"] {
            try await rejectReceipt("cursor "+invalid,{replace($0,["job","cursor","after"],.string(invalid))},step:true)
        }
        try await rejectReceipt("created cursor",{replace($0,["job","cursor","after"],.string("1"))})
        try await rejectReceipt("created phase",{replace($0,["job","cursor","phase"],.string("events"))})
        try await rejectReceipt("created last UUID",{replace($0,["job","lastRequestId"],.string(foreignLast))})
        try await rejectReceipt("reused complete",{replace(replace($0,["job","cursor","phase"],.string("complete")),["job","cursor","after"],.string("0"))},reuse:true)
        try await rejectReceipt("same phase no progress",{replace($0,["job","cursor","after"],.string("0"))},step:true)
        try await rejectReceipt("phase regression",{replace($0,["job","cursor","phase"],.string("legacyEvents"))},step:true,beforePhase:.events)
        try await rejectReceipt("complete cursor nonzero",{replace($0,["job","cursor","phase"],.string("complete"))},step:true)
        try await rejectReceipt("nil snapshot but nonzero bounds",{replace($0,["job","cut","legacySnapshotDigest"],.null)},step:true)
        try await rejectReceipt("retained beyond cut",{replace($0,["job","cut","legacyRetainedEvents"],.string("15"))},step:true)
        try await rejectReceipt("count sources exceed scanned",{replace($0,["job","counts","lessons"],.string("1"))},step:true)
        try await rejectReceipt("missing exceeds sources",{replace($0,["job","counts","missingTargets"],.string("2"))},step:true)
        try await rejectReceipt("delta source exceeds scanned",{replace($0,["effect","delta","lessons"],.string("1"))},step:true)
        job=nil
        mutateStatus={replace($0,["canContinue"],.bool(true))}
        do {_=try await store.historyStatus(target,scope:scope,stillCurrent:{current});fatalError("missing job canContinue")}catch{}
        mutateStatus=nil
        _=try await store.beginHistory(target,scope:scope,stillCurrent:{current})
        mutateStatus={replace($0,["canContinue"],.bool(false))}
        do {_=try await store.historyStatus(target,scope:scope,stillCurrent:{current});fatalError("current active job cannot continue")}catch{}
        mutateStatus={replace($0,["job","cursor","after"],.string("99"))}
        do {_=try await store.historyStatus(target,scope:scope,stillCurrent:{current});fatalError("status out of bound cursor")}catch{}
        mutateStatus=nil;job=nil
        // Empty phases may legitimately jump to Complete with zero scanned delta.
        let emptyCut=Wire.Cut(legacyEventSeq:"0",legacyItem:"0",legacySnapshotSeq:"0",legacySnapshotDigest:nil,legacyRetainedEvents:"0",eventSeq:"0",lessonNumber:"0")
        precondition(emptyCut.valid && Wire.Cursor(phase:.complete,after:"0").valid(for:emptyCut))
        precondition(!Wire.Cursor(phase:.complete,after:"1").valid(for:emptyCut))
        _=try await store.beginHistory(target,scope:scope,stillCurrent:{current})
        stepCursor = .init(phase:.complete,after:"0");stepDelta=zero
        let completed=try await store.stepHistory(job!,limit:1,scope:scope,stillCurrent:{current})
        precondition(completed.receipt.job.cursor.phase == .complete && completed.receipt.effect.delta!.zero)
        stepCursor=nil;stepDelta=nil;job=nil
        let initialSteps=steps
        let roster=try await store.historyCrews(scope,stillCurrent:{current});precondition(roster.count==2 && roster[0]["removedAt"].string != nil)
        let start=try await store.beginHistory(target,scope:scope,stillCurrent:{current})
        precondition(start.receipt.effect.created==true && start.receipt.job.counts.zero && steps==initialSteps)
        let reused=try await store.beginHistory(target,scope:scope,stillCurrent:{current})
        precondition(reused.receipt.effect.created==false && reused.receipt.job.lastRequestId==foreignLast && reused.receipt.requestId != foreignLast && steps==initialSteps)
        lose=true
        do {_=try await store.stepHistory(job!,limit:1,scope:scope,stillCurrent:{current});fatalError("lost Step acknowledged")}catch{}
        let original=try store.historyPending(scope,crew:target).only(),request=try store.historyPending(scope,crew:target).only().witness.requestId
        precondition(original.record.crew==target && scope.origin.crew==presentation)
        let advanced=try await store.historyStatus(target,scope:scope,stillCurrent:{current})
        precondition(advanced.job?.revision=="2")
        try require(journal.unknownRecords().count==1) // current status advancement proves no original UUID.
        let oldBegins=begins,oldSteps=steps
        do {_=try await store.beginHistory(target,scope:scope,stillCurrent:{current});fatalError("pending page did not fence Begin")}catch{}
        do {_=try await store.stepHistory(job!,limit:1,scope:scope,stillCurrent:{current});fatalError("pending page resent")}catch{}
        precondition(begins==oldBegins && steps==oldSteps)
        journal=try CrewdPresentationJournal(root:root,scope:journalScope);store=make()
        let pending=try store.historyPending(scope,crew:target).only();precondition(pending.witness.requestId==request)
        corrupt=true
        do {_=try await store.lookupHistory(pending,scope:scope,stillCurrent:{current});fatalError("wrong before revision accepted")}catch{}
        corrupt=false;dismiss=true
        do {_=try await store.lookupHistory(pending,scope:scope,stillCurrent:{current});fatalError("closed view acknowledged")}catch{}
        current=true;dismiss=false;lookupMissing=true
        let absent=try await store.lookupHistory(pending,scope:scope,stillCurrent:{current});precondition(absent==nil)
        try require(journal.unknownRecords().count==1)
        lookupMissing=false
        let found=try await store.lookupHistory(pending,scope:scope,stillCurrent:{current})
        precondition(found?.receipt.requestId==request && found?.receipt.job.revision=="2" && steps==oldSteps)
        try require(journal.unknownRecords().isEmpty)
        let sources=try await store.edges(scope,subject:.init(kind:.work,id:"W1"),after:nil,stillCurrent:{current})
        precondition(sources.items[0].origin.title.contains("真实完成事件") && sources.items[1].origin.title.contains("非原完成事件") && sources.items.allSatisfy{!$0.origin.manual})
        let beforeEvent=store.historyInvalidation
        store.handleEvent("artifact.historyChanged",.object(["crewId":.string(target),"jobId":.string(job!.id)]),connection:origin.connection)
        precondition(store.historyInvalidation != beforeEvent && steps==oldSteps && begins==oldBegins)
        // Another unresolved old-epoch Step must not block an explicit new current cut.
        do {_=try await store.stepHistory(job!,limit:1,scope:scope,stillCurrent:{current});fatalError("second interrupted Step acknowledged")}catch{}
        let old=try store.historyPending(scope,crew:target).only()
        authority = .init(dbId:db,restoreEpoch:UUID().uuidString.lowercased());scope=try await store.scope(stillCurrent:{current})
        let oldState=try await store.historyStatus(target,job:job!.id,scope:scope,stillCurrent:{current});precondition(!oldState.canContinue)
        do {_=try await store.stepHistory(oldState.job!,limit:1,scope:scope,stillCurrent:{current});fatalError("old epoch continued")}catch{}
        lookupMissing=true
        let historicalNone=try await store.lookupHistory(old,scope:scope,stillCurrent:{current});precondition(historicalNone==nil)
        lose=false
        let newCut=try await store.beginHistory(target,scope:scope,stillCurrent:{current})
        precondition(newCut.receipt.scope==authority && newCut.receipt.effect.created==true)
        lookupMissing=false
        let history=try await store.lookupHistory(old,scope:scope,stillCurrent:{current});precondition(history?.historical==true)
        try require(journal.unknownRecords().isEmpty)
        fail=CrewdClientError(message:"stale step",code:-32009,data:.object(["refusal":.string("artifact.history_changed")]))
        do {_=try await store.stepHistory(job!,limit:1,scope:scope,stillCurrent:{current});fatalError("CAS accepted")}catch{}
        try require(journal.unknownRecords().isEmpty)
        fail=CrewdClientError(message:"generic SQL failure",code:-32009)
        do {_=try await store.stepHistory(job!,limit:1,scope:scope,stillCurrent:{current});fatalError("SQL failure accepted")}catch{}
        try require(journal.unknownRecords().count==1)
        let beforeMethods=methods.count;historyCap=false
        do {_=try await store.historyStatus(target,scope:scope,stillCurrent:{current});fatalError("old Core history request")}catch{}
        precondition(methods.count==beforeMethods)
        let oldOrigins=try await store.edges(scope,subject:.init(kind:.work,id:"W1"),after:nil,stillCurrent:{current});precondition(oldOrigins.items.isEmpty)
        let raceRoot=root.appendingPathComponent("race"),raceNamespace=UUID()
        _=try CrewdPresentationJournal(root:raceRoot,scope:.init(namespace:raceNamespace,localDirectory:raceRoot.path,authorityID:"db",databaseID:"db",grantID:nil,grantRevision:nil))
        let children=[Process(),Process()]
        for child in children {child.executableURL=URL(fileURLWithPath:CommandLine.arguments[0]);child.arguments=["--prepare",raceRoot.path,raceNamespace.uuidString];try child.run()}
        for child in children {child.waitUntilExit()};precondition(children.map(\.terminationStatus).sorted()==[0,23])
        precondition(!methods.contains(Artifact.submit) && !methods.contains(Artifact.lookup))
        print("PASS malformed direct/lookup receipts retain original UUID; canonical bounded cursor/phase/counts/Begin/status coherence; history Begin reuse foreign lastRequest UUID; explicit Step only; cross-crew archived roster; status never confirms UUID; actual journal reopen; missing/closed/corrupt retains; exact counters/revision/cursor; epoch history and explicit new cut; typed CAS vs generic SQL; capability/legacy origin opt-in; event is read hint; two real process prepare winner")
        print("begins=\(begins) steps=\(steps) lookups=\(lookups)")
    }
}
private func require(_ value:@autoclosure ()throws->Bool)rethrows {let result=try value();precondition(result)}
private extension Array {func only()throws->Element {precondition(count==1);return self[0]}}

private func replace(_ raw:CrewdValue,_ path:[String],_ value:CrewdValue)->CrewdValue {
    guard let key=path.first,case .object(var object)=raw else {preconditionFailure("invalid fixture path")}
    object[key]=path.count==1 ? value:replace(object[key] ?? .null,Array(path.dropFirst()),value)
    return .object(object)
}
