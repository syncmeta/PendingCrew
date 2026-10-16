import Foundation
@main struct StopSubmissionGateRegression {
 @MainActor static func main() async throws {
  let root=URL(fileURLWithPath:CommandLine.arguments[1])
  let workspace=CrewdWorkspace(dataDirectory:root)
  workspace.originalJournalRequired=true; await workspace.connect()
  await workspace.prepareOriginalJournal(root:root.appendingPathComponent("frontend-presentation/unknown-submissions"))
  precondition(workspace.originalJournalReady)
  let params:CrewdValue = .object(["crewId":.string("crew"),"role":.string("worker"),"executor":.string("codex"),"brief":.string("original"),"isolation":.bool(false)])
  _ = try await workspace.submitPublicCommand(.sessionStart,params:params,crew:"crew")
  let original=workspace.durableUnknown.first!
  let prepared=root.appendingPathComponent("frontend-presentation/unknown-submissions/"+workspace.presentationScope!.namespace.uuidString.lowercased()+"/prepared")
  let countBefore=try FileManager.default.contentsOfDirectory(atPath:prepared.path).count
  let exit=Task {try await workspace.exitOwnedLocalHelper(mode:.safePause)}
  while !workspace.stopping {await Task.yield()}
  let envelope=try CrewdPublicCommandEnvelope(request:UUID(),binding:workspace.client.runtimeIdentity,operation:.sessionStart,params:params)
  let attempts:[() async throws -> Void] = [
   {_ = try await workspace.submitPublicCommand(.sessionStart,params:params,crew:"crew")},
   {_ = try await workspace.performValue("session.start",params)},
   {_ = try await workspace.publicCommands.submit(envelope,crew:"crew")},
   {_ = try await workspace.makeTerminalCommands(for:workspace.client).submit(envelope,crew:"crew")}
  ]
  for attempt in attempts {
   do {try await attempt();preconditionFailure("stop accepted new submission")}
   catch let error as CrewdClientError {precondition(error.kind == .notSubmitted)}
  }
  let countAfter=try FileManager.default.contentsOfDirectory(atPath:prepared.path).count
  precondition(countBefore==countAfter)
  let lookup=try await workspace.lookupOriginalCommand(original)
  precondition(lookup?.phase == .unknown)
  do {_ = try await exit.value;preconditionFailure("timedOut became stopped")} catch {}
  precondition(workspace.stopping)
  let target=CrewdRetiredManagement.Target(database:"retired-db",instance:"retired-instance")
  _ = try await CrewdRetiredManagement.exit(target:target,mode:.safePause) {
   .init(target:target,call:{method,fields in
    if method == "lifecycle.status" {return .object(["role":.string("owner"),"ownerPresent":.bool(true)])}
    precondition(method == "lifecycle.exit" && fields["timeoutMs"].int == 120000)
    return .object(["status":.string("stopped")])
   },close:{})
  }
  workspace.retire()
  print("PASS actual Workspace stop gate: formal, compatibility, held public and terminal submit reject before journal; original UUID pure lookup remains; timeout stays stopped-for-admission; retired safe deadline120000")
 }
}
