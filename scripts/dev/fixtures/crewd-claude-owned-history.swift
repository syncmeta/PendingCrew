import Foundation
import CryptoKit
func o(_ f:[String:CrewdValue])->CrewdValue{.object(f)}
func s(_ value:String)->CrewdValue{.string(value)}
func set(_ value:CrewdValue,_ key:String,_ item:CrewdValue)->CrewdValue{guard case .object(var f)=value else{fatalError()};f[key]=item;return o(f)}
@MainActor func denied(_ action:()async throws->Void)async{do{try await action();fatalError("expected rejection")}catch{}}
@MainActor final class Rig{
 let request=UUID().uuidString.lowercased(),native=UUID().uuidString.lowercased()
 var token:CrewdClaudeHistoryStore.Origin? = .init(generation:UUID(),connection:UUID(),authority:s("authority+grant"),runtime:s("db+binding"),crew:"crew")
 var calls:[String]=[],writes:[(String,CrewdValue)]=[],blocked=false
 var route:((String,CrewdValue)async throws->CrewdValue)?
 lazy var store=make()
 var target:CrewdValue{o(["sessionId":s("session"),"generation":s("1"),"controlRevision":s("1"),"turnId":.null])}
 func make()->CrewdClaudeHistoryStore{.init(session:"session",crew:"crew",origin:{self.token},read:{m,p in self.calls.append(m);if let route=self.route{return try await route(m,p)};return try self.result(m,p)},perform:{m,p in self.writes.append((m,p));return o(["accepted":.bool(true)])},unknown:{self.blocked})}
 var inspect:CrewdValue{o(["sessionId":s("session"),"crewId":s("crew"),"executorProfileId":s("claude"),"backend":s("existingClaudePty"),"historyScope":s("pcwManagedTasks"),"fullHistoryLoad":.bool(false),"nativeAcp":.bool(false),"manualResume":o(["eligibility":s("eligible"),"originalAgentSessionId":s(native),"rechecksProgramAndSourceOnLaunch":.bool(true)]),"currentRuntimeId":.null,"taskTextLimitBytes":.integer(1048576),"responseObservationLimitBytes":.integer(262144),"retainedObservationsPerTask":.integer(4)])}
 var summary:CrewdValue{o(["requestId":s(request),"runtimeId":s("runtime"),"createdAt":s("2026-10-23T00:00:00Z"),"updatedAt":s("2026-10-23T00:00:00Z"),"phase":s("completed"),"receiptRevision":s("2"),"promptId":s("prompt"),"observationCount":s("1"),"omittedObservations":s("0"),"latestObservedAt":s("2026-10-23T00:00:00Z"),"waitingObserved":.bool(false),"currentRuntime":.bool(false)])}
 func observation(_ text:String?=" exact body \n",availability:String="observed")->CrewdValue{let data=text.map{Data($0.utf8)};return o(["observationNo":s("1"),"promptId":s("prompt"),"observedAt":s("2026-10-23T00:00:00Z"),"text":text.map(CrewdValue.string) ?? .null,"sourceBytes":data.map{s(String($0.count))} ?? .null,"capturedBytes":s(String(data?.count ?? 0)),"sourceSha256":data.map{s(SHA256.hash(data:$0).map{String(format:"%02x",$0)}.joined())} ?? .null,"availability":s(availability)])}
 var page:CrewdValue{o(["sessionId":s("session"),"tasks":.array([summary]),"nextCursor":.null,"membershipCut":s("12"),"observedAt":s("2026-10-23T00:00:00Z")])}
 var detail:CrewdValue{o(["sessionId":s("session"),"task":summary,"originalTask":o(["text":s("  original task \n[managed delimiters preserved]"),"managedEnvelope":.null,"fidelity":s("authorizedTask")]),"observations":.array([observation()]),"latestObservation":set(observation(),"text",.null),"waitingObservationNo":s("1"),"observedAt":s("2026-10-23T00:00:00Z")])}
 func result(_ method:String,_ params:CrewdValue)throws->CrewdValue{switch method{case "session.claude.inspect":return inspect;case "session.claude.history":return page;case "session.claude.taskGet":return detail;case "session.controlTarget":return target;default:throw CrewdPublicWire.invalid()}}
}
@main struct Tests{
 @MainActor static func main()async throws{
 let mode=CommandLine.arguments.last!,r=Rig()
 switch mode{
 case "independent":
  let policy=OriginalWorkspaceSlice(),scope=CrewdPresentationJournal.Scope(namespace:UUID(),localDirectory:nil,authorityID:"authority",databaseID:"database",grantID:"grant",grantRevision:"1")
  policy.durableUnknown=[.init(format:1,id:UUID(),scope:scope,method:"command.submit",params:o(["command":o(["operation":s("session.resume"),"params":o(["sessionId":s("member-a")])])]),crew:"crew",preparedAt:Date())]
  precondition(policy.hasOriginalUnknown(method:"session.resume",crew:"crew",session:"member-a"))
  r.blocked=policy.hasOriginalUnknown(method:"session.resume",crew:"crew",session:r.store.sessionID)
  await r.store.page();let confirmation=try await r.store.prepareResume();await r.store.resume(confirmation)
  precondition(r.store.tasks.count==1 && r.writes.count==1 && r.writes[0].1["sessionId"]==s("session"))
  policy.durableUnknown.append(.init(format:1,id:UUID(),scope:scope,method:"session.resume",params:o([:]),crew:"crew",preparedAt:Date()))
  r.blocked=policy.hasOriginalUnknown(method:"session.resume",crew:"crew",session:r.store.sessionID)
  await denied{_ = try await r.store.prepareResume()}
  await r.store.page();precondition(r.store.tasks.count==1 && r.writes.count==1)
 case "readonly":
  await r.store.inspect();await r.store.page();await r.store.select(r.request)
  precondition(r.store.tasks.count==1 && r.store.detail["originalTask"]["text"]==r.detail["originalTask"]["text"] && r.writes.isEmpty)
  precondition(r.calls==["session.claude.inspect","session.claude.history","session.claude.taskGet"])
 case "wrappers":
  await denied{_ = try CrewdClaudeWire.inspect(o(["state":s("known"),"result":r.inspect]),session:"session",crew:"crew")}
  await denied{_ = try CrewdClaudeWire.page(o(["result":r.page]),session:"session",cut:nil)}
  await denied{_ = try CrewdClaudeWire.detail(set(r.detail,"sessionId",s("other")),session:"session",request:r.request)}
 case "nullable":
  guard case .object(var f)=r.inspect else{fatalError()};f.removeValue(forKey:"currentRuntimeId")
  await denied{_ = try CrewdClaudeWire.inspect(o(f),session:"session",crew:"crew")}
  await denied{_ = try CrewdClaudeWire.inspect(set(r.inspect,"manualResume",set(r.inspect["manualResume"],"originalAgentSessionId",.null)),session:"session",crew:"crew")}
 case "bodies":
  try CrewdClaudeWire.observation(r.observation(""),head:false)
  try CrewdClaudeWire.observation(r.observation(nil,availability:"missing"),head:false)
  let truncated=set(set(set(r.observation("中"),"availability",s("truncated")),"sourceBytes",s("9")),"sourceSha256",s(String(repeating:"a",count:64)))
  try CrewdClaudeWire.observation(truncated,head:false)
  await denied{try CrewdClaudeWire.observation(set(truncated,"capturedBytes",s("2")),head:false)}
  try CrewdClaudeWire.observation(set(r.observation(nil,availability:"missing"),"availability",s("observationLimit")),head:true)
  let legacy=o(["text":.null,"managedEnvelope":s("EXACT [NOT SPLIT]"),"fidelity":s("exactManagedEnvelope")]);_ = try CrewdClaudeWire.detail(set(r.detail,"originalTask",legacy),session:"session",request:r.request)
 case "cut":
  r.route={m,p in if m=="session.claude.history"{return set(r.page,"nextCursor",s("originalCursor"))};return try r.result(m,p)}
  await r.store.page();precondition(r.store.cut=="12")
  r.route={m,p in precondition(p["cursor"] == s("originalCursor"));return set(r.page,"membershipCut",s("13"))}
  await r.store.page(more:true);precondition(r.store.tasks.count==1 && r.store.cut=="12" && r.store.failure != nil && r.writes.isEmpty)
 case "source":
  r.route={m,p in let value=try r.result(m,p);r.token = .init(generation:UUID(),connection:UUID(),authority:s("another"),runtime:s("db"),crew:"crew");return value}
  await r.store.inspect();precondition(r.store.inspection == .null && r.store.failure==nil)
  await r.store.page();precondition(r.store.tasks.isEmpty && r.store.failure==nil)
 case "selection":
  let other=UUID().uuidString.lowercased();var continuation:CheckedContinuation<CrewdValue,Error>?
  r.route={m,p in if m=="session.claude.taskGet",p["requestId"]==s(r.request){return try await withCheckedThrowingContinuation{continuation=$0}};return set(set(r.detail,"task",set(r.summary,"requestId",s(other))),"originalTask",o(["text":s("second selection"),"managedEnvelope":.null,"fidelity":s("authorizedTask")]))}
  let first=Task{await r.store.select(r.request)};while continuation==nil{await Task.yield()};await r.store.select(other);continuation?.resume(returning:r.detail);await first.value
  precondition(r.store.selectedRequest==other && r.store.detail["originalTask"]["text"]==s("second selection"))
 case "resume":
  let confirmation=try await r.store.prepareResume();precondition(r.writes.isEmpty);await r.store.resume(confirmation)
  precondition(r.writes.count==1 && r.writes[0].0=="session.resume" && r.writes[0].1==r.target && r.writes[0].1["brief"] == .null)
 case "stale","eligibility","native","resumeScope":
  let confirmation=try await r.store.prepareResume()
  r.route={m,p in if m=="session.controlTarget"{return mode=="stale" ? set(r.target,"controlRevision",s("2")):r.target};if mode=="eligibility"{return set(r.inspect,"manualResume",set(r.inspect["manualResume"],"eligibility",s("running")))};if mode=="native"{return set(r.inspect,"manualResume",set(r.inspect["manualResume"],"originalAgentSessionId",s(UUID().uuidString.lowercased())))};return r.inspect}
  if mode=="resumeScope"{r.token = .init(generation:UUID(),connection:UUID(),authority:s("new"),runtime:s("new"),crew:"crew")}
  await r.store.resume(confirmation);precondition(r.writes.isEmpty)
 case "unknown":
  r.blocked=true;await denied{_ = try await r.store.prepareResume()};await r.store.page();precondition(r.store.tasks.count==1 && r.writes.isEmpty)
  r.blocked=false;let independent=r.make();let c=try await independent.prepareResume();await independent.resume(c);precondition(r.writes.count==1)
 case "unsupported","refused":
  r.route={_,_ in throw CrewdClientError(message:"read unavailable",code:mode=="unsupported" ? -32601:-32003)}
  await r.store.inspect();precondition(r.store.inspection == .null && r.store.unsupported == (mode=="unsupported") && r.writes.isEmpty)
 default:fatalError("mode")
 }
 print("PASS "+mode)
 }
}
