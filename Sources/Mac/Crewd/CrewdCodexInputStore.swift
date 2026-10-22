import Foundation
import CryptoKit
import Combine

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
        let (_,owner)=try source(context);let records=try owner.codexInputRecords().filter{$0.scope==owner.scope}
        let pending=Set(try owner.unknownRecords().map(\.id))
        let discarded=Set(records.filter{$0.method=="session.codex.inputDiscard"}.compactMap{$0.params["input"]["inputId"].string})
        let used=Set(records.filter{$0.method=="command.submit"}.flatMap{row->[String] in let p=row.params["command"]["params"];return (p["codexInputs"] == .null ? p["inputs"]:p["codexInputs"])["inputs"].array.compactMap{$0["inputId"].string}})
        return try records.filter{$0.method=="session.codex.inputPrepare" && $0.params["crewId"] == .string(context.crew) && $0.params["sessionId"].string==context.session}.map{row in
            let result=try owner.codexInputResult(row),ref=result?["input"],id=ref?["inputId"].string
            let state=pending.contains(row.id) ? "unknown":id.map{used.contains($0) ? "used":discarded.contains($0) ? "discarded":"prepared"} ?? "refused"
            return Entry(record:row,reference:ref,state:state)
        }.sorted{$0.record.preparedAt<$1.record.preparedAt}
    }
    func pending(_ context:Context)throws->[CrewdPresentationJournal.Prepared]{let (_,owner)=try source(context);return try owner.unknownRecords().filter{$0.scope==owner.scope && ["session.codex.inputPrepare","session.codex.inputDiscard"].contains($0.method) && $0.params["crewId"] == .string(context.crew) && $0.params["sessionId"].string==context.session}}
    func selected(_ ids:Set<UUID>,context:Context)throws->[CrewdValue]{
        let rows=try entries(context).filter{ids.contains($0.id)}
        guard rows.count==ids.count,rows.allSatisfy({$0.state=="prepared"}) else{throw CrewdClientError(message:"所选输入已被任务引用、丢弃或仍未确认；未重用。",kind:.notSubmitted)}
        return rows.compactMap(\.reference)
    }
    func prepare(data:Data,filename:String,mime:String,kind:String,context:Context)async throws{
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
        busy.insert(operation);defer{busy.remove(operation);revision=UUID()}
        let record=try owner.prepare(method:"session.codex.inputPrepare",params:.object(metadata),crew:context.crew)
        try await stage(record,params:.object(fields),token:token,owner:owner)
    }
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
            try? owner.finish(record,state:CrewdClientError.isUncertainSubmission(error) ? "unknown":"refused",detail:"Original staging result requires lookup; no bytes or client path retained")
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
