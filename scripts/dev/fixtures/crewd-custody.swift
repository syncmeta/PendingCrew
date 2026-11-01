import Foundation
@main struct CustodyFixture {
 @MainActor static func main() async throws {
  let caps:Set<String>=["adminCase.custodyGet","adminCase.custodyPropose","adminCase.custodyTransferGet"]
  let original="00000000-0000-4000-8000-000000000001", current="00000000-0000-4000-8000-000000000002"
  let target:CrewdValue = .object(["id":.string("W1"),"crewId":.string("a"),"revision":.string("1"),"kind":.string("overdue"),"scope":.string("in_scope")])
  func change(_ v:CrewdValue,_ key:String,_ value:CrewdValue)->CrewdValue {guard case .object(var d)=v else{fatalError()};d[key]=value;return .object(d)}
  func remove(_ v:CrewdValue,_ key:String)->CrewdValue {guard case .object(var d)=v else{fatalError()};d.removeValue(forKey:key);return .object(d)}
  func result(_ resolved:Bool)->CrewdValue {.object(["id":.string("W1"),"sourceKinds":.array([.string("overdue")]),"state":.string(resolved ? "resolved":"pending"),"resolution":resolved ? .string("closed"):.null,"resolvedBy":resolved ? .string("other"):.null,"actorSessionId":.null,"eventSeq":resolved ? .string("20"):.null,"wakeId":.null,"reason":.string(resolved ? "closed_with_evidence":"original_problem_remains")])}
  func transfer(_ id:String,phase:String="accepted")->CrewdValue {.object(["requestId":.string(id),"caseId":.string("AC1"),"crewId":.string("a"),"predecessorReceiptId":.string(original),"fromSessionId":.string("old"),"toSessionId":.string("new"),"phase":.string(phase),"revision":.string("transfer-v"),"reason":.string(phase == "accepted" ? "captain_accepted":"proposed"),"requestReason":.string("reason"),"targets":.array([target]),"wakeId":.integer(1),"createdAt":.integer(1),"deliveredAt":phase == "accepted" ? .string("date"):.null,"acceptedAt":phase == "accepted" ? .integer(2):.null,"acceptanceNote":phase == "accepted" ? .string("accepted"):.null,"supersededBy":.null])}
  func custody(_ phase:String="stale")->CrewdValue {
   let discharged=phase == "discharged", hasCurrent=phase != "stale"
   return .object(["caseId":.string("AC1"),"crewId":.string("a"),"revision":.string("v"),"originalRequestId":.string(original),"currentCustodianSessionId":.string(hasCurrent ? "new":"old"),"currentReceiptId":.string(hasCurrent ? current:original),"phase":.string(phase),"reason":.string(discharged ? "verified":"state_changed"),"targets":.array([target]),"targetResults":.array([result(discharged)]),"outOfScope":.array([]),"pending":.null,"currentRecord":hasCurrent ? transfer(current):.null,"discharge":discharged ? .object(["revision":.string("proof"),"verifiedAt":.integer(3),"targets":.array([result(true)])]):.null])
  }
  let base:CrewdValue = .object(["id":.string("AC1"),"crewId":.string("a"),"revision":.string("v"),"createdAt":.integer(1),"source":.object(["sessionId":.string("s"),"budgetEpoch":.integer(1),"restoreEpoch":.string("e"),"announcementId":.string("m")]),"targets":.array([]),"decision":.string("observed"),"decisionRecord":.null,"execution":.string("not_started"),"executionRecord":.null,"retirement":.string("not_started"),"retirementRecord":.null,"outcomeRecord":.null,"handoffRecord":.null])
  let badRecord=change(base,"custodyRecord",.object(["caseId":.string("foreign")]))
  let bad=CrewdWorkStore {_,_ in badRecord};bad.selectCrew("a");bad.configureCapabilities(caps)
  var rejected=false;do {_ = try await bad.adminCase(id:"AC1")}catch{rejected=true};precondition(rejected,"wrong identity must be refused")
  let legacy=CrewdWorkStore {_,_ in base};legacy.selectCrew("a");_ = try await legacy.adminCase(id:"AC1")
  legacy.configureCapabilities(caps);rejected=false;do {_ = try await legacy.adminCase(id:"AC1")}catch{rejected=true};precondition(rejected,"new capability requires explicit nullable custodyRecord")
  for phase in ["stale","active","discharged"] {
   let value=custody(phase),store=CrewdWorkStore {_,_ in custody(phase)};store.selectCrew("a");store.configureCapabilities(caps)
   let actual=try await store.custody(caseID:"AC1");precondition(actual==value)
   if phase == "active" {precondition(actual["currentRecord"]["phase"] == .string("accepted") && actual["targetResults"].array[0]["state"] == .string("pending") && actual["discharge"] == .null)}
  }
  for key in ["caseId","crewId","pending","currentRecord","targetResults","outOfScope","discharge"] {
   let v=remove(custody(),key),s=CrewdWorkStore {_,_ in v};s.selectCrew("a");s.configureCapabilities(caps)
   rejected=false;do {_ = try await s.custody(caseID:"AC1")}catch{rejected=true};precondition(rejected,"incomplete custody accepted: \(key)")
  }
  for v in [change(custody(),"caseId",.string("AC2")),change(custody(),"crewId",.string("b")),change(custody(),"phase",.string("unknown")),change(custody(),"phase",.string("discharged")),change(custody("active"),"currentRecord",remove(transfer(current),"acceptedAt"))] {
   let s=CrewdWorkStore {_,_ in v};s.selectCrew("a");s.configureCapabilities(caps);rejected=false;do {_ = try await s.custody(caseID:"AC1")}catch{rejected=true};precondition(rejected)
  }
  var writes=0,lookups=0,request="",notFound=false
  let uncertain=CrewdWorkStore {method,p in
   if method == "adminCase.custodyGet" {return custody()}
   if method == "adminCase.custodyPropose" {writes += 1;request=p["requestId"].string!;precondition(p["crewId"] == .string("a") && p["caseId"] == .string("AC1"));throw CrewdClientError(message:"connection lost")}
   precondition(method == "adminCase.custodyTransferGet" && p["requestId"] == .string(request));lookups += 1
   if notFound {throw CrewdClientError(message:"not found",code:-32004,data:.object(["refusal":.string("resource.not_found")]))}
   return transfer(request,phase:"waiting")
  }
  uncertain.selectCrew("a");uncertain.configureCapabilities(caps);_ = try await uncertain.custody(caseID:"AC1")
  do {_ = try await uncertain.proposeCustody(caseID:"AC1",revision:"v",reason:"reason");fatalError()}catch{}
  precondition(writes==1 && uncertain.custodyRequests(caseID:"AC1").first?.status == .unknown)
  _ = try await uncertain.custody(caseID:"AC1")
  do {_ = try await uncertain.proposeCustody(caseID:"AC1",revision:"v",reason:"reason");fatalError()}catch{}
  precondition(writes==1)
  notFound=true;do {_ = try await uncertain.custodyTransfer(requestID:request,caseID:"AC1");fatalError()}catch{}
  precondition(uncertain.custodyRequests(caseID:"AC1").first?.status == .notFound && uncertain.custodyRequests(caseID:"AC1").first?.id==request)
  do {_ = try await uncertain.proposeCustody(caseID:"AC1",revision:"v",reason:"reason");fatalError()}catch{}
  precondition(writes==1)
  notFound=false
  _ = try await uncertain.custodyTransfer(requestID:request,caseID:"AC1")
  precondition(writes==1 && lookups==2 && uncertain.custodyRequests(caseID:"AC1").first?.status == .recorded)
  notFound=true;do {_ = try await uncertain.custodyTransfer(requestID:request,caseID:"AC1");fatalError()}catch{}
  precondition(uncertain.custodyRequests(caseID:"AC1").first?.id==request && uncertain.custodyRequests(caseID:"AC1").first?.status == .recorded && writes==1)
  do {_ = try await uncertain.proposeCustody(caseID:"AC1",revision:"v",reason:"reason");fatalError()}catch{}
  precondition(writes==1,"NotFound does not replay or grant a new operation without fresh read")
  var held:CheckedContinuation<CrewdValue,Error>?,heldID="",lateWrites=0
  let late=CrewdWorkStore {method,p in
   if method == "adminCase.custodyGet" {return custody()}
   precondition(method == "adminCase.custodyPropose");lateWrites += 1;heldID=p["requestId"].string!
   return try await withCheckedThrowingContinuation {held=$0}
  }
  late.selectCrew("a");late.configureCapabilities(caps);_ = try await late.custody(caseID:"AC1")
  let pending=Task {try await late.proposeCustody(caseID:"AC1",revision:"v",reason:"reason")}
  while held==nil {await Task.yield()}
  do {_ = try await late.proposeCustody(caseID:"AC1",revision:"v",reason:"reason");fatalError()}catch{}
  precondition(lateWrites==1)
  late.invalidateConnection();held!.resume(returning:change(custody(),"pending",transfer(heldID,phase:"waiting")))
  do {_ = try await pending.value;fatalError("late reply adopted")}catch is CancellationError{}
  precondition(late.custodyRequests(caseID:"AC1").first?.status == .unknown)
  late.selectCrew("b");precondition(late.custodyRequests(caseID:"AC1").isEmpty)
  late.selectCrew("a");precondition(late.custodyRequests(caseID:"AC1").first?.id==heldID)
  var readHeld:CheckedContinuation<CrewdValue,Error>?
  let scoped=CrewdWorkStore {_,_ in try await withCheckedThrowingContinuation {readHeld=$0}}
  scoped.selectCrew("a");scoped.configureCapabilities(caps)
  let oldRead=Task {try await scoped.custody(caseID:"AC1")}
  while readHeld==nil {await Task.yield()}
  scoped.selectCrew("b");readHeld!.resume(returning:custody())
  do {_ = try await oldRead.value;fatalError("old crew custody mixed in")}catch is CancellationError{}
  print("custody fixture passed: strict facts, separate acceptance/discharge, one mutation, UUID lookup, late connection")
 }
}
