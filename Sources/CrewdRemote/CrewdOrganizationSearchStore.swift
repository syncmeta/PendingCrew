import Foundation
import Combine

/// One presentation's pure reads. Snapshot order never becomes a second ledger.
@MainActor final class CrewdOrganizationSearchStore:ObservableObject {
    typealias Wire=CrewdOrganizationSearchWire
    typealias Origin=CrewdKnowledgeStore.Origin
    typealias RPC=CrewdKnowledgeStore.RPC
    typealias Pause=@MainActor () async throws->Void
    enum Phase:Equatable {case idle,loading,building,ready,failed,expired,unavailable,sourceChanged}
    private let origin:()->Origin?
    private let rpc:RPC
    private let available:()->Bool
    private let knowledgeAvailable:()->Bool
    private let pause:Pause
    @Published private(set) var items:[Wire.Hit]=[]
    @Published private(set) var snapshot:Wire.Snapshot?
    @Published private(set) var nextCursor:String?
    @Published private(set) var scope:Wire.Scope?
    @Published private(set) var phase=Phase.idle
    @Published private(set) var busy=false
    @Published private(set) var failure:String?
    @Published private(set) var neighbors:[Wire.Expansion]=[]
    @Published private(set) var expansionSnapshot:Wire.Snapshot?
    @Published private(set) var expansionNext:String?
    @Published private(set) var expansionPhase=Phase.idle
    @Published private(set) var expansionBusy=false
    @Published private(set) var expansionFailure:String?
    private var activeSource:Data?
    private var query:Wire.Query?
    private var ticket=UUID(),expansionTicket=UUID()
    private var cursors=Set<Data>(),expansionCursors=Set<Data>()
    private var expanded:CrewdManagedTableV2Wire.Target?
    private var expansionScope:Wire.Scope?
    private var defaults=false
    init(origin:@escaping ()->Origin?,rpc:@escaping RPC,available:@escaping ()->Bool,knowledgeAvailable:@escaping ()->Bool={false},pause:@escaping Pause={try await Task.sleep(nanoseconds:350_000_000)}) {
        self.origin=origin;self.rpc=rpc;self.available=available;self.knowledgeAvailable=knowledgeAvailable;self.pause=pause
    }
    static func identity(_ o:Origin)->Data {
        let raw:CrewdValue = .object(["presentation":(try? CrewdKnowledgeStore.encode(o.scope)) ?? .null,"observation":.string(o.observation.uuidString),"connection":.string(o.connection.uuidString),"crew":.string(o.crew)])
        return (try? Wire.encoder.encode(raw)) ?? Data()
    }
    var supported:Bool {available()}
    var sourceKey:Data {guard available(),let o=origin() else{return Data("unavailable".utf8)};return Self.identity(o)}
    var sourceCurrent:Bool {activeSource != nil && available() && activeSource==sourceKey}
    private func valid(_ o:Origin,_ id:UUID,_ current:@MainActor ()->Bool,expansion:Bool=false)->Bool {
        !Task.isCancelled && current() && available() && origin().map(Self.identity)==Self.identity(o) && activeSource==Self.identity(o) && (expansion ? expansionTicket:ticket)==id
    }
    func cancel(clear:Bool=true) {
        ticket=UUID();expansionTicket=UUID();busy=false;expansionBusy=false
        if clear {items=[];snapshot=nil;nextCursor=nil;scope=nil;query=nil;activeSource=nil;failure=nil;phase = .idle;neighbors=[];expansionSnapshot=nil;expansionNext=nil;expansionFailure=nil;expansionPhase = .idle;expanded=nil;cursors=[];expansionCursors=[]}
    }
    func invalidateVisibleResults() {cancel();phase = .sourceChanged;failure=Wire.Problem.sourceChanged.localizedDescription}
    func handleEvent(_ name:String,connection:UUID) {
        guard origin()?.connection==connection,activeSource != nil else{return}
        // A new message or status update does not discard the fixed search cut.
        if ["crew.changed","crew.purged","channel.changed","channel.purged","message.removed","events.lagged"].contains(name) {
            invalidateVisibleResults()
        } else if name==CrewdManagedTableV2Wire.changed {
            cancelExpansion();expansionPhase = .sourceChanged;expansionFailure="关联已变化，请重新展开。"
        }
    }
    func start(_ requested:Wire.Query,limit:Int=20,current:@MainActor ()->Bool) async {
        guard current(),!Task.isCancelled else{return}
        cancel();query=requested
        guard available(),let o=origin(),current(),!Task.isCancelled else {phase = .unavailable;failure=Wire.Problem.unavailable.localizedDescription;return}
        activeSource=Self.identity(o)
        guard requested.valid,(1...200).contains(limit) else {phase=requested.query.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty ? .idle:.failed;failure=phase == .idle ? nil:"请缩短搜索内容或核对筛选范围。";return}
        let id=UUID();ticket=id;busy=true;phase = .loading
        defer{if ticket==id{busy=false}}
        do {
            var buildingScope:Wire.Scope?
            for attempt in 0..<3 {
                let raw=try await rpc(Wire.search,requested.parameters(scope:buildingScope,cursor:nil,limit:limit))
                guard valid(o,id,current) else{return}
                let page=try page(raw,request:requested,origin:o,expectedScope:buildingScope,snapshot:nil,limit:limit)
                if page.indexState == .building {
                    buildingScope=page.scope;phase = .building
                    if attempt<2 {try await pause();guard valid(o,id,current) else{return};continue}
                    return
                }
                scope=page.scope;snapshot=page.snapshot;items=page.items;nextCursor=page.nextCursor
                if let cursor=page.nextCursor{cursors.insert(Data(cursor.utf8))};phase = .ready;return
            }
        }catch{if valid(o,id,current){fail(error,append:false)}}
    }
    func more(limit:Int=20,current:@MainActor ()->Bool) async {
        guard !Task.isCancelled,!busy,sourceCurrent,let o=origin(),let query,let scope,let snapshot,let after=nextCursor,(1...200).contains(limit),current() else{return}
        let id=ticket;busy=true;failure=nil
        defer{if ticket==id{busy=false}}
        do {
            let raw=try await rpc(Wire.search,query.parameters(scope:scope,cursor:after,limit:limit));guard valid(o,id,current) else{return}
            let page=try page(raw,request:query,origin:o,expectedScope:scope,snapshot:snapshot,limit:limit)
            if page.indexState == .building {phase = .building;return}
            let prior=Set(items.map{Data($0.id.utf8)})
            guard page.items.allSatisfy({!prior.contains(Data($0.id.utf8))}),page.nextCursor.map({!cursors.contains(Data($0.utf8))}) ?? true else{throw Wire.Problem.invalid}
            items+=page.items;nextCursor=page.nextCursor;if let cursor=page.nextCursor{cursors.insert(Data(cursor.utf8))};phase = .ready
        }catch{if valid(o,id,current){fail(error,append:true)}}
    }
    private func page(_ raw:CrewdValue,request:Wire.Query,origin:Origin,expectedScope:Wire.Scope?,snapshot:Wire.Snapshot?,limit:Int)throws->Wire.Page {
        try Wire.closedPage(raw,expansion:false);let page=try Wire.decode(raw,as:Wire.Page.self)
        guard Wire.scopeValid(page.scope,database:origin.scope.databaseID),expectedScope.map({CrewdManagedTableV2Wire.exact($0,page.scope)}) ?? true,page.items.count<=limit,
              page.items.allSatisfy({$0.valid && request.accepts($0)}),CrewdManagedTableV2Wire.unique(page.items.map(\.id)) else{throw Wire.Problem.invalid}
        if page.indexState == .building {guard page.snapshot==nil,page.items.isEmpty,page.nextCursor==nil else{throw Wire.Problem.invalid}}
        else {guard let returned=page.snapshot,returned.valid,snapshot.map({$0==returned}) ?? true,page.nextCursor.map({CrewdManagedTableV2Wire.opaque($0,max:65_536)}) ?? true else{throw Wire.Problem.invalid}}
        return page
    }
    private func isExpired(_ error:Error)->Bool {(error as? CrewdClientError).map{$0.code == -32009 && $0.refusal=="organizationSearch.cursor_expired"}==true || (error as? Wire.Problem) == .expired}
    private func forbidden(_ error:Error)->Bool {(error as? CrewdClientError)?.code == -32003}
    private func fail(_ error:Error,append:Bool) {
        failure=error.localizedDescription
        if isExpired(error){items=[];snapshot=nil;nextCursor=nil;phase = .expired;failure=Wire.Problem.expired.localizedDescription}
        else if forbidden(error){items=[];snapshot=nil;nextCursor=nil;phase = .sourceChanged}
        else if (error as? CrewdClientError)?.code == -32009 && (error as? CrewdClientError)?.refusal=="organizationSearch.unavailable" {items=[];snapshot=nil;nextCursor=nil;phase = .unavailable}
        else {phase = .failed;if !append{items=[];snapshot=nil;nextCursor=nil}}
    }
    func cancelExpansion() {expansionTicket=UUID();expansionBusy=false;neighbors=[];expansionSnapshot=nil;expansionNext=nil;expansionScope=nil;expanded=nil;expansionCursors=[];expansionFailure=nil;expansionPhase = .idle}
    func expand(_ target:CrewdManagedTableV2Wire.Target,includeSystemDefaults:Bool,limit:Int=20,current:@MainActor ()->Bool) async {
        guard current(),!Task.isCancelled else{return}
        cancelExpansion()
        guard sourceCurrent,let o=origin(),target.valid,(1...200).contains(limit),current(),!isKnowledge(target) || knowledgeAvailable() else{expansionPhase = .unavailable;expansionFailure=Wire.Problem.unavailable.localizedDescription;return}
        let id=UUID();expansionTicket=id;expanded=target;defaults=includeSystemDefaults;expansionBusy=true;expansionPhase = .loading
        defer{if expansionTicket==id{expansionBusy=false}}
        do {
            var firstScope=scope
            for attempt in 0..<3 {
                let raw=try await rpc(Wire.expand,expandParams(target,scope:firstScope,after:nil,limit:limit));guard valid(o,id,current,expansion:true) else{return}
                let page=try expansionPage(raw,target:target,origin:o,expectedScope:firstScope,snapshot:nil,limit:limit)
                if page.indexState == .building {
                    firstScope=page.scope;expansionPhase = .building
                    if attempt<2 {try await pause();guard valid(o,id,current,expansion:true) else{return};continue};return
                }
                expansionScope=page.scope;expansionSnapshot=page.snapshot;neighbors=page.items;expansionNext=page.nextCursor
                if let cursor=page.nextCursor{expansionCursors.insert(Data(cursor.utf8))};expansionPhase = .ready;return
            }
        }catch{if valid(o,id,current,expansion:true){expandFailed(error,append:false)}}
    }
    func expandMore(limit:Int=20,current:@MainActor ()->Bool) async {
        guard !Task.isCancelled,!expansionBusy,sourceCurrent,let o=origin(),let target=expanded,let scope=expansionScope,let snapshot=expansionSnapshot,let after=expansionNext,current() else{return}
        let id=expansionTicket;expansionBusy=true;expansionFailure=nil
        defer{if expansionTicket==id{expansionBusy=false}}
        do {
            let raw=try await rpc(Wire.expand,expandParams(target,scope:scope,after:after,limit:limit));guard valid(o,id,current,expansion:true) else{return}
            let page=try expansionPage(raw,target:target,origin:o,expectedScope:scope,snapshot:snapshot,limit:limit)
            if page.indexState == .building {expansionPhase = .building;return}
            let seen=Set(neighbors.map(\.relationIdentity))
            guard page.items.allSatisfy({!seen.contains($0.relationIdentity)}),page.nextCursor.map({!expansionCursors.contains(Data($0.utf8))}) ?? true else{throw Wire.Problem.invalid}
            neighbors+=page.items;expansionNext=page.nextCursor;if let cursor=page.nextCursor{expansionCursors.insert(Data(cursor.utf8))};expansionPhase = .ready
        }catch{if valid(o,id,current,expansion:true){expandFailed(error,append:true)}}
    }
    private func isKnowledge(_ target:CrewdManagedTableV2Wire.Target)->Bool {target.kind == .document || target.kind == .knowledge}
    private func expandParams(_ target:CrewdManagedTableV2Wire.Target,scope:Wire.Scope?,after:String?,limit:Int)throws->CrewdValue {
        guard (1...200).contains(limit) else{throw Wire.Problem.invalid}
        return .object(["scope":try scope.map(CrewdKnowledgeStore.encode) ?? .null,"target":try CrewdKnowledgeStore.encode(target),"cursor":after.map(CrewdValue.string) ?? .null,"limit":.integer(Int64(limit)),"includeSystemDefaults":.bool(defaults)])
    }
    private func expansionPage(_ raw:CrewdValue,target:CrewdManagedTableV2Wire.Target,origin:Origin,expectedScope:Wire.Scope?,snapshot:Wire.Snapshot?,limit:Int)throws->Wire.ExpansionPage {
        try Wire.closedPage(raw,expansion:true);let page=try Wire.decode(raw,as:Wire.ExpansionPage.self)
        guard Wire.scopeValid(page.scope,database:origin.scope.databaseID),expectedScope.map({CrewdManagedTableV2Wire.exact($0,page.scope)}) ?? true,page.items.count<=limit,
              page.items.allSatisfy({$0.valid(for:target) && (!isKnowledge($0.neighbor.target) || knowledgeAvailable())}),Set(page.items.map(\.relationIdentity)).count==page.items.count else{throw Wire.Problem.invalid}
        if page.indexState == .building {guard page.snapshot==nil,page.items.isEmpty,page.nextCursor==nil else{throw Wire.Problem.invalid}}
        else {guard let returned=page.snapshot,returned.valid,snapshot.map({$0==returned}) ?? true,page.nextCursor.map({CrewdManagedTableV2Wire.opaque($0,max:65_536)}) ?? true else{throw Wire.Problem.invalid}}
        return page
    }
    private func expandFailed(_ error:Error,append:Bool) {
        expansionFailure=error.localizedDescription
        if isExpired(error){neighbors=[];expansionNext=nil;expansionSnapshot=nil;expansionPhase = .expired;expansionFailure=Wire.Problem.expired.localizedDescription}
        else if forbidden(error){neighbors=[];expansionNext=nil;expansionSnapshot=nil;expansionPhase = .sourceChanged}
        else if (error as? CrewdClientError)?.code == -32009 && (error as? CrewdClientError)?.refusal=="organizationSearch.unavailable" {neighbors=[];expansionNext=nil;expansionSnapshot=nil;expansionPhase = .unavailable}
        else {expansionPhase = .failed;if !append{neighbors=[];expansionNext=nil;expansionSnapshot=nil}}
    }
}
