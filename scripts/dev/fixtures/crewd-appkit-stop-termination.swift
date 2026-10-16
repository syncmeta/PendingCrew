// Native regression for the real PendingCrewApplicationDelegate + QuitPolicy.
// Compile with the delegate class extracted verbatim from PendingCrewApp.swift,
// CrewdQuitPolicy.swift and a CrewdValue fixture. Legacy-only dependencies below
// are stubs; the exercised crewd delegate, coordinator and NSPanel are production.
// Run success / force / unknown separately; no Core, model or candidate is opened.
import AppKit
func log(_ text:String) { FileHandle.standardOutput.write(Data((text+"\n").utf8)) }
enum DaemonLaunchRace { enum ChildState {} }
enum AppQuitDaemonPolicy {
 enum Reason { case sparkleInstall, explicitQuit }
 enum Stop { case alreadyStopped(String) }
 enum Decision { case allowTermination, cancelTermination, stopFailed(String) }
 static func resolveReason(sparkleInstalling:Bool,userRequestedQuit:Bool)->Reason { .explicitQuit }
 static func evaluate(reason:Reason,runningSessionCount:Int,confirmInterruption:(Int)->Bool,stopDaemon:()->Stop)->Decision { .cancelTermination }
 static func stopAfterPendingLaunch(pendingLaunch:(()->DaemonLaunchRace.ChildState)?,presence:()->SessionOrchestratorLock.Presence,stopDaemon:()->Stop)->Stop { .alreadyStopped("") }
}
enum PendingCrewDaemonPaths { struct Paths { let lock=URL(fileURLWithPath:"/unused") };static func standard()->Paths { Paths() } }
enum SessionOrchestratorLock { struct Holder { var kind="";var pid:Int32=0 };enum Presence {case held(Holder),absent};static func presence(dataRoot:URL)->Presence {.absent} }
struct DaemonStopper { let dataRoot:URL;func stop()->AppQuitDaemonPolicy.Stop {.alreadyStopped("")} }
@MainActor final class AppQuitLifecycle {
 static let shared=AppQuitLifecycle();var sparkleInstalling=false,userRequestedQuit=false,isSystemQuitEvent=false
 var crewdExit:((CrewdQuitMode) async throws->Void)?;var crewdMayForceStop:(()->Bool)?
 var runningSessionCount:(()->Int)?;var pauseViewer:(()->(()->DaemonLaunchRace.ChildState)?)?;var resumeViewer:(()->Void)?
}
@MainActor final class Probe:NSObject,NSApplicationDelegate {
 let production=PendingCrewApplicationDelegate()
 let mode=CommandLine.arguments.last!
 var safe=0,force=0,delegates=0,done=false
 var stopWait:CheckedContinuation<Void,Error>?
 func applicationShouldTerminate(_ sender:NSApplication)->NSApplication.TerminateReply {
  delegates += 1;log("delegate \(delegates)")
  let result=production.applicationShouldTerminate(sender)
  log("decision \(result.rawValue)");return result
 }
 func applicationDidFinishLaunching(_ n:Notification) {
  UserDefaults.standard.setVolatileDomain([CrewdQuitPreference.key:"background"],forName:UserDefaults.argumentDomain)
  AppQuitLifecycle.shared.crewdMayForceStop={true}
  AppQuitLifecycle.shared.crewdExit={ [self] mode in
   if mode == .direct { force += 1;log("force request");done=true;stopWait?.resume(throwing:NSError(domain:"safe timeout",code:1));stopWait=nil;return }
   safe += 1;log("safe request")
   if self.mode == "unknown" { throw NSError(domain:"unknown",code:1) }
   try await withCheckedThrowingContinuation {stopWait=$0}
  }
  DispatchQueue.main.asyncAfter(deadline:.now()+0.1) { NSApp.terminate(nil) }
  DispatchQueue.main.asyncAfter(deadline:.now()+0.25) { self.panel()?.performClose(nil);log("hide") }
  DispatchQueue.main.asyncAfter(deadline:.now()+0.4) { precondition(self.safe==1 && !self.done);NSApp.terminate(nil);precondition(self.panel()?.isVisible==true);log("repeat kept alive") }
  DispatchQueue.main.asyncAfter(deadline:.now()+0.6) {
   precondition(self.safe==1 && self.force==0 && self.delegates==2)
   if self.mode == "force" {
    func find(_ view:NSView)->NSButton? {if let b=view as? NSButton,b.title=="强制关闭" {return b};return view.subviews.compactMap {find($0)}.first}
    let button=find(self.panel()!.contentView!)!;precondition(button.isEnabled);button.performClick(nil)
   } else if self.mode == "unknown" {
    precondition(self.panel()?.title=="停工尚未确认");log("PASS unknown stays alive; no resubmit");exit(0)
   } else {self.done=true;self.stopWait!.resume();self.stopWait=nil;log("settled")}
  }
  DispatchQueue.global().asyncAfter(deadline:.now()+3) {log("FAIL bounded timeout");exit(99)}
 }
 func panel()->NSWindow? {NSApp.windows.first {$0 is NSPanel}}
 func applicationWillTerminate(_ n:Notification) { precondition(done && safe==1 && delegates==3 && (mode != "force" || force==1));log("PASS actual delegate terminates only after settlement") }
}
@main struct Main {
 @MainActor static func main() { let app=NSApplication.shared;let delegate=Probe();app.delegate=delegate;app.setActivationPolicy(.accessory);app.run() }
}
