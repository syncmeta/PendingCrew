import Foundation
@main struct AdminCasesFixture {
 @MainActor static func main() async throws {
  func row(_ id:String="AC00000000000000000001",crew:String="a")->CrewdValue {.object(["id":.string(id),"crewId":.string(crew),"revision":.string("v"),"createdAt":.integer(1),"source":.object(["sessionId":.string("s"),"budgetEpoch":.integer(1),"restoreEpoch":.string("e"),"announcementId":.string("m")]),"targets":.array([.object(["id":.string("W1"),"crewId":.string("a"),"revision":.string("1"),"kind":.string("overdue"),"scope":.string("in_scope")])]),"decision":.string("observed"),"decisionRecord":.null,"execution":.string("not_started"),"retirement":.string("not_started")])}
  var calls=0,delay=false,foreign=false
  var held:CheckedContinuation<CrewdValue,Error>?
  let store=CrewdWorkStore {method,p in
   calls += 1
   if delay {return try await withCheckedThrowingContinuation {held=$0}}
   if method == "adminCase.get" {return row(crew:foreign ? "b":"a")}
   precondition(method == "adminCase.list" && p["crewId"] == .string("a"))
   if p["after"] != .null {precondition(p["after"] == .string("opaque:epoch/2"));return .object(["crewId":.string("a"),"items":.array([]),"nextAfter":.null])}
   return .object(["crewId":.string("a"),"items":.array([row()]),"nextAfter":.string("opaque:epoch/2")])
  }
  store.selectCrew("a")
  let first=try await store.adminCases()
  _ = try await store.adminCases(after:first["nextAfter"].string)
  _ = try await store.adminCase(id:"AC00000000000000000001")
  foreign=true
  do {_ = try await store.adminCase(id:"AC00000000000000000001");fatalError("foreign accepted")} catch {}
  delay=true
  let task=Task {try await store.adminCases()}
  while held == nil {await Task.yield()}
  store.selectCrew("b");held!.resume(returning:.null)
  do {_ = try await task.value;fatalError("old crew accepted")} catch is CancellationError {}
  let task2=Task {try await store.adminCase(id:"AC00000000000000000001")}
  held=nil
  while held == nil {await Task.yield()}
  store.invalidateConnection();held!.resume(returning:row())
  do {_ = try await task2.value;fatalError("old connection accepted")} catch is CancellationError {}
  for key in ["decision","execution","retirement","source","targets"] {
   guard case .object(var fields)=row() else {fatalError()};fields.removeValue(forKey:key)
   let bad:CrewdValue = .object(fields)
   let broken=CrewdWorkStore {_,_ in bad};broken.selectCrew("a")
   do {_ = try await broken.adminCase(id:"AC00000000000000000001");fatalError("malformed accepted")} catch {}
  }
  precondition(calls == 6)
  for phase in ["prepared","dispatched","answered","unknown","unconfigured","waiting_admission","stale"] {
   guard case .object(var fields)=row() else {fatalError()}
   fields["decision"] = .string(phase)
   fields["decisionRecord"] = .object(["decisionId":.string("d"),"phase":.string(phase),"probability":phase == "answered" ? .number(0.9):.null,"adopted":.bool(phase == "answered"),"reason":.string(phase == "answered" ? "native_answered":"prepared"),"snapshotWitness":.string("w")])
   let payload:CrewdValue = .object(fields)
   let decoded=CrewdWorkStore {_,_ in payload};decoded.selectCrew("a")
   _ = try await decoded.adminCase(id:"AC00000000000000000001")
  }
  for invalid in ["future_state","running","resolved"] {
   guard case .object(var fields)=row() else {fatalError()};fields["decision"] = .string(invalid)
   let bad:CrewdValue = .object(fields)
   let broken=CrewdWorkStore {_,_ in bad};broken.selectCrew("a")
   do {_ = try await broken.adminCase(id:"AC00000000000000000001");fatalError("unknown phase accepted")} catch {}
  }
  for badProbability:CrewdValue in [.number(-0.1),.number(1.1),.string("0.9")] {
   guard case .object(var fields)=row() else {fatalError()};fields["decision"] = .string("answered")
   fields["decisionRecord"] = .object(["decisionId":.string("d"),"phase":.string("answered"),"probability":badProbability,"adopted":.bool(false),"reason":.string("native_answered"),"snapshotWitness":.string("w")])
   let bad:CrewdValue = .object(fields)
   let broken=CrewdWorkStore {_,_ in bad};broken.selectCrew("a")
   do {_ = try await broken.adminCase(id:"AC00000000000000000001");fatalError("bad probability accepted")} catch {}
  }
  print("admin cases opaque pagination, detail, malformed/foreign, crew and connection stale rejection passed")
 }
}
