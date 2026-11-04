import Foundation
@main struct LegacyFixture {
 @MainActor static func main() async throws {
  func change(_ v:CrewdValue,_ k:String,_ x:CrewdValue)->CrewdValue {guard case .object(var d)=v else{fatalError()};d[k]=x;return .object(d)}
  let target:CrewdValue = .object(["id":.string("W1"),"crewId":.string("a"),"revision":.string("7"),"kind":.string("overdue"),"scope":.string("in_scope")])
  let preview:CrewdValue = .object(["messageId":.string("m"),"crewId":.string("a"),"sessionId":.string("s"),"revision":.string("p"),"canConfirm":.bool(true),"blockers":.array([]),"targets":.array([target]),"budgetEpoch":.integer(0),"restoreEpoch":.string("epoch"),"existingCaseId":.null,"observedAt":.integer(1)])
  var response=preview,writes=0,queries=0,request="",notFound=false
  var prepared:CrewdValue = .null
  func receipt()->CrewdValue {.object(["requestId":.string(request),"messageId":.string("m"),"crewId":.string("a"),"sessionId":.string("s"),"caseId":.string("AC1"),"created":.bool(true),"confirmedAt":.integer(2),"note":.string("checked"),"previewRevision":.string("p")])}
  let store=CrewdWorkStore {method,p in
   if method=="adminCase.legacyPreview" {return response}
   if method=="adminCase.legacyConfirm" {writes += 1;prepared=p;request=p["requestId"].string!;throw CrewdClientError(message:"transport lost")}
   precondition(method=="adminCase.legacyReceipt" && p["requestId"] == .string(request));queries += 1
   if notFound {throw CrewdClientError(message:"currently not found",code:-32004,data:.object(["refusal":.string("resource.not_found")]))}
   return receipt()
  }
  store.selectCrew("a");store.configureCapabilities(["adminCase.legacyList","adminCase.legacyGet","adminCase.legacyPreview","adminCase.legacyConfirm","adminCase.legacyReceipt","adminCase.legacyRecovery"])
  for bad in [change(preview,"crewId",.string("foreign")),change(preview,"targets",.array([change(target,"scope",.string("out_of_scope"))])),change(preview,"targets",.array([]))] {
   response=bad;do {_ = try await store.legacyPreview(messageID:"m",sessionID:"s");fatalError("invalid preview accepted")}catch{}
  }
  response=preview;_ = try await store.legacyPreview(messageID:"m",sessionID:"s")
  do {_ = try await store.confirmLegacy(messageID:"m",sessionID:"s",revision:"p",note:"checked");fatalError()}catch{}
  precondition(writes==1 && store.legacyIntents.first?.status == .unknown)
  _ = try await store.legacyPreview(messageID:"m",sessionID:"s")
  do {_ = try await store.confirmLegacy(messageID:"m",sessionID:"s",revision:"p",note:"checked");fatalError()}catch{}
  precondition(writes==1)
  store.invalidateConnection()
  precondition(store.legacyIntents.isEmpty,"previous connection cannot expose historical requests")
  try store.restoreLegacyIntents([prepared])
  precondition(store.legacyIntents.first?.id==request && store.legacyIntents.first?.status == .unknown)
  _ = try await store.legacyPreview(messageID:"m",sessionID:"s")
  do {_ = try await store.confirmLegacy(messageID:"m",sessionID:"s",revision:"p",note:"checked");fatalError()}catch{}
  precondition(writes==1,"restoring the original request must keep confirmation fenced")
  notFound=true;do {_ = try await store.legacyReceipt(requestID:request);fatalError()}catch{}
  precondition(store.legacyIntents.first?.id==request && store.legacyIntents.first?.status == .notFound)
  do {_ = try await store.confirmLegacy(messageID:"m",sessionID:"s",revision:"p",note:"checked");fatalError()}catch{}
  precondition(writes==1,"fresh preview required after lookup")
  notFound=false;_ = try await store.legacyReceipt(requestID:request)
  precondition(queries==2 && writes==1 && store.legacyIntents.first?.status == .recorded)
  var held:CheckedContinuation<CrewdValue,Error>?
  let late=CrewdWorkStore {_,_ in try await withCheckedThrowingContinuation{held=$0}}
  late.selectCrew("a");let task=Task {try await late.legacyPreview(messageID:"m",sessionID:"s")}
  while held==nil {await Task.yield()};late.invalidateConnection();held!.resume(returning:preview)
  do {_ = try await task.value;fatalError("late preview adopted")}catch is CancellationError{}
  print("legacy current evidence, epoch-zero preview, strict scope, unknown UUID restore without replay, pure lookup and late connection passed")
 }
}
