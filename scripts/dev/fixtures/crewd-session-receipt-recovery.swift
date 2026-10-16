import Foundation
@main struct SessionReceiptRecovery {
 @MainActor static func main() async throws {
  let root=URL(fileURLWithPath:CommandLine.arguments[1]), phase=CommandLine.arguments[2]
  let workspace=CrewdWorkspace(dataDirectory:root)
  workspace.originalJournalRequired=true; await workspace.connect()
  await workspace.prepareOriginalJournal(root:root.appendingPathComponent("frontend-presentation/unknown-submissions"))
  precondition(workspace.originalJournalReady)
  let context=CrewdDrafts.Context.initialTask(crew:"crew",session:"original-composer")
  workspace.messageDrafts.set("Explicit task",for:context)
  let snapshot=workspace.messageDrafts.snapshot(for:context)
  do { _ = try await workspace.performValue("session.start",.object(["crewId":.string("crew"),"role":.string("worker"),"executor":.string("codex"),"brief":.string(snapshot.text),"isolation":.bool(false)])); preconditionFailure("dispatched must remain unresolved") }
  catch { workspace.recordUncertainSessionStart(snapshot,error:error) }
  precondition(workspace.uncertainSessionStartRecords.count == 1 && workspace.uncertainSessionStartRecords[0].request != nil)
  let record=workspace.durableUnknown.first!
  let id=workspace.uncertainSessionStartRecords[0].request!
  precondition(record.params["requestId"].string.flatMap(UUID.init(uuidString:)) == id)
  // An unmatched transport failure is not cleared alongside a known UUID.
  workspace.recordUncertainSessionStart(snapshot,error:CrewdClientError(message:"unmatched transport failure"))
  let outcome = root.appendingPathComponent("frontend-presentation/unknown-submissions/"+workspace.presentationScope!.namespace.uuidString.lowercased()+"/outcomes/"+record.id.uuidString.lowercased()+".json")
  if phase == "missing-outcome" {
   try FileManager.default.removeItem(at:outcome) // Synthetic crash window in this fresh fixture only.
   precondition(workspace.hasOriginalUnknown(method:"session.start",crew:"crew"))
  }
  var receipt: CrewdPublicReceipt?
  do { receipt=try await workspace.lookupOriginalCommand(record) }
  catch { if phase != "foreign" { throw error } }
  if ["rejected","suppressed","launched","missing-outcome"].contains(phase) {
   precondition(receipt != nil && !workspace.hasOriginalUnknown(method:"session.start",crew:"crew"))
   precondition(workspace.uncertainSessionStartRecords.count == 1 && workspace.uncertainSessionStartRecords[0].request == nil)
  } else {
   precondition(workspace.hasOriginalUnknown(method:"session.start",crew:"crew"))
   precondition(workspace.uncertainSessionStartRecords.count == 2)
  }
  let reopened = try CrewdPresentationJournal(root:root.appendingPathComponent("frontend-presentation/unknown-submissions"),scope:workspace.presentationScope!)
  let pending = try reopened.unknownRecords()
  precondition(pending.contains { $0.id == record.id } == !["rejected","suppressed","launched","missing-outcome"].contains(phase))
  if phase == "missing-outcome" {
   precondition(!FileManager.default.fileExists(atPath:outcome.path))
  } else {
   let decoder = JSONDecoder(); decoder.dateDecodingStrategy = .iso8601
   let originalOutcome = try decoder.decode(CrewdPresentationJournal.Outcome.self,from:Data(contentsOf:outcome))
   precondition(originalOutcome.state == "unknown")
  }
  workspace.retire()
  print("PASS original UUID dispatched→lookup \(phase): matched draft only; unmatched/Unknown/missing/historical/foreign remain; no resend")
 }
}
