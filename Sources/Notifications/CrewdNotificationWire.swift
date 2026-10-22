import Foundation

/// Exact API3 producer contract. APNs hints are never execution authority.
enum CrewdNotificationWire {
    enum Environment:String,Codable,CaseIterable {case sandbox,production}
    enum Kind:String,Codable,CaseIterable {
        case done,reported,blocked,askOpened,replyReady,responseFailed,humanApprovalRequired,humanMention
        var label:String{switch self{case .done:return "工作完成";case .reported:return "进展汇报";case .blocked:return "工作受阻";case .askOpened:return "新增人待办";case .replyReady:return "回复就绪";case .responseFailed:return "回复失败";case .humanApprovalRequired:return "需要人工确认";case .humanMention:return "有人提到我"}}
    }
    struct Context:Codable,Equatable {
        let action:String,authorityId:String,dbId:String,crewId:String
        let kind:Kind
        let source:CrewdValue
        init(value:CrewdValue)throws{
            try exact(value,["action","authorityId","dbId","crewId","kind","source"])
            guard value["action"] == .string("open_context"),let kind=Kind(rawValue:value["kind"].string ?? "") else{throw invalid()}
            action="open_context";authorityId=try uuid(value["authorityId"]);dbId=try text(value["dbId"],maximum:256);crewId=try text(value["crewId"]);self.kind=kind
            let source=value["source"]
            switch source["type"].string {
            case "workEvent":
                try exact(source,["type","eventSeq","workId","askId","entryNo","objectId"],optional:["workId","askId","entryNo"])
                _ = try decimal(source["eventSeq"],zero:false);_ = try text(source["objectId"])
                for key in ["workId","askId","entryNo"] where source[key] != .null{_ = try text(source[key])}
                guard [.done,.reported,.blocked,.askOpened].contains(kind),CrewdWorkID(try text(source["objectId"]),kind:kind == .askOpened ? .human:.plan) != nil else{throw invalid()}
            case "codexTurn":
                try exact(source,["type","sessionId","threadId","turnId"])
                for key in ["sessionId","threadId","turnId"]{_ = try text(source[key])}
                guard [.replyReady,.responseFailed].contains(kind) else{throw invalid()}
            case "managedResponse":
                try exact(source,["type","sessionId","runtimeId","requestId","promptId","receiptRevision"])
                for key in ["sessionId","runtimeId","requestId","promptId"]{_ = try text(source[key])};_ = try decimal(source["receiptRevision"],zero:false)
                guard [.replyReady,.responseFailed].contains(kind) else{throw invalid()}
            case "decision":
                try exact(source,["type","sessionId","generation","decisionId"])
                for key in ["sessionId","generation","decisionId"]{_ = try text(source[key])}
                guard kind == .humanApprovalRequired else{throw invalid()}
            case "message":
                try exact(source,["type","messageId","messageSeq","channelId"])
                _ = try text(source["messageId"]);_ = try text(source["channelId"]);_ = try decimal(source["messageSeq"],zero:false)
                guard kind == .humanMention else{throw invalid()}
            default:throw invalid()
            }
            self.source=source
        }
        init(from decoder:Decoder)throws{try self.init(value:CrewdValue(from:decoder))}
    }
    struct Hint:Codable,Equatable {
        let installationId:String,deliveryId:String
        let context:Context
        init(value:CrewdValue)throws{
            try exact(value,["installationId","deliveryId","context"])
            installationId=try uuid(value["installationId"]);deliveryId=try decimal(value["deliveryId"],zero:false);context=try .init(value:value["context"])
        }
        init(from decoder:Decoder)throws{try self.init(value:CrewdValue(from:decoder))}
    }
    struct Status:Equatable {
        let available:Bool,enabled:Bool,revision:String
        let reason:String?,environment:Environment?,topic:String?
        init(_ value:CrewdValue)throws{
            try exact(value,["available","enabled","reason","revision","environment","topic"],optional:["reason","environment","topic"])
            available=try boolean(value["available"]);enabled=try boolean(value["enabled"]);revision=try decimal(value["revision"],zero:true)
            reason=try optionalText(value["reason"]);topic=try optionalText(value["topic"])
            if value["environment"] == .null{environment=nil}else{guard let e=Environment(rawValue:value["environment"].string ?? "") else{throw invalid()};environment=e}
        }
    }
    struct Installation:Equatable {
        let id:String,generation:String
        let revoked:Bool
        let environment:Environment
        let topic:String,crews:[String],kinds:[Kind]
        init(_ value:CrewdValue)throws{
            try exact(value,["installationId","generation","revoked","environment","topic","crewIds","kinds"])
            id=try uuid(value["installationId"]);generation=try decimal(value["generation"],zero:false);revoked=try boolean(value["revoked"])
            guard let e=Environment(rawValue:value["environment"].string ?? "") else{throw invalid()};environment=e;topic=try text(value["topic"])
            crews=try strings(value["crewIds"],maximum:64)
            kinds=try strings(value["kinds"],maximum:8).map{guard let v=Kind(rawValue:$0) else{throw invalid()};return v}
        }
    }
    struct Delivery:Identifiable,Equatable {
        let id:String,context:Context,state:String,expiresAt:String
        let apnsId:String?,reason:String?
        init(_ value:CrewdValue)throws{
            try exact(value,["deliveryId","context","state","apnsId","reason","expiresAt"],optional:["apnsId","reason"])
            id=try decimal(value["deliveryId"],zero:false);context=try .init(value:value["context"]);state=try text(value["state"])
            guard ["pending","unknown","acceptedByApns","rejected","suppressed"].contains(state) else{throw invalid()}
            expiresAt=try text(value["expiresAt"]);guard ISO8601DateFormatter().date(from:expiresAt) != nil || ISO8601DateFormatter.fractional.date(from:expiresAt) != nil else{throw invalid()}
            apnsId=value["apnsId"] == .null ? nil:try uuid(value["apnsId"]);reason=try optionalText(value["reason"])
        }
        var label:String{switch state{case "pending":return "待发送";case "unknown":return "发送结果未知";case "acceptedByApns":return "Apple 已接受（不代表已展示）";case "rejected":return "Apple 已拒绝";default:return "已抑制"}}
    }
    /// Mutations and lookup both return the original closed Known envelope.
    static func knownReceipt(_ value:CrewdValue,method:String,params:CrewdValue)throws->Installation{
        try exact(value,["kind","operation","result"])
        guard value["kind"] == .string("known"),value["operation"] == .string(method) else{throw invalid()}
        return try validateReceipt(value["result"],method:method,params:params)
    }
    static func validateReceipt(_ value:CrewdValue,method:String,params:CrewdValue)throws->Installation{
        let result=try Installation(value)
        guard result.id==params["installationId"].string,let old=UInt64(try decimal(params["expectedGeneration"],zero:true)),old<UInt64.max,result.generation==String(old+1),result.revoked==(method=="notification.installationRevoke") else{throw invalid()}
        if method=="notification.installationRegister" {
            guard result.environment.rawValue==params["environment"].string,result.topic==params["topic"].string,result.crews==(try strings(params["crewIds"],maximum:64)),result.kinds.map(\.rawValue)==(try strings(params["kinds"],maximum:8)) else{throw invalid()}
        }else{guard method=="notification.installationRevoke" else{throw invalid()}}
        return result
    }
    static func exact(_ value:CrewdValue,_ keys:Set<String>,optional:Set<String>=[])throws{
        guard case .object(let fields)=value,Set(fields.keys).isSubset(of:keys),keys.subtracting(optional).isSubset(of:Set(fields.keys)) else{throw invalid()}
    }
    static func text(_ value:CrewdValue,maximum:Int=512)throws->String{guard let s=value.string,!s.isEmpty,s.utf8.count<=maximum,s==s.trimmingCharacters(in:.whitespacesAndNewlines),!s.unicodeScalars.contains(where:CharacterSet.controlCharacters.contains) else{throw invalid()};return s}
    static func uuid(_ value:CrewdValue)throws->String{let s=try text(value);guard UUID(uuidString:s) != nil else{throw invalid()};return s}
    static func decimal(_ value:CrewdValue,zero:Bool)throws->String{let s=try text(value,maximum:20);guard let n=UInt64(s),String(n)==s,(zero || n>0) else{throw invalid()};return s}
    static func boolean(_ value:CrewdValue)throws->Bool{guard case .bool(let b)=value else{throw invalid()};return b}
    static func optionalText(_ value:CrewdValue)throws->String?{value == .null ? nil:try text(value)}
    static func strings(_ value:CrewdValue,maximum:Int)throws->[String]{guard case .array(let values)=value,!values.isEmpty,values.count<=maximum else{throw invalid()};let rows=try values.map{try text($0)};guard Set(rows).count==rows.count else{throw invalid()};return rows}
    static func invalid()->CrewdClientError{.init(message:"通知原身份或回执不完整；未执行导航或重发。")}
}
private extension ISO8601DateFormatter {static var fractional:ISO8601DateFormatter{let f=ISO8601DateFormatter();f.formatOptions=[.withInternetDateTime,.withFractionalSeconds];return f}}
