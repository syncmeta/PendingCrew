#!/usr/bin/env python3
"""Run the exact production scheduleSync body against controlled transport seams.
Usage: python3 SCRIPT SERVICE_SOURCE UNIQUE_LABEL
Only the scheduling body is real; coordinator/transport/UI publication are stubs.
Does not prove actual Core, Cloud, storage or UI behavior.
"""
from pathlib import Path
import subprocess,sys
source=Path(sys.argv[1]).read_text();method=source[source.index('    private func scheduleSync()'):source.index('    func synchronizePersonal()')].replace('private func scheduleSync','func scheduleSync')
stub='''import Foundation
@MainActor final class Adapter {}
@MainActor final class Owner {
 struct ID {var account:String?="account"}
 var id=ID();var revisions=0;var calls=0
 var pending:CheckedContinuation<Bool,Error>?
 func synchronizeCore(using adapter:Adapter)async throws->Bool{calls+=1;return try await withCheckedThrowingContinuation{pending=$0}}
}
@MainActor final class Coordinator {var current=Owner();var optedIn=true;func synchronize()async throws{}}
enum AppleSyncFailure:Error {case accountChanged}
@MainActor final class Service {
 var active=true,usingCore=true,cloudOptedIn=false,syncAgain=false,syncBusy=false,coreSynchronized=false
 var syncID:UUID?;var syncTask:Task<Void,Never>?;var coreTransport:Adapter?=Adapter();var coordinator=Coordinator();var coreSyncedRevisions=0
 func publishProjection(){}
'''+method+'''
}
@main struct Probe {
 @MainActor static func settle()async {for _ in 0..<20{await Task.yield()}}
 @MainActor static func main()async throws {
 let s=Service();s.scheduleSync();await settle();let old=s.coordinator.current
 s.coreTransport=Adapter();s.scheduleSync();old.pending!.resume(throwing:AppleSyncFailure.accountChanged);await settle()
 precondition(old.calls==2,"queued reconnect pass lost after old failure")
 old.pending!.resume(throwing:AppleSyncFailure.accountChanged);await settle()
 precondition(old.calls==2 && s.syncTask==nil,"failure alone retried")
 let t=Service();t.scheduleSync();await settle();let original=t.coordinator.current
 t.coordinator.current=Owner();t.coreTransport=Adapter();original.pending!.resume(returning:true);await settle()
 precondition(!t.coreSynchronized,"late result marked new namespace synced")
 print("PASS queued reconnect, no failure retry, late namespace result")
 }
}
'''
p=Path('/private/tmp/personal-scheduler-'+sys.argv[2]);p.mkdir(exist_ok=False);(p/'main.swift').write_text(stub)
subprocess.run(['xcrun','swiftc','-parse-as-library','-module-cache-path','/private/tmp/pcw-personal-module-cache',str(p/'main.swift'),'-o',str(p/'probe')],check=True)
subprocess.run([str(p/'probe')],check=True)
