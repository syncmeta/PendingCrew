import Foundation
import Combine
import CryptoKit

/// Closed PCW-owned observations. No provider transcript or execution authority.
enum CrewdClaudeWire {
    static func exact(_ value:CrewdValue,_ keys:Set<String>)throws{guard case .object(let fields)=value,Set(fields.keys)==keys else{throw CrewdPublicWire.invalid()}}
    static func text(_ value:CrewdValue,max:Int=4096)throws->String{guard let s=value.string,!s.isEmpty,s.utf8.count<=max else{throw CrewdPublicWire.invalid()};return s}
    static func optionalText(_ value:CrewdValue,max:Int=4096)throws{guard value == .null || value.string.map({$0.utf8.count<=max}) == true else{throw CrewdPublicWire.invalid()}}
    static func decimal(_ value:CrewdValue)throws->UInt64{let s=try text(value,max:20);guard s=="0" || s.first != "0",s.utf8.allSatisfy({(48...57).contains($0)}),let n=UInt64(s) else{throw CrewdPublicWire.invalid()};return n}
    static func uuid(_ value:CrewdValue)throws{let s=try text(value);guard UUID(uuidString:s)?.uuidString.lowercased()==s else{throw CrewdPublicWire.invalid()}}
    static func flag(_ value:CrewdValue)throws{_ = try CrewdPublicWire.boolean(value)}
    static func inspect(_ value:CrewdValue,session:String,crew:String)throws->CrewdValue{
        try exact(value,["sessionId","crewId","executorProfileId","backend","historyScope","fullHistoryLoad","nativeAcp","manualResume","currentRuntimeId","taskTextLimitBytes","responseObservationLimitBytes","retainedObservationsPerTask"])
        guard value["sessionId"] == .string(session),value["crewId"] == .string(crew),value["executorProfileId"] == .string("claude"),value["backend"] == .string("existingClaudePty"),value["historyScope"] == .string("pcwManagedTasks"),value["fullHistoryLoad"] == .bool(false),value["nativeAcp"] == .bool(false),value["taskTextLimitBytes"].int==1048576,value["responseObservationLimitBytes"].int==262144,value["retainedObservationsPerTask"].int==4 else{throw CrewdPublicWire.invalid()}
        try optionalText(value["currentRuntimeId"])
        let resume=value["manualResume"];try exact(resume,["eligibility","originalAgentSessionId","rechecksProgramAndSourceOnLaunch"])
        try CrewdCodexWire.choice(resume["eligibility"],["eligible","running","notConfigured","missingOriginalId","previousBirthLiveOrUnknown","removedCrew","missingWorkdir"])
        guard resume["rechecksProgramAndSourceOnLaunch"] == .bool(true) else{throw CrewdPublicWire.invalid()}
        if resume["originalAgentSessionId"] != .null{try uuid(resume["originalAgentSessionId"])}
        guard resume["eligibility"] != .string("eligible") || resume["originalAgentSessionId"].string != nil else{throw CrewdPublicWire.invalid()};return value
    }
    static func summary(_ value:CrewdValue)throws{
        try exact(value,["requestId","runtimeId","createdAt","updatedAt","phase","receiptRevision","promptId","observationCount","omittedObservations","latestObservedAt","waitingObserved","currentRuntime"])
        try uuid(value["requestId"]);_ = try text(value["runtimeId"]);_ = try text(value["createdAt"]);_ = try text(value["updatedAt"])
        try CrewdCodexWire.choice(value["phase"],["prepared","content_accepted","running","cancel_requested","raw_revoked","unknown","completed","aborted","failed"])
        _ = try decimal(value["receiptRevision"]);let count=try decimal(value["observationCount"]),omitted=try decimal(value["omittedObservations"]);guard omitted<=count else{throw CrewdPublicWire.invalid()}
        try optionalText(value["promptId"]);try optionalText(value["latestObservedAt"]);try flag(value["waitingObserved"]);try flag(value["currentRuntime"])
        guard value["waitingObserved"] != .bool(true) || (value["currentRuntime"] == .bool(true) && value["phase"] == .string("completed")) else{throw CrewdPublicWire.invalid()}
    }
    static func page(_ value:CrewdValue,session:String,cut:String?)throws->CrewdValue{
        try exact(value,["sessionId","tasks","nextCursor","membershipCut","observedAt"])
        guard value["sessionId"] == .string(session),case .array(let rows)=value["tasks"],rows.count<=64 else{throw CrewdPublicWire.invalid()}
        try rows.forEach(summary);guard Set(rows.compactMap{$0["requestId"].string}).count==rows.count else{throw CrewdPublicWire.invalid()}
        _ = try decimal(value["membershipCut"]);_ = try text(value["observedAt"]);try optionalText(value["nextCursor"],max:64*1024)
        guard cut==nil || value["membershipCut"] == cut.map(CrewdValue.string) else{throw CrewdPublicWire.invalid()};return value
    }
    static func observation(_ value:CrewdValue,head:Bool)throws{
        try exact(value,["observationNo","promptId","observedAt","text","sourceBytes","capturedBytes","sourceSha256","availability"])
        _ = try decimal(value["observationNo"]);_ = try text(value["promptId"]);_ = try text(value["observedAt"])
        let captured=try decimal(value["capturedBytes"]);guard captured<=262144 else{throw CrewdPublicWire.invalid()}
        try CrewdCodexWire.choice(value["availability"],["observed","missing","truncated","retentionLimit","observationLimit"])
        try optionalText(value["text"],max:262144)
        if head{guard value["text"] == .null else{throw CrewdPublicWire.invalid()}}
        if let text=value["text"].string{guard text.utf8.count==captured else{throw CrewdPublicWire.invalid()}}
        if value["sourceBytes"] == .null{guard value["sourceSha256"] == .null,["missing","retentionLimit","observationLimit"].contains(value["availability"].string ?? ""),value["text"] == .null,captured==0 else{throw CrewdPublicWire.invalid()}}
        else{let bytes=try decimal(value["sourceBytes"]),hash=try text(value["sourceSha256"],max:64);guard bytes>=captured,hash.utf8.count==64,hash.utf8.allSatisfy({(48...57).contains($0)||(97...102).contains($0)}) else{throw CrewdPublicWire.invalid()}
            if !head,value["availability"] == .string("observed"){guard let text=value["text"].string,UInt64(text.utf8.count)==bytes,SHA256.hash(data:Data(text.utf8)).map({String(format:"%02x",$0)}).joined()==hash else{throw CrewdPublicWire.invalid()}}
        }
    }
    static func detail(_ value:CrewdValue,session:String,request:String)throws->CrewdValue{
        try exact(value,["sessionId","task","originalTask","observations","latestObservation","waitingObservationNo","observedAt"])
        guard value["sessionId"] == .string(session),value["task"]["requestId"] == .string(request),case .array(let observations)=value["observations"],observations.count<=4 else{throw CrewdPublicWire.invalid()};try summary(value["task"])
        let original=value["originalTask"];try exact(original,["text","managedEnvelope","fidelity"]);try CrewdCodexWire.choice(original["fidelity"],["authorizedTask","exactManagedEnvelope"]);try optionalText(original["text"],max:1048576);try optionalText(original["managedEnvelope"],max:2*1048576)
        guard original["text"] == .null || (original["managedEnvelope"] == .null && original["fidelity"] == .string("authorizedTask")),original["managedEnvelope"] == .null || (original["text"] == .null && original["fidelity"] == .string("exactManagedEnvelope")) else{throw CrewdPublicWire.invalid()}
        for item in observations{try observation(item,head:false)};guard Set(observations.compactMap{$0["observationNo"].string}).count==observations.count else{throw CrewdPublicWire.invalid()}
        if value["latestObservation"] != .null{try observation(value["latestObservation"],head:true)}
        if value["waitingObservationNo"] != .null{_ = try decimal(value["waitingObservationNo"])}
        _ = try text(value["observedAt"]);return value
    }
}

/// One disposable source-bound read projection; never a transcript or retry cache.
@MainActor final class CrewdClaudeHistoryStore:ObservableObject {
    struct Origin:Equatable{let generation:UUID;let connection:UUID;let authority:CrewdValue;let runtime:CrewdValue;let crew:String}
    struct ResumeConfirmation{let origin:Origin;let target:CrewdCommandTarget;let nativeID:String}
    typealias Call=(String,CrewdValue)async throws->CrewdValue
    let sessionID:String,crewID:String
    private let origin:()->Origin?
    private let read:Call,perform:Call
    private let unknown:()->Bool
    @Published private(set) var inspection:CrewdValue = .null
    @Published private(set) var tasks:[CrewdValue]=[]
    @Published private(set) var detail:CrewdValue = .null
    @Published private(set) var cursor:String?
    @Published private(set) var cut:String?
    @Published private(set) var observedAt:String?
    @Published private(set) var failure:String?
    @Published private(set) var unsupported=false
    @Published private(set) var reading=false
    @Published private(set) var resuming=false
    @Published private(set) var selectedRequest:String?
    private var retired=false
    private var selection=UUID(),pageRequest=UUID(),inspectRequest=UUID()
    init(session:String,crew:String,origin:@escaping()->Origin?,read:@escaping Call,perform:@escaping Call,unknown:@escaping()->Bool){sessionID=session;crewID=crew;self.origin=origin;self.read=read;self.perform=perform;self.unknown=unknown}
    private func captured()throws->Origin{guard !retired,let value=origin(),value.crew==crewID else{throw CancellationError()};return value}
    private func current(_ value:Origin)throws{guard try captured()==value else{throw CancellationError()}}
    var resumeEligible:Bool{inspection["manualResume"]["eligibility"] == .string("eligible") && !unknown() && !resuming}
    var currentOrigin:Origin?{origin()}
    func inspect()async{
        let expected=origin();let request=UUID();inspectRequest=request
        do{let token=try captured();let raw=try await read("session.claude.inspect",.object(["sessionId":.string(sessionID)]));try current(token);guard inspectRequest==request else{return};inspection=try CrewdClaudeWire.inspect(raw,session:sessionID,crew:crewID);unsupported=false;failure=nil}
        catch{guard !retired,origin()==expected,inspectRequest==request else{return};inspection = .null;unsupported=(error as? CrewdClientError)?.code == -32601;failure=unsupported ? "此Core未提供PCW受管历史；没有推测原会话恢复资格。":"受管状态未能核对；尚未启动或恢复。"}
    }
    func page(more:Bool=false)async{
        guard !reading else{return};let expected=origin();reading=true;let request=UUID();pageRequest=request;defer{if pageRequest==request{reading=false}}
        do{let token=try captured();if more,cursor==nil{return};let next=more ? cursor:nil,originalCut=more ? cut:nil
            let raw=try await read("session.claude.history",.object(["sessionId":.string(sessionID),"cursor":next.map(CrewdValue.string) ?? .null,"limit":.integer(32)]));try current(token);guard pageRequest==request else{return}
            let parsed=try CrewdClaudeWire.page(raw,session:sessionID,cut:originalCut),rows=parsed["tasks"].array
            guard !more || Set(tasks.compactMap{$0["requestId"].string}).isDisjoint(with:rows.compactMap{$0["requestId"].string}) else{throw CrewdPublicWire.invalid()}
            tasks=more ? tasks+rows:rows;cursor=parsed["nextCursor"].string;cut=parsed["membershipCut"].string;observedAt=parsed["observedAt"].string;failure=nil
        }catch{if !retired,origin()==expected,pageRequest==request{failure="原任务页未能核对；可明确从首页刷新，不会换源或恢复会话。"}}
    }
    func select(_ request:String)async{
        let expected=origin();selection=UUID();let id=selection;selectedRequest=request;detail = .null
        do{let token=try captured();let raw=try await read("session.claude.taskGet",.object(["sessionId":.string(sessionID),"requestId":.string(request)]));try current(token);guard selection==id,selectedRequest==request else{return};detail=try CrewdClaudeWire.detail(raw,session:sessionID,request:request);failure=nil}
        catch{if !retired,origin()==expected,selection==id{failure="原任务正文不可用或当前没有读取权限；未拼接缺失正文。"}}
    }
    func prepareResume()async throws->ResumeConfirmation{
        let token=try captured();guard !unknown() else{throw CrewdPublicWire.invalid()}
        let raw=try await read("session.claude.inspect",.object(["sessionId":.string(sessionID)]));try current(token);let checked=try CrewdClaudeWire.inspect(raw,session:sessionID,crew:crewID)
        guard checked["manualResume"]["eligibility"] == .string("eligible"),let nativeID=checked["manualResume"]["originalAgentSessionId"].string else{throw CrewdClientError(message:"原会话当前不具备恢复资格；未启动。",kind:.notSubmitted)}
        let target=try CrewdCommandTarget(await read("session.controlTarget",.object(["sessionId":.string(sessionID)])));try current(token);guard target.sessionID==sessionID else{throw CrewdPublicWire.invalid()}
        inspection=checked;return .init(origin:token,target:target,nativeID:nativeID)
    }
    func resume(_ confirmation:ResumeConfirmation)async{
        guard !resuming else{return};resuming=true;defer{resuming=false}
        do{try current(confirmation.origin);guard !unknown() else{throw CrewdPublicWire.invalid()};let fresh=try await prepareResume();guard fresh.origin==confirmation.origin,fresh.target==confirmation.target,fresh.nativeID==confirmation.nativeID else{throw CrewdClientError(message:"确认期间原会话或程序资格已改变；未重发恢复。",kind:.notSubmitted)}
            _ = try await perform("session.resume",confirmation.target.value);try current(confirmation.origin);failure=nil;await inspect()
        }catch{if !retired,origin()==confirmation.origin{failure="原恢复未确认或被拒绝；只核对原UUID，不创建替代会话或重送旧任务。"}}
    }
    func invalidate(){inspection = .null;tasks=[];detail = .null;cursor=nil;cut=nil;observedAt=nil;selectedRequest=nil;selection=UUID();pageRequest=UUID();inspectRequest=UUID();reading=false;unsupported=false;failure=nil}
    func retire(){invalidate();retired=true}
}
