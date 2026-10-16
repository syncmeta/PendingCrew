import Foundation
import Combine
@MainActor protocol PendingCrewBackend {}
@MainActor final class CrewdPendingCrewBackend: PendingCrewBackend { let workspace:CrewdWorkspace; init(workspace:CrewdWorkspace) { self.workspace=workspace } }
@MainActor final class AppQuitLifecycle { static let shared=AppQuitLifecycle(); var crewdExit:((CrewdQuitMode) async throws -> Void)? }
// Outward UI backend holder/quit hook and unused remote pairing are substitutes.
// AppModel, RemoteWindow's local path, Workspace, Client, IPC, config and journal
// are the actual production implementations. No remote flow is exercised here.
struct CrewdRemotePairSummary: Equatable { let id:UUID; let paired:Bool }
enum CrewdRemoteCredentials { static func application() -> Int { 0 } }
final class CrewdRemotePairing {
 static let changed=Notification.Name("unused-regression-pairing")
 init(credentials:Int) {}
 func summaries() throws -> [CrewdRemotePairSummary] { [] }
 func paired(_ id:UUID) throws -> (Int,Int,CrewdRemoteReceipt) { throw CocoaError(.featureUnsupported) }
 func forget(_ id:UUID) throws {}
}
enum CrewdRemoteTransport {
 @MainActor static func connector(id:UUID,pairing:CrewdRemotePairing) -> CrewdWorkspace.Connector { { _ in throw CocoaError(.featureUnsupported) } }
}
@main struct OriginalInitializationRegression {
 @MainActor static func main() async throws {
  let root=CommandLine.arguments[1], mode=CommandLine.arguments[2]
  let canonical=try XCTDirectory(root)
  precondition(CrewdFrontendConfiguration.directory?.path == canonical)
  precondition(CrewdFrontendConfiguration.candidateDirectory(path:canonical+"/missing/leaf")?.path == canonical+"/missing/leaf")
  let alias=canonical+"/leaf-alias"
  try FileManager.default.createSymbolicLink(atPath:alias,withDestinationPath:canonical)
  precondition(CrewdFrontendConfiguration.candidateDirectory(path:alias) == nil)
  precondition(CrewdFrontendConfiguration.candidateDirectory(path:canonical+"/pcw-delivery-validation.f401/missing") == nil)
  let daily=FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/PendingCrew").path
  precondition(CrewdFrontendConfiguration.candidateDirectory(path:daily+"/child") == nil)
  let model=AppModel()
  var finished=false, preReadySamples=0
  let task=Task { await model.connectCrewdAuthority(); finished=true }
  while !finished {
   if let workspace=model.crewdWindow.workspace, workspace.client.connected, !workspace.client.readyForWork {
    preReadySamples += 1
    precondition(model.crewdBackend == nil && !workspace.originalJournalReady)
    await workspace.prepareOriginalJournal(root:URL(fileURLWithPath:canonical+"/frontend-presentation/unknown-submissions"))
    precondition(!FileManager.default.fileExists(atPath:canonical+"/frontend-presentation/unknown-submissions"))
    do { _ = try await workspace.client.call("crew.create", .object([:])) ; preconditionFailure("execution before ready") }
    catch let error as CrewdClientError { precondition(error.kind == .notSubmitted) }
   }
   try await Task.sleep(for:.milliseconds(10))
  }
  await task.value
  for _ in 0..<30 { if model.crewdBackend != nil || mode == "journal-refusal" { break }; try await Task.sleep(for:.milliseconds(10)) }
  let workspace=model.crewdWindow.workspace!
  precondition(workspace.client.readyForWork)
  if mode == "journal-refusal" {
   precondition(model.crewdBackend == nil && !workspace.originalJournalReady)
   precondition(workspace.error?.contains("原前端提交历史未就绪") == true)
   let error=workspace.error
   await workspace.prepareOriginalJournal(root:URL(fileURLWithPath:canonical+"/frontend-presentation/unknown-submissions"))
   precondition(model.crewdBackend == nil && workspace.error == error)
  } else {
   if mode == "delayed" { precondition(preReadySamples > 20) }
   precondition(model.crewdBackend?.workspace === workspace && workspace.originalJournalReady)
   precondition(workspace.presentationScope?.localDirectory == canonical)
   precondition(workspace.presentationScope?.databaseID == workspace.client.runtimeIdentity["dbId"].string)
   precondition(workspace.error == nil, workspace.error ?? "")
  }
  workspace.client.disconnect()
  try await Task.sleep(for:.milliseconds(100))
  precondition(model.crewdBackend == nil)
  workspace.retire()
  print("PASS original initialization \(mode): readiness samples=\(preReadySamples), canonical=\(canonical); no pre-ready execution or journal poisoning; disconnect unpublishes")
 }
 static func XCTDirectory(_ path:String) throws -> String {
  guard let pointer=realpath(path,nil) else { throw CocoaError(.fileReadNoSuchFile) }; defer {free(pointer)}
  return String(cString:pointer)
 }
}
