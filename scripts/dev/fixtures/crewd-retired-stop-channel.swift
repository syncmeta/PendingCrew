import Foundation
@main struct RetiredStopChannelRegression {
 @MainActor static func main() async throws {
  let target=CrewdRetiredManagement.Target(database:"db",instance:"instance")
  let channel=CrewdRetiredStopChannel()
  var connections=0,closed=0
  var safe:CheckedContinuation<CrewdValue,Error>?,force:CheckedContinuation<CrewdValue,Error>?
  let connect:() async throws -> CrewdRetiredManagement.Connection = {
   connections += 1
   return .init(target:target,call:{method,params in
    if method == "lifecycle.status" { return .object(["role":.string("owner"),"ownerPresent":.bool(true)]) }
    precondition(method == "lifecycle.exit")
    return try await withCheckedThrowingContinuation { if params["mode"].string == "direct" {force=$0} else {safe=$0} }
   },close:{closed += 1})
  }
  let cooperative=Task {try await channel.exit(target:target,mode:.safePause,connect:connect)}
  while safe == nil {await Task.yield()}
  let forced=Task {try await channel.exit(target:target,mode:.direct,connect:connect)}
  while force == nil {await Task.yield()}
  precondition(connections==1 && closed==0)
  force!.resume(returning:.object(["status":.string("stopped")]));force=nil
  let didForce=try await forced.value;precondition(didForce && closed==0)
  safe!.resume(returning:.object(["status":.string("timedOut")]));safe=nil
  do {_ = try await cooperative.value;preconditionFailure("timeout counted as stopped")} catch {}
  precondition(connections==1 && closed==1)
  print("PASS production retired stop channel: cooperative and force share one authenticated management socket; early force completion does not close pending original request; timeout remains unconfirmed; final borrower closes once")
 }
}
