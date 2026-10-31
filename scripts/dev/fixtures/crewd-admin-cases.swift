import Foundation
@main struct AdminCasesFixture {
 @MainActor static func main() async throws {
  func row(_ id:String="AC00000000000000000001",crew:String="a")->CrewdValue {.object(["id":.string(id),"crewId":.string(crew),"revision":.string("v"),"createdAt":.integer(1),"source":.object(["sessionId":.string("s"),"budgetEpoch":.integer(1),"restoreEpoch":.string("e"),"announcementId":.string("m")]),"targets":.array([.object(["id":.string("W1"),"crewId":.string("a"),"revision":.string("1"),"kind":.string("overdue"),"scope":.string("in_scope")])]),"decision":.string("observed"),"decisionRecord":.null,"execution":.string("not_started"),"executionRecord":.null,"retirement":.string("not_started"),"retirementRecord":.null,"outcomeRecord":.null,"handoffRecord":.null])}
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
  for key in ["decision","execution","retirement","source","targets","handoffRecord"] {
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
  for phase in ["registered","requested","running","unknown","stale"] {
   guard case .object(var fields)=row() else {fatalError()}
   fields["execution"] = .string(phase)
   fields["executionRecord"] = .object(["sessionId":.string("admin"),"attemptId":.string("attempt"),"phase":.string(phase),"reason":.string(phase == "running" ? "started":"registered"),"generation":phase == "running" ? .string("1"):.null,"effectsCount":.integer(0)])
   let payload:CrewdValue = .object(fields)
   let decoded=CrewdWorkStore {_,_ in payload};decoded.selectCrew("a")
   _ = try await decoded.adminCase(id:"AC00000000000000000001")
  }
  for phase in ["resolved","failed","waiting_admission"] {
   guard case .object(var fields)=row() else {fatalError()};fields["execution"] = .string(phase)
   let bad:CrewdValue = .object(fields)
   let broken=CrewdWorkStore {_,_ in bad};broken.selectCrew("a")
   do {_ = try await broken.adminCase(id:"AC00000000000000000001");fatalError("future execution accepted")} catch {}
  }
  for badGeneration:CrewdValue in [.null,.string("01"),.integer(1)] {
   guard case .object(var fields)=row() else {fatalError()};fields["execution"] = .string("running")
   fields["executionRecord"] = .object(["sessionId":.string("admin"),"attemptId":.string("a"),"phase":.string("running"),"reason":.string("started"),"generation":badGeneration,"effectsCount":.integer(1)])
   let bad:CrewdValue = .object(fields)
   let broken=CrewdWorkStore {_,_ in bad};broken.selectCrew("a")
   do {_ = try await broken.adminCase(id:"AC00000000000000000001");fatalError("bad running receipt accepted")} catch {}
  }
  for phase in ["unresolved","resolved","stale"] {
   guard case .object(var fields)=row() else {fatalError()}
   fields["outcomeRecord"] = .object(["caseId":fields["id"]!,"crewId":.string("a"),"revision":.string("v"),"verifiedAt":.integer(1),"phase":.string(phase),"reason":.string(phase == "resolved" ? "verified":"targets_unresolved"),"targets":.array([.object(["id":.string("W1"),"sourceKinds":.array([.string("overdue")]),"state":.string(phase == "resolved" ? "resolved":"pending"),"resolution":phase == "resolved" ? .string("closed"):.null,"resolvedBy":phase == "resolved" ? .string("case_admin"):.null,"actorSessionId":phase == "resolved" ? .string("admin"):.null,"eventSeq":phase == "resolved" ? .string("12"):.null,"wakeId":.null,"reason":.string(phase == "resolved" ? "closed_with_evidence":"notification_pending")])]),"outOfScope":.array([]),"missing":.array([]),"pending":.array([]),"blocked":.array([]),"resolvedByAdmin":.array([]),"resolvedByOthers":.array([])])
   let payload:CrewdValue = .object(fields)
   let decoded=CrewdWorkStore {_,_ in payload};decoded.selectCrew("a")
   _ = try await decoded.adminCase(id:"AC00000000000000000001")
  }
  for badOutcome:CrewdValue in [.object([:]),.object(["caseId":.string("other"),"crewId":.string("a"),"phase":.string("resolved")])] {
   guard case .object(var fields)=row() else {fatalError()};fields["outcomeRecord"]=badOutcome
   let bad:CrewdValue = .object(fields)
   let broken=CrewdWorkStore {_,_ in bad};broken.selectCrew("a")
   do {_ = try await broken.adminCase(id:"AC00000000000000000001");fatalError("malformed outcome accepted")} catch {}
  }
  for phase in ["prepared","dispatched","decision_ready","exit_requested","parked","unknown","stale","suppressed"] {
   guard case .object(var fields)=row() else {fatalError()};fields["retirement"] = .string(phase)
   fields["execution"] = .string("running")
   fields["executionRecord"] = .object(["sessionId":.string("admin"),"attemptId":.string("a"),"phase":.string("running"),"reason":.string("started"),"generation":.string("1"),"effectsCount":.integer(1)])
   fields["retirementRecord"] = .object(["decisionId":.string("retirement"),"phase":.string(phase),"reason":.string("answered"),"probability":.number(0.9),"adopted":.bool(phase == "parked"),"generation":.string("1")])
   let payload:CrewdValue = .object(fields)
   let decoded=CrewdWorkStore {_,_ in payload};decoded.selectCrew("a")
   _ = try await decoded.adminCase(id:"AC00000000000000000001")
  }
  for phase in ["done","exit_requested"] {
   guard case .object(var fields)=row() else {fatalError()};fields["retirement"] = .string(phase)
   let payload:CrewdValue = .object(fields)
   let broken=CrewdWorkStore {_,_ in payload};broken.selectCrew("a")
   do {_ = try await broken.adminCase(id:"AC00000000000000000001");fatalError("unconfirmed retirement accepted")} catch {}
  }
  guard case .object(var mismatched)=row() else {fatalError()}
  mismatched["execution"] = .string("running")
  mismatched["executionRecord"] = .object(["sessionId":.string("admin"),"attemptId":.string("a"),"phase":.string("running"),"reason":.string("started"),"generation":.string("1"),"effectsCount":.integer(1)])
  mismatched["retirement"] = .string("parked")
  mismatched["retirementRecord"] = .object(["decisionId":.string("r"),"phase":.string("parked"),"reason":.string("answered"),"probability":.number(0.9),"adopted":.bool(true),"generation":.string("2")])
  let mismatch:CrewdValue = .object(mismatched)
  let mismatchStore=CrewdWorkStore {_,_ in mismatch};mismatchStore.selectCrew("a")
  do {_ = try await mismatchStore.adminCase(id:"AC00000000000000000001");fatalError("foreign generation parked accepted")} catch {}
  func handoff(_ phase:String="waiting")->CrewdValue {
   .object(["requestId":.string("req"),"caseId":.string("AC00000000000000000001"),"crewId":.string("a"),"revision":.string("h1"),"phase":.string(phase),"reason":.string(phase == "accepted" ? "captain_accepted":"requested"),"adminSessionId":.string("admin"),"sourceSessionId":.string("s"),"captainSessionId":.string("captain"),"requestReason":.string("需要机长持续跟进"),"targets":row()["targets"],"wakeId":.integer(2),"createdAt":.integer(1),"deliveredAt":phase == "delivered" ? .string("2026-10-31T01:00:00Z"):.null,"acceptedAt":phase == "accepted" ? .integer(2):.null,"acceptanceNote":phase == "accepted" ? .string("我来负责后续跟进"):.null])
  }
  func handoffRow(_ handoff:CrewdValue)->CrewdValue {
   guard case .object(var fields)=row() else {fatalError()}
   fields["handoffRecord"] = handoff
   fields["execution"] = .string("running")
   fields["executionRecord"] = .object(["sessionId":.string("admin"),"attemptId":.string("a"),"phase":.string("running"),"reason":.string("started"),"generation":.string("1"),"effectsCount":.integer(0)])
   return .object(fields)
  }
  for phase in ["waiting","delivered","accepted","stale"] {
   let payload=handoffRow(handoff(phase)),decoded=CrewdWorkStore {_,_ in handoffRow(handoff(phase))};decoded.selectCrew("a")
   let result=try await decoded.adminCase(id:"AC00000000000000000001")
   precondition(result == payload && result["outcomeRecord"] == .null && result["retirement"] == .string("not_started"),"handoff fabricated resolution or retirement")
  }
  for (key,badValue):(String,CrewdValue) in [("caseId",.string("other")),("sourceSessionId",.string("other")),("adminSessionId",.string("other")),("phase",.string("resolved")),("reason",.string("future")),("acceptedAt",.null),("acceptanceNote",.null),("deliveredAt",.integer(1)),("wakeId",.string("2"))] {
   guard case .object(var fields)=handoff("accepted") else {fatalError()};fields[key]=badValue
   let bad=handoffRow(.object(fields)),broken=CrewdWorkStore {_,_ in handoffRow(.object(fields))};broken.selectCrew("a")
   precondition(bad["handoffRecord"][key] == badValue)
   do {_ = try await broken.adminCase(id:"AC00000000000000000001");fatalError("malformed handoff accepted: \(key)")} catch {}
  }
  for reason in ["requested","notification_delivered","captain_accepted","state_changed","stopped","recipient_changed","notification_dropped"] {
   guard case .object(var fields)=handoff("stale") else {fatalError()};fields["reason"] = .string(reason)
   let payload=handoffRow(.object(fields)),decoded=CrewdWorkStore {_,_ in handoffRow(.object(fields))};decoded.selectCrew("a")
   let result=try await decoded.adminCase(id:"AC00000000000000000001");precondition(result == payload)
  }
  guard case .object(var transferred)=handoffRow(handoff("accepted")) else {fatalError()}
  transferred["outcomeRecord"] = .object(["caseId":transferred["id"]!,"crewId":.string("a"),"revision":.string("v"),"verifiedAt":.integer(3),"phase":.string("unresolved"),"reason":.string("targets_unresolved"),"targets":.array([]),"outOfScope":.array([]),"missing":.array([]),"pending":.array([.string("W1")]),"blocked":.array([]),"resolvedByAdmin":.array([]),"resolvedByOthers":.array([])])
  transferred["retirement"] = .string("parked")
  transferred["retirementRecord"] = .object(["decisionId":.string("r"),"phase":.string("parked"),"reason":.string("answered"),"probability":.number(0.9),"adopted":.bool(true),"generation":.string("1")])
  let transferPayload:CrewdValue = .object(transferred),transferStore=CrewdWorkStore {_,_ in .object(transferred)};transferStore.selectCrew("a")
  let transferResult=try await transferStore.adminCase(id:"AC00000000000000000001")
  precondition(transferResult == transferPayload && transferResult["outcomeRecord"]["phase"] == .string("unresolved") && transferResult["retirement"] == .string("parked"))
  print("admin cases opaque pagination, detail, malformed/foreign, crew and connection stale rejection passed")
 }
}
