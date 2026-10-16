import Foundation
@main struct StopCoordinatorRegression {
 @MainActor static func main() async throws {
  var stopped=0, forced=0, completed=0
  var stopWait:CheckedContinuation<Void,Error>?, forceWait:CheckedContinuation<Void,Error>?
  let coordinator=CrewdStopCoordinator(stop:{stopped += 1;try await withCheckedThrowingContinuation {stopWait=$0}},force:{forced += 1;try await withCheckedThrowingContinuation {forceWait=$0}},mayForce:{true})
  coordinator.completed={completed += 1}
  coordinator.begin(); coordinator.begin()
  while stopWait == nil { await Task.yield() }
  precondition(stopped==1 && forced==0 && completed==0 && coordinator.state == .stopping)
  // Hiding the panel has no cancellation API on this retained coordinator.
  coordinator.requestForce(); coordinator.requestForce()
  while forceWait == nil { await Task.yield() }
  precondition(forced==1 && completed==0 && coordinator.state == .forcing)
  stopWait!.resume(throwing:NSError(domain:"cooperative timeout",code:1));stopWait=nil
  await Task.yield(); precondition(completed==0)
  forceWait!.resume();forceWait=nil
  while completed==0 { await Task.yield() }
  precondition(completed==1 && coordinator.state == .stopped)
  coordinator.requestForce(); precondition(forced==1)
  var observerStop=0,observerForce=0,observerDone=0
  let observer=CrewdStopCoordinator(stop:{observerStop += 1},force:{observerForce += 1},mayForce:{false})
  observer.completed={observerDone += 1};observer.begin(forceImmediately:true);observer.requestForce()
  while observerDone==0 { await Task.yield() }
  precondition(observerStop==1 && observerForce==0 && observerDone==1)
  var unknownDone=0
  let unknown=CrewdStopCoordinator(stop:{throw NSError(domain:"unknown",code:1)},force:{throw NSError(domain:"force unknown",code:1)},mayForce:{true})
  unknown.completed={unknownDone += 1};unknown.begin()
  while unknown.state == .stopping { await Task.yield() }
  precondition(unknownDone==0 && unknown.forceAvailable)
  unknown.requestForce()
  while unknown.state == .forcing { await Task.yield() }
  precondition(unknownDone==0 && unknown.forceAvailable)
  print("PASS production stop coordinator: one cooperative request; force joins independently; timeout never completes; success once; observer never force; no cancellation on hidden prompt")
 }
}
