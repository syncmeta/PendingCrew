import Foundation
import Combine

/// Uses the existing public command journal and selected authenticated Core.
/// This is a transport adapter, not a second branch owner or synchronization loop.
@MainActor final class CrewPersonalCoreTransport:ObservableObject,CoreDraftBranchTransport {
    typealias Wire=CrewPersonalCoreWire
    typealias Record=CrewdPresentationJournal.Prepared
    struct Dependencies {
        let origin:()->CrewdPublicCommands.Binding?
        let selected:(PersonalCoreDraftProjection)->Bool
        let rpc:(String,CrewdValue)async throws->CrewdValue
        let submit:(CrewdPublicOperation,CrewdValue,String,String?)async throws->CrewdPublicReceipt
        let history:()throws->[Record]
        let unknown:()->Set<UUID>
        let lookup:(Record)async throws->CrewdPublicReceipt?
        let acknowledge:(Record,CrewdPublicReceipt)throws->Void
        let fence:(Record)async throws->CrewdPublicReceipt
    }
    private let io:Dependencies
    private var valid=true
    private var statusOrigin:CrewdPublicCommands.Binding?
    func ready(_ projection:PersonalCoreDraftProjection)->Bool{valid && status?.available==true && status?.projection==projection && io.origin()==statusOrigin}
    @Published private(set) var status:Wire.Status?
    @Published private(set) var issues:[AppleDraftKey:String]=[:]
    @Published private(set) var message:String?
    @Published private(set) var members:[Wire.Member]=[]
    var changed:()->Void={}
    init(_ io:Dependencies){self.io=io}
    func invalidate(){valid=false;changed()}
    private func origin()throws->CrewdPublicCommands.Binding{try Task.checkCancellation();guard valid,let value=io.origin() else{throw AppleSyncFailure.accountChanged};return value}
    private func check(_ original:CrewdPublicCommands.Binding)throws{try Task.checkCancellation();guard valid,io.origin()==original else{throw AppleSyncFailure.accountChanged}}
    func refreshStatus()async throws->Wire.Status{
        let captured=try origin(),raw=try await io.rpc("personalDraft.status",.object([:]))
        try check(captured);let result=try Wire.Status(raw);guard result.database==captured.databaseID else{throw Wire.invalid()}
        statusOrigin=captured;status=result;changed();return result
    }
    private func qualified(_ projection:PersonalCoreDraftProjection)async throws->(CrewdPublicCommands.Binding,Wire.Status,CrewdValue){
        guard io.selected(projection) else{throw AppleSyncFailure.accountChanged}
        let captured=try origin(),current=try await refreshStatus();try check(captured)
        guard io.selected(projection),current.available,current.projection==projection,let scope=current.scope else{throw AppleSyncFailure.accountChanged};return(captured,current,scope)
    }
    private func history()throws->[Record]{try io.history().filter{$0.scope==io.origin()?.scope}}
    var pending:[Record]{
        guard let rows=try? history() else{return []};let unknown=io.unknown()
        return rows.filter{unknown.contains($0.id) && belongs($0)}
    }
    private func belongs(_ row:Record)->Bool{
        let p=row.params["command"]["params"]
        guard let status else{return false}
        if row.params["command"]["operation"].string=="personalDraft.ownerConfigure"{return p["ownerEpoch"].string==status.ownerEpoch}
        if p["scope"]["memberId"] != .null,p["scope"]["memberId"].string != status.member?.id{return false}
        return (p["scope"]["spaceId"].string ?? p["spaceId"].string)==status.space && (p["scope"]["spaceEpoch"].string ?? p["spaceEpoch"].string)==status.epoch
    }
    private func blocks(_ row:Record,ids:Set<UUID>,key:AppleDraftKey?)throws->Bool{
        guard belongs(row) else{return false};let p=row.params["command"]["params"]
        switch row.params["command"]["operation"].string{
        case "personalDraft.checkpoint":return p["branchId"].string.flatMap(UUID.init(uuidString:)).map{ids.contains($0)} ?? false
        case "personalDraft.resolve":return try key.map{try Wire.key($0)==p["key"]} ?? false
        default:return false
        }
    }
    /// User-requested lookup/fence only. NotFound alone never opens the gate.
    func recover(_ row:Record,sealMissing:Bool)async throws{
        let captured=try origin();guard row.scope==captured.scope,belongs(row) else{throw AppleSyncFailure.accountChanged}
        if sealMissing{_ = try await io.fence(row);try check(captured);message="原请求已核对；封闭未提交请求不代表原草稿已同步。"}
        else if let receipt=try await io.lookup(row){try check(captured);if receipt.phase == .committed{try Wire.validateResult(receipt.value["result"],command:row.params["command"],expectedWriter:status?.member?.writer)};if [.committed,.rejected,.suppressed].contains(receipt.phase){try io.acknowledge(row,receipt)};message="已读取原请求结果；没有重发。"}
        else{try check(captured);message="尚未找到原回执；原项仍待核对，不会重传。"}
        changed()
    }
    private func mutation(_ op:CrewdPublicOperation,_ params:CrewdValue,crew:String)async throws->CrewdValue{
        let captured=try origin(),receipt=try await io.submit(op,params,crew,status?.member?.writer);try check(captured)
        guard !receipt.historical,receipt.phase == .committed else{throw CrewdClientError(message:"原个人草稿请求尚未成功确认；请核对原 UUID，不会重发。")}
        try Wire.validateResult(receipt.value["result"],command:.object(["operation":.string(op.rawValue),"params":params]))
        return receipt.value["result"]
    }
    private func readBranches(scope:CrewdValue,key:AppleDraftKey,origin captured:CrewdPublicCommands.Binding)async throws->[AppleBranch]{
        var cursor:CrewdValue = .null,seen=Set<String>(),cut:CrewdValue?,result:[AppleBranch]=[]
        for _ in 0..<65{
            let v=try await io.rpc("personalDraft.branches",.object(["scope":scope,"key":try Wire.key(key),"cursor":cursor,"limit":.integer(64)]));try check(captured)
            try Wire.fields(v,["scope","branches","membershipCut","nextCursor","complete","observedAt"])
            guard v["scope"]==scope,case .array(let rows)=v["branches"],rows.count<=64 else{throw Wire.invalid()}
            _ = try Wire.revision(v["membershipCut"],zero:true);_ = try Wire.text(v["observedAt"])
            if let cut{guard cut==v["membershipCut"] else{throw Wire.invalid()}}else{cut=v["membershipCut"]}
            for row in rows{let branch=try Wire.branch(row);guard branch.key==key,!result.contains(where:{$0.id==branch.id}) else{throw Wire.invalid()};result.append(branch)}
            if try Wire.bool(v["complete"]){guard v["nextCursor"] == .null else{throw Wire.invalid()};return result}
            let next=try Wire.text(v["nextCursor"]);guard seen.insert(next).inserted else{throw Wire.invalid()};cursor = .string(next)
        };throw Wire.invalid()
    }
    private func readAll(scope:CrewdValue,origin captured:CrewdPublicCommands.Binding)async throws->[AppleBranch]{
        var cursor:CrewdValue = .null,seen=Set<String>(),cut:CrewdValue?,keys=Set<AppleDraftKey>(),complete=false
        for _ in 0..<65{
            let v=try await io.rpc("personalDraft.catalog",.object(["scope":scope,"cursor":cursor,"limit":.integer(64)]));try check(captured)
            try Wire.fields(v,["scope","entries","membershipCut","nextCursor","complete","observedAt"])
            guard v["scope"]==scope,case .array(let rows)=v["entries"],rows.count<=64 else{throw Wire.invalid()}
            _ = try Wire.revision(v["membershipCut"],zero:true);_ = try Wire.text(v["observedAt"])
            if let cut{guard cut==v["membershipCut"] else{throw Wire.invalid()}}else{cut=v["membershipCut"]}
            for row in rows{try Wire.fields(row,["key","activeBranches","retiredBranches"]);guard let active=row["activeBranches"].int,let retired=row["retiredBranches"].int,active>=0,retired>=0,active+retired<=4096,keys.insert(try Wire.key(row["key"])).inserted else{throw Wire.invalid()}}
            if try Wire.bool(v["complete"]){guard v["nextCursor"] == .null else{throw Wire.invalid()};complete=true;break}
            let next=try Wire.text(v["nextCursor"]);guard seen.insert(next).inserted else{throw Wire.invalid()};cursor = .string(next)
        }
        guard complete else{throw Wire.invalid()};var result:[AppleBranch]=[]
        for key in keys.sorted(by:{$0.crewID<$1.crewID}){guard key.databaseID==captured.databaseID else{throw Wire.invalid()};result += try await readBranches(scope:scope,key:key,origin:captured);guard result.count<=4096 else{throw Wire.invalid()}}
        guard Set(result.map(\.id)).count==result.count else{throw Wire.invalid()};return result
    }
    func exchangeCore(projection:PersonalCoreDraftProjection,branches:[AppleBranch])async throws->[AppleBranch]{
        let(captured,_,scope)=try await qualified(projection)
        var remote=try await readAll(scope:scope,origin:captured);issues=[:]
        let records=try history(),unresolved=pending
        for local in branches where local.deviceID==projection.writerID && !local.retired {
            try check(captured);guard io.selected(projection) else{throw AppleSyncFailure.accountChanged}
            guard local.key.databaseID==projection.databaseID else{issues[local.key]="此草稿属于其它数据库，保留本机原文。";continue}
            if remote.contains(where:{$0.id==local.id && $0.retired}){continue}
            let previous=remote.first{$0.id==local.id}
            if let previous,previous.revision>=local.revision{continue}
            if try unresolved.contains(where:{try blocks($0,ids:[local.id],key:local.key)}){issues[local.key]="该分支原请求待核对；本机可继续编辑。";continue}
            let attempted=records.filter{belongs($0) && $0.params["command"]["operation"] == .string("personalDraft.checkpoint") && $0.params["command"]["params"]["branchId"].string==local.id.uuidString.lowercased()}.compactMap{UInt64($0.params["command"]["params"]["revision"].string ?? "")}.max() ?? 0
            guard local.revision>attempted else{issues[local.key]="此原版本尚未同步；请核对原请求，或明确保存当前内容为新版本。";continue}
            do{
                guard let bytes=local.payload else{throw Wire.invalid()};let payload=try JSONDecoder().decode(Wire.Payload.self,from:bytes)
                let p:CrewdValue = .object(["scope":scope,"branchId":.string(local.id.uuidString.lowercased()),"key":try Wire.key(local.key),"expectedRevision":.string(String(previous?.revision ?? 0)),"revision":.string(String(local.revision)),"payload":try Wire.payload(payload)])
                let result=try await mutation(.personalDraftCheckpoint,p,crew:local.key.crewID);try check(captured)
                let confirmed=try Wire.branch(result["branch"]);guard confirmed.deviceID==projection.writerID else{throw Wire.invalid()}
                remote.removeAll{$0.id==confirmed.id};remote.append(confirmed)
            }catch{try check(captured);issues[local.key]="此分支尚未同步；原文及原请求已保留："+error.localizedDescription}
        }
        changed();return remote
    }
    func resolveCore(projection:PersonalCoreDraftProjection,key:AppleDraftKey,retiring:[AppleBranch],replacement:AppleBranch?)async throws->[AppleBranch]{
        let(captured,_,scope)=try await qualified(projection)
        guard key.databaseID==projection.databaseID,!(try pending.contains(where:{try blocks($0,ids:Set(retiring.map(\.id)),key:key)})) else{throw CrewdClientError(message:"此原分支操作仍待核对；合并原文继续本机保留。")}
        var repl:CrewdValue = .null
        if let replacement{guard replacement.deviceID==projection.writerID,let bytes=replacement.payload else{throw Wire.invalid()};repl = .object(["branchId":.string(replacement.id.uuidString.lowercased()),"revision":.string(String(replacement.revision)),"payload":try Wire.payload(JSONDecoder().decode(Wire.Payload.self,from:bytes))])}
        let p:CrewdValue = .object(["scope":scope,"key":try Wire.key(key),"retiring":.array(retiring.map{.object(["branchId":.string($0.id.uuidString.lowercased()),"revision":.string(String($0.revision))])}),"replacement":repl])
        let result=try await mutation(.personalDraftResolve,p,crew:key.crewID);try check(captured)
        var confirmed=retiring.map{var b=$0;b.retired=true;b.payload=nil;return b}
        if result["branch"] != .null{let b=try Wire.branch(result["branch"]);guard b.deviceID==projection.writerID else{throw Wire.invalid()};confirmed.append(b)}
        changed();return confirmed
    }
    private func controlsAvailable(_ op:CrewdPublicOperation,grant:String?=nil)throws{
        guard !pending.contains(where:{$0.params["command"]["operation"] == .string(op.rawValue) && (grant==nil || $0.params["command"]["params"]["grantId"].string==grant)}) else{throw CrewdClientError(message:"此设置的原请求尚未确认，请先核对原 UUID。",kind:.notSubmitted)}
    }
    func configureOwner(localEnabled:Bool,acknowledgeRestore:Bool)async throws{
        let current=try await refreshStatus();guard current.owner,let ownerEpoch=current.ownerEpoch else{throw AppleSyncFailure.invalid};try controlsAvailable(.personalDraftOwner)
        _ = try await mutation(.personalDraftOwner,.object(["ownerEpoch":.string(ownerEpoch),"expectedSpaceId":current.space.map(CrewdValue.string) ?? .null,"expectedSpaceRevision":.string(current.revision),"localEnabled":.bool(localEnabled),"acknowledgeRestore":.bool(acknowledgeRestore)]),crew:"")
        _ = try await refreshStatus()
    }
    func consent(_ enabled:Bool)async throws{
        let current=try await refreshStatus();guard let scope=current.scope,current.member?.revoked==false else{throw AppleSyncFailure.invalid};try controlsAvailable(.personalDraftConsent)
        _ = try await mutation(.personalDraftConsent,.object(["scope":scope,"enabled":.bool(enabled)]),crew:"");_ = try await refreshStatus()
    }
    func loadMembers()async throws{
        let captured=try origin(),current=try await refreshStatus();try check(captured)
        guard current.owner,let space=current.space,let epoch=current.epoch else{throw AppleSyncFailure.invalid}
        let v=try await io.rpc("personalDraft.members",.object(["spaceId":.string(space),"spaceEpoch":.string(epoch)]));try check(captured)
        try Wire.fields(v,["spaceId","spaceEpoch","revision","members"]);guard v["spaceId"].string==space,v["spaceEpoch"].string==epoch,case .array(let rows)=v["members"],rows.count<=64 else{throw Wire.invalid()};_ = try Wire.revision(v["revision"],zero:true);members=try rows.map(Wire.Member.init);changed()
    }
    func setMember(grant:CrewdRemoteGrant,enabled:Bool)async throws{
        let current=try await refreshStatus();guard current.owner,let space=current.space,let epoch=current.epoch else{throw AppleSyncFailure.invalid};try controlsAvailable(.personalDraftMember,grant:grant.id)
        _ = try await mutation(.personalDraftMember,.object(["spaceId":.string(space),"spaceEpoch":.string(epoch),"expectedSpaceRevision":.string(current.revision),"grantId":.string(grant.id),"grantRevision":.string(grant.revision),"clientSpkiSha256":.string(grant.clientSpkiSha256),"enabled":.bool(enabled)]),crew:"");try await loadMembers()
    }
}
