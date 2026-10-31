import Foundation
@main struct AttributionFixture {
 @MainActor static func main() async throws {
  let link:CrewdValue = .object(["kind":.string("work"),"id":.string("W9"),"revision":.string("2")])
  var calls:[String]=[], delay=false
  var held:CheckedContinuation<CrewdValue,Error>?
  func view(_ action:String="reset")->CrewdValue {.object(["crewId":.string("a"),"messageId":.string("m"),"revision":.string("opaque"),"source":.string(action == "reset" ? "none":"human"),"link":action == "set" ? link:.null,"override":action == "reset" ? .null:.object(["action":.string(action),"link":action == "set" ? link:.null,"actor":.string("human"),"atMs":.integer(1)]),"candidates":.array([]),"candidatesTruncated":.bool(true),"sampleOnly":.bool(true),"resolvedCandidate":.object(["link":link,"summary":.string("sample-external exact target")])])}
  let store=CrewdWorkStore {method,p in
   calls.append(method)
   precondition(p["crewId"] == .string("a") && p["messageId"] == .string("m"))
   if delay {return try await withCheckedThrowingContinuation {held=$0}}
   if method == "message.attribution.set" {precondition(p["expectedRevision"] == .string("opaque"));return view(p["action"].string!)}
   precondition(method == "message.attribution.get");return view()
  }
  store.selectCrew("a")
  _ = try await store.messageAttribution(crew:"a",message:"m",target:.object(["kind":.string("work"),"id":.string("W9")]))
  for action in ["set","clear","reset"] {_ = try await store.setMessageAttribution(crew:"a",message:"m",revision:"opaque",action:action,link:action == "set" ? link:nil)}
  delay=true
  let task=Task {try await store.setMessageAttribution(crew:"a",message:"m",revision:"opaque",action:"clear")}
  while held == nil {await Task.yield()}
  store.invalidateConnection();held!.resume(returning:view("clear"))
  do {_ = try await task.value;fatalError("stale accepted")} catch is CancellationError {}
  precondition(calls.count == 5)
  for key in ["crewId","messageId","revision","source","override","resolvedCandidate"] {
   guard case .object(var fields)=view() else {fatalError()};fields.removeValue(forKey:key)
   let bad:CrewdValue = .object(fields)
   let broken=CrewdWorkStore {_,_ in bad}
   do {_ = try await broken.messageAttribution(crew:"a",message:"m");fatalError("missing field accepted")} catch {}
  }
  var exactReads=0
  let summaryStore=CrewdWorkStore {method,p in
   precondition(method == "message.attribution.get");exactReads += 1
   if exactReads == 2 {precondition(p["target"]["id"] == .string("W9"))}
   return view("set")
  }
  let enriched=try await summaryStore.messageAttributionWithSummary(crew:"a",message:"m")
  precondition(exactReads == 2 && enriched["resolvedCandidate"]["summary"] == .string("sample-external exact target"))
  print("attribution exact resolution, set/clear/reset CAS, stale mutation rejection and no replay passed")
 }
}
