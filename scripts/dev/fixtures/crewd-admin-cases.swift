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
  try await effectReport()
  try await humanBrief()
  try await secretaryBrief()
  try await requirementReviews()
  print("admin cases opaque pagination, detail, malformed/foreign, crew and connection stale rejection passed")
 }

 @MainActor private static func effectReport() async throws {
  let id = "AC00000000000000000001"
  let first: [String: CrewdValue] = [
   "ordinal": .string("1"), "workId": .string("W1"), "actorSessionId": .string("admin"),
   "occurredAtMs": .integer(25), "firstEventSeq": .string("70"), "lastEventSeq": .string("72"),
   "resultRevision": .string("3"), "kind": .string("closed"), "closureKind": .string("done"),
   "detailsTruncated": .bool(true), "evidence": .object(["kind": .string("text"), "value": .string("原证据")]),
   "note": .string("关闭原因")
  ]
  var legacy = first
  legacy["ordinal"] = .string("2")
  legacy["firstEventSeq"] = .string("75")
  legacy["lastEventSeq"] = .string("75")
  legacy["kind"] = .string("unlinked")
  legacy["closureKind"] = nil
  legacy["evidence"] = nil
  legacy["note"] = nil
  legacy["detailsTruncated"] = .bool(false)
  var page: CrewdValue = .object(["crewId": .string("a"), "id": .string(id), "items": .array([.object(first)]), "nextAfter": .string("opaque")])
  var calls: [CrewdValue] = []
  var hold = false
  var continuation: CheckedContinuation<CrewdValue, Error>?
  let store = CrewdWorkStore { method, params in
   precondition(method == "adminCase.effects" && params["crewId"] == .string("a") && params["id"] == .string(id))
   precondition(params["limit"] == .integer(20))
   calls.append(params)
   if hold { return try await withCheckedThrowingContinuation { continuation = $0 } }
   return page
  }
  store.selectCrew("a")
  do { _ = try await store.adminCaseEffects(id: id); fatalError("capability ignored") } catch {}
  precondition(calls.isEmpty)
  store.configureCapabilities(["adminCase.effects"])
  let firstPage = try await store.adminCaseEffects(id: id)
  precondition(firstPage["items"].array.first?["detailsTruncated"] == .bool(true))
  page = .object(["crewId": .string("a"), "id": .string(id), "items": .array([.object(legacy)])])
  let secondPage = try await store.adminCaseEffects(id: id, after: firstPage["nextAfter"].string)
  precondition(calls.last?["after"] == .string("opaque"))
  precondition(secondPage["items"].array.first?["relatedWorkId"] == .null)
  for (field, value) in [("detailsTruncated", CrewdValue.null), ("lastEventSeq", .string("69")), ("evidence", .null)] {
   var bad = first
   bad[field] = value
   page = .object(["crewId": .string("a"), "id": .string(id), "items": .array([.object(bad)])])
   do { _ = try await store.adminCaseEffects(id: id); fatalError("malformed effect accepted") } catch {}
  }
  page = .object(["crewId": .string("b"), "id": .string(id), "items": .array([])])
  do { _ = try await store.adminCaseEffects(id: id); fatalError("foreign page accepted") } catch {}
  hold = true
  let stale = Task { try await store.adminCaseEffects(id: id) }
  while continuation == nil { await Task.yield() }
  store.selectCrew("b")
  continuation!.resume(returning: firstPage)
  do { _ = try await stale.value; fatalError("old report accepted") } catch is CancellationError {}
 }

 @MainActor private static func humanBrief() async throws {
  let wait: CrewdValue = .object([
   "askId": .string("H53"), "kind": .string("question"), "question": .string("仍需决策"), "questionTruncated": .bool(false),
   "askedAtMs": .integer(5), "waitedMs": .integer(25), "tier": .string("pinned"),
   "notifiedTier": .string("fresh"), "blockingWorkIds": .array([.string("W12")]), "blockersTruncated": .bool(false)
  ])
  guard case .object(var taskFields) = wait else { fatalError() }
  taskFields["askId"] = .string("H54")
  taskFields["kind"] = .string("human_task")
  taskFields["tier"] = .string("fresh")
  let manualTask: CrewdValue = .object(taskFields)
  let caseRecord: CrewdValue = .object([
   "caseId": .string("AC00000000000000000001"), "createdAtMs": .integer(12),
   "decision": .string("observed"), "execution": .string("not_started"), "outcomePhase": .string("unresolved")
  ])
  let patrol: CrewdValue = .object([
   "state": .string("complete"), "runId": .string("original"), "startedAtMs": .integer(7),
   "completedAtMs": .integer(10), "scannedCounts": .object(["work": .integer(14)]),
   "findingCounts": .object(["long_waits": .integer(1)]), "findingsTruncated": .bool(true)
  ])
  let original: [String: CrewdValue] = [
   "crewId": .string("a"), "observedAtMs": .integer(30), "humanWaits": .array([wait, manualTask]),
   "humanWaitsTruncated": .bool(false), "cases": .array([caseRecord]),
   "caseCoverage": .string("recent_sample"), "patrol": patrol
  ]
  var response: CrewdValue = .object(original)
  var held: CheckedContinuation<CrewdValue, Error>?
  var hold = false
  var calls = 0
  let store = CrewdWorkStore { method, params in
   precondition(method == "adminCase.humanBrief" && params == .object(["crewId": .string("a")]))
   calls += 1
   if hold { return try await withCheckedThrowingContinuation { held = $0 } }
   return response
  }
  store.selectCrew("a")
  do { _ = try await store.humanBrief(); fatalError("brief capability ignored") } catch {}
  precondition(calls == 0)
  store.configureCapabilities(["adminCase.humanBrief"])
  let report = try await store.humanBrief()
  precondition(report.waits == [wait, manualTask] && report.cases == [caseRecord] && report.patrol == patrol)
  for (field, invalid) in [
   ("crewId", CrewdValue.string("b")), ("humanWaits", .array([wait, wait])),
   ("caseCoverage", .string("unavailable")), ("patrol", .object(["state": .string("complete")]))
  ] {
   var payload = original
   payload[field] = invalid
   response = .object(payload)
   do { _ = try await store.humanBrief(); fatalError("invalid brief accepted") } catch {}
  }
  hold = true
  let pending = Task { try await store.humanBrief() }
  while held == nil { await Task.yield() }
  store.invalidateConnection()
  held!.resume(returning: .object(original))
  do { _ = try await pending.value; fatalError("old brief accepted") } catch is CancellationError {}
 }

 @MainActor private static func secretaryBrief() async throws {
  let revision=String(repeating:"a",count:64), sourceID="SB11111111-1111-4111-8111-111111111111", caseID="AC00000000000000000001"
  let generation="1", attempt="22222222-2222-4222-8222-222222222222", request="33333333-3333-4333-8333-333333333333"
  let refs:CrewdValue = .array([.object(["kind":.string("human_wait"),"id":.string("H53")])])
  let facts:CrewdValue = .object([
   "crewId":.string("a"),"observedAtMs":.integer(30),"humanWaitsTruncated":.bool(true),"caseCoverage":.string("recent_sample"),"cases":.array([]),
   "humanWaits":.array([.object(["askId":.string("H53"),"kind":.string("question"),"question":.string("原始问题"),"questionTruncated":.bool(true),"askedAtMs":.integer(1),"waitedMs":.integer(29),"tier":.string("fresh"),"notifiedTier":.string("fresh"),"blockingWorkIds":.array([]),"blockersTruncated":.bool(false)])]),
   "patrol":.object(["state":.string("not_recorded"),"scannedCounts":.object([:]),"findingCounts":.object([:]),"findingsTruncated":.bool(false)])
  ])
  let source:CrewdValue = .object(["sourceId":.string(sourceID),"caseId":.string(caseID),"crewId":.string("a"),"scope":.object(["dbId":.string("database"),"restoreEpoch":.string("44444444-4444-4444-8444-444444444444")]),"judgmentEpoch":.string("judgment"),"configurationRevision":.string("config"),"revision":.string(revision),"observedAtMs":.integer(30),"facts":facts,"references":refs])
  let record:CrewdValue = .object([
   "id":.string(caseID),"crewId":.string("a"),"revision":.string("case-revision"),"createdAt":.integer(30),
   "source":.object(["sessionId":.string("captain"),"budgetEpoch":.integer(0),"restoreEpoch":.string("judgment"),"announcementId":.string(""),"provenance":.object(["kind":.string("secretary_brief"),"sourceId":.string(sourceID),"sourceRevision":.string(revision),"observedAtMs":.integer(30)])]),
   "targets":.array([]),"decision":.string("observed"),"decisionRecord":.null,"execution":.string("running"),"executionRecord":.object(["sessionId":.string("bound-author"),"attemptId":.string(attempt),"phase":.string("running"),"reason":.string("started"),"generation":.string(generation),"effectsCount":.integer(0)]),"retirement":.string("not_started"),"retirementRecord":.null,"outcomeRecord":.null,"handoffRecord":.null
  ])
  let publication:CrewdValue = .object(["sourceId":.string(sourceID),"caseId":.string(caseID),"crewId":.string("a"),"sourceRevision":.string(revision),"authorSessionId":.string("bound-author"),"attemptId":.string(attempt),"generation":.string(generation),"requestId":.string(request),"revision":.string(String(repeating:"b",count:64)),"body":.string("原问题仍待确认；部分样本。"),"references":refs,"publishedAtMs":.integer(35)])
  let original:[String:CrewdValue] = ["crewId":.string("a"),"observedAtMs":.integer(40),"state":.string("current"),"currentRevision":.string(revision),"source":source,"case":record,"publication":publication]
  var response:CrewdValue = .object(original),calls=0
  var hold=false,held:CheckedContinuation<CrewdValue,Error>?
  let store=CrewdWorkStore {method,params in
   calls += 1
   if method == "adminCase.get" {return record}
   precondition(method == "adminCase.secretaryBrief" && params == .object(["crewId":.string("a")]))
   if hold {return try await withCheckedThrowingContinuation {held=$0}}
   return response
  }
  store.selectCrew("a")
  do {_ = try await store.secretaryBrief();fatalError("secretary capability ignored")} catch {}
  precondition(calls==0)
  store.configureCapabilities(["adminCase.secretaryBrief.v1"])
  let value=try await store.secretaryBrief();precondition(value.state=="current" && value.body==publication["body"].string && !value.canRequest)
  do {_ = try await store.adminCase(id:caseID);fatalError("old Stop deep link accepted Secretary")} catch {}
  for (key,bad):(String,CrewdValue) in [("authorSessionId",.string("other")),("generation",.string("2")),("sourceRevision",.string(String(repeating:"c",count:64))),("requestId",.string("not-uuid")),("references",.array([.object(["kind":.string("human_wait"),"id":.string("H54")])])),("body",.string(String(repeating:"界",count:2731)))] {
   guard case .object(var body)=publication else {fatalError()};body[key]=bad
   var payload=original;payload["publication"] = .object(body);response = .object(payload)
   do {_ = try await store.secretaryBrief();fatalError("invalid Secretary publication accepted: \(key)")} catch {}
  }
  guard case .object(var completedSource)=source,case .object(var completedFacts)=facts else {fatalError()}
  let run="55555555-5555-4555-8555-555555555555"
  let patrolRef:CrewdValue = .object(["kind":.string("patrol"),"id":.string(run)])
  completedFacts["patrol"] = .object(["state":.string("complete"),"runId":.string(run),"startedAtMs":.integer(7),"completedAtMs":.integer(10),"scannedCounts":.object(["work":.integer(1)]),"findingCounts":.object(["unowned":.integer(1)]),"findingsTruncated":.bool(false)])
  completedSource["facts"] = .object(completedFacts);completedSource["references"] = .array(refs.array+[patrolRef]);completedSource["patrolRevision"] = .string(String(repeating:"e",count:64))
  var completed=original;completed["source"] = .object(completedSource);response = .object(completed)
  let stable=try await store.secretaryBrief();precondition(stable.source["facts"]["patrol"]["completedAtMs"] == .integer(10))
  completedSource["patrolRevision"] = .null;completed["source"] = .object(completedSource);response = .object(completed)
  do {_ = try await store.secretaryBrief();fatalError("completed patrol without evidence revision accepted")} catch {}
  completedSource["patrolRevision"] = .string(String(repeating:"e",count:64))
  completedFacts["patrol"] = .object(["state":.string("in_progress"),"runId":.string(run),"startedAtMs":.integer(7),"scannedCounts":.object([:]),"findingCounts":.object([:]),"findingsTruncated":.bool(false)])
  completedSource["facts"] = .object(completedFacts);completed["source"] = .object(completedSource);response = .object(completed)
  do {_ = try await store.secretaryBrief();fatalError("in-flight patrol became Secretary generation facts")} catch {}
  var stale=original;stale["state"] = .string("stale");stale["currentRevision"] = .string(String(repeating:"d",count:64));response = .object(stale)
  let old=try await store.secretaryBrief();precondition(old.state=="stale" && old.body != nil && old.canRequest)
  var unknown=original;unknown["state"] = .string("unknown");unknown["publication"] = .null
  guard case .object(var unknownCase)=record,case .object(var unknownExecution)=record["executionRecord"] else {fatalError()}
  unknownExecution["phase"] = .string("unknown");unknownExecution["reason"] = .string("restart_unknown")
  unknownCase["execution"] = .string("unknown");unknownCase["executionRecord"] = .object(unknownExecution)
  unknown["case"] = .object(unknownCase);response = .object(unknown)
  let uncertain=try await store.secretaryBrief();precondition(!uncertain.canRequest)
  hold=true
  let pending=Task {try await store.secretaryBrief()};while held == nil {await Task.yield()}
  store.selectCrew("b");held!.resume(returning:.object(original))
  do {_ = try await pending.value;fatalError("late Secretary result crossed crew")} catch is CancellationError {}
  store.selectCrew("a");held=nil
  let disconnected=Task {try await store.secretaryBrief()};while held == nil {await Task.yield()}
  store.invalidateConnection();held!.resume(returning:.object(original))
  do {_ = try await disconnected.value;fatalError("late Secretary result crossed connection")} catch is CancellationError {}
 }

 @MainActor private static func requirementReviews() async throws {
  let candidate = String(repeating: "a", count: 64)
  let review: [String: CrewdValue] = [
   "crewId": .string("a"), "firstId": .string("#9"), "secondId": .string("#12"), "channelId": .string("group"),
   "revision": .string("1"), "disposition": .string("group"), "note": .string("保留两条来源"),
   "changedAtMs": .integer(31), "candidateRevision": .string(candidate)
  ]
  var result = CrewdValue.object(review)
  var responseCrew = "a"
  let store = CrewdWorkStore { method, params in
   precondition(params["crewId"] == .string("a") && params["limit"] == .integer(20))
   if method == "adminCase.similarRequirements" {
    let item: CrewdValue = .object([
     "firstId": .string("#9"), "secondId": .string("#12"), "channelId": .string("group"),
     "candidateRevision": .string(candidate), "reviewRevision": .string("0"),
     "probability": .number(0.94), "observedAtMs": .integer(25)
    ])
    return .object(["crewId": .string("a"), "enabled": .bool(true), "items": .array([item]), "nextAfter": .null])
   }
   precondition(method == "adminCase.requirementReviews")
   return .object(["crewId": .string(responseCrew),
                   "items": .array([.object(["review": result, "effective": .bool(true)])]), "nextAfter": .null])
  }
  store.selectCrew("a")
  do { _ = try await store.requirementReviewPage(); fatalError("unsupported review read") } catch {}
  store.configureCapabilities(["adminCase.requirementReviews"])
  let page = try await store.similarRequirementPage(after: nil)
  precondition(page["items"].array.first?["candidateRevision"] == .string(candidate))
  let saved = try await store.requirementReviewPage()
  precondition(saved["items"].array.first?["review"] == .object(review))
  for (key, invalid) in [("crewId", CrewdValue.string("b")), ("secondId", .string("#7")),
                         ("revision", .string("01")), ("candidateRevision", .null)] {
   var malformed = review
   malformed[key] = invalid
   result = .object(malformed)
   do { _ = try await store.requirementReviewPage(); fatalError("invalid saved pair accepted") } catch {}
  }
  result = .object(review)
  responseCrew = "b"
  do { _ = try await store.requirementReviewPage(); fatalError("foreign review page accepted") } catch {}
 }
}
