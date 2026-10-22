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
    private var refreshing=false
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
    var supported:Bool{!invalidated && available() && sourceScope() != nil}
    private func current(_ scope:String?)->Bool{!invalidated && scope != nil && sourceScope() == scope}
    func refresh()async {
        guard !refreshing,supported else{return}
        refreshing=true;defer{refreshing=false}
        let scope=sourceScope()
        do{
            let raw=try await readValue("session.codex.inspect",.object(["sessionId":.string(sessionID)]))
            let parsed=try CrewdCodexInspection(raw,session:sessionID)
            guard current(scope) else{return}
            inspection=parsed;error=nil
        }catch{if current(scope){self.error=error.localizedDescription}}
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
    func submit(_ operation:CrewdPublicOperation,fields:[String:CrewdValue]=[:],target:CrewdCommandTarget?=nil)async {
        guard !busy,let inspection,supported else{return}
        let id=operation == .codexTitle ? "title":String(operation.rawValue.dropFirst("session.codex.".count))
        guard inspection.capability(id)?.available == true else{return}
        busy=true;defer{busy=false}
        let scope=sourceScope()
        let exact=target ?? inspection.target
        guard exact.sessionID == sessionID else{return}
        var params=fields;params["target"]=exact.value
        do{
            let value=try await performValue(operation.rawValue,.object(params))
            guard current(scope) else{return}
            lastResult=value;error=nil
            await refresh()
        }catch{if current(scope){self.error=error.localizedDescription}}
    }
    func retire(){invalidated=true;subscription?.cancel();subscription=nil;inspection=nil;history=[];models=[];tools = .null;usage=[]}
}
