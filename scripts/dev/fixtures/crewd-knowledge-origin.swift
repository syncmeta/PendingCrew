import Foundation
@main struct KnowledgeOriginRegression {
 @MainActor static func main() async throws {
  let root=URL(fileURLWithPath:CommandLine.arguments[1]), control=URL(fileURLWithPath:CommandLine.arguments[2])
  let workspace=CrewdWorkspace(dataDirectory:root,authorityRecordID:try CrewdPresentationJournal.localNamespace(directory:root))
  workspace.originalJournalRequired=true
  await workspace.connect()
  await workspace.prepareOriginalJournal(root:root.appendingPathComponent("frontend-presentation/unknown-submissions"))
  precondition(workspace.originalJournalReady)
  let scope=workspace.presentationScope!
  precondition(scope.authorityID == nil, "local remote pairing is intentionally absent")
  let created=try await workspace.performValue("crew.create",.object(["title":.string("Knowledge origin control"),"workingDirectory":.string(root.appendingPathComponent("work").path)]))
  let crew=created["id"].string!
  try Data(crew.utf8).write(to:control)
  let receipt = try await workspace.submitPublicCommand(.sessionStart,params:.object(["crewId":.string(crew),"role":.string("captain"),"executor":.string("codex"),"brief":.string("Generate saved knowledge"),"isolation":.bool(false)]),crew:crew)
  print("session receipt",receipt.value.pretty)
  for _ in 0..<100 {
   await workspace.knowledge.refresh()
   if !workspace.knowledge.objects.isEmpty { break }
   try await Task.sleep(for:.milliseconds(50))
  }
  let store=workspace.knowledge
  precondition(store.workspace?.context.authorityId == scope.databaseID)
  precondition(store.workspace?.context.dbId == scope.databaseID)
  precondition(store.objects.count == 1,store.status)
  let context=store.workspace!.context
  await store.open(.init(namespace:context.namespace,kind:"view",id:store.objects[0].identity))
  precondition(store.renderer.context != nil && !store.renderer.executionEnabled && !store.renderer.writeEnabled)
  precondition(store.detail != nil)
  // A remote pairing scope stays part of Origin even though its authority is
  // deliberately different from SQLite authority. No remote transport is faked.
  var origin=CrewdKnowledgeStore.Origin(scope:.init(namespace:scope.namespace,localDirectory:nil,authorityID:"paired-authority",databaseID:scope.databaseID,grantID:"grant",grantRevision:"1"),observation:UUID(),connection:UUID(),crew:crew)
  var response=context
  let scoped=CrewdKnowledgeStore(origin:{origin},rpc:{method,_ in
   if method == "data.workspace.get" { return try CrewdKnowledgeStore.encode(CrewdDataWorkspaceInfo(context:response,state:"uninitialized",engineVersion:"sqlite",reason:nil)) }
   preconditionFailure("unexpected read")
  },journal:{try CrewdPresentationJournal(root:root.appendingPathComponent("remote-journal"),scope:origin.scope)})
  await scoped.refresh(); precondition(scoped.workspace != nil)
  origin = .init(scope:.init(namespace:scope.namespace,localDirectory:nil,authorityID:"paired-authority",databaseID:scope.databaseID,grantID:"grant",grantRevision:"2"),observation:origin.observation,connection:origin.connection,crew:crew)
  scoped.synchronizeOrigin(); precondition(scoped.workspace == nil)
  response.dbId="other-db"; await scoped.refresh(); precondition(scoped.workspace == nil)
  response=context; response.authorityId="other-authority"; await scoped.refresh(); precondition(scoped.workspace == nil)
  workspace.retire(); precondition(store.renderer.context == nil)
  print("PASS production Workspace→journal→Origin→Store: local pairing nil, formal Agent MCP saved web reads; code remains disabled; separate remote grant change and foreign db/authority refuse; retire revokes")
 }
}
