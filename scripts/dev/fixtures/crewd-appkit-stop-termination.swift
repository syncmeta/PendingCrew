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
 var appOnly:Bool { ["unavailable","cancel","pending-app"].contains(mode) }
 func applicationShouldTerminate(_ sender:NSApplication)->NSApplication.TerminateReply {
  delegates += 1;log("delegate \(delegates)")
  let result=production.applicationShouldTerminate(sender)
  log("decision \(result.rawValue)");return result
 }
 func applicationDidFinishLaunching(_ n:Notification) {
  UserDefaults.standard.setVolatileDomain([CrewdQuitPreference.key:mode == "force" || mode == "unavailable" ? "direct" : "background"],forName:UserDefaults.argumentDomain)
  AppQuitLifecycle.shared.crewdMayForceStop={ !self.appOnly }
  AppQuitLifecycle.shared.crewdExit={ [self] mode in
   if mode == .direct { force += 1;log("backend direct request");done=true;stopWait?.resume(throwing:NSError(domain:"safe timeout",code:1));stopWait=nil;return }
   safe += 1;log("safe request")
   if ["unknown","unavailable","cancel"].contains(self.mode) { throw NSError(domain:"authentication unavailable or original result unknown",code:1) }
   try await withCheckedThrowingContinuation {stopWait=$0}
  }
  DispatchQueue.main.asyncAfter(deadline:.now()+0.1) { NSApp.terminate(nil) }
  guard mode != "force" else { return }
  DispatchQueue.main.asyncAfter(deadline:.now()+0.25) { self.panel()?.performClose(nil);log("hide") }
  DispatchQueue.main.asyncAfter(deadline:.now()+0.4) { precondition(self.safe==1 && !self.done);NSApp.terminate(nil);precondition(self.panel()?.isVisible==true);log("repeat kept alive without replay") }
  DispatchQueue.main.asyncAfter(deadline:.now()+0.6) {
   precondition(self.safe==1 && self.force==0 && self.delegates==2)
   if self.appOnly {
    func find(_ view:NSView)->NSButton? {if let b=view as? NSButton,b.title=="强制关闭 App…" {return b};return view.subviews.compactMap {find($0)}.first}
    guard let content=self.panel()?.contentView,let button=find(content),button.isEnabled else {log("FAIL no usable App-only exit after original unconfirmed stop");exit(86)}
    button.performClick(nil)
   } else if self.mode == "unknown" {
    precondition(self.panel()?.title=="停工尚未确认");log("PASS original unknown retained; no replay or fabricated stop");exit(0)
   } else {self.done=true;self.stopWait!.resume();self.stopWait=nil;log("settled")}
  }
  // This only answers the fixture's own production NSAlert, never another App
  // or an OS permission dialog. No AX, Keychain, Core or process injection.
  let confirmation = Timer(timeInterval:0.8,repeats:false) { _ in
   guard self.appOnly else { return }
   guard let window=NSApp.modalWindow else {log("FAIL missing explicit App-only confirmation");exit(87)}
   func strings(_ view:NSView)->[String] { (view as? NSTextField).map {[$0.stringValue]} ?? view.subviews.flatMap {strings($0)} }
   let text=strings(window.contentView!).joined(separator:" ")
   precondition(text.contains("后台") && text.contains("确认"))
   NSApp.stopModal(withCode:self.mode == "cancel" ? .alertSecondButtonReturn : .alertFirstButtonReturn)
  }
  RunLoop.main.add(confirmation,forMode:.modalPanel)
  DispatchQueue.main.asyncAfter(deadline:.now()+1.1) {
   if self.mode == "cancel" {precondition(self.safe==1 && self.force==0 && self.delegates==2 && !self.done && self.panel()?.isVisible==true);log("PASS App-only cancellation keeps UI and original request; no RPC replay");exit(0)}
  }
  DispatchQueue.global().asyncAfter(deadline:.now()+4) {log("FAIL bounded timeout");exit(99)}
 }
 func panel()->NSWindow? {NSApp.windows.first {$0 is NSPanel && $0.title != ""}}
 func applicationWillTerminate(_ n:Notification) {
  if mode == "force" {precondition(done && safe==0 && force==1 && delegates==2);log("PASS original direct backend stop settles once")}
  else if appOnly {precondition(!done && safe==1 && force==0 && delegates==3);log("PASS explicit App-only termination; backend remains unconfirmed; no new RPC")}
  else {precondition(done && safe==1 && force==0 && delegates==3);log("PASS actual delegate terminates only after safe settlement")}
 }
}
@main struct Main {
 @MainActor static func main() { let app=NSApplication.shared;let delegate=Probe();app.delegate=delegate;app.setActivationPolicy(.accessory);app.run() }
}
