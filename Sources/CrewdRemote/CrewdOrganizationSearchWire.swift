import Foundation

/// Separate read-only DTOs: no old Object/Work/receipt codec is widened.
enum CrewdOrganizationSearchWire {
    static let capability="organizationSearch.v1",search="organization.search",expand="organization.search.expand"
    typealias Managed=CrewdManagedTableV2Wire
    typealias Scope=Managed.Scope
    enum Domain:String,Codable,CaseIterable,Identifiable {case all,messages,objects;var id:Self{self};var title:String{switch self{case .all:"消息与对象";case .messages:"仅消息";case .objects:"仅对象"}}}
    enum Kind:String,Codable,CaseIterable,Identifiable {
        case decision,risk,lesson,work,requirement,ask,table,row,artifact,message
        var id:Self{self}
        var title:String{switch self{case .decision:"决定";case .risk:"风险";case .lesson:"教训";case .work:"计划";case .requirement:"原始需求";case .ask:"待人处理";case .table:"组织表";case .row:"表中记录";case .artifact:"产物";case .message:"消息"}}
        var targetKind:Managed.TargetKind {Managed.TargetKind(rawValue:rawValue)!}
    }
    enum IndexState:String,Decodable {case building,ready}
    enum Status:String,Decodable {case current,historical,archived;var title:String{switch self{case .current:"当前";case .historical:"历史";case .archived:"已归档"}}}
    enum MatchOn:String,Decodable {case title,text,historicalName,reference;var title:String{switch self{case .title:"标题匹配";case .text:"正文匹配";case .historicalName:"旧名称或用途匹配";case .reference:"编号或引用匹配"}}}
    struct Query:Codable {
        var query:String
        var domain:Domain = .all
        var kinds:[Kind]?=nil
        var tableId:String?=nil
        var crewId:String?=nil
        var channelId:String?=nil
        var afterTime:String?=nil
        var beforeTime:String?=nil
        var includeSystemDefaults=false
        enum CodingKeys:String,CodingKey {case query,domain="in",kinds,tableId,crewId,channelId,afterTime,beforeTime,includeSystemDefaults}
        var identity:Data {(try? CrewdOrganizationSearchWire.encoder.encode(self)) ?? Data()}
        var effectiveKinds:Set<Kind> {
            var selected=Set(kinds ?? Kind.allCases)
            if domain == .messages {selected.formIntersection([.message])}
            if domain == .objects {selected.remove(.message)}
            if tableId != nil {selected.formIntersection([.row])}
            if channelId != nil {selected.formIntersection([.message])}
            return selected
        }
        var valid:Bool {
            guard !query.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty,query.unicodeScalars.count<=256,query.utf8.count<=1024,!effectiveKinds.isEmpty,
                  crewId.map({Managed.opaque($0)}) ?? true,channelId.map({Managed.opaque($0)}) ?? true else{return false}
            if let tableId {guard CrewdManagedDirectoryWire.objectNumber(tableId,prefix:"T") != nil,channelId==nil else{return false}}
            if let afterTime,time(afterTime)==nil{return false};if let beforeTime,time(beforeTime)==nil{return false}
            if let a=afterTime.flatMap(time),let b=beforeTime.flatMap(time),a>=b{return false}
            return true
        }
        func parameters(scope:Scope?,cursor:String?,limit:Int)throws->CrewdValue {
            guard valid,(1...200).contains(limit) else{throw Problem.invalid}
            var result=try CrewdOrganizationSearchWire.encode(self)
            guard case .object(var fields)=result else{throw Problem.invalid}
            fields["scope"]=try scope.map(CrewdOrganizationSearchWire.encode) ?? .null
            fields["cursor"]=cursor.map(CrewdValue.string) ?? .null
            fields["limit"] = .integer(Int64(limit));result = .object(fields);return result
        }
        func accepts(_ hit:Hit)->Bool {
            effectiveKinds.contains(hit.kind) && (tableId==nil || hit.kind == .row && bytes(hit.tableId,tableId)) &&
            (crewId==nil || bytes(hit.ownerCrewId,crewId))
        }
    }
    struct Snapshot:Decodable,Equatable {
        let sortedAt:String;let expiresAt:String
        static func ==(a:Self,b:Self)->Bool {bytes(a.sortedAt,b.sortedAt) && bytes(a.expiresAt,b.expiresAt)}
        var valid:Bool {guard let a=time(sortedAt),let b=time(expiresAt) else{return false};return b>a && b.timeIntervalSince(a)<=901}
    }
    struct Hit:Decodable,Identifiable {
        let target:Managed.Target;let kind:Kind;let ownerCrewId:String?;let tableId:String?;let tableTitle:String?;let title:String;let excerpt:String?;let matchedOn:MatchOn;let status:Status;let changedAt:String?
        var id:String {target.id}
        var group:String {kind == .row ? "row:"+(tableId ?? "invalid"):kind.rawValue}
        var groupTitle:String {kind == .row ? (tableTitle ?? "组织表")+" · "+(tableId ?? ""):kind.title}
        var valid:Bool {
            guard target.valid,target.kind==kind.targetKind,excerpt.map({$0.unicodeScalars.count<=280}) ?? true,changedAt.map({time($0) != nil}) ?? true else{return false}
            if kind == .artifact {guard ownerCrewId==nil else{return false}}else{guard ownerCrewId.map({Managed.opaque($0)})==true else{return false}}
            return kind == .row ? tableId.map({CrewdManagedDirectoryWire.objectNumber($0,prefix:"T") != nil})==true && tableTitle.map({!$0.isEmpty})==true:tableId==nil && tableTitle==nil
        }
    }
    struct Page:Decodable {let scope:Scope;let indexState:IndexState;let snapshot:Snapshot?;let items:[Hit];let nextCursor:String?}
    enum Direction:String,Decodable {case outgoing,incoming;var title:String{self == .outgoing ? "指向":"来自"}}
    struct Expansion:Decodable,Identifiable {
        let direction:Direction;let edge:Managed.Edge;let neighbor:Managed.ResolvedTarget;let status:Status;let changedAt:String?
        var id:String {let raw=(try? CrewdOrganizationSearchWire.encoder.encode(edge)) ?? Data();return direction.rawValue+":"+raw.base64EncodedString()}
        // Summary/current labels do not create a second relation identity.
        var relationIdentity:Data {var raw=Data(direction.rawValue.utf8);raw.append(0);raw.append(Data(edge.id.utf8));raw.append(0);raw.append((try? CrewdOrganizationSearchWire.encoder.encode(edge.source)) ?? Data());raw.append(0);raw.append((try? CrewdOrganizationSearchWire.encoder.encode(edge.target)) ?? Data());raw.append(0);raw.append((try? CrewdOrganizationSearchWire.encoder.encode(edge.origin)) ?? Data());return raw}
        func valid(for target:Managed.Target)->Bool {
            guard edge.valid,neighbor.target.valid,neighbor.summary.unicodeScalars.count<=280,changedAt.map({time($0) != nil}) ?? true,
                  neighbor.ownerCrewId.map({Managed.opaque($0)}) ?? true,let source=edge.source.target else{return false}
            if neighbor.target.kind == .message {guard neighbor.messageChannelId.map({Managed.opaque($0)})==true else{return false}}else if neighbor.messageChannelId != nil{return false}
            switch direction {
            case .outgoing:return Managed.exact(source,target) && Managed.exact(neighbor.target,edge.target)
            case .incoming:return Managed.exact(edge.target,target) && !Managed.exact(source,target) && Managed.exact(neighbor.target,source)
            }
        }
    }
    struct ExpansionPage:Decodable {let scope:Scope;let indexState:IndexState;let snapshot:Snapshot?;let items:[Expansion];let nextCursor:String?}
    enum Problem:LocalizedError,Equatable {case unavailable,invalid,sourceChanged,expired
        var errorDescription:String?{switch self{case .unavailable:"当前连接暂时无法搜索组织对象。";case .invalid:"搜索响应与原请求不一致，请重新搜索。";case .sourceChanged:"搜索来源或权限已变化，请重新搜索。";case .expired:"搜索已过期，请重新搜索。"}}
    }
    static func bytes(_ a:String?,_ b:String?)->Bool {switch(a,b){case(nil,nil):true;case(.some(let x),.some(let y)):x.utf8.elementsEqual(y.utf8);default:false}}
    static func time(_ text:String)->Date? {CrewdManagedDirectoryWire.managedTime(text)}
    static func scopeValid(_ scope:Scope,database:String?)->Bool {bytes(scope.dbId,database) && Managed.uuid(scope.restoreEpoch)}
    static var encoder:JSONEncoder {let encoder=JSONEncoder();encoder.outputFormatting=[.sortedKeys];return encoder}
    static func encode<T:Encodable>(_ value:T)throws->CrewdValue {try JSONDecoder().decode(CrewdValue.self,from:encoder.encode(value))}
    static func decode<T:Decodable>(_ raw:CrewdValue,as type:T.Type)throws->T {try JSONDecoder().decode(type,from:encoder.encode(raw))}
    static func fields(_ raw:CrewdValue,_ allowed:Set<String>)throws {guard case .object(let fields)=raw,Set(fields.keys).isSubset(of:allowed) else{throw Problem.invalid}}
    static func closedTarget(_ raw:CrewdValue,source:Bool=false)throws {
        guard let kind=raw["kind"].string else{throw Problem.invalid}
        switch kind {
        case "human":try fields(raw,["kind"])
        case "artifact":try fields(raw,source ? ["kind","reference","metadataCrewId"]:["kind","reference"])
        case "document","knowledge":try fields(raw,["kind","source"])
        default:try fields(raw,["kind","id"])
        }
    }
    static func closedPage(_ raw:CrewdValue,expansion:Bool)throws {
        try fields(raw,["scope","indexState","snapshot","items","nextCursor"])
        try fields(raw["scope"],["dbId","restoreEpoch"])
        if raw["snapshot"] != .null {try fields(raw["snapshot"],["sortedAt","expiresAt"])}
        guard case .array(let items)=raw["items"] else{throw Problem.invalid}
        for item in items {
            if expansion {
                try fields(item,["direction","edge","neighbor","status","changedAt"])
                try fields(item["edge"],["id","ownerCrewId","source","target","summary","origin","editable"])
                try fields(item["neighbor"],["target","summary","ownerCrewId","messageChannelId"])
                try closedTarget(item["edge"]["source"],source:true);try closedTarget(item["edge"]["target"]);try closedTarget(item["neighbor"]["target"])
            } else {try fields(item,["target","kind","ownerCrewId","tableId","tableTitle","title","excerpt","matchedOn","status","changedAt"]);try closedTarget(item["target"])}
        }
    }
}

/// Immutable display provenance, not a permission or executable session identity.
struct CrewdSessionDisplayWitness {
    let authority:UUID
    let connection:UUID
    let observation:UUID
    let scope:Data?
    static func scopeBytes(_ scope:CrewdPresentationJournal.Scope?)->Data? {
        guard let scope,scope.authenticatedDatabaseKnown else{return nil}
        return try? CrewdOrganizationSearchWire.encoder.encode(scope)
    }
    func matches(authority:UUID,connection:UUID,observation:UUID,scope:Data?)->Bool {
        self.authority==authority && self.connection==connection && self.observation==observation && self.scope != nil && self.scope==scope
    }
}
