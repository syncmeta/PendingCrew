#!/usr/bin/env python3
"""Execute the actual AppKit delegate's pre-lifecycle termination gate.
Only AppKit presentation is stubbed; gate source is extracted verbatim.
"""
import pathlib,subprocess,sys,json
source=pathlib.Path(sys.argv[1]).read_text()
start=source.index('    func applicationShouldTerminate(')
end=source.index('        let lifecycle = AppQuitLifecycle.shared',start)
method=source[start:end]+'        return .terminateNow\n    }\n'
program='''import Foundation
@MainActor enum NSApplication {enum TerminateReply {case terminateNow,terminateCancel};static let shared=NSApplicationStub()}
@MainActor final class NSApplicationStub {}
@MainActor final class NSAlert {
 enum Response{case alertFirstButtonReturn,alertSecondButtonReturn}
 var messageText="",informativeText=""
 func addButton(withTitle:String){}
 func runModal()->Response{.alertFirstButtonReturn}
}
@MainActor enum CrewPersonalDraftService {static var unsavedExitToken:String?="new revision B"}
@MainActor final class Progress{func present(){}}
@MainActor final class OriginalGate {
 var crewdTerminationAllowed=true,waitingForDaemon=false
 var stopProgress:Progress?
 var unsavedDraftExitConfirmation:String?="previous revision A"
'''+method.replace('sender: NSApplication','sender: NSApplicationStub')+'''}
@main struct GateFixture {
 @MainActor static func main(){
  let gate=OriginalGate()
  precondition(gate.applicationShouldTerminate(NSApplication.shared) == .terminateCancel,"final termination must re-confirm newer undurable input after stop wait")
  gate.unsavedDraftExitConfirmation=CrewPersonalDraftService.unsavedExitToken
  precondition(gate.applicationShouldTerminate(NSApplication.shared) == .terminateNow,"exact confirmed input must still permit final/forced exit")
  print("PASS actual original final terminate gate rechecks new revision and permits exact confirmed exit")
 }
}
'''
base=pathlib.Path(sys.argv[2]);base.with_suffix('.swift').write_text(program)
cache='/Users/hey/Library/Developer/Xcode/DerivedData/PendingCrew-aehzthzfjzjnxdfaqjkozipivxpn/ModuleCache.noindex'
subprocess.run(['xcrun','swiftc','-module-cache-path',cache,'-parse-as-library',str(base.with_suffix('.swift')),'-o',str(base)],check=True)
p=subprocess.run([str(base)],capture_output=True,text=True)
base.with_suffix('.json').write_text(json.dumps(dict(exit=p.returncode,stdout=p.stdout,stderr=p.stderr[:1600]),ensure_ascii=False,indent=2))
print(p.returncode,p.stdout,p.stderr[:160]);sys.exit(0 if p.returncode == int(sys.argv[3]) else 1)
