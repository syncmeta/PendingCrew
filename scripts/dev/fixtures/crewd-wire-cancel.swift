import Foundation
@main struct CancelQA {
 @MainActor static func main() async throws {
  let root=URL(fileURLWithPath:CommandLine.arguments[1]);let client=CrewdClient();var events=0;var replacement:Task<Void,Error>?
  client.onEvent={name,data in
   if name=="work.changed" && data["marker"] == .string("unaffected") {events+=1}
   if name=="switch.authority" {client.disconnect();replacement=Task {try await client.connect(path:root.appendingPathComponent("s").path)}}
  }
  try await client.connect(path:root.appendingPathComponent("s").path)
  let before=Task {try await client.call("pre-cancel")};before.cancel()
  do {_ = try await before.value;preconditionFailure("pre-cancel accepted")}catch {precondition(!CrewdClientError.isUncertainSubmission(error))}
  let sent=Task {try await client.call("hold")}
  for _ in 0..<200 {if FileManager.default.fileExists(atPath:root.appendingPathComponent("held").path){break};try await Task.sleep(for:.milliseconds(10))}
  precondition(FileManager.default.fileExists(atPath:root.appendingPathComponent("held").path));sent.cancel()
  do {_ = try await sent.value;preconditionFailure("sent cancellation accepted")}catch {precondition(CrewdClientError.isUncertainSubmission(error))}
  let other=try await client.call("other");precondition(other == .string("other-ok") && events==1 && client.connected)
  do {_ = try await client.call("error");preconditionFailure("structured error accepted")}catch let error as CrewdClientError {precondition(error.code == -32009 && error.data["currentWork"]["revision"] == .string("9007199254740993") && !CrewdClientError.isUncertainSubmission(error))}
  do {_=try await client.call("switch");preconditionFailure("old generation pending survived switch")}catch {precondition(CrewdClientError.isUncertainSubmission(error))}
  guard let replacement else {preconditionFailure("replacement connection absent")};try await replacement.value
  let fresh=try await client.call("probe");precondition(fresh == .string("fresh-ok"));client.disconnect()
  let methods=try String(contentsOf:root.appendingPathComponent("requests"),encoding:.utf8).split(separator:"\n")
  precondition(!methods.contains("pre-cancel") && methods.filter{$0=="hold"}.count==1 && methods.filter{$0=="other"}.count==1 && methods.filter{$0=="probe"}.count==1)
  print("PASS: pre-send cancellation zero writes; actual sent cancellation unknown; unrelated response/event and structured error survive; late reply ignored; callback generation switch plus old EOF cannot kill replacement; reconnect does not replay")
 }
}
