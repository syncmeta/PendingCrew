import Foundation
import Combine

/// Channel references only; work lifecycle remains in the organization WorkStore.
@MainActor final class CrewdChannelWorkStore:ObservableObject {
    typealias Call=@MainActor(String,CrewdValue)async throws->CrewdValue
    @Published private(set) var crew=""
    @Published private(set) var channel=""
    @Published private(set) var capabilities:Set<String>=[]
    @Published private(set) var items:[CrewdValue]=[]
    @Published private(set) var todoAttributes:[String:CrewdValue]=[:]
    @Published private(set) var linkRevision:String?
    @Published private(set) var sourceRevision:String?
    @Published private(set) var loaded=false
    @Published private(set) var loading=false
    @Published private(set) var writing=false
    @Published private(set) var activationEpoch=UUID()
    @Published private(set) var failure:CrewdClientError?
    private let call:Call
    private var generation=UUID(),query=UUID()
    private var activeOwners=Set<UUID>()
    private let defaultActiveOwner=UUID()
    func setActive(_ value:Bool,owner:UUID?=nil){
        let owner=owner ?? defaultActiveOwner
        if value{activeOwners.insert(owner)}else{activeOwners.remove(owner)}
        if activeOwners.isEmpty{refreshTask?.cancel()}
    }
    private var refreshTask:Task<Void,Never>?
    init(call:@escaping Call){self.call=call}
    func selectChannel(crew:String,channel:String){guard self.crew != crew || self.channel != channel else{return};generation=UUID();query=UUID();refreshTask?.cancel();self.crew=crew;self.channel=channel;items=[];todoAttributes=[:];linkRevision=nil;sourceRevision=nil;loaded=false;loading=false;failure=nil}
    func configureCapabilities(_ value:Set<String>){capabilities=value}
    func invalidateConnection(){generation=UUID();query=UUID();refreshTask?.cancel();capabilities=[];loaded=false;loading=false;activationEpoch=UUID()}
    private func check(_ token:UUID,_ request:UUID?=nil)throws{guard generation == token,!Task.isCancelled,request == nil || query == request else{throw CancellationError()}}
    private func require()throws{guard capabilities.contains("channelWork"),!crew.isEmpty,!channel.isEmpty else{throw CrewdClientError(message:"配套后台尚未开放当前会话的工作引用。",kind:.notSubmitted)}}
    private func revision(_ value:CrewdValue)throws->String{guard let s=value.string,!s.isEmpty,s.allSatisfy({$0.isASCII && $0.isNumber}),(s == "0" || s.first != "0") else{throw CrewdClientError(message:"会话工作引用版本无效。")};return s}
    nonisolated static func workID(_ object:CrewdValue)->CrewdWorkID? {
        let kind:CrewdWorkID.Kind
        switch object["kind"].string {case "requirement":kind = .requirement;case "work":kind = .plan;case "humanTask":kind = .human;default:return nil}
        guard let id=object["id"].string else{return nil};return CrewdWorkID(id,kind:kind)
    }
    nonisolated static func referenceKey(_ object:CrewdValue)->String? {
        if let id=workID(object){return id.raw}
        return nil
    }
    nonisolated static func validTags(_ tags:[String])->Bool {
        tags.count <= 16 && Set(tags).count == tags.count && tags.allSatisfy { !$0.isEmpty && $0 == $0.trimmingCharacters(in:.whitespacesAndNewlines) && $0.utf8.count <= 64 } && tags.reduce(0) { $0+$1.utf8.count } <= 1024
    }
    nonisolated private static func validTodo(_ value:CrewdValue,object:CrewdValue)->Bool {
        guard value["object"] == object,let id=workID(object),id.kind != .plan,value["crewId"].string != nil,
              let revision=value["revision"].string,!revision.isEmpty,revision.allSatisfy({$0.isASCII && $0.isNumber}),(revision == "0" || revision.first != "0"),
              ["requirement","agentAsk","humanTask"].contains(value["origin"].string ?? "") else{return false}
        let owner=value["owner"]
        guard owner == .null || owner["kind"] == .string("human") || (owner["kind"] == .string("session") && owner["id"].string?.isEmpty == false) else{return false}
        return value["manualState"] == .null || ["open","done"].contains(value["manualState"].string ?? "")
    }
    func handleEvent(_ name:String,_ value:CrewdValue){
        guard name == "events.lagged" || (name == "channel.workChanged" && value["channelId"] == .string(channel)) || name.hasPrefix("work.") || name.hasPrefix("ask.") || name == "todo.attributesChanged" else{return}
        guard !activeOwners.isEmpty,capabilities.contains("channelWork"),!channel.isEmpty else{return}
        refreshTask?.cancel();let token=generation
        refreshTask=Task{[weak self] in try? await Task.sleep(for:.milliseconds(150));guard let self,!Task.isCancelled,self.generation == token else{return};do{try await self.refresh()}catch{}}
    }
    func refresh()async throws{
        try require();let token=generation,request=UUID(),scope=channel;query=request;loading=true;failure=nil
        defer{if query == request{loading=false}}
        do{
            var result:[CrewdValue]=[],seen=Set<String>(),cursors=Set<String>(),cursor:String?,links:String?,source:String?
            var expectedTotal:Int?,expectedCounts:[String:Int]?
            repeat{
                var params:[String:CrewdValue]=["channelId":.string(scope),"limit":.integer(100)];if let cursor{params["after"] = .string(cursor)}
                let page=try await call("channel.workList",.object(params));try check(token,request)
                guard page["channelId"] == .string(scope),case .array = page["items"],page["items"].array.count <= 100,(page["next"] == .null || page["next"].string != nil) else{throw CrewdClientError(message:"会话工作引用分页结构或会话身份无效。")}
                let currentLinks=try revision(page["linkRevision"])
                guard let currentSource=page["sourceRevision"].string,!currentSource.isEmpty,currentSource.utf8.count <= 4096 else{throw CrewdClientError(message:"会话内容来源版本无效。") }
                guard links == nil || links == currentLinks,source == nil || source == currentSource else{throw CrewdClientError(message:"会话或工作账已变化，请重新读取；不能合并不同版本的分页。",code:-32009,data:.object(["currentLinkRevision":.string(currentLinks),"currentSourceRevision":.string(currentSource)]))}
                links=currentLinks;source=currentSource
                if page["total"] != .null || page["tagCounts"] != .null {
                    guard let total=page["total"].int,total >= 0,case .array(let counts)=page["tagCounts"] else{throw CrewdClientError(message:"群内容完整计数格式无效。")}
                    var map:[String:Int]=[:]
                    for count in counts {
                        guard let tag=count["tag"].string,Self.validTags([tag]),let value=count["count"].int,value >= 0,map[tag] == nil else{throw CrewdClientError(message:"群标签完整计数无效。")}
                        map[tag]=value
                    }
                    guard expectedTotal == nil || expectedTotal == total,expectedCounts == nil || expectedCounts == map else{throw CrewdClientError(message:"群内容计数在分页中发生变化。")}
                    expectedTotal=total;expectedCounts=map
                }
                for item in page["items"].array{
                    guard let key=Self.referenceKey(item["object"]),seen.insert(key).inserted,item["crewId"].string != nil,let label=item["label"].string,label.count <= 120,case .bool = item["labelTruncated"],item["status"].string != nil else{throw CrewdClientError(message:"会话工作引用缺少公共身份、可见内容或有限摘要。")}
                    if item["revision"] != .null {_ = try revision(item["revision"])}
                    if item["tags"] != .null {
                        guard case .array(let tags)=item["tags"],tags.allSatisfy({$0.string != nil}),Self.validTags(tags.compactMap(\.string)) else{throw CrewdClientError(message:"会话标签结构无效。")}
                    }
                    if item["todo"] != .null {guard Self.validTodo(item["todo"],object:item["object"]) else{throw CrewdClientError(message:"待办源属性身份无效。")}}
                    result.append(item)
                }
                cursor=page["next"].string
                if let cursor {guard !cursor.isEmpty,cursor.utf8.count <= 4096,cursors.insert(cursor).inserted else{throw CrewdClientError(message:"会话工作引用游标无效。")}}
            }while cursor != nil
            if let total=expectedTotal{guard total == result.count else{throw CrewdClientError(message:"群内容分页未覆盖完整可见引用。")}}
            if let expectedCounts {
                var counts:[String:Int]=[:]
                for item in result{for tag in item["tags"].array.compactMap(\.string){counts[tag,default:0] += 1}}
                guard expectedCounts.filter({$0.value > 0}) == counts else{throw CrewdClientError(message:"群标签计数与完整引用快照不符。")}
            }
            try check(token,request);items=result;linkRevision=links;sourceRevision=source;loaded=true
            todoAttributes=Dictionary(result.compactMap { item in
                guard item["todo"] != .null,let key=Self.workID(item["object"])?.raw else{return nil}
                return (key,item["todo"])
            },uniquingKeysWith:{a,_ in a})
        }catch{if token == generation,query == request,!(error is CancellationError){failure=(error as? CrewdClientError) ?? CrewdClientError(message:error.localizedDescription)};throw error}
    }
    @discardableResult func changeLink(object:CrewdValue,expectedRevision:String,linked:Bool)async throws->CrewdValue{
        try require();guard Self.referenceKey(object) != nil else{throw CrewdClientError(message:"只能关联公共需求、计划或人待办。",kind:.notSubmitted)}
        guard !writing else{throw CrewdClientError(message:"会话关联正在提交。",kind:.notSubmitted)}
        writing=true;let token=generation,scope=channel;defer{writing=false}
        do{
            let value=try await call(linked ? "channel.workLink":"channel.workUnlink",.object(["channelId":.string(scope),"expectedLinkRevision":.string(expectedRevision),"object":object]));try check(token)
            guard value["channelId"] == .string(scope),value["object"] == object,value["linked"] == .bool(linked) else{throw CrewdClientError(message:"会话关联回执身份或结果不符。")}
            _ = try revision(value["linkRevision"])
            do{try await refresh()}catch{}
            return value
        }catch{if token == generation,!(error is CancellationError){failure=(error as? CrewdClientError) ?? CrewdClientError(message:error.localizedDescription)};throw error}
    }
    func attributes(_ object:CrewdValue)async throws->CrewdValue {
        try require();let token=generation
        guard let id=Self.workID(object),id.kind != .plan else{throw CrewdClientError(message:"请选择待办源对象。",kind:.notSubmitted)}
        let value=try await call("todo.attributesGet",.object(["object":object]));try check(token)
        guard Self.validTodo(value,object:object) else{throw CrewdClientError(message:"待办源属性回复身份不符。")}
        todoAttributes[id.raw]=value;return value
    }
    @discardableResult func submit(_ snapshot:CrewdTodoMutationDrafts.Snapshot)async throws->CrewdValue {
        try require();guard snapshot.key.crew == crew,snapshot.key.channel == channel,!writing else{throw CrewdClientError(message:"待办的原作用域已切换或正在提交。",kind:.notSubmitted)}
        let params=try snapshot.parameters()
        writing=true;let token=generation;defer{writing=false}
        do {
            let value=try await call(snapshot.key.operation,.object(params));try check(token)
            try validateReceipt(value,snapshot:snapshot)
            do{try await refresh()}catch{}
            return value
        }catch{if token == generation,!(error is CancellationError){failure=(error as? CrewdClientError) ?? CrewdClientError(message:error.localizedDescription)};throw error}
    }
    func lookup(_ snapshot:CrewdTodoMutationDrafts.Snapshot)async throws->CrewdValue? {
        try require();let token=generation
        guard snapshot.key.crew == crew,snapshot.key.channel == channel else{throw CancellationError()}
        let receipt=try await call("resource.requestLookup",.object(["requestId":.string(snapshot.requestID.uuidString.lowercased())]));try check(token)
        guard receipt["requestId"].string.flatMap(UUID.init(uuidString:)) == snapshot.requestID else{throw CrewdClientError(message:"原请求核对身份不符。")}
        if receipt["status"] == .string("notFound") {
            guard receipt["operation"] == .null,receipt["result"] == .null else{throw CrewdClientError(message:"原请求未找到的回复不能附带操作结果。")}
            return nil
        }
        guard receipt["status"] == .string("committed"),receipt["operation"] == .string(snapshot.key.operation) else{throw CrewdClientError(message:"原请求核对操作或状态不符。")}
        try validateReceipt(receipt["result"],snapshot:snapshot)
        do{try await refresh()}catch{}
        return receipt["result"]
    }
    private func validateReceipt(_ value:CrewdValue,snapshot:CrewdTodoMutationDrafts.Snapshot)throws {
        if snapshot.key.operation == "humanTask.create" {
            guard value["requestId"].string.flatMap(UUID.init(uuidString:)) == snapshot.requestID,
                  value["channelAssociation"]["channelId"] == .string(snapshot.key.channel),case .bool = value["channelAssociation"]["linked"],
                  let id=value["ask"]["id"].string ?? value["ask"]["ask"].string,CrewdWorkID(id,kind:.human) != nil,
                  Self.validTodo(value["todo"],object:.object(["kind":.string("humanTask"),"id":.string(id)])),
                  value["todo"]["origin"] == .string("humanTask"),value["ask"]["question"] == .string(snapshot.fields.text),
                  value["ask"]["crewId"] == .string(snapshot.key.crew),case .bool = value["reused"] else{throw CrewdClientError(message:"人待办创建回执与原输入身份不符。")}
        }else if snapshot.key.operation == "channel.workTagsSet" {
            guard value["channelId"] == .string(snapshot.key.channel),value["object"] == snapshot.object,
                  value["tags"].array.compactMap(\.string) == snapshot.fields.tags else{throw CrewdClientError(message:"本群标签回执与原输入不符。")}
            _ = try revision(value["linkRevision"])
        }else{
            guard Self.validTodo(value,object:snapshot.object) else{throw CrewdClientError(message:"待办源属性回执与原输入身份不符。")}
            if snapshot.key.operation == "todo.assign" {guard value["owner"] == snapshot.fields.owner else{throw CrewdClientError(message:"负责人回执与原输入不符。")}}
            if snapshot.key.operation == "todo.setState" {guard value["manualState"] == .string(snapshot.fields.state) else{throw CrewdClientError(message:"人工状态回执与原输入不符。")}}
            todoAttributes[snapshot.key.object]=value
        }
    }

}

/// Window-owned uncertain link commands; never a persisted channel/work authority.
struct CrewdChannelLinkDrafts {
    struct Key:Hashable {let crew:String;let channel:String}
    struct Snapshot:Equatable {let key:Key;let object:CrewdValue;let expectedRevision:String;let linked:Bool;let requestID:UUID}
    private var pending:[Key:UUID]=[:]
    private var unknown:[Key:[Snapshot]]=[:]
    func busy(_ key:Key)->Bool {pending[key] != nil}
    func uncertain(_ key:Key)->[Snapshot] {unknown[key] ?? []}
    var allUnknownSnapshots:[Snapshot] {unknown.values.flatMap{$0}.sorted{$0.requestID.uuidString < $1.requestID.uuidString}}
    mutating func begin(_ key:Key,object:CrewdValue,expectedRevision:String,linked:Bool)->Snapshot? {
        guard !busy(key) else{return nil}
        let snapshot=Snapshot(key:key,object:object,expectedRevision:expectedRevision,linked:linked,requestID:UUID());pending[key]=snapshot.requestID;return snapshot
    }
    mutating func finish(_ snapshot:Snapshot,accepted:Bool,uncertain:Bool=false){
        if pending[snapshot.key] == snapshot.requestID {pending[snapshot.key]=nil}
        if uncertain,!(unknown[snapshot.key] ?? []).contains(snapshot) {unknown[snapshot.key,default:[]].append(snapshot)}
        if accepted {unknown[snapshot.key]?.removeAll{$0.requestID == snapshot.requestID}}
    }
    /// Explicit user acknowledgement after a fresh read, not inference of an old commit.
    mutating func acknowledge(_ key:Key){guard !busy(key) else{return};unknown[key]=nil}
}

/// Input snapshots only. The durable original-request journal belongs to the
/// workspace; Unknown snapshots can only use resource.requestLookup.
struct CrewdTodoMutationDrafts {
    struct Key:Hashable {let crew:String;let channel:String;let object:String;let operation:String}
    struct Fields:Equatable {var text="";var owner:CrewdValue = .null;var state="open";var tags:[String]=[];var revision=""}
    struct Snapshot:Equatable {
        let key:Key;let fields:Fields;let requestID:UUID
        var object:CrewdValue { .object(["kind":.string(key.object.hasPrefix("H") ? "humanTask":"requirement"),"id":.string(key.object)]) }
        func parameters()throws->[String:CrewdValue] {
            var p:[String:CrewdValue]=["requestId":.string(requestID.uuidString.lowercased()),"channelId":.string(key.channel)]
            if key.operation == "humanTask.create" {
                guard !fields.text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty else{throw CrewdClientError(message:"请写下需要人处理的事项。",kind:.notSubmitted)}
                p["crewId"] = .string(key.crew);p["text"] = .string(fields.text)
            } else {
                guard let id=CrewdChannelWorkStore.workID(object),id.kind != .plan else{throw CrewdClientError(message:"待办原始身份无效。",kind:.notSubmitted)}
                p["object"]=object
                switch key.operation {
                case "todo.assign":p["expectedRevision"] = .string(fields.revision);p["owner"]=fields.owner
                case "todo.setState":
                    guard ["open","done"].contains(fields.state) else{throw CrewdClientError(message:"请选择待办状态。",kind:.notSubmitted)}
                    p["expectedRevision"] = .string(fields.revision);p["state"] = .string(fields.state)
                case "channel.workTagsSet":
                    guard CrewdChannelWorkStore.validTags(fields.tags) else{throw CrewdClientError(message:"最多16个标签，每个1至64字节，总计最多1024字节。",kind:.notSubmitted)}
                    p["expectedLinkRevision"] = .string(fields.revision);p["tags"] = .array(fields.tags.map(CrewdValue.string))
                default:throw CrewdClientError(message:"未知待办操作。",kind:.notSubmitted)
                }
            }
            return p
        }
    }
    private var entries:[Key:Snapshot]=[:],pending:[Key:UUID]=[:],unknown:[Key:[Snapshot]]=[:]
    mutating func acknowledge(_ request:UUID){
        for key in Array(unknown.keys){unknown[key]?.removeAll{$0.requestID == request}}
        for key in Array(entries.keys) where entries[key]?.requestID == request{entries[key]=nil}
    }
    func fields(_ key:Key)->Fields {entries[key]?.fields ?? Fields()}
    func exists(_ key:Key)->Bool {entries[key] != nil}
    func busy(_ key:Key)->Bool {pending[key] != nil}
    func uncertain(_ key:Key)->[Snapshot] {unknown[key] ?? []}
    var allUnknownSnapshots:[Snapshot] {unknown.values.flatMap{$0}}
    mutating func edit(_ key:Key,_ fields:Fields){guard entries[key]?.fields != fields else{return};entries[key]=Snapshot(key:key,fields:fields,requestID:UUID())}
    mutating func begin(_ key:Key)->Snapshot? {guard !busy(key),uncertain(key).isEmpty,let v=entries[key] else{return nil};pending[key]=v.requestID;return v}
    mutating func finish(_ v:Snapshot,accepted:Bool,uncertain:Bool=false){
        if pending[v.key] == v.requestID{pending[v.key]=nil}
        if uncertain,!(unknown[v.key] ?? []).contains(v){unknown[v.key,default:[]].append(v)}
        if accepted{unknown[v.key]?.removeAll{$0.requestID == v.requestID};if entries[v.key] == v{entries[v.key]=nil}}
    }
}
