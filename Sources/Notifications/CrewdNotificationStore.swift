import Foundation
import Combine
import CryptoKit

/// One original authority/grant consumer. No provider secret or platform SDK lives here.
@MainActor final class CrewdNotificationStore:ObservableObject {
    typealias Wire=CrewdNotificationWire
    struct Origin:Equatable {
        let scope:CrewdPresentationJournal.Scope
        let observation:UUID,connection:UUID
        let remoteRecipient:Bool
    }
    struct Opened:Identifiable {
        let id=UUID()
        let hint:Wire.Hint
        let origin:Origin
        let current:CrewdValue
    }
    private let origin:()->Origin?
    private let rpc:(String,CrewdValue)async throws->CrewdValue
    private let journal:()throws->CrewdPresentationJournal
    private let defaults:UserDefaults
    @Published private(set) var status:Wire.Status?
    @Published private(set) var installation:Wire.Installation?
    @Published private(set) var unknown:[CrewdPresentationJournal.Prepared]=[]
    @Published private(set) var deliveries:[Wire.Delivery]=[]
    @Published private(set) var nextDelivery:String?
    @Published private(set) var installationIDs:[String]=[]
    private var selectionResolved=false
    private var stateOrigin:Origin?
    var selectionReady:Bool{selectionResolved && stateOrigin==origin()}
    func canNavigate(_ value:Opened)->Bool{origin()==value.origin}
    @Published private(set) var busy=false
    @Published var failure:String?
    @Published var opened:Opened?
    var currentOrigin:Origin?{origin()}
    var remoteEligible:Bool{origin()?.remoteRecipient==true}
    var optedIn:Bool{guard let token=origin() else{return false};return defaults.bool(forKey:preferenceKey(token))}
    init(origin:@escaping ()->Origin?,rpc:@escaping (String,CrewdValue)async throws->CrewdValue,journal:@escaping ()throws->CrewdPresentationJournal,defaults:UserDefaults = .standard){self.origin=origin;self.rpc=rpc;self.journal=journal;self.defaults=defaults}
    private func preferenceKey(_ token:Origin)->String{let encoder=JSONEncoder();encoder.outputFormatting=[.sortedKeys];let bytes=(try? encoder.encode(token.scope)) ?? Data();return "pendingcrew.notifications.optin."+SHA256.hash(data:bytes).map{String(format:"%02x",$0)}.joined()}
    private func current(_ token:Origin)throws{guard origin()==token else{throw CrewdClientError(message:"原电脑连接或设备授权已变化；不会在新连接继续通知操作。",kind:.notSubmitted)}}
    private func source()throws->Origin{guard let token=origin() else{throw CrewdClientError(message:"当前后台尚未开放原生通知。",kind:.notSubmitted)};return token}
    func refresh()async {
        guard !busy else{return};busy=true;defer{busy=false}
        do{
            let token=try source()
            if stateOrigin != token{status=nil;installation=nil;unknown=[];installationIDs=[];deliveries=[];nextDelivery=nil;selectionResolved=false;stateOrigin=nil}
            let value=try await rpc("notification.status",.object([:]));try current(token)
            status=try .init(value);stateOrigin=token;try reload(token)
            let records=try journal().notificationHistory().filter{$0.scope==token.scope}
            installationIDs=Array(Set(records.compactMap{$0.params["installationId"].string})).sorted()
            guard installationIDs.count<=128 else{throw Wire.invalid()}
                        // Multiple original registrations require an explicit user selection;
            // timestamps, names and directory order never establish identity.
            if token.remoteRecipient,installationIDs.count==1,let id=installationIDs.first {
                try await readInstallation(id,token:token);selectionResolved=true
            }else if installationIDs.isEmpty{installation=nil;selectionResolved=true}
            else{installation=nil;selectionResolved=false}

            failure=selectionResolved ? nil:"存在多份原设备注册，请明确选择要核对的一份。"
        }catch{failure="通知状态未能核对；原请求与设备授权保留。"}
    }
    private func readInstallation(_ id:String,token:Origin)async throws{
        let found=try await rpc("notification.installationGet",.object(["installationId":.string(id)]));try current(token)
        if found == .null{installation=nil;deliveries=[];nextDelivery=nil}else{let parsed=try Wire.Installation(found);guard parsed.id==id else{throw Wire.invalid()};if installation?.id != parsed.id{deliveries=[];nextDelivery=nil};installation=parsed}
    }
    func selectInstallation(_ id:String)async throws{
        let token=try source();guard !busy,stateOrigin==token,token.remoteRecipient,installationIDs.contains(id) else{throw Wire.invalid()};busy=true;defer{busy=false}
        try await readInstallation(id,token:token);selectionResolved=true
        defaults.set(id,forKey:preferenceKey(token)+".installation")
    }
    private func reload(_ token:Origin)throws{
        let history=try journal();guard history.scope==token.scope else{throw Wire.invalid()}
        unknown=try history.unknownRecords().filter{$0.scope==token.scope && ["notification.installationRegister","notification.installationRevoke"].contains($0.method)}
    }
    func preparePermission()throws->Origin{
        let token=try source();try reload(token)
        guard !busy,token.remoteRecipient,selectionReady,unknown.isEmpty,status?.enabled==true,status?.available==true else{throw CrewdClientError(message:"需先连接有通知配置的已配对电脑，并核对原未确认请求。",kind:.notSubmitted)}
        return token
    }
    func consent(_ enabled:Bool,for token:Origin)throws{try current(token);defaults.set(enabled,forKey:preferenceKey(token))}
    func subscriptions(for token:Origin)->([String],[Wire.Kind])?{
        guard let data=defaults.data(forKey:preferenceKey(token)+".subscription"),let value=try? JSONDecoder().decode(CrewdValue.self,from:data),let crews=try? Wire.strings(value["crewIds"],maximum:64),let kinds=try? Wire.strings(value["kinds"],maximum:8).map({text->Wire.Kind in guard let k=Wire.Kind(rawValue:text) else{throw Wire.invalid()};return k}) else{return nil};return (crews,kinds)
    }
    func saveSubscription(crews:[String],kinds:[Wire.Kind],for token:Origin)throws{
        try current(token);_ = try Wire.strings(.array(crews.map(CrewdValue.string)),maximum:64);_ = try Wire.strings(.array(kinds.map{.string($0.rawValue)}),maximum:8)
        let value=CrewdValue.object(["crewIds":.array(crews.map(CrewdValue.string)),"kinds":.array(kinds.map{.string($0.rawValue)})])
        defaults.set(try JSONEncoder().encode(value),forKey:preferenceKey(token)+".subscription")
    }
    func register(token:Data,environment:Wire.Environment,topic:String,crews:[String],kinds:[Wire.Kind],expectedOrigin:Origin)async throws{
        try current(expectedOrigin)
        guard expectedOrigin.remoteRecipient,selectionReady,optedIn,!token.isEmpty,token.count<=2048,crews.count>0,crews.count<=64,Set(crews).count==crews.count,!kinds.isEmpty,Set(kinds).count==kinds.count,status?.environment==environment,status?.topic==topic else{throw Wire.invalid()}
        let old=installation
        let id=old?.revoked==false ? old!.id:UUID().uuidString.lowercased()
        var values:[String:CrewdValue]=["requestId":.string(UUID().uuidString.lowercased()),"installationId":.string(id),"expectedGeneration":.string(old?.revoked==false ? old!.generation:"0"),"environment":.string(environment.rawValue),"topic":.string(topic),"crewIds":.array(crews.map(CrewdValue.string)),"kinds":.array(kinds.map{.string($0.rawValue)})]
        values["deviceToken"] = .string(token.map{String(format:"%02x",$0)}.joined())
        var redacted=values;redacted["deviceToken"]=nil;redacted["tokenDigest"] = .string(SHA256.hash(data:token).map{String(format:"%02x",$0)}.joined())
        try await mutate("notification.installationRegister",params:.object(values),redacted:.object(redacted),token:expectedOrigin)
    }
    func revoke()async throws {
        let token=try source();try consent(false,for:token)
        guard stateOrigin==token,selectionReady,let old=installation,!old.revoked else{throw CrewdClientError(message:"没有已核对的设备通知注册；未发送撤销。",kind:.notSubmitted)}
        let params=CrewdValue.object(["requestId":.string(UUID().uuidString.lowercased()),"installationId":.string(old.id),"expectedGeneration":.string(old.generation)])
        try await mutate("notification.installationRevoke",params:params,redacted:params,token:token)
    }
    private func mutate(_ method:String,params:CrewdValue,redacted:CrewdValue,token:Origin)async throws{
        try current(token);try reload(token)
        guard token.remoteRecipient,!busy,unknown.isEmpty else{throw CrewdClientError(message:"原通知请求未确认或正在处理，请只读查询原请求。",kind:.notSubmitted)}
        busy=true;defer{busy=false}
        let owner=try journal();guard owner.scope==token.scope else{throw Wire.invalid()}
        let prepared=try owner.prepare(method:method,params:redacted,crew:params["crewIds"].array.first?.string ?? "")
        defaults.set(params["installationId"].string,forKey:preferenceKey(token)+".installation")
        do{
            let result=try await rpc(method,params)
            let parsed=try Wire.validateReceipt(result,method:method,params:redacted)
            try owner.finish(prepared,state:"replyObserved",detail:"Validated original notification installation receipt; no token retained")
            try current(token);installation=parsed;selectionResolved=true;try reload(token);failure=nil
        }catch{
            // Malformed success is Unknown; it cannot disappear after reopen.
            try? owner.finish(prepared,state:CrewdClientError.isUncertainSubmission(error) ? "unknown":"refused",detail:"Original notification request requires lookup; token redacted")
            if origin()==token{try? reload(token);failure="通知请求尚未确认；只可查原请求，不会重新注册或重发。"}
            throw error
        }
    }
    func lookup(_ record:CrewdPresentationJournal.Prepared)async throws{
        let token=try source();try current(token)
        guard !busy,record.scope==token.scope,let id=record.params["requestId"].string else{throw Wire.invalid()};busy=true;defer{busy=false}
        let value=try await rpc("notification.requestLookup",.object(["requestId":.string(id)]));try current(token)
        switch value["kind"].string {
        case "known":
            try Wire.exact(value,["kind","operation","result"]);guard value["operation"] == .string(record.method) else{throw Wire.invalid()}
            let parsed=try Wire.validateReceipt(value["result"],method:record.method,params:record.params)
            try journal().acknowledge(record,detail:"Validated original notification receipt; no resend")
            installation=parsed;selectionResolved=true;try reload(token);failure=nil
        case "unknown":
            try Wire.exact(value,["kind","operation"]);guard value["operation"] == .string(record.method) else{throw Wire.invalid()};failure="原请求仍未确认；不会再次发送。"
        case "notFound":try Wire.exact(value,["kind"]);failure="尚未找到原请求回执；这不授权重新发送。"
        default:throw Wire.invalid()
        }
    }
    func readDeliveries(more:Bool=false)async throws{
        let token=try source();guard token.remoteRecipient,stateOrigin==token,selectionReady,let install=installation,!busy else{throw Wire.invalid()};busy=true;defer{busy=false}
        var fields:[String:CrewdValue]=["installationId":.string(install.id),"limit":.integer(30)]
        if more{guard let nextDelivery else{return};fields["afterDeliveryId"] = .string(nextDelivery)}
        let value=try await rpc("notification.deliveryList",.object(fields));try current(token)
        try Wire.exact(value,["items","nextAfterDeliveryId"],optional:["nextAfterDeliveryId"])
        guard case .array(let rows)=value["items"],rows.count<=30 else{throw Wire.invalid()}
        let parsed=try rows.map(Wire.Delivery.init)
        let cursor=value["nextAfterDeliveryId"] == .null ? nil:try Wire.decimal(value["nextAfterDeliveryId"],zero:false)
        if more{guard !parsed.contains(where:{item in deliveries.contains(where:{$0.id==item.id})}) else{throw Wire.invalid()};deliveries += parsed}else{deliveries=parsed}
        nextDelivery=cursor
    }
    /// All reads stay on the same verified grant. No fallback/reconnect/command.
    @discardableResult func open(_ hint:Wire.Hint)async throws->Opened{
        let token=try source()
        guard token.remoteRecipient,token.scope.authorityID==hint.context.authorityId,token.scope.databaseID==hint.context.dbId else{throw Wire.invalid()}
        let own=try await rpc("notification.installationGet",.object(["installationId":.string(hint.installationId)]));try current(token)
        let installation=try Wire.Installation(own);guard installation.id==hint.installationId,!installation.revoked else{throw Wire.invalid()}
        let row=try await rpc("notification.deliveryGet",.object(["installationId":.string(hint.installationId),"deliveryId":.string(hint.deliveryId)]));try current(token)
        let delivery=try Wire.Delivery(row);guard delivery.id==hint.deliveryId,delivery.context==hint.context else{throw Wire.invalid()}
        let context=hint.context,source=context.source
        let crew=try await rpc("crew.get",.object(["crewId":.string(context.crewId)]));try current(token)
        guard crew["crew"]["id"] == .string(context.crewId) else{throw Wire.invalid()}
        let present:CrewdValue
        switch source["type"].string {
        case "workEvent":
            let id=try Wire.text(source["objectId"])
            if context.kind == .askOpened{
                present=try await rpc("ask.get",.object(["crewId":.string(context.crewId),"ask":.string(id)]));try current(token)
                guard present["ask"] == .string(id),present["crewId"] == .string(context.crewId) else{throw Wire.invalid()}
            }else{
                present=try await rpc("work.get",.object(["id":.string(id)]));try current(token)
                guard present["id"] == .string(id),present["crewId"] == .string(context.crewId) else{throw Wire.invalid()}
            }
        case "message":
            let channel=try Wire.text(source["channelId"])
            let channels=try await rpc("channel.list",.object(["crewId":.string(context.crewId)]));try current(token)
            guard channels.array.contains(where:{$0["channel"]["crewId"] == .string(context.crewId) && $0["channel"]["id"] == .string(channel)}) else{throw Wire.invalid()}
            let seq=Int64(try Wire.decimal(source["messageSeq"],zero:false));guard let seq,seq>0 else{throw Wire.invalid()}
            let rows=try await rpc("message.list",.object(["channelId":.string(channel),"afterSeq":.integer(seq-1),"limit":.integer(1)]));try current(token)
            guard let message=rows.array.first,message["id"]==source["messageId"],message["seq"].int==Int(exactly:seq),message["channelId"]==source["channelId"] else{throw Wire.invalid()};present=message
        case "codexTurn","managedResponse","decision":
            let id=try Wire.text(source["sessionId"])
            let session=try await rpc("session.get",.object(["sessionId":.string(id)]));try current(token)
            guard session["id"] == .string(id),session["crewId"] == .string(context.crewId) else{throw Wire.invalid()}
            present=try await rpc("session.inspect",.object(["sessionId":.string(id),"maxLines":.integer(80)]));try current(token)
            guard present["live"]["sessionId"]==source["sessionId"] else{throw Wire.invalid()}
        default:throw Wire.invalid()
        }
        try current(token);let result=Opened(hint:hint,origin:token,current:present);opened=result;return result
    }
}
