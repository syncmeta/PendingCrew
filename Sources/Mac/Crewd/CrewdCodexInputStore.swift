import Foundation
import CryptoKit
import Combine

struct CrewdCodexInputChoice:Codable,Equatable {
    var scope:CrewdPresentationJournal.Scope?
    var ids=Set<UUID>()
    var skills:[CrewdValue]=[]
    @MainActor func value(store:CrewdCodexInputStore,context:CrewdCodexInputStore.Context)throws->CrewdValue?{
        if ids.isEmpty && skills.isEmpty{return nil}
        guard scope == (try store.choiceScope()) else{throw CrewdClientError(message:"原输入选择属于其它电脑或授权；请重新核对原选择，不会跨设备复用。",kind:.notSubmitted)}
        let refs=try store.selected(ids,context:context)
        var fields:[String:CrewdValue]=[:];if !refs.isEmpty{fields["inputs"] = .array(refs)};if !skills.isEmpty{fields["skills"] = .array(skills)}
        return try CrewdCodexInputWire.selection(.object(fields))
    }
}

/// Original staged inputs belong to this actor/grant, crew and optional session.
/// This owner never sends a task and never shares references with personal drafts.
@MainActor final class CrewdCodexInputStore:ObservableObject {
    struct Origin:Equatable{let scope:CrewdPresentationJournal.Scope;let generation:UUID;let connection:UUID;let crew:String}
    struct Context:Equatable{let crew:String;let session:String?}
    struct Entry:Identifiable{let record:CrewdPresentationJournal.Prepared;let reference:CrewdValue?;let state:String;var id:UUID{record.id}}
    typealias Call=(String,CrewdValue)async throws->CrewdValue
    private let origin:()->Origin?
    private let rpc:Call
    private let journal:()throws->CrewdPresentationJournal
    @Published private(set) var revision=UUID()
    @Published private(set) var busy=Set<String>()
    @Published var error:String?
    init(origin:@escaping ()->Origin?,rpc:@escaping Call,journal:@escaping ()throws->CrewdPresentationJournal){self.origin=origin;self.rpc=rpc;self.journal=journal}
    func refreshProjection(){revision=UUID()}
    var currentOrigin:Origin?{origin()}
    private func current(_ expected:Origin)throws{guard origin()==expected else{throw CrewdClientError(message:"原电脑授权或连接已变化；原输入保留，未在新连接继续。",kind:.notSubmitted)}}
    private func source(_ context:Context)throws->(Origin,CrewdPresentationJournal){guard let token=origin(),token.crew==context.crew,!context.crew.isEmpty else{throw CrewdPublicWire.invalid()};let owner=try journal();guard owner.scope==token.scope else{throw CrewdPublicWire.invalid()};return(token,owner)}
    func entries(_ context:Context)throws->[Entry]{
        let owner=try journal();let records=try owner.codexInputRecords().filter{$0.scope==owner.scope}
        let pending=Set(try owner.unknownRecords().map(\.id))
        let discarded=Set(records.filter{$0.method=="session.codex.inputDiscard"}.compactMap{$0.params["input"]["inputId"].string})
        let used=Set(records.filter{$0.method=="command.submit"}.flatMap{row->[String] in let p=row.params["command"]["params"];return (p["codexInputs"] == .null ? p["inputs"]:p["codexInputs"])["inputs"].array.compactMap{$0["inputId"].string}})
        return try records.filter{$0.method=="session.codex.inputPrepare" && $0.params["crewId"] == .string(context.crew) && $0.params["sessionId"].string==context.session}.map{row in
            let result=try owner.codexInputResult(row),ref=result?["input"],id=ref?["inputId"].string
            let state=pending.contains(row.id) ? "unknown":id.map{used.contains($0) ? "used":discarded.contains($0) ? "discarded":"prepared"} ?? "refused"
            return Entry(record:row,reference:ref,state:state)
        }.sorted{$0.record.preparedAt<$1.record.preparedAt}
    }
    func pending(_ context:Context)throws->[CrewdPresentationJournal.Prepared]{let owner=try journal();return try owner.unknownRecords().filter{$0.scope==owner.scope && ["session.codex.inputPrepare","session.codex.inputDiscard"].contains($0.method) && $0.params["crewId"] == .string(context.crew) && $0.params["sessionId"].string==context.session}}
    func selected(_ ids:Set<UUID>,context:Context)throws->[CrewdValue]{
        _ = try source(context)
        let rows=try entries(context).filter{ids.contains($0.id)}
        guard rows.count==ids.count,rows.allSatisfy({$0.state=="prepared"}) else{throw CrewdClientError(message:"所选输入已被任务引用、丢弃或仍未确认；未重用。",kind:.notSubmitted)}
        return rows.compactMap(\.reference)
    }
    struct LocalPreparation{let record:CrewdPresentationJournal.Prepared;let params:CrewdValue;let origin:Origin;let owner:CrewdPresentationJournal;let busyKey:String}
    func prepareLocal(data:Data,filename:String,mime:String,kind:String,context:Context)throws->LocalPreparation{
        let (token,owner)=try source(context)
        guard !data.isEmpty,data.count<=(kind=="text" ? 512*1024:4*1024*1024),filename.utf8.count<=128,!filename.isEmpty,!filename.contains("/"),!filename.contains("\\"),!filename.unicodeScalars.contains(where:CharacterSet.controlCharacters.contains),["image","audio","text"].contains(kind) else{throw CrewdPublicWire.invalid()}
        if kind=="text"{guard String(data:data,encoding:.utf8) != nil,!data.contains(0),mime=="text/plain" else{throw CrewdPublicWire.invalid()}}
        let hash=SHA256.hash(data:data).map{String(format:"%02x",$0)}.joined(),operation=context.crew+"/"+(context.session ?? "new")+"/"+hash
        guard !busy.contains(operation) else{throw CrewdPublicWire.invalid()}
        let pending=try self.pending(context);guard !pending.contains(where:{$0.method=="session.codex.inputPrepare" && $0.params["sha256"] == .string(hash)}) else{throw CrewdClientError(message:"这份输入的原上传结果未确认；请只读查原UUID，不会补传。",kind:.notSubmitted)}
        var fields:[String:CrewdValue]=["requestId":.string(UUID().uuidString.lowercased()),"crewId":.string(context.crew),"kind":.string(kind),"filename":.string(filename),"mimeType":.string(mime),"sha256":.string(hash)]
        if let session=context.session{fields["sessionId"] = .string(session)}
        var metadata=fields;metadata["bytes"] = .integer(Int64(data.count))
        fields["bytesBase64"] = .string(data.base64EncodedString())
        let record=try owner.prepare(method:"session.codex.inputPrepare",params:.object(metadata),crew:context.crew)
        do{try owner.recordCodexInputOriginal(data,record:record)}catch{try? owner.finish(record,state:"refused",detail:"Original file was not accepted locally; no upload dispatched");throw CrewdClientError(message:"文件未能本机保全，尚未加入或上传；请保留原文件后重新选择。",kind:.notSubmitted)}
        busy.insert(operation);revision=UUID()
        return .init(record:record,params:.object(fields),origin:token,owner:owner,busyKey:operation)
    }
    func sendPrepared(_ value:LocalPreparation)async throws{
        defer{busy.remove(value.busyKey);revision=UUID()}
        try current(value.origin)
        try await stage(value.record,params:value.params,token:value.origin,owner:value.owner)
    }
    func prepare(data:Data,filename:String,mime:String,kind:String,context:Context)async throws{let original=try prepareLocal(data:data,filename:filename,mime:mime,kind:kind,context:context);try await sendPrepared(original)}
    private func choiceKey(_ context:Context,lane:String)throws->CrewdValue{guard ["start","queue","steer"].contains(lane) else{throw CrewdPublicWire.invalid()};return .object(["crewId":.string(context.crew),"sessionId":context.session.map(CrewdValue.string) ?? .null,"lane":.string(lane)])}
    func restoreChoice(_ context:Context,lane:String)throws->(CrewdCodexInputChoice,UUID?){
        let owner=try journal();guard let row=try owner.codexChoice(choiceKey(context,lane:lane)) else{return(.init(scope:owner.scope),nil)}
        let choice=try JSONDecoder().decode(CrewdCodexInputChoice.self,from:JSONEncoder().encode(row.value));guard choice.scope==owner.scope else{throw CrewdPublicWire.invalid()};return(choice,row.revision)
    }
    func choiceScope()throws->CrewdPresentationJournal.Scope{try journal().scope}
    func saveChoice(_ choice:CrewdCodexInputChoice,context:Context,lane:String,expected:UUID?,scope:CrewdPresentationJournal.Scope)throws->UUID{
        guard choice.scope==scope,choice.ids.count+choice.skills.count<=16 else{throw CrewdPublicWire.invalid()}
        let owner=try journal();guard owner.scope==scope,origin().map({$0.scope==scope && $0.crew==context.crew}) ?? true else{throw CrewdClientError(message:"原输入选择的电脑或授权已变化；未覆盖新连接。",kind:.notSubmitted)}
        let value=try JSONDecoder().decode(CrewdValue.self,from:JSONEncoder().encode(choice))
        return try owner.saveCodexChoice(value,key:choiceKey(context,lane:lane),expected:expected)
    }
    func original(_ entry:Entry)throws->Data{try journal().codexInputOriginal(entry.record)}
    func discard(_ entry:Entry,context:Context)async throws{
        let (token,owner)=try source(context);guard entry.state=="prepared",let ref=entry.reference,try selected([entry.id],context:context).count==1 else{throw CrewdPublicWire.invalid()}
        let key=ref["inputId"].string ?? "";guard !busy.contains(key) else{throw CrewdPublicWire.invalid()};busy.insert(key);defer{busy.remove(key);revision=UUID()}
        let params=CrewdValue.object(["requestId":.string(UUID().uuidString.lowercased()),"crewId":.string(context.crew),"input":ref])
        // sessionId is local observation metadata only, never a discard RPC field.
        var metadata=params;if case .object(var v)=params,let session=context.session{v["sessionId"] = .string(session);metadata = .object(v)}
        let record=try owner.prepare(method:"session.codex.inputDiscard",params:metadata,crew:context.crew)
        try await stage(record,params:params,token:token,owner:owner)
    }
    private func stage(_ record:CrewdPresentationJournal.Prepared,params:CrewdValue,token:Origin,owner:CrewdPresentationJournal)async throws{
        do{
            let result=try await rpc(record.method,params)
            try CrewdCodexInputWire.staging(result,record:record)
            try owner.recordCodexInputResult(result,record:record)
            try owner.finish(record,state:"replyObserved",detail:"Validated original staging result; not task delivery")
            try current(token);error=nil
        }catch{
            try? owner.finish(record,state:CrewdClientError.isUncertainSubmission(error) ? "unknown":"refused",detail:"Original staging result requires lookup; original bytes retained only in private local journal; no client path retained")
            if origin()==token{self.error="原暂存操作未确认；原UUID保留，不会重传。"};throw error
        }
    }
    func lookup(_ record:CrewdPresentationJournal.Prepared)async throws{
        let context=Context(crew:record.crew,session:record.params["sessionId"].string),(token,owner)=try source(context)
        guard record.scope==token.scope,let id=record.params["requestId"].string,["session.codex.inputPrepare","session.codex.inputDiscard"].contains(record.method),!busy.contains(id) else{throw CrewdPublicWire.invalid()}
        busy.insert(id);defer{busy.remove(id);revision=UUID()}
        let value=try await rpc("session.codex.inputLookup",.object(["crewId":.string(context.crew),"requestId":.string(id)]));try current(token)
        switch value["state"].string{
        case "known":try CrewdCodexInputWire.exact(value,["state","result"]);try CrewdCodexInputWire.staging(value["result"],record:record);try owner.recordCodexInputResult(value["result"],record:record);try owner.acknowledge(record,detail:"Original staging lookup known; not input delivery or reuse authority");error=nil
        case "notFound":try CrewdCodexInputWire.exact(value,["state"]);error="尚无原暂存回执；不表示可以重新上传。"
        default:throw CrewdPublicWire.invalid()
        }
    }
}
