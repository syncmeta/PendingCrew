import Foundation

/// Closed producer contract. Original provider JSON is read-only evidence;
/// settings never accept arbitrary provider methods or configuration keys.
enum CrewdCodexWire {
    static func settings(_ value:CrewdValue)throws->CrewdValue {
        let allowed:Set<String>=["model","effort","fast","approvalPolicy","approvalsReviewer","sandbox","collaborationMode","summary"]
        guard case .object(let fields)=value,!fields.isEmpty,Set(fields.keys).isSubset(of:allowed) else{throw CrewdPublicWire.invalid()}
        for (key,item) in fields {
            switch key {
            case "model","effort":
                guard let text=item.string,!text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty,text.utf8.count<=256,!text.unicodeScalars.contains(where:CharacterSet.controlCharacters.contains) else{throw CrewdPublicWire.invalid()}
            case "fast":_ = try CrewdPublicWire.boolean(item)
            case "approvalsReviewer":try choice(item,["user","autoReview"])
            case "collaborationMode":try choice(item,["default","plan"])
            case "summary":try choice(item,["auto","concise","detailed","none"])
            case "approvalPolicy":
                if item.string != nil{try choice(item,["untrusted","onRequest","never"])}
                else{
                    guard case .object(let wrapper)=item,Set(wrapper.keys)==["granular"],case .object(let granular)=item["granular"],Set(granular.keys)==["mcpElicitations","requestPermissions","rules","sandboxApproval","skillApproval"] else{throw CrewdPublicWire.invalid()}
                    for bit in granular.values{_ = try CrewdPublicWire.boolean(bit)}
                }
            case "sandbox":
                guard case .object(let sandbox)=item else{throw CrewdPublicWire.invalid()}
                switch item["type"].string {
                case "readOnly","workspaceWrite":guard Set(sandbox.keys)==["type","networkAccess"] else{throw CrewdPublicWire.invalid()};_ = try CrewdPublicWire.boolean(item["networkAccess"])
                case "dangerFullAccess":guard Set(sandbox.keys)==["type"] else{throw CrewdPublicWire.invalid()}
                default:throw CrewdPublicWire.invalid()
                }
            default:throw CrewdPublicWire.invalid()
            }
        }
        return value
    }
    static func review(_ value:CrewdValue)throws->CrewdValue {
        guard case .object(var fields)=value else{throw CrewdPublicWire.invalid()}
        let required:Set<String>
        switch value["type"].string {
        case "uncommittedChanges":required=["type"]
        case "baseBranch":required=["type","branch"]
        case "commit":required=["type","sha"];if fields["title"] == nil{fields["title"] = .null}
        case "custom":required=["type","instructions"]
        default:throw CrewdPublicWire.invalid()
        }
        let allowed=required.union(value["type"] == .string("commit") ? ["title"]:[])
        guard required.isSubset(of:Set(fields.keys)),Set(fields.keys).isSubset(of:allowed) else{throw CrewdPublicWire.invalid()}
        for key in required.subtracting(["type"]){try text(fields[key] ?? .null)}
        if let title=fields["title"],title != .null{try text(title)}
        return .object(fields)
    }
    static func text(_ value:CrewdValue)throws{
        guard let value=value.string,!value.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty,value.utf8.count<=128*1024 else{throw CrewdPublicWire.invalid()}
    }
    static func validateResult(_ value:CrewdValue,command:CrewdValue)throws{
        guard value["operation"] == command["operation"],value["sessionId"] == command["params"]["target"]["sessionId"],value["providerResult"] == .null else{throw CrewdPublicWire.invalid()}
        let application:String
        switch command["operation"].string {
        case "session.codex.configure":application="followingTurns"
        case "session.codex.compact","session.codex.review":application="started"
        case "session.codex.steer":application="currentTurn"
        case "session.codex.queue":application="queued"
        case "session.codex.titleSet","session.codex.fork":application="metadata"
        default:throw CrewdPublicWire.invalid()
        }
        _ = try CrewdPublicWire.boolean(value["providerAccepted"])
        guard command["operation"] == .string("session.codex.queue") || value["providerAccepted"] == .bool(true) else{throw CrewdPublicWire.invalid()}
        guard value["application"] == .string(application) else{throw CrewdPublicWire.invalid()}
        if command["operation"] == .string("session.codex.fork"){
            _ = try CrewdPublicWire.token(value["nativeThreadId"]);_ = try CrewdPublicWire.token(value["newSessionId"])
        }
        if command["operation"] == .string("session.codex.steer"){
            guard value["acceptedTurnId"] == command["params"]["target"]["turnId"],value["acceptedTurnId"].string != nil else{throw CrewdPublicWire.invalid()}
        }
    }
    static func choice(_ value:CrewdValue,_ choices:Set<String>)throws{
        guard let text=value.string,choices.contains(text) else{throw CrewdPublicWire.invalid()}
    }
}

struct CrewdCodexInspection:Equatable {
    struct Capability:Equatable,Identifiable {
        let id:String
        let availability:String
        let protocolKind:String
        let detail:String?
        var available:Bool{availability == "available"}
        var explanation:String {
            let state:String
            switch availability {
            case "available":state="可用"
            case "requiresRunning":state="需明确恢复会话后使用"
            case "providerRejected":state="本机 Codex 拒绝此能力"
            case "pcwNotImplemented":state="PendingCrew 尚未接线"
            default:state="当前状态不可用"
            }
            return detail.map{state+" · "+$0} ?? state
        }
    }
    let value:CrewdValue
    let target:CrewdCommandTarget
    let capabilities:[Capability]
    var sessionID:String{target.sessionID}
    init(_ value:CrewdValue,session:String)throws{
        guard value["sessionId"] == .string(session),case .array(let capabilities)=value["capabilities"],capabilities.count<=64 else{throw CrewdPublicWire.invalid()}
        target=try CrewdCommandTarget(value["target"])
        guard target.sessionID == session else{throw CrewdPublicWire.invalid()}
        _ = try CrewdPublicWire.boolean(value["running"]);_ = try CrewdPublicWire.boolean(value["working"]);_ = try CrewdPublicWire.boolean(value["historical"])
        try CrewdCodexWire.choice(value["title"]["source"],["human","task"])
        try CrewdCodexWire.choice(value["title"]["status"],["pending","synced","unknown","rejected"])
        if value["settingsStatus"] != .null{try CrewdCodexWire.choice(value["settingsStatus"],["pending","accepted","unknown","rejected"])}
        guard let queued=value["queuedInputs"].int,UInt32(exactly:queued) != nil else{throw CrewdPublicWire.invalid()}
        self.capabilities=try capabilities.map{item in
            try CrewdCodexWire.choice(item["availability"],["available","requiresRunning","providerRejected","pcwNotImplemented","unavailable"])
            try CrewdCodexWire.choice(item["protocol"],["stable","experimental"])
            return .init(id:try CrewdPublicWire.token(item["id"]),availability:item["availability"].string!,protocolKind:item["protocol"].string!,detail:item["detail"].string)
        }
        guard Set(self.capabilities.map(\.id)).count == self.capabilities.count else{throw CrewdPublicWire.invalid()}
        self.value=value
    }
    func capability(_ id:String)->Capability?{capabilities.first{$0.id == id}}
}
