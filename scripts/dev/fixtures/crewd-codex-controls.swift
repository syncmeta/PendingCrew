import Foundation
@main struct CodexClientWireRegression {
 @MainActor static func main()async throws {
  let target:CrewdValue = .object(["sessionId":.string("source"),"generation":.string("1"),"controlRevision":.string("2"),"turnId":.null])
  let binding:CrewdValue = .object(["dbId":.string("db"),"runtimeBindingId":.string("runtime")])
  for name in ["configure","compact","steer","queue","review","titleSet","fork"] {
   guard let operation=CrewdPublicOperation(rawValue:"session.codex."+name) else{fputs("FAIL original public command seam rejects Codex control "+name+"\n",stderr);exit(5)}
   var fields:[String:CrewdValue]=["target":target]
   switch name {
    case "configure":fields["patch"] = .object(["fast":.bool(false)])
    case "steer","queue":fields["text"] = .string("  原任务 Todo\n")
    case "review":fields["reviewTarget"] = .object(["type":.string("uncommittedChanges")])
    case "titleSet":fields["title"] = .string("原任务标题")
    default:break
   }
   let envelope=try CrewdPublicCommandEnvelope(request:UUID(),binding:binding,operation:operation,params:.object(fields))
   precondition(envelope.params["target"] == target)
   if name == "steer" || name == "queue"{precondition(envelope.params["text"] == .string("  原任务 Todo\n"))}
   if name == "fork"{precondition(envelope.params["lastTurnId"] == .null && envelope.params["title"] == .null)}
   do{fields["providerMethod"] = .string("arbitrary");_ = try CrewdPublicCommandEnvelope(request:UUID(),binding:binding,operation:operation,params:.object(fields));preconditionFailure("unknown public field accepted")}catch{}
  }
  let originalBody="  原任务 Todo\n"
  let start:CrewdValue = .object(["crewId":.string("crew"),"role":.string("worker"),"executor":.string("codex"),"brief":.string(originalBody),"isolation":.bool(false)])
  let unchanged=try CrewdPublicCommandEnvelope(request:UUID(),binding:binding,operation:.sessionStart,params:start)
  precondition(unchanged.params["brief"] == .string(originalBody))
  if case .object(let fields)=unchanged.params{precondition(fields["codexOptions"] == nil,"absent new options preserves old API3 shape")}
  var configured=start
  if case .object(var fields)=configured{fields["codexOptions"] = .object(["fast":.bool(false)]);configured = .object(fields)}
  let initial=try CrewdPublicCommandEnvelope(request:UUID(),binding:binding,operation:.sessionStart,params:configured)
  precondition(initial.params["codexOptions"]["fast"] == .bool(false))
  let command:CrewdValue = .object(["operation":.string("session.codex.queue"),"params":.object(["target":target,"text":.string(originalBody)])])
  let queue:CrewdValue = .object(["operation":.string("session.codex.queue"),"sessionId":.string("source"),"providerAccepted":.bool(false),"application":.string("queued")])
  try CrewdCodexWire.validateResult(queue,command:command)
  for bad:CrewdValue in [.object(["sessionId":.string("other")]),.object(["operation":.string("session.codex.queue"),"sessionId":.string("other"),"providerAccepted":.bool(true),"application":.string("queued")])]{
   do{try CrewdCodexWire.validateResult(bad,command:command);preconditionFailure("uncorrelated control result accepted")}catch{}
  }
  let capability:CrewdValue = .object(["id":.string("queue"),"availability":.string("available"),"protocol":.string("stable")])
  let historyCapability:CrewdValue = .object(["id":.string("history"),"availability":.string("available"),"protocol":.string("stable")])
  let inspection:CrewdValue = .object(["sessionId":.string("source"),"threadId":.string("native-source"),"target":target,"running":.bool(true),"working":.bool(false),"historical":.bool(false),"queuedInputs":.integer(0),"title":.object(["source":.string("task"),"status":.string("synced")]),"capabilities":.array([capability,historyCapability])])
  var scope:String?="source-scope",reads:[String]=[],effects:[CrewdValue]=[]
  let store=CrewdCodexSessionStore(sessionID:"source",read:{method,params in
   reads.append(method)
   precondition(params["sessionId"] == .string("source"))
   if method == "session.codex.inspect"{return inspection}
   scope="different-source"
   return .object(["sessionId":.string("source"),"threadId":.string("native-source"),"items":.array([.object(["id":.string("must-not-appear")])])])
  },submit:{method,params in effects.append(params);precondition(method == "session.codex.queue");return queue},scope:{scope},available:{true})
  await store.refresh()
  precondition(store.inspection?.sessionID == "source" && effects.isEmpty,"opening inspect cannot execute")
  await store.submit(.codexQueue,fields:["text":.string(originalBody)])
  precondition(effects.count == 1 && effects[0]["text"] == .string(originalBody) && effects[0]["target"] == target)
  await store.read("history")
  precondition(store.history.isEmpty,"late history from an old source cannot populate new source")
  scope=nil
  await store.submit(.codexQueue,fields:["text":.string("blocked")])
  precondition(effects.count == 1,"unqualified source cannot execute")
  store.retire();scope="source-scope";await store.refresh()
  precondition(store.inspection == nil,"retired projection cannot repopulate")
  print("PASS real Codex store read-only open, exact original target/body, late-source suppression, retirement and no replay")
  print("PASS original closed Codex command envelope, canonical optional fields and unchanged task body")
 }
}
