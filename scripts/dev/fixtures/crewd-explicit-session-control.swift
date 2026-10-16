import Foundation
@main struct ExplicitControlRegression {
 @MainActor static func main() async throws {
  let root=URL(fileURLWithPath:CommandLine.arguments[1]),mode=CommandLine.arguments[2]
  let workspace=CrewdWorkspace(dataDirectory:root);workspace.originalJournalRequired=true;await workspace.connect()
  await workspace.prepareOriginalJournal(root:root.appendingPathComponent("frontend-presentation/unknown-submissions"))
  precondition(workspace.originalJournalReady)
  let op = mode == "interrupt" ? "session.interrupt" : mode == "stop" ? "session.stop" : "session.resume"
  var fields:[String:CrewdValue]=["sessionId":.string("selected-session")]
  if mode == "stop" {fields["reason"] = .string("explicit stop")}
  if mode == "typed" {fields["generation"] = .string("typed-old");fields["controlRevision"] = .string("7");fields["turnId"] = .null}
  if mode == "mixed" {fields["unexpected"] = .bool(true)}
  if mode == "missing" {fields["sessionId"] = .string("")}
  let params=CrewdValue.object(fields)
  let action=Task {try await workspace.performValue(op,params)}
  var stop:Task<Bool,Error>?
  if ["late","stopping"].contains(mode) {
   try await Task.sleep(for:.milliseconds(100))
   if mode == "late" {workspace.retire()}
   else {stop=Task {try await workspace.exitOwnedLocalHelper(mode:.safePause)}}
  }
  do {_ = try await action.value;precondition(mode == "interrupt" || mode == "stop")}
  catch let error as CrewdClientError {
   if ["foreign","failure","late","stopping","mixed","missing"].contains(mode) {precondition(error.kind == .notSubmitted)}
  }
  if let stop {do {_ = try await stop.value}catch {}}
  let journalRoot=root.appendingPathComponent("frontend-presentation/unknown-submissions")
  let journal=try CrewdPresentationJournal(root:journalRoot,scope:workspace.presentationScope!)
  let prepared=journalRoot.appendingPathComponent(journal.scope.namespace.uuidString.lowercased()+"/prepared")
  let records=try FileManager.default.contentsOfDirectory(atPath:prepared.path)
  let expected=["resume","interrupt","stop","typed"].contains(mode) ? 1 : 0
  precondition(records.count==expected)
  if mode == "resume" {
   let unknown=try journal.unknownRecords();precondition(unknown.count==1)
   let reopened=try CrewdPresentationJournal(root:journalRoot,scope:journal.scope)
   let reopenedIDs=try reopened.unknownRecords().map(\.id);precondition(reopenedIDs==unknown.map(\.id))
  }
  workspace.retire()
  print("PASS actual Workspace explicit control \(mode); prepared=\(records.count)")
 }
}
