import Foundation
@main struct CoordinationFixture {
 @MainActor static func main() async throws {
  var response:CrewdValue = .object(["crewId":.string("a"),"id":.string("W1"),"record":.null])
  var delay=false
  var held:CheckedContinuation<CrewdValue,Error>?
  let store=CrewdWorkStore {method,params in
   precondition(method == "work.coordinationStatus" && params["crewId"] == .string("a") && params["id"] == .string("W1"))
   if delay {return try await withCheckedThrowingContinuation {held=$0}}
   return response
  }
  store.selectCrew("a")
  _ = try await store.coordinationStatus(crew:"a",id:"W1")
  for bad:CrewdValue in [.null,.object(["crewId":.string("a"),"id":.string("W1")]),.object(["crewId":.string("b"),"id":.string("W1"),"record":.null]),.object(["crewId":.string("a"),"id":.string("W1"),"record":.object([:])])] {
   response=bad
   do {_ = try await store.coordinationStatus(crew:"a",id:"W1");fatalError("malformed accepted")} catch {}
  }
  var row:[String:CrewdValue] = ["decisionId":.string("d"),"phase":.string("settled"),"reason":.string("answered"),"meaningfulProgress":.number(0.2),"needsCoordination":.number(0.8),"adopted":.bool(true),"recipient":.null,"wakeId":.null,"wakeState":.null]
  response = .object(["crewId":.string("a"),"id":.string("W1"),"record":.object(row)])
  _ = try await store.coordinationStatus(crew:"a",id:"W1")
  row["needsCoordination"] = .number(2)
  response = .object(["crewId":.string("a"),"id":.string("W1"),"record":.object(row)])
  do {_ = try await store.coordinationStatus(crew:"a",id:"W1");fatalError("invalid probability")} catch {}
  delay=true
  let task=Task {try await store.coordinationStatus(crew:"a",id:"W1")}
  while held == nil {await Task.yield()}
  store.invalidateConnection();held!.resume(returning:.null)
  do {_ = try await task.value;fatalError("stale accepted")} catch is CancellationError {}
  print("coordination read-only, absent record, malformed and stale rejection passed")
 }
}
