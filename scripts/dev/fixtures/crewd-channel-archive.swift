import Foundation
@main struct ArchiveFixture {
 @MainActor static func main() async throws {
  var archived=false,calls=0,delay=false
  var held:CheckedContinuation<CrewdValue,Error>?
  func channel()->CrewdValue {.object(["id":.string("c"),"crewId":.string("a"),"name":.string("topic"),"archivedAt":archived ? .string("now"):.null])}
  func check()->CrewdValue {
   var facts:[String:CrewdValue]=["incomplete":.bool(false),"lastMessageSeq":.null,"lastMessageAt":.null]
   for key in ["linkedWork","liveWork","linkedRequirements","openRequirements","openAsks","pendingWakes","staleWakes","inflightWakes","activeDeliveries","pendingJudgments"] {facts[key] = .integer(0)}
   return .object(["channel":channel(),"revision":.string("v"),"canArchive":.bool(!archived),"canRestore":.bool(archived),"blockers":.array([]),"facts":.object(facts),"lastAction":.null])
  }
  let store=CrewdWorkStore {method,p in
   if method == "channel.list" {return .array([channel()])}
   precondition(p["channelId"] == .string("c"));calls += 1
   if delay {return try await withCheckedThrowingContinuation {held=$0}}
   if method != "channel.archiveCheck" {precondition(p["expectedRevision"] == .string("v"));archived=method == "channel.archiveVerified"}
   return check()
  }
  store.selectCrew("a")
  _ = try await store.archiveCheck(channel:"c",crew:"a")
  do {_ = try await store.verifyArchive(channel:"c",crew:"a",revision:"v",restore:false,evidence:"",note:"note");fatalError("empty accepted")} catch {}
  precondition(calls == 1)
  _ = try await store.verifyArchive(channel:"c",crew:"a",revision:"v",restore:false,evidence:"proof",note:"note")
  let found=try await store.archivedChannels();precondition(found.count == 1)
  _ = try await store.verifyArchive(channel:"c",crew:"a",revision:"v",restore:true,evidence:"",note:"restore")
  delay=true
  let task=Task {try await store.archiveCheck(channel:"c",crew:"a")}
  while held == nil {await Task.yield()}
  store.invalidateConnection();held!.resume(returning:check())
  do {_ = try await task.value;fatalError("stale accepted")} catch is CancellationError {}
  print("archive check, evidence mandatory, archive/list/restore, stale connection and no replay passed")
 }
}
