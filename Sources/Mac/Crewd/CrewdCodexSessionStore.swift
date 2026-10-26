import Foundation
import Combine

/// A projection of one exact Core session. It owns no runner or replay queue.
@MainActor final class CrewdCodexSessionStore:ObservableObject {
    @Published private(set) var inspection:CrewdCodexInspection?
    @Published private(set) var history:[CrewdValue]=[]
    @Published private(set) var historyCursor:String?
    @Published private(set) var models:[CrewdValue]=[]
    @Published private(set) var tools:CrewdValue = .null
    @Published private(set) var usage:[CrewdValue]=[]
    @Published private(set) var usageCursor:String?
    private var usageLoaded=false
    enum ReadState:Equatable { case notRead, waitingForSource, loading, loaded, failed }
    @Published private(set) var readState:ReadState = .notRead
    @Published private(set) var busy=false
    @Published private(set) var error:String?
    @Published private(set) var lastResult:CrewdValue = .null
    typealias Call=(String,CrewdValue)async throws->CrewdValue
    private let readValue:Call
    private let performValue:Call
    private let sourceScope:()->String?
    private let available:()->Bool
    let sessionID:String
    private var subscription:AnyCancellable?
    private var refreshID:UUID?
    private var refreshingScope:String?
    private var inspectionScope:String?
    private var invalidated=false
    init(sessionID:String,read:@escaping Call,submit:@escaping Call,scope:@escaping()->String?,available:@escaping()->Bool){
        self.sessionID=sessionID;readValue=read;performValue=submit;sourceScope=scope;self.available=available
    }
    func observe(_ publisher:PassthroughSubject<(String,CrewdValue),Never>){
        subscription=publisher.sink{[weak self] name,data in
            guard let self else{return}
            if name == "events.lagged" || (name.hasPrefix("session.") && data["sessionId"] == .string(self.sessionID)){Task{await self.refresh();if self.usageLoaded{await self.read("usage")}}}
        }
    }
    var protocolAvailable:Bool{available()}
    var readScope:String?{!invalidated && available() ? sourceScope():nil}
    var supported:Bool{readScope != nil}
    private func current(_ scope:String?)->Bool{!invalidated && scope != nil && sourceScope() == scope}
    func refresh()async {
        guard !invalidated else{return}
        guard let scope=readScope else{
            refreshID=nil;refreshingScope=nil;inspectionScope=nil
            inspection=nil;error=nil;readState = .waitingForSource
            return
        }
        guard refreshingScope != scope else{return}
        let id=UUID();refreshID=id;refreshingScope=scope
        if inspectionScope != scope{inspection=nil;error=nil}
        readState = .loading
        defer{
            if refreshID==id{
                refreshingScope=nil
                if readState == .loading{readState = .waitingForSource}
            }
        }
        do{
            try Task.checkCancellation()
            let raw=try await readValue("session.codex.inspect",.object(["sessionId":.string(sessionID)]))
            try Task.checkCancellation()
            let parsed=try CrewdCodexInspection(raw,session:sessionID)
            guard current(scope),refreshID==id else{return}
            inspection=parsed;inspectionScope=scope;error=nil;readState = .loaded
        }catch{if current(scope),refreshID==id,!Task.isCancelled{self.error=error.localizedDescription;readState = .failed}}
    }
    func read(_ kind:String,more:Bool=false)async {
        guard supported,!busy else{return}
        let capability=kind == "usage" ? "history":kind
        guard inspection?.capability(capability)?.available == true else{return}
        busy=true;defer{busy=false}
        let scope=sourceScope()
        var params:[String:CrewdValue]=["sessionId":.string(sessionID)]
        if kind == "history"{params["itemsView"] = .string("full");params["sortDirection"] = .string("asc");params["limit"] = .integer(30);if more,let historyCursor{params["cursor"] = .string(historyCursor)}}
        if kind == "usage"{params["limit"] = .integer(100);if more,let usageCursor{params["after"] = .string(usageCursor)}}
        do{
            let raw=try await readValue(kind == "usage" ? "session.usage":"session.codex."+kind,.object(params))
            guard current(scope) else{return}
            switch kind {
            case "history":
                guard raw["sessionId"] == .string(sessionID),raw["threadId"] == inspection?.value["threadId"],raw["threadId"].string != nil,case .array(let rows)=raw["items"] else{throw CrewdPublicWire.invalid()}
                history=more ? history+rows:rows;historyCursor=raw["nextCursor"].string
            case "models":guard case .array(let rows)=raw["items"] else{throw CrewdPublicWire.invalid()};models=rows
            case "tools":guard case .array=raw["mcpServers"],case .array=raw["skills"] else{throw CrewdPublicWire.invalid()};tools=raw
            case "usage":guard case .array(let rows)=raw["items"],rows.allSatisfy({$0["sessionId"] == .string(sessionID)}) else{throw CrewdPublicWire.invalid()};usage=more ? usage+rows:rows;usageCursor=raw["nextAfter"].string;usageLoaded=true
            default:throw CrewdPublicWire.invalid()
            }
            error=nil
        }catch{if current(scope){self.error=error.localizedDescription}}
    }
    /// The explicit control retains its inspected CAS target. A stale target is
    /// rejected by Core; it is never silently refreshed and replayed.
    @discardableResult func submit(_ operation:CrewdPublicOperation,fields:[String:CrewdValue]=[:],target:CrewdCommandTarget?=nil)async -> Bool {
        guard !busy,let inspection,supported else{return false}
        let id=operation == .codexTitle ? "title":String(operation.rawValue.dropFirst("session.codex.".count))
        guard inspection.capability(id)?.available == true else{return false}
        busy=true;defer{busy=false}
        let scope=sourceScope()
        let exact=target ?? inspection.target
        guard exact.sessionID == sessionID else{return false}
        var params=fields;params["target"]=exact.value
        do{
            let raw=try await readValue("session.controlTarget",.object(["sessionId":.string(sessionID)]))
            let fresh=try CrewdCommandTarget(raw)
            guard current(scope),fresh.sessionID==sessionID else{return false}
            // Confirmation preserves its exact target; steer never redirects to a new turn.
            guard target==nil || fresh==exact else{throw CrewdClientError(message:"确认期间原会话代际变化；请重新核对，不会自动重发。",kind:.notSubmitted)}
            if operation == .codexSteer{guard fresh.generation==exact.generation,fresh.turnID==exact.turnID,fresh.turnID != nil else{throw CrewdPublicWire.invalid()}}
            params["target"]=fresh.value
            let value=try await performValue(operation.rawValue,.object(params))
            guard current(scope) else{return false}
            lastResult=value;error=nil
            await refresh();return true
        }catch{if current(scope){self.error=error.localizedDescription};return false}
    }
    func retire(){invalidated=true;refreshID=nil;refreshingScope=nil;inspectionScope=nil;readState = .waitingForSource;subscription?.cancel();subscription=nil;inspection=nil;history=[];models=[];tools = .null;usage=[]}
}
