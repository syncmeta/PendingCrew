import Foundation
import Combine

/// Current projections and durable original intent only; Core remains the data authority.
@MainActor final class CrewdManagedTableV2Store:ObservableObject {
    typealias Wire=CrewdManagedTableV2Wire
    typealias Origin=CrewdKnowledgeStore.Origin
    typealias Scope=CrewdManagedDirectoryStore.BrowseScope
    typealias RPC=CrewdKnowledgeStore.RPC
    typealias Problem=CrewdManagedDirectoryWire.Problem
    private let origin:()->Origin?
    private let rpc:RPC
    private let journal:()throws->CrewdPresentationJournal
    private let tablesAvailable:()->Bool
    private let linksAvailable:()->Bool
    @Published private(set) var invalidation=UUID()
    init(origin:@escaping ()->Origin?,rpc:@escaping RPC,journal:@escaping ()throws->CrewdPresentationJournal,
         tablesAvailable:@escaping ()->Bool,linksAvailable:@escaping ()->Bool) {
        self.origin=origin;self.rpc=rpc;self.journal=journal;self.tablesAvailable=tablesAvailable;self.linksAvailable=linksAvailable
    }
    var supportsTables:Bool {tablesAvailable()}
    var supportsLinks:Bool {linksAvailable()}
    func owns(_ scope:Scope)->Bool {origin()==scope.origin && (supportsTables || supportsLinks)}
    private func valid(_ o:Origin,_ current:@MainActor ()->Bool)->Bool {current() && !Task.isCancelled && origin()==o && (supportsTables || supportsLinks)}
    func handleEvent(_ name:String,_ raw:CrewdValue,connection:UUID) {
        guard let o=origin(),o.connection==connection else {return}
        if name==Wire.changed {
            guard supportsLinks,case .object(let fields)=raw,Set(fields.keys)==["crewId"],raw["crewId"].string.map({Wire.opaque($0)})==true else {return}
        } else if !["events.lagged","crew.changed","crew.purged","channel.changed","channel.purged","message.removed","work.changed","ask.changed","onto.changed","data.changed"].contains(name) {return}
        invalidation=UUID()
    }
    func scope(stillCurrent:@MainActor ()->Bool) async throws->Scope {
        guard let o=origin(),valid(o,stillCurrent) else {throw Problem.unavailable}
        let raw=try await rpc("managedObject.scope",.object([:]))
        guard valid(o,stillCurrent) else {throw Problem.stale}
        let a=try CrewdKnowledgeStore.decode(raw,Wire.Scope.self)
        guard a.dbId==o.scope.databaseID,Wire.uuid(a.restoreEpoch) else {throw Problem.invalid}
        return .init(origin:o,authority:a)
    }
    private func check(_ scope:Scope,_ current:@MainActor ()->Bool) async throws {
        guard try await self.scope(stillCurrent:current)==scope else {throw Problem.stale}
    }
    private func call<T:Decodable>(_ method:String,_ params:CrewdValue,_ scope:Scope,_ current:@MainActor ()->Bool) async throws->T {
        guard valid(scope.origin,current) else {throw Problem.stale}
        let raw=try await rpc(method,params)
        guard valid(scope.origin,current) else {throw Problem.stale}
        return try CrewdKnowledgeStore.decode(raw,T.self)
    }
    private func page(_ ids:[String],next:String?,after:String?,prefix:Character,limit:Int=16)->Bool {
        ids.count<=limit && ids.allSatisfy{CrewdManagedDirectoryWire.objectNumber($0,prefix:prefix) != nil} && Wire.unique(ids) &&
            (next==nil || (next==ids.last && next != after)) && !ids.contains(where:{$0==after})
    }
    func tables(_ scope:Scope,after:String?,current:@MainActor ()->Bool) async throws->Wire.TablePage {
        guard supportsTables else {throw Problem.unavailable}
        let result:Wire.TablePage=try await call(Wire.tableList,.object(["after":after.map(CrewdValue.string) ?? .null]),scope,current)
        guard result.items.allSatisfy(\.valid),page(result.items.map(\.id),next:result.nextAfter,after:after,prefix:"T") else {throw Problem.invalid}
        try await check(scope,current);return result
    }
    func table(_ id:String,scope:Scope,current:@MainActor ()->Bool) async throws->Wire.Table {
        guard supportsTables,CrewdManagedDirectoryWire.objectNumber(id,prefix:"T") != nil else {throw Problem.unavailable}
        let table:Wire.Table=try await call(Wire.tableGet,.object(["id":.string(id)]),scope,current)
        guard table.valid,table.id==id else {throw Problem.invalid};try await check(scope,current);return table
    }
    func rows(_ id:String,scope:Scope,after:String?,current:@MainActor ()->Bool) async throws->Wire.RowPage {
        guard supportsTables,CrewdManagedDirectoryWire.objectNumber(id,prefix:"T") != nil else {throw Problem.unavailable}
        let result:Wire.RowPage=try await call(Wire.rows,.object(["tableId":.string(id),"after":after.map(CrewdValue.string) ?? .null]),scope,current)
        guard result.table.id==id,result.table.valid,result.items.allSatisfy({$0.matches(result.table)}),page(result.items.map(\.id),next:result.nextAfter,after:after,prefix:"O") else {throw Problem.invalid}
        try await check(scope,current);return result
    }
    func row(_ id:String,scope:Scope,current:@MainActor ()->Bool) async throws->(Wire.Table,Wire.Row) {
        guard supportsTables,CrewdManagedDirectoryWire.objectNumber(id,prefix:"O") != nil else {throw Problem.unavailable}
        let row:Wire.Row=try await call(Wire.rowGet,.object(["id":.string(id)]),scope,current)
        let table=try await self.table(row.tableId,scope:scope,current:current)
        guard row.id==id,row.matches(table) else {throw Problem.stale};try await check(scope,current);return (table,row)
    }
    func resolve(_ target:Wire.Target,scope:Scope,current:@MainActor ()->Bool) async throws->Wire.ResolvedTarget {
        guard supportsLinks,target.valid else {throw Problem.unavailable}
        let result:Wire.ResolvedTarget=try await call(Wire.resolveTarget,.object(["target":try CrewdKnowledgeStore.encode(target)]),scope,current)
        guard Wire.exact(result.target,target),result.summary.unicodeScalars.count<=280,result.ownerCrewId.map({Wire.opaque($0)}) ?? true else {throw Problem.invalid}
        guard result.target.kind == .message ? result.messageChannelId.map({Wire.opaque($0)})==true:result.messageChannelId==nil else {throw Problem.invalid}
        try await check(scope,current);return result
    }
    func artifacts(_ scope:Scope,after:CrewdArtifactWire.Reference?,current:@MainActor ()->Bool) async throws->CrewdArtifactWire.Page {
        guard supportsLinks else {throw Problem.unavailable}
        let page:CrewdArtifactWire.Page=try await call(CrewdArtifactWire.list,.object(["after":try after.map(CrewdKnowledgeStore.encode) ?? .null]),scope,current)
        guard page.items.count<=16,page.items.allSatisfy(\.valid),Wire.unique(page.items.map(\.id)),
              page.nextAfter==nil || (page.nextAfter==page.items.last?.reference && page.nextAfter != after) else{throw Problem.invalid}
        try await check(scope,current);return page
    }
    func previewArtifact(_ input:CrewdArtifactWire.Input,crew:String,scope:Scope,current:@MainActor ()->Bool) async throws->CrewdArtifactWire.Reference {
        guard supportsLinks else {throw Problem.unavailable}
        let reference:CrewdArtifactWire.Reference=try await call(CrewdArtifactWire.preview,.object(["crewId":.string(crew),"input":try CrewdKnowledgeStore.encode(input)]),scope,current)
        guard reference.valid,reference.namespace.kind != "unassigned" || reference.namespace.crewId?.utf8.elementsEqual(crew.utf8)==true else {throw Problem.invalid}
        try await check(scope,current);return reference
    }
    func links(_ scope:Scope,source:Wire.Source?=nil,target:Wire.Target?=nil,after:String?=nil,includeAdapters:Bool=true,limit:Int=16,current:@MainActor ()->Bool) async throws->Wire.EdgePage {
        guard supportsLinks,(source != nil) != (target != nil),(1...64).contains(limit),source?.valid ?? true,target?.valid ?? true else {throw Problem.unavailable}
        var query:[String:CrewdValue]=["direction":.string(source==nil ? "incoming":"outgoing"),"includeAdapters":.bool(includeAdapters)]
        if let source {query["source"]=try CrewdKnowledgeStore.encode(source)}
        if let target {query["target"]=try CrewdKnowledgeStore.encode(target)}
        let result:Wire.EdgePage=try await call(Wire.links,.object(["query":.object(query),"after":after.map(CrewdValue.string) ?? .null,"limit":.integer(Int64(limit))]),scope,current)
        guard result.items.count<=limit,result.items.allSatisfy(\.valid),Wire.unique(result.items.map(\.id)),
              result.nextAfter==nil || (result.nextAfter.map{Wire.opaque($0,max:65_536)}==true && result.nextAfter != after) else {throw Problem.invalid}
        if let source {
            guard result.source.map({Wire.exact($0,source)})==true,result.manualLinkRevision.flatMap(Wire.number) != nil,
                  result.items.allSatisfy({Wire.exact($0.source,source)}),result.outgoingHiddenSet.map({Wire.opaque($0.marker)}) ?? true else {throw Problem.invalid}
        } else {
            guard result.source==nil,result.manualLinkRevision==nil,result.outgoingHiddenSet==nil,
                  result.items.allSatisfy({$0.target.id==target!.id}) else {throw Problem.invalid}
        }
        try await check(scope,current);return result
    }
    struct Witness:Codable {
        let managedScope:Wire.Scope
        let crewId:String
        let target:String
        let digest:String
        let table:Wire.Submit?
        let manual:Wire.ManualSubmit?
        var tableId:String?=nil
        var requestId:String {table?.requestId ?? manual?.requestId ?? ""}
        var method:String {manual==nil ? Wire.observation:Wire.linkObservation}
        var valid:Bool {
            guard (table != nil) != (manual != nil),Wire.uuid(requestId),Wire.opaque(crewId),!target.isEmpty else {return false}
            if let table {return Wire.exact(managedScope,table.scope) && table.crewId.utf8.elementsEqual(crewId.utf8) && (try? table.digest())==digest}
            return Wire.exact(managedScope,manual!.scope) && manual!.crewId.utf8.elementsEqual(crewId.utf8) && (try? manual!.digest())==digest
        }
    }
    struct Pending:Identifiable {let record:CrewdPresentationJournal.Prepared;let witness:Witness;var id:UUID{record.id}}
    struct Observation {let receipt:Wire.Receipt;let historical:Bool}
    struct ManualObservation {let receipt:Wire.ManualReceipt;let historical:Bool}
    func pending(_ scope:Scope)throws->[Pending] {
        guard owns(scope) else {throw Problem.stale}
        let ledger=try journal();guard ledger.scope==scope.origin.scope else {throw Problem.stale}
        return try ledger.unknownRecords().filter{[Wire.observation,Wire.linkObservation].contains($0.method) && $0.scope==ledger.scope}.map {record in
            let w=try CrewdKnowledgeStore.decode(record.params,Witness.self)
            guard w.valid,w.method==record.method,w.crewId.utf8.elementsEqual(record.crew.utf8),w.managedScope.dbId==scope.authority.dbId else {throw Problem.invalid}
            return .init(record:record,witness:w)
        }
    }
    struct DefinitiveRefusal:LocalizedError {
        let underlying:Error
        var errorDescription:String? {underlying.localizedDescription}
    }
    private func refusal(_ error:Error)->Bool {
        let e=error as? CrewdClientError
        return e?.kind == .notSubmitted || CrewdManagedDirectoryStore.decisionInputRefused(error) ||
            (e?.code == -32009 && ["resource.frozen","managedTable.schema_changed","managed_object.row_changed","managedLink.changed"].contains(e?.refusal ?? ""))
    }
    private func prepared(_ w:Witness,scope:Scope,current:@MainActor ()->Bool)throws->(CrewdPresentationJournal,CrewdPresentationJournal.Prepared) {
        guard w.valid,valid(scope.origin,current) else {throw Problem.stale}
        let ledger=try journal();guard ledger.scope==scope.origin.scope else {throw Problem.stale}
        return (ledger,try ledger.prepareManagedV2(CrewdKnowledgeStore.encode(w),crew:w.crewId,manual:w.manual != nil))
    }
    private func target(_ raw:String,crew:String,scope:Scope)->String {scope.authority.restoreEpoch+":"+String(crew.utf8.count)+":"+crew+":"+raw}
    func submit(_ operation:Wire.Operation,crew:String,scope:Scope,tableId:String?=nil,current:@MainActor ()->Bool) async throws->Observation {
        guard supportsTables,operation.valid else {throw Problem.unavailable};try await check(scope,current)
        let originalTable:String?
        if operation.kind=="patchRow" {
            let (actual,row)=try await self.row(operation.rowId!,scope:scope,current:current)
            guard actual.schemaRevision==operation.expectedSchemaRevision,row.revision==operation.expectedRowRevision,
                  actual.crewId.utf8.elementsEqual(crew.utf8),tableId==nil || tableId==actual.id else {throw CrewdClientError(message:"记录或定义已变化，请对照新版本后明确重新核对。",kind:.notSubmitted)}
            originalTable=actual.id
        } else {originalTable=operation.tableId}
        let p=Wire.Submit(scope:scope.authority,crewId:crew,requestId:UUID().uuidString.lowercased(),operation:operation)
        let w=Witness(managedScope:p.scope,crewId:crew,target:target(operation.target,crew:crew,scope:scope),digest:try p.digest(),table:p,manual:nil,tableId:originalTable)
        let (ledger,record)=try prepared(w,scope:scope,current:current)
        var writeReplyReceived=false
        do {
            let r:Wire.Receipt=try await call(Wire.submit,CrewdKnowledgeStore.encode(p),scope,current)
            writeReplyReceived=true
            try Self.verify(r,original:p,tableId:originalTable);try await check(scope,current)
            try ledger.finish(record,state:"replyObserved");return .init(receipt:r,historical:false)
        } catch {
            let known = !writeReplyReceived && refusal(error)
            try? ledger.finish(record,state:known ? "refused":"unknown")
            if known {throw DefinitiveRefusal(underlying:error)}
            throw error
        }
    }
    func submitLink(_ operation:Wire.ManualOperation,crew:String,scope:Scope,current:@MainActor ()->Bool) async throws->ManualObservation {
        guard supportsLinks,operation.valid else {throw Problem.unavailable};try await check(scope,current)
        let p=Wire.ManualSubmit(scope:scope.authority,crewId:crew,requestId:UUID().uuidString.lowercased(),operation:operation)
        let w=Witness(managedScope:p.scope,crewId:crew,target:target(operation.source.key,crew:crew,scope:scope),digest:try p.digest(),table:nil,manual:p)
        let (ledger,record)=try prepared(w,scope:scope,current:current)
        var writeReplyReceived=false
        do {
            let r:Wire.ManualReceipt=try await call(Wire.linkSubmit,CrewdKnowledgeStore.encode(p),scope,current)
            writeReplyReceived=true
            try Self.verify(r,original:p);try await check(scope,current)
            try ledger.finish(record,state:"replyObserved");return .init(receipt:r,historical:false)
        } catch {
            let known = !writeReplyReceived && refusal(error)
            try? ledger.finish(record,state:known ? "refused":"unknown")
            if known {throw DefinitiveRefusal(underlying:error)}
            throw error
        }
    }
    private func current(_ pending:Pending,scope:Scope)throws->Bool {try self.pending(scope).contains{$0.id==pending.id && Wire.exact($0.witness,pending.witness)}}
    private func lookupParams(_ w:Witness)throws->CrewdValue {.object(["scope":try CrewdKnowledgeStore.encode(w.managedScope),"crewId":.string(w.crewId),"requestId":.string(w.requestId)])}
    func lookup(_ original:Pending,scope:Scope,current:@MainActor ()->Bool) async throws->Observation? {
        guard supportsTables,let p=original.witness.table,try self.current(original,scope:scope) else {throw Problem.stale}
        let result:Wire.Lookup=try await call(Wire.lookup,lookupParams(original.witness),scope,current);try await check(scope,current)
        switch (result.status,result.receipt) {
        case ("known",.some(let r)),("historical",.some(let r)):
            guard (result.status=="known")==Wire.exact(p.scope,scope.authority) else {throw Problem.invalid}
            try Self.verify(r,original:p,tableId:original.witness.tableId);try await check(scope,current)
            try journal().acknowledge(original.record,detail:"已核对原表格请求；未重新提交。")
            return .init(receipt:r,historical:result.status=="historical")
        case ("notFound",nil) where Wire.exact(p.scope,scope.authority):return nil
        case ("historical",nil):return nil
        default:throw Problem.invalid
        }
    }
    func lookupLink(_ original:Pending,scope:Scope,current:@MainActor ()->Bool) async throws->ManualObservation? {
        guard supportsLinks,let p=original.witness.manual,try self.current(original,scope:scope) else {throw Problem.stale}
        let result:Wire.ManualLookup=try await call(Wire.linkLookup,lookupParams(original.witness),scope,current);try await check(scope,current)
        switch (result.status,result.receipt) {
        case ("known",.some(let r)),("historical",.some(let r)):
            guard (result.status=="known")==Wire.exact(p.scope,scope.authority) else {throw Problem.invalid}
            try Self.verify(r,original:p);try await check(scope,current)
            try journal().acknowledge(original.record,detail:"已核对原手工关联请求；未重新提交。")
            return .init(receipt:r,historical:result.status=="historical")
        case ("notFound",nil) where Wire.exact(p.scope,scope.authority):return nil
        case ("historical",nil):return nil
        default:throw Problem.invalid
        }
    }
}

extension CrewdManagedTableV2Store {
    private static func version(_ before:String,_ after:String,changed:Bool)->Bool {
        guard let b=Wire.number(before),let a=Wire.number(after) else {return false}
        if !changed {return a==b}
        let (next,overflow)=b.addingReportingOverflow(1);return !overflow && a==next
    }
    private static func event(_ id:String?,changed:Bool)->Bool {changed ? id.map(Wire.uuid)==true:id==nil}
    private static func ids(_ values:[String]?,fields:Bool=false)->Bool {
        guard let values,Wire.unique(values) else {return false}
        return values.allSatisfy{fields ? Wire.fieldNumber($0) != nil:Wire.positive($0)}
    }
    static func verify(_ r:Wire.Receipt,original p:Wire.Submit,tableId:String?=nil)throws {
        guard p.operation.valid,Wire.exact(r.scope,p.scope),r.crewId.utf8.elementsEqual(p.crewId.utf8),r.requestId==p.requestId,
              r.intentDigest == (try p.digest()),r.operationKind==p.operation.kind,!r.recordedAt.isEmpty else {throw Problem.invalid}
        let e=r.effect,o=p.operation
        guard CrewdManagedDirectoryWire.objectNumber(e.tableId,prefix:"T") != nil else {throw Problem.invalid}
        switch o.kind {
        case "createTable":
            let expected=(1...o.fields!.count).map{"p"+String($0)}
            guard e.kind=="tableCreated",e.schemaRevision=="1",e.fieldIds==expected,e.titleFieldId==expected[o.titleFieldIndex!],event(r.eventId,changed:true),
                  e.rowId==nil,e.originalSchemaRevision==nil,e.resultSchemaRevision==nil,e.changedFieldIds==nil,e.changed==nil,e.rowRevision==nil,e.edgeIds==nil,e.originalRowRevision==nil,e.resultRowRevision==nil,e.addedEdgeIds==nil,e.removedEdgeIds==nil else {throw Problem.invalid}
        case "alterTable":
            guard e.kind=="schemaChanged",e.tableId==o.tableId,e.originalSchemaRevision==o.expectedSchemaRevision,let changed=e.changed,let result=e.resultSchemaRevision,
                  version(o.expectedSchemaRevision!,result,changed:changed),event(r.eventId,changed:changed),ids(e.changedFieldIds,fields:true),
                  e.rowId==nil,e.schemaRevision==nil,e.fieldIds==nil,e.titleFieldId==nil,e.rowRevision==nil,e.edgeIds==nil,e.originalRowRevision==nil,e.resultRowRevision==nil,e.addedEdgeIds==nil,e.removedEdgeIds==nil else {throw Problem.invalid}
            let fields=e.changedFieldIds!,change=o.change!
            if !changed {guard fields.isEmpty else {throw Problem.invalid}}
            else if change.kind=="addField" {guard fields.count==1 else {throw Problem.invalid}}
            else if ["renameTable","setPurpose"].contains(change.kind) {guard fields.isEmpty else {throw Problem.invalid}}
            else {guard fields==[change.fieldId!] else {throw Problem.invalid}}
        case "addRow":
            let count=o.patches!.reduce(0){$0+($1.change.values?.count ?? 0)}
            guard e.kind=="rowAdded",e.tableId==o.tableId,e.rowId.map({CrewdManagedDirectoryWire.objectNumber($0,prefix:"O") != nil})==true,
                  e.schemaRevision==o.expectedSchemaRevision,e.rowRevision=="1",ids(e.edgeIds),e.edgeIds!.count==count,event(r.eventId,changed:true),
                  e.fieldIds==nil,e.titleFieldId==nil,e.originalSchemaRevision==nil,e.resultSchemaRevision==nil,e.changedFieldIds==nil,e.changed==nil,e.originalRowRevision==nil,e.resultRowRevision==nil,e.addedEdgeIds==nil,e.removedEdgeIds==nil else {throw Problem.invalid}
        case "patchRow":
            guard e.kind=="rowPatched",tableId != nil,e.tableId==tableId,e.rowId==o.rowId,e.schemaRevision==o.expectedSchemaRevision,e.originalRowRevision==o.expectedRowRevision,
                  let result=e.resultRowRevision,let changed=e.changed,version(o.expectedRowRevision!,result,changed:changed),event(r.eventId,changed:changed),ids(e.changedFieldIds,fields:true),ids(e.addedEdgeIds),ids(e.removedEdgeIds),
                  Set(e.addedEdgeIds!).isDisjoint(with:e.removedEdgeIds!),e.fieldIds==nil,e.titleFieldId==nil,e.originalSchemaRevision==nil,e.resultSchemaRevision==nil,e.rowRevision==nil,e.edgeIds==nil else {throw Problem.invalid}
            let submitted=Set(o.patches!.filter{$0.change.kind != "keep"}.map(\.fieldId))
            guard Set(e.changedFieldIds!).isSubset(of:submitted) else {throw Problem.invalid}
            if changed {guard !e.changedFieldIds!.isEmpty else {throw Problem.invalid}}
            else {guard e.changedFieldIds!.isEmpty,e.addedEdgeIds!.isEmpty,e.removedEdgeIds!.isEmpty else {throw Problem.invalid}}
        default:throw Problem.invalid
        }
    }
    static func verify(_ r:Wire.ManualReceipt,original p:Wire.ManualSubmit)throws {
        let o=p.operation,e=r.effect
        guard o.valid,Wire.exact(r.scope,p.scope),r.crewId.utf8.elementsEqual(p.crewId.utf8),r.requestId==p.requestId,r.intentDigest == (try p.digest()),!r.recordedAt.isEmpty,
              Wire.exact(r.source,o.source),r.originalManualLinkRevision==o.expectedManualLinkRevision,Wire.positive(e.edgeId) else {throw Problem.invalid}
        if o.kind=="link" {
            guard e.kind=="linked",let added=e.added,version(r.originalManualLinkRevision,r.resultManualLinkRevision,changed:added),event(r.eventId,changed:added) else {throw Problem.invalid}
        } else {
            guard e.kind=="unlinked",e.added==nil,e.edgeId==o.expectedEdgeId,version(r.originalManualLinkRevision,r.resultManualLinkRevision,changed:true),event(r.eventId,changed:true) else {throw Problem.invalid}
        }
    }
}
