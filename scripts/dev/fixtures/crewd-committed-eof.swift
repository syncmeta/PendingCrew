import Foundation
@main struct CommittedEOFQA {
 @MainActor static func main() async throws {
  let root=URL(fileURLWithPath:CommandLine.arguments[1]),socket=root.appendingPathComponent("proxy.sock").path
  let client=CrewdClient();try await client.connect(path:socket)
  let crew=try await client.call("crew.create",.object(["title":.string("Committed EOF synthetic crew"),"workingDirectory":.string(root.appendingPathComponent("work").path)]))["id"].string!
  let store=CrewdWorkStore{method,params in try await client.call(method,params)};store.configureCapabilities(client.capabilities);store.selectCrew(crew)
  var drafts=CrewdRequirementDrafts();drafts.set(crew,text:"Original actual accepted request whose wire receipt is lost");let snapshot=drafts.begin(crew)!
  do {_=try await store.createRequirement(requestID:snapshot.requestID,text:snapshot.text);preconditionFailure("proxy delivered the deliberately withheld receipt")}catch {precondition(CrewdClientError.isUncertainSubmission(error));drafts.finish(snapshot,accepted:false,uncertain:true)}
  precondition(!client.connected && drafts.uncertain(crew)==[snapshot],"actual EOF must remain uncertain with exact original payload")
  drafts.set(crew,text:"New edited input survives confirmation")
  try await client.connect(path:socket);store.configureCapabilities(client.capabilities);try await store.refresh()
  precondition(store.requirements.count==1,"read-only reconnect must show the one committed requirement")
  let original=store.requirements.values.first!
  precondition(original["text"] == .string(snapshot.text));try Data().write(to:root.appendingPathComponent("readonly-complete"))
  precondition(drafts.confirm(snapshot),"original UUID confirmation must remain explicit")
  let confirmed=try await store.createRequirement(requestID:snapshot.requestID,text:snapshot.text);drafts.finish(snapshot,accepted:true)
  precondition(confirmed["requirementId"] == original["requirementId"] && drafts.text(crew)=="New edited input survives confirmation")
  try await store.refresh();precondition(store.requirements.count==1)
  client.disconnect();print("PASS: normal packaged backend committed request + actual socket EOF; original unknown UUID/draft; read-only reconnect zero replay; explicit same-request confirmation one requirement")
 }
}
