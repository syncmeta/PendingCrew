import Foundation

@main struct ManagedV2StoreProof {
    typealias Wire=CrewdManagedTableV2Wire
    typealias Store=CrewdManagedTableV2Store
    @MainActor static func main() async throws {
        if CommandLine.arguments.count==4,CommandLine.arguments[1]=="--prepare" {
            let root=URL(fileURLWithPath:CommandLine.arguments[2]),id=UUID(uuidString:CommandLine.arguments[3])!
            let journal=try CrewdPresentationJournal(root:root,scope:.init(namespace:id,localDirectory:root.path,authorityID:"db",databaseID:"db",grantID:nil,grantRevision:nil))
            do{_=try journal.prepareManagedV2(.object(["crewId":.string("a"),"target":.string("epoch:a:O2")]),crew:"a",manual:false);exit(0)}catch{exit(23)}
        }
        let root=FileManager.default.temporaryDirectory.appendingPathComponent("pcw-v2-store-"+UUID().uuidString)
        defer{try? FileManager.default.removeItem(at:root)}
        let ledgerScope=CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:root.path,authorityID:"db",databaseID:"db",grantID:nil,grantRevision:nil)
        var ledger=try CrewdPresentationJournal(root:root,scope:ledgerScope)
        let origin=Store.Origin(scope:ledgerScope,observation:UUID(),connection:UUID(),crew:"b")
        var authority=Wire.Scope(dbId:"db",restoreEpoch:UUID().uuidString.lowercased())
        let fields:[Wire.Field]=[.init(id:"p1",name:"名称",kind:.init(kind:"text"),nullable:false,hidden:false),.init(id:"p2",name:"实数",kind:.init(kind:"real"),nullable:true,hidden:false),.init(id:"p3",name:"关联",kind:.init(kind:"link",target:.init(kind:"work"),many:true),nullable:true,hidden:false)]
        let table=Wire.Table(id:"T1",crewId:"a",schemaRevision:"1",title:"真实类型表",purpose:nil,titleFieldId:"p1",rowWritePolicy:.authorCaptainHuman,author:.init(kind:"human"),fields:fields,nextFieldNumber:"4",frozen:false,v1Readable:false,v1Writable:false)
        func row(_ revision:String,_ real:Double)->Wire.Row {.init(id:"O2",crewId:"a",tableId:"T1",schemaRevision:"1",revision:revision,author:.init(kind:"human"),scalarCells:[.init(fieldId:"p1",value:.text("original")),.init(fieldId:"p2",value:.real(real))],linkCells:[.init(fieldId:"p3",visible:[],hiddenSet:.init(keepToken:String(repeating:"a",count:64)),incompleteRequiredLink:false)],frozen:false)}
        // Exercise the exact production draft builder, including passive degraded links.
        let hiddenRow=row("1",0.0)
        let scalarOnly=try Wire.draftPatches(table:table,row:hiddenRow,text:["p1":"edited"],nulls:[],links:[:],edited:["p1"])
        precondition(scalarOnly.count==1 && scalarOnly[0].fieldId=="p1" && scalarOnly[0].change.kind=="setScalar")
        let retained=try Wire.draftPatches(table:table,row:hiddenRow,text:[:],nulls:[],links:["p3":[]],edited:["p3"])
        precondition(retained[0].change.keepHiddenSetToken==hiddenRow.links("p3")?.hiddenSet?.keepToken)
        do{_=try Wire.draftPatches(table:table,row:hiddenRow,text:[:],nulls:[],links:["p3":[.init(kind:"reference",target:.init(kind:.work,idValue:"W3"))]],edited:["p3"]);fatalError("visible net-add against hidden set accepted")}catch{}
        let wideFields=(1...64).map{Wire.Field(id:"p"+String($0),name:"field"+String($0),kind:.init(kind:"text"),nullable:true,hidden:false)}
        let wide=Wire.Table(id:"T9",crewId:"a",schemaRevision:"1",title:"64 fields",purpose:nil,titleFieldId:"p1",rowWritePolicy:.legacyCrewWriters,author:.init(kind:"legacyUnknown"),fields:wideFields,nextFieldNumber:"65",frozen:false,v1Readable:false,v1Writable:false)
        precondition(wide.valid)
        let widePatches=try Wire.draftPatches(table:wide,row:nil,text:[:],nulls:Set(wideFields.map(\.id)),links:[:],edited:[])
        precondition(widePatches.count==64 && Wire.Operation(kind:"addRow",tableId:wide.id,expectedSchemaRevision:"1",patches:widePatches).valid)
        precondition(Wire.Cell.real(0.0) != Wire.Cell.real(-0.0))
        precondition(Wire.number(String(Int64.max))==UInt64(Int64.max))
        precondition(Wire.number("9223372036854775808")==nil && !Wire.positive("9223372036854775808") && Wire.fieldNumber("p9223372036854775808")==nil)
        let numeric=try JSONDecoder().decode(CrewdValue.self,from:Data("18446744073709551615".utf8))
        precondition(numeric == .unsigned(UInt64.max))
        var currentRow=row("1",0.0),live=true,tablesCap=true,linksCap=true
        var lost=false,scopeFailure=false,writeObserved=false,missing=false,retired=false,corrupt=false,dismissLookup=false
        var methods:[String]=[],submits=0,lookups=0,manuals=0
        var records:[String:CrewdValue]=[:]
        var refusal:CrewdClientError?
        var manualRevision="0"
        let at="2026-11-05T00:00:00Z"
        func value<T:Encodable>(_ input:T)throws->CrewdValue {try CrewdKnowledgeStore.encode(input)}
        func make()->Store {Store(origin:{origin},rpc:{method,p in
            methods.append(method)
            switch method {
            case "managedObject.scope":if scopeFailure && writeObserved {throw CrewdClientError(message:"scope revoked after committed reply",code:-32003,data:.object(["refusal":.string("resource.forbidden")]))};return try value(authority)
            case Wire.tableGet:return try value(table)
            case Wire.rowGet:return try value(currentRow)
            case Wire.tableList:return .object(["items":try value([table]),"nextAfter":.null])
            case Wire.rows:return .object(["table":try value(table),"items":try value([currentRow]),"nextAfter":.null])
            case Wire.links:
                let outgoing=p["query"]["direction"].string=="outgoing"
                return .object(["source":outgoing ? p["query"]["source"]:.null,"manualLinkRevision":outgoing ? .string("0"):.null,"outgoingHiddenSet":.null,"items":.array([]),"nextAfter":.string("opaque-cursor-next")])
            case Wire.submit:
                submits+=1
                if let refusal {throw refusal}
                let input=try CrewdKnowledgeStore.decode(p,Wire.Submit.self),o=input.operation
                let effect:Wire.Effect
                switch o.kind {
                case "createTable":effect = .init(kind:"tableCreated",tableId:"T1",schemaRevision:"1",fieldIds:["p1"],titleFieldId:"p1")
                case "patchRow":
                    precondition(o.expectedRowRevision==currentRow.revision)
                    guard case .real(let real)?=o.patches?.first?.change.value else{preconditionFailure("actual typed Real required")}
                    let previous=currentRow.revision,changed=currentRow.scalar("p2") != .real(real)
                    let next=changed ? String(UInt64(previous)!+1):previous
                    if changed {currentRow=row(next,real)}
                    effect = .init(kind:"rowPatched",tableId:"T1",rowId:"O2",schemaRevision:"1",changedFieldIds:changed ? ["p2"]:[],changed:changed,originalRowRevision:previous,resultRowRevision:next,addedEdgeIds:[],removedEdgeIds:[])
                default:preconditionFailure("unexpected test write")
                }
                let receipt=Wire.Receipt(scope:input.scope,crewId:input.crewId,requestId:input.requestId,recordedAt:at,operationKind:o.kind,intentDigest:try input.digest(),eventId:effect.changed == false ? nil:UUID().uuidString.lowercased(),effect:effect)
                let encoded=try value(receipt);records[input.requestId]=encoded;writeObserved=true
                if lost {throw CrewdClientError(message:"actual reply interrupted before observation")};return encoded
            case Wire.lookup,Wire.linkLookup:
                lookups+=1
                if dismissLookup {live=false}
                let old=try CrewdKnowledgeStore.decode(p["scope"],Wire.Scope.self)
                if retired {return .object(["status":.string("historical"),"receipt":.null])}
                if missing {return .object(["status":.string(old==authority ? "notFound":"historical"),"receipt":.null])}
                guard var original=records[p["requestId"].string!] else{preconditionFailure("must lookup exact original")}
                if corrupt,case .object(var r)=original,case .object(var e)=r["effect"] {e["resultRowRevision"] = .string("99");r["effect"] = .object(e);original = .object(r)}
                return .object(["status":.string(old==authority ? "known":"historical"),"receipt":original])
            case Wire.linkSubmit:
                manuals+=1
                let input=try CrewdKnowledgeStore.decode(p,Wire.ManualSubmit.self)
                let before=manualRevision;manualRevision=String(UInt64(before)!+1)
                let r=Wire.ManualReceipt(scope:input.scope,crewId:input.crewId,requestId:input.requestId,recordedAt:at,intentDigest:try input.digest(),source:input.operation.source,originalManualLinkRevision:before,resultManualLinkRevision:manualRevision,eventId:UUID().uuidString.lowercased(),effect:.init(kind:"linked",edgeId:"1",added:true))
                let encoded=try value(r);records[input.requestId]=encoded;writeObserved=true
                if lost {throw CrewdClientError(message:"manual reply interrupted")};return encoded
            default:preconditionFailure("unexpected or legacy fallback RPC: "+method)
            }
        },journal:{ledger},tablesAvailable:{tablesCap},linksAvailable:{linksCap})}
        var store=make(),scope=try await make().scope(stillCurrent:{live})
        // Newly authored purpose is required, while older nil-purpose tables remain readable.
        precondition(table.valid && table.purpose==nil)
        let beforeInvalid=methods.count
        for purpose in ["","   "] {
            let emptyCreate=Wire.Operation(kind:"createTable",title:"table",purpose:purpose,fields:[.init(name:"title",kind:.init(kind:"text"),nullable:false)],titleFieldIndex:0,rowWritePolicy:.authorCaptainHuman)
            let emptyChange=Wire.Operation(kind:"alterTable",tableId:"T1",expectedSchemaRevision:"1",change:.init(kind:"setPurpose",purpose:purpose))
            for invalid in [emptyCreate,emptyChange] {
                precondition(!invalid.valid)
                do{_=try await store.submit(invalid,crew:"a",scope:scope,current:{live});fatalError("empty purpose dispatched")}catch{}
            }
        }
        precondition(methods.count==beforeInvalid && submits==0)
        try require(store.pending(scope).isEmpty)
        let page=try await store.rows("T1",scope:scope,after:nil,current:{live});precondition(page.items[0].matches(table))
        let empty=try await store.links(scope,source:.init(kind:"work",id:"W3"),current:{live})
        precondition(empty.items.isEmpty && empty.nextAfter=="opaque-cursor-next")
        let incoming=try await store.links(scope,target:.init(kind:.work,idValue:"W3"),current:{live})
        precondition(incoming.items.isEmpty && incoming.nextAfter != nil)
        do{_=try await store.links(scope,source:.init(kind:"work",id:"W3"),after:"opaque-cursor-next",current:{live});fatalError("cursor self-loop accepted")}catch{}
        let operation=Wire.Operation(kind:"patchRow",rowId:"O2",expectedSchemaRevision:"1",expectedRowRevision:"1",patches:[.init(fieldId:"p2",change:.init(kind:"setScalar",value:.real(-0.0)))])
        lost=true
        do{_=try await store.submit(operation,crew:"a",scope:scope,current:{live});fatalError("lost reply acknowledged")}catch{}
        var pending=try store.pending(scope).only()
        guard case .real(let original)?=pending.witness.table?.operation.patches?.first?.change.value else{fatalError("journal lost typed real")}
        precondition(original.bitPattern==Double(-0.0).bitPattern && pending.witness.crewId=="a" && scope.origin.crew=="b")
        guard case .real(let savedZero)=currentRow.scalar("p2") else{fatalError("saved typed zero missing")}
        precondition(savedZero.bitPattern==Double(-0.0).bitPattern && currentRow.revision=="2")
        ledger=try CrewdPresentationJournal(root:root,scope:ledgerScope);store=make();pending=try store.pending(scope).only()
        currentRow=row("3",-0.0) // A subsequent legitimate state cannot replace the original immutable result.
        corrupt=true
        do{_=try await store.lookup(pending,scope:scope,current:{live});fatalError("malformed revision acknowledged")}catch{}
        corrupt=false;missing=true
        let absent=try await store.lookup(pending,scope:scope,current:{live});precondition(absent==nil)
        missing=false;retired=true
        let historyNil=try await store.lookup(pending,scope:scope,current:{live});precondition(historyNil==nil)
        try require(store.pending(scope).count==1)
        retired=false;dismissLookup=true
        do{_=try await store.lookup(pending,scope:scope,current:{live});fatalError("dismissed lookup acknowledged")}catch{}
        live=true;dismissLookup=false
        let beforeSubmits=submits
        let recovered=try await store.lookup(pending,scope:scope,current:{live})
        precondition(recovered?.receipt.effect.resultRowRevision=="2" && submits==beforeSubmits)
        try require(store.pending(scope).isEmpty)
        // Repeating the exact negative-zero value is a true no-op; changing its sign is not.
        lost=false;writeObserved=false
        let sameZero=Wire.Operation(kind:"patchRow",rowId:"O2",expectedSchemaRevision:"1",expectedRowRevision:"3",patches:[.init(fieldId:"p2",change:.init(kind:"setScalar",value:.real(-0.0)))])
        let unchanged=try await store.submit(sameZero,crew:"a",scope:scope,current:{live})
        precondition(unchanged.receipt.effect.changed==false && unchanged.receipt.effect.originalRowRevision=="3" && unchanged.receipt.effect.resultRowRevision=="3" && unchanged.receipt.eventId==nil)
        guard case .real(let unchangedZero)=currentRow.scalar("p2") else{fatalError("no-op lost typed zero")}
        precondition(unchangedZero.bitPattern==Double(-0.0).bitPattern && currentRow.revision=="3")
        // A later *read* refusal after the write reply is not a definitive original refusal.
        scopeFailure=true;writeObserved=false
        let update=Wire.Operation(kind:"patchRow",rowId:"O2",expectedSchemaRevision:"1",expectedRowRevision:"3",patches:[.init(fieldId:"p2",change:.init(kind:"setScalar",value:.real(0.0)))])
        do{_=try await store.submit(update,crew:"a",scope:scope,current:{live});fatalError("post-write scope refusal accepted")}catch{}
        scopeFailure=false;try require(store.pending(scope).count==1)
        guard case .real(let positiveZero)=currentRow.scalar("p2") else{fatalError("positive-zero mutation missing")}
        precondition(positiveZero.bitPattern==Double(0.0).bitPattern && currentRow.revision=="4")
        let scoped=try store.pending(scope).only();_=try await store.lookup(scoped,scope:scope,current:{live})
        try require(store.pending(scope).isEmpty)
        refusal = .init(message:"known precommit rejection",code:-32602,data:.object(["refusal":.string("resource.invalid")]))
        let create=Wire.Operation(kind:"createTable",title:"table",purpose:"明确用途",fields:[.init(name:"title",kind:.init(kind:"text"),nullable:false)],titleFieldIndex:0,rowWritePolicy:.authorCaptainHuman)
        do{_=try await store.submit(create,crew:"a",scope:scope,current:{live});fatalError("refusal accepted")}catch{}
        try require(store.pending(scope).isEmpty)
        for tag in ["managedTable.schema_changed","managed_object.row_changed","managedLink.changed"] {
            refusal = .init(message:"known typed precommit conflict",code:-32009,data:.object(["refusal":.string(tag)]))
            do{_=try await store.submit(create,crew:"a",scope:scope,current:{live});fatalError("typed conflict accepted")}catch{}
            try require(store.pending(scope).isEmpty)
        }
        refusal = .init(message:"generic SQL",code:-32009)
        do{_=try await store.submit(create,crew:"a",scope:scope,current:{live});fatalError("SQL accepted")}catch{}
        try require(store.pending(scope).count==1)
        // A new restore epoch does not replay the unresolved original or mistake absence for proof.
        authority = .init(dbId:"db",restoreEpoch:UUID().uuidString.lowercased());scope=try await store.scope(stillCurrent:{live})
        missing=true;let historical=try await store.lookup(store.pending(scope).only(),scope:scope,current:{live});precondition(historical==nil)
        refusal=nil;missing=false;lost=true;writeObserved=false
        let manual=Wire.ManualOperation(kind:"link",source:.init(kind:"artifact",reference:.init(namespace:.init(kind:"unassigned",crewId:"a"),key:"file:src/main.rs"),metadataCrewId:"b"),target:.init(kind:"reference",target:.init(kind:.work,idValue:"W3")),expectedManualLinkRevision:"0")
        do{_=try await store.submitLink(manual,crew:"b",scope:scope,current:{live});fatalError("manual lost ack")}catch{}
        let originalManual=try store.pending(scope).first{$0.witness.manual != nil}!
        ledger=try CrewdPresentationJournal(root:root,scope:ledgerScope);store=make()
        let manualBefore=manuals,manualResult=try await store.lookupLink(originalManual,scope:scope,current:{live})
        precondition(manualResult?.receipt.effect.edgeId=="1" && manuals==manualBefore)
        let hint=store.invalidation;store.handleEvent("links.changed",.object(["crewId":.string("a"),"hiddenCount":.integer(1)]),connection:origin.connection);precondition(store.invalidation==hint)
        store.handleEvent("links.changed",.object(["crewId":.string("a")]),connection:origin.connection);precondition(store.invalidation != hint)
        let count=methods.count;tablesCap=false;linksCap=false
        do{_=try await store.table("T1",scope:scope,current:{live});fatalError("old Core table request")}catch{}
        do{_=try await store.resolve(.init(kind:.human),scope:scope,current:{live});fatalError("old Core link request")}catch{}
        precondition(methods.count==count)
        precondition(!CrewdClient.eventTopics(capabilities:[],includePersonalDraft:false).contains("links"))
        precondition(CrewdClient.eventTopics(capabilities:[Wire.linksCapability],includePersonalDraft:false).contains("links"))
        precondition(CrewdClient.eventTopics(capabilities:[Wire.linksCapability],includePersonalDraft:true).contains("links"))
        // Core's signed-i64 grammar cannot emit a changed max-revision receipt.
        tablesCap=true;linksCap=true;lost=false;writeObserved=false;currentRow=row(String(Int64.max),0.0)
        let pendingBeforeOverflow=try store.pending(scope).count
        let overflow=Wire.Operation(kind:"patchRow",rowId:"O2",expectedSchemaRevision:"1",expectedRowRevision:String(Int64.max),patches:[.init(fieldId:"p2",change:.init(kind:"setScalar",value:.real(-0.0)))])
        do{_=try await store.submit(overflow,crew:"a",scope:scope,current:{live});fatalError("out-of-i64 direct receipt acknowledged")}catch{}
        let impossible=try store.pending(scope).first{$0.witness.table?.operation.expectedRowRevision==String(Int64.max)}!
        do{_=try await store.lookup(impossible,scope:scope,current:{live});fatalError("out-of-i64 lookup receipt acknowledged")}catch{}
        try require(store.pending(scope).count==pendingBeforeOverflow+1)
        let raceRoot=root.appendingPathComponent("race"),raceNamespace=UUID()
        _=try CrewdPresentationJournal(root:raceRoot,scope:.init(namespace:raceNamespace,localDirectory:raceRoot.path,authorityID:"db",databaseID:"db",grantID:nil,grantRevision:nil))
        let children=[Process(),Process()]
        for child in children {child.executableURL=URL(fileURLWithPath:CommandLine.arguments[0]);child.arguments=["--prepare",raceRoot.path,raceNamespace.uuidString];try child.run()}
        for child in children{child.waitUntilExit()};precondition(children.map(\.terminationStatus).sorted()==[0,23])
        print("PASS actual V2 Store transport/journal +0→-0→same -0 no-op→+0 bitwise Store sequence, original UUID restart/no resubmit, current retired historical nil retained, post-write scope refusal Unknown, strict effect CAS/i64 overflow pending, exact foreign source crew, manual artifact metadata scope, strict event hint, empty purpose Create/SetPurpose zero RPC and legacy nil readable, old capability zero new API, two process prepare winner")
        print("submit=\(submits) lookup=\(lookups) manual=\(manuals)")
    }
}
private extension Array {func only()throws->Element{precondition(count==1);return self[0]}}

private func require(_ value:@autoclosure ()throws->Bool)rethrows{let actual=try value();precondition(actual)}
