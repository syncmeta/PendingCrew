import Foundation
@main struct CoordinationFixture {
 @MainActor static func main() async throws {
  var response:CrewdValue = .object(["crewId":.string("a"),"id":.string("W1"),"record":.null])
  var delay=false
  var expectedDecision: String?
  var reads = 0
  var childDetail: CrewdValue = .null
  var held:CheckedContinuation<CrewdValue,Error>?
  let store=CrewdWorkStore {method,params in
   if method == "work.get" {
    precondition(params == .object(["id":.string("W9")]))
    return childDetail
   }
   precondition(method == "work.coordinationStatus" && params["crewId"] == .string("a") && params["id"] == .string("W1") &&
      params["decisionId"].string == expectedDecision)
   reads += 1
   if delay {return try await withCheckedThrowingContinuation {held=$0}}
   return response
  }
  store.selectCrew("a")
  let absent = try await store.coordinationStatus(crew:"a",id:"W1")
  precondition(absent.record == .null && absent.lastAssignment == nil && absent.lastSplit == nil)
  for bad:CrewdValue in [.null,.object(["crewId":.string("a"),"id":.string("W1")]),.object(["crewId":.string("b"),"id":.string("W1"),"record":.null]),.object(["crewId":.string("a"),"id":.string("W1"),"record":.object([:])])] {
   response=bad
   do {_ = try await store.coordinationStatus(crew:"a",id:"W1");fatalError("malformed accepted")} catch {}
  }
  var row:[String:CrewdValue] = ["decisionId":.string("d"),"phase":.string("settled"),"reason":.string("answered"),"meaningfulProgress":.number(0.2),"needsCoordination":.number(0.8),"adopted":.bool(true),"recipient":.null,"wakeId":.null,"wakeState":.null]
  response = .object(["crewId":.string("a"),"id":.string("W1"),"record":.object(row)])
  let deliveredWithoutReceipt = try await store.coordinationStatus(crew:"a",id:"W1")
  precondition(deliveredWithoutReceipt.lastAssignment == nil)
  row["needsCoordination"] = .number(2)
  response = .object(["crewId":.string("a"),"id":.string("W1"),"record":.object(row)])
  do {_ = try await store.coordinationStatus(crew:"a",id:"W1");fatalError("invalid probability")} catch {}
  row["needsCoordination"] = .number(0.8)
  row["recipient"] = .string("captain")
  row["wakeId"] = .integer(9)
  row["wakeState"] = .string("delivered")
  response = .object(["crewId":.string("a"),"id":.string("W1"),"record":.object(row),"lastAssignment":.null])
  let justDelivered = try await store.coordinationStatus(crew:"a",id:"W1")
  precondition(justDelivered.record["wakeState"] == .string("delivered") && justDelivered.lastAssignment == nil && justDelivered.lastSplit == nil)
  let oldDecision = "00000000-0000-0000-0000-000000000001"
  let exact:[String:CrewdValue] = ["crewId":.string("a"),"id":.string("W1"),"decisionId":.string(oldDecision),
    "captain":.string("captain"),"previousOwner":.string("old"),"owner":.string("new"),"previousRevision":.string("3"),
    "revision":.string("4"),"eventSeq":.string("107"),"assignedAtMs":.integer(1_790_000_000_123)]
  func payload(_ assignment:CrewdValue,record:CrewdValue = .null,requested:CrewdValue? = nil,
               split:CrewdValue? = nil,requestedSplit:CrewdValue? = nil) -> CrewdValue {
   var fields: [String:CrewdValue] = ["crewId":.string("a"),"id":.string("W1"),"record":record,"lastAssignment":assignment]
   if let requested { fields["requestedAssignment"] = requested }
   if let split { fields["lastSplit"] = split }
   if let requestedSplit { fields["requestedSplit"] = requestedSplit }
   return .object(fields)
  }
  response = payload(.object(exact))
  let historical = try await store.coordinationStatus(crew:"a",id:"W1")
  precondition(historical.record == .null && historical.lastAssignment?.decisionID == oldDecision &&
     historical.lastAssignment?.owner == "new" && historical.lastAssignment?.eventSequence == "107")
  row["phase"] = .string("prepared")
  row["reason"] = .string("prepared")
  row["adopted"] = .bool(false)
  row["wakeState"] = .null
  response = payload(.object(exact),record:.object(row))
  let newer = try await store.coordinationStatus(crew:"a",id:"W1")
  precondition(newer.record["decisionId"] == .string("d") && newer.lastAssignment == historical.lastAssignment)
  for (key,value) in [
   ("crewId",CrewdValue.string("b")),("id",.string("W2")),("decisionId",.string("not-a-uuid")),
   ("captain",.null),("owner",.string("old")),("previousRevision",.string("03")),("revision",.string("5")),
   ("eventSeq",.string("0")),("assignedAtMs",.number(1.0)),("assignedAtMs",.integer(-1))
  ] {
   var bad = exact; bad[key] = value
   response = payload(.object(bad),record:.object(row))
   do {_ = try await store.coordinationStatus(crew:"a",id:"W1");fatalError("invalid assignment \(key)")} catch {}
  }
  let readsBeforeMalformed = reads
  do {_ = try await store.coordinationStatus(crew:"a",id:"W1",decisionID:"malformed");fatalError("invalid history identity")} catch {}
  precondition(reads == readsBeforeMalformed)
  var newerAssignment = exact
  newerAssignment["decisionId"] = .string("00000000-0000-0000-0000-000000000002")
  newerAssignment["previousRevision"] = .string("8")
  newerAssignment["revision"] = .string("9")
  newerAssignment["eventSeq"] = .string("121")
  response = payload(.object(newerAssignment),record:.object(row),requested:.object(exact))
  do {_ = try await store.coordinationStatus(crew:"a",id:"W1");fatalError("unsolicited history")} catch {}
  expectedDecision = oldDecision
  let original = try await store.coordinationStatus(crew:"a",id:"W1",decisionID:oldDecision)
  precondition(original.lastAssignment?.revision == "9" && original.requestedAssignment == historical.lastAssignment)
  response = payload(.object(newerAssignment),requested:.object(newerAssignment))
  do {_ = try await store.coordinationStatus(crew:"a",id:"W1",decisionID:oldDecision);fatalError("wrong historical identity")} catch {}
  response = payload(.object(newerAssignment))
  let notFound = try await store.coordinationStatus(crew:"a",id:"W1",decisionID:oldDecision)
  precondition(notFound.requestedAssignment == nil && notFound.lastAssignment?.revision == "9")

  let splitDecision = "00000000-0000-0000-0000-000000000003"
  let latestDecision = "00000000-0000-0000-0000-000000000004"
  let first:[String:CrewdValue] = ["id":.string("W9"),"owner":.string("worker"),"revision":.string("1"),"eventSeq":.string("124")]
  let second:[String:CrewdValue] = ["id":.string("W10"),"owner":.null,"revision":.string("1"),"eventSeq":.string("125")]
  let split:[String:CrewdValue] = ["crewId":.string("a"),"id":.string("W1"),"decisionId":.string(splitDecision),
      "captain":.string("captain"),"parentRevision":.string("9"),"splitAtMs":.integer(1_790_000_000_127),
      "children":.array([.object(first),.object(second)])]
  func changed(_ source:[String:CrewdValue],_ key:String,_ value:CrewdValue) -> [String:CrewdValue] {
   var result=source;result[key]=value;return result
  }
  let newest = changed(split,"decisionId",.string(latestDecision))
  expectedDecision = splitDecision
  response = payload(.object(newerAssignment),split:.object(newest),requestedSplit:.object(split))
  let exactSplit = try await store.coordinationStatus(crew:"a",id:"W1",decisionID:splitDecision)
  precondition(exactSplit.lastAssignment?.revision == "9" && exactSplit.lastSplit?.decisionID == latestDecision)
  guard case .split(let historicSplit)? = exactSplit.requestedOutcome else { fatalError("missing split outcome") }
  precondition(historicSplit.decisionID == splitDecision && historicSplit.parentRevision == "9" &&
               historicSplit.children.map { $0.id.raw } == ["W9","W10"] &&
               historicSplit.children.map(\.owner) == ["worker",nil] &&
               historicSplit.children.map(\.eventSequence) == ["124","125"])
  response = payload(.object(newerAssignment),split:.object(newest),requestedSplit:.object(newest))
  do {_ = try await store.coordinationStatus(crew:"a",id:"W1",decisionID:splitDecision);fatalError("wrong split decision")} catch {}
  response = payload(.object(changed(newerAssignment,"decisionId",.string(splitDecision))),
                     requested:.object(changed(newerAssignment,"decisionId",.string(splitDecision))),requestedSplit:.object(split))
  do {_ = try await store.coordinationStatus(crew:"a",id:"W1",decisionID:splitDecision);fatalError("mutually exclusive outcomes")} catch {}
  expectedDecision = nil
  response = payload(.object(newerAssignment),split:.object(newest),requestedSplit:.object(split))
  do {_ = try await store.coordinationStatus(crew:"a",id:"W1");fatalError("unsolicited split history")} catch {}
  var missingOwner=first;missingOwner.removeValue(forKey:"owner")
  for bad in [
   changed(split,"id",.string("W2")), changed(split,"crewId",.string("other")), changed(split,"parentRevision",.string("09")),
   changed(split,"children",.array([.object(first)])), changed(split,"children",.array(Array(repeating:.object(first),count:9))),
   changed(split,"children",.array([.object(first),.object(changed(second,"id",.string("W9")))])),
   changed(split,"children",.array([.object(first),.object(changed(second,"id",.string("W1")))])),
   changed(split,"children",.array([.object(first),.object(changed(second,"revision",.string("2")))])),
   changed(split,"children",.array([.object(first),.object(changed(second,"eventSeq",.string("124")))])),
   changed(split,"children",.array([.object(missingOwner),.object(second)])),
   changed(split,"children",.array([.object(second),.object(first)]))
  ] {
   response = payload(.null,split:.object(bad))
   do {_ = try await store.coordinationStatus(crew:"a",id:"W1");fatalError("invalid split receipt")} catch {}
  }
  let plan:[String:CrewdValue] = ["id":.string("W9"),"crewId":.string("a"),"revision":.string("5"),"status":.string("open"),
    "title":.string("current child"),"sources":.array([]),"parent":.string("W1")]
  childDetail = .object(plan)
  let child = try await store.coordinationChildDetail(historicSplit.children[0].id,crew:"a",parent:"W1")
  precondition(child.id.raw == "W9" && child.revision == "5" && store.plans[child.id] == nil)
  for bad in [changed(plan,"parent",.string("W2")),changed(plan,"crewId",.string("other")),changed(plan,"id",.string("W8"))] {
   childDetail = .object(bad)
   do {_ = try await store.coordinationChildDetail(historicSplit.children[0].id,crew:"a",parent:"W1");fatalError("wrong child details")} catch {}
  }
  expectedDecision = splitDecision
  delay=true
  let task=Task {try await store.coordinationStatus(crew:"a",id:"W1",decisionID:splitDecision)}
  while held == nil {await Task.yield()}
  store.invalidateConnection();held!.resume(returning:payload(.object(newerAssignment),split:.object(newest),requestedSplit:.object(split)))
  do {_ = try await task.value;fatalError("stale accepted")} catch is CancellationError {}
  print("PASS legacy coordination, separate assignment/split, exact history, scoped child detail, malformed and stale rejection")
 }
}
