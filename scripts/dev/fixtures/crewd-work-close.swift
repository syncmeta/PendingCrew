import Foundation
@main struct WorkCloseFixture {
 @MainActor static func main() async throws {
  let id=CrewdWorkID("W1",kind:.plan)!
  var calls=0, badID=false, delay=false
  var held:CheckedContinuation<CrewdValue,Error>?
  func result(_ p:CrewdValue)->CrewdValue {.object(["id":.string(badID ? "W2":"W1"),"crewId":.string("a"),"revision":.string("2"),"status":.string(p["kind"] == .string("done") ? "done":"dropped"),"title":.string("work"),"owner":.null,"frozen":.bool(false),"note":.null,"nextCheckAt":.null,"parent":.null,"blockedOn":.array([]),"sources":.array([]),"goal":.string("goal"),"dependsOn":.array([]),"acceptance":.string("accept"),"evidence":p["evidence"]])}
  let store=CrewdWorkStore { method,p in
   guard method == "work.close" else {throw CrewdClientError(message:"fixture refresh unavailable")}
   calls += 1
   precondition(p["id"] == .string("W1") && p["expectedRevision"] == .string("1"))
   if delay {return try await withCheckedThrowingContinuation {held=$0}}
   return result(p)
  }
  store.selectCrew("a")
  do {_ = try await store.closePlan(id,expectedRevision:"1",kind:"done",evidenceKind:"text",evidence:" ",note:"reason");fatalError("empty evidence accepted")} catch {}
  precondition(calls == 0)
  for kind in ["done","duplicate","superseded"] {_ = try await store.closePlan(id,expectedRevision:"1",kind:kind,evidenceKind:"text",evidence:"proof",note:"reason")}
  badID=true
  do {_ = try await store.closePlan(id,expectedRevision:"1",kind:"done",evidenceKind:"text",evidence:"proof",note:"reason");fatalError("foreign result accepted")} catch {}
  delay=true
  let task=Task {try await store.closePlan(id,expectedRevision:"1",kind:"done",evidenceKind:"text",evidence:"proof",note:"reason")}
  while held == nil {await Task.yield()}
  store.invalidateConnection();held!.resume(returning:.null)
  do {_ = try await task.value;fatalError("stale accepted")} catch is CancellationError {}
  precondition(calls == 5)
  var historyCalls=0
  let history=CrewdWorkStore {method,p in
   precondition(method == "work.closureHistory" && p["id"] == .string("W1") && p["before"] == .null)
   historyCalls += 1
   if historyCalls == 1 {
    precondition(p["beforeSeq"] == .null)
    return .object(["items":.array([.object(["seq":.string("8"),"at":.string("now"),"actor":.object(["kind":.string("human")]),"closure":.object(["kind":.string("done"),"note":.null,"evidence":.null])])]),"nextBeforeSeq":.string("8")])
   }
   precondition(p["beforeSeq"] == .string("8"))
   return .object(["items":.array([]),"nextBeforeSeq":.null])
  }
  history.selectCrew("a")
  let page=try await history.closureHistory(id)
  _ = try await history.closureHistory(id,beforeSeq:page["nextBeforeSeq"].string)
  precondition(historyCalls == 2)
  var pendingHistory:CheckedContinuation<CrewdValue,Error>?
  let changing=CrewdWorkStore {_,_ in try await withCheckedThrowingContinuation {pendingHistory=$0}}
  changing.selectCrew("a")
  let oldHistory=Task {try await changing.closureHistory(id)}
  while pendingHistory == nil {await Task.yield()}
  changing.selectCrew("b");pendingHistory!.resume(returning:.object(["items":.array([]),"nextBeforeSeq":.null]))
  do {_ = try await oldHistory.value;fatalError("old scope history accepted")} catch is CancellationError {}
  print("work.close evidence mandatory, three kinds, CAS wire, foreign and stale receipt rejection, no replay passed")
 }
}
