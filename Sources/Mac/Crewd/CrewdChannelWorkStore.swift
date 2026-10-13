#if os(macOS)
import Foundation
import Combine

/// Channel references only; work lifecycle remains in the organization WorkStore.
@MainActor final class CrewdChannelWorkStore:ObservableObject {
    typealias Call=@MainActor(String,CrewdValue)async throws->CrewdValue
    @Published private(set) var crew=""
    @Published private(set) var channel=""
    @Published private(set) var capabilities:Set<String>=[]
    @Published private(set) var items:[CrewdValue]=[]
    @Published private(set) var linkRevision:String?
    @Published private(set) var sourceRevision:String?
    @Published private(set) var loaded=false
    @Published private(set) var loading=false
    @Published private(set) var writing=false
    @Published private(set) var activationEpoch=UUID()
    @Published private(set) var failure:CrewdClientError?
    private let call:Call
    private var generation=UUID(),query=UUID()
    private var active=true
    func setActive(_ value:Bool){active=value;if !value{refreshTask?.cancel()}}
    private var refreshTask:Task<Void,Never>?
    init(call:@escaping Call){self.call=call}
    func selectChannel(crew:String,channel:String){guard self.crew != crew || self.channel != channel else{return};generation=UUID();query=UUID();refreshTask?.cancel();self.crew=crew;self.channel=channel;items=[];linkRevision=nil;sourceRevision=nil;loaded=false;loading=false;failure=nil}
    func configureCapabilities(_ value:Set<String>){capabilities=value}
    func invalidateConnection(){generation=UUID();query=UUID();refreshTask?.cancel();capabilities=[];loaded=false;loading=false;activationEpoch=UUID()}
    private func check(_ token:UUID,_ request:UUID?=nil)throws{guard generation == token,!Task.isCancelled,request == nil || query == request else{throw CancellationError()}}
    private func require()throws{guard capabilities.contains("channelWork"),!crew.isEmpty,!channel.isEmpty else{throw CrewdClientError(message:"配套后台尚未开放当前会话的工作引用。",kind:.notSubmitted)}}
    private func revision(_ value:CrewdValue)throws->String{guard let s=value.string,!s.isEmpty,s.allSatisfy({$0.isASCII && $0.isNumber}),(s == "0" || s.first != "0") else{throw CrewdClientError(message:"会话工作引用版本无效。")};return s}
    static func workID(_ object:CrewdValue)->CrewdWorkID? {
        let kind:CrewdWorkID.Kind
        switch object["kind"].string {case "requirement":kind = .requirement;case "work":kind = .plan;case "humanTask":kind = .human;default:return nil}
        guard let id=object["id"].string else{return nil};return CrewdWorkID(id,kind:kind)
    }
    func handleEvent(_ name:String,_ value:CrewdValue){
        guard name == "events.lagged" || (name == "channel.workChanged" && value["channelId"] == .string(channel)) || name.hasPrefix("work.") || name.hasPrefix("ask.") else{return}
        guard active,capabilities.contains("channelWork"),!channel.isEmpty else{return}
        refreshTask?.cancel();let token=generation
        refreshTask=Task{[weak self] in try? await Task.sleep(for:.milliseconds(150));guard let self,!Task.isCancelled,self.generation == token else{return};do{try await self.refresh()}catch{}}
    }
    func refresh()async throws{
        try require();let token=generation,request=UUID(),scope=channel;query=request;loading=true;failure=nil
        defer{if query == request{loading=false}}
        do{
            var result:[CrewdValue]=[],seen=Set<String>(),cursors=Set<String>(),cursor:String?,links:String?,source:String?
            repeat{
                var params:[String:CrewdValue]=["channelId":.string(scope),"limit":.integer(100)];if let cursor{params["after"] = .string(cursor)}
                let page=try await call("channel.workList",.object(params));try check(token,request)
                guard page["channelId"] == .string(scope),case .array = page["items"],page["items"].array.count <= 100,(page["next"] == .null || page["next"].string != nil) else{throw CrewdClientError(message:"会话工作引用分页结构或会话身份无效。")}
                let currentLinks=try revision(page["linkRevision"]),currentSource=try revision(page["sourceRevision"])
                guard links == nil || links == currentLinks,source == nil || source == currentSource else{throw CrewdClientError(message:"会话或工作账已变化，请重新读取；不能合并不同版本的分页。",code:-32009,data:.object(["currentLinkRevision":.string(currentLinks),"currentSourceRevision":.string(currentSource)]))}
                links=currentLinks;source=currentSource
                for item in page["items"].array{
                    guard let id=Self.workID(item["object"]),seen.insert(id.raw).inserted,item["crewId"].string != nil,let label=item["label"].string,label.count <= 120,case .bool = item["labelTruncated"],item["status"].string != nil else{throw CrewdClientError(message:"会话工作引用缺少公共身份、可见内容或有限摘要。")}
                    if item["revision"] != .null {_ = try revision(item["revision"])}
                    result.append(item)
                }
                cursor=page["next"].string
                if let cursor {guard !cursor.isEmpty,cursor.utf8.count <= 4096,cursors.insert(cursor).inserted else{throw CrewdClientError(message:"会话工作引用游标无效。")}}
            }while cursor != nil
            try check(token,request);items=result;linkRevision=links;sourceRevision=source;loaded=true
        }catch{if token == generation,query == request,!(error is CancellationError){failure=(error as? CrewdClientError) ?? CrewdClientError(message:error.localizedDescription)};throw error}
    }
    @discardableResult func changeLink(object:CrewdValue,expectedRevision:String,linked:Bool)async throws->CrewdValue{
        try require();guard Self.workID(object) != nil else{throw CrewdClientError(message:"只能关联公共需求、计划或人类事项。",kind:.notSubmitted)}
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
}

/// Window-owned uncertain link commands; never a persisted channel/work authority.
struct CrewdChannelLinkDrafts {
    struct Key:Hashable {let crew:String;let channel:String}
    struct Snapshot:Equatable {let key:Key;let object:CrewdValue;let expectedRevision:String;let linked:Bool;let requestID:UUID}
    private var pending:[Key:UUID]=[:]
    private var unknown:[Key:[Snapshot]]=[:]
    func busy(_ key:Key)->Bool {pending[key] != nil}
    func uncertain(_ key:Key)->[Snapshot] {unknown[key] ?? []}
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
#endif
