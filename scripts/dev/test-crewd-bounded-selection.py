#!/usr/bin/env python3
"""Root-owned execution only: actual freshly built App module + private Unix peers.
No App build, Core, network socket, SecurityAgent, real vault, pairing, or model task.
"""
import argparse, hashlib, json, os, pathlib, socket, subprocess, tempfile, threading, time

CASES = ['credential-timeout', 'credential-cancel-local-late-success',
         'credential-cancel-local-late-failure', 'different-target-busy',
         'refresh-coalesced', 'coalesced-two-windows', 'close-pending',
         'late-connection-new-attempt', 'credential-epoch-aba', 'connection-timeout', 'journal-timeout', 'journal-failure', 'hint-blocked', 'hint-scope-cancel', 'cross-window-credential-forget', 'cross-window-credential-replace', 'cross-window-scope-isolation', 'cross-window-hint-invalidation', 'late-credential-notification', 'cancel-list-observation', 'cross-window-cancelled-fanout', 'pending-independent-owner', 'sheet-dismissal-lifecycle', 'broker-independent-owner-coalescing']
SOURCES = ['Sources/CrewdRemote/CrewdRemotePairing.swift', 'Sources/CrewdRemote/CrewdRemoteTransport.swift',
           'Sources/CrewdRemote/CrewdRemoteWindow.swift', 'Sources/Stores/AppModel.swift',
           'Sources/PersonalSync/CrewPersonalDraftService.swift',
           'Sources/Mac/Crewd/CrewdOriginalConnectionIntegration.swift',
           'Sources/Views/IOSRemotePairingView.swift', 'Sources/CrewdRemote/CrewdNativeApp.swift',
           'Sources/CrewdRemote/CrewdRemoteMobileRoot.swift']

def sha(path): return hashlib.sha256(path.read_bytes()).hexdigest()

def check_ordinary_ios_entry(ios, ipad):
    assert 'IOSRemotePairingView()' in ipad, 'ordinary iPad shell no longer binds this form'
    marker = 'struct IOSRemotePairingView:View'
    assert ios.count(marker) == 1, 'ordinary form was not uniquely found'
    ordinary = ios.split(marker, 1)[1]
    assert '.sheet(item:computerSheetBinding){session in' in ordinary
    assert 'computerList(session:session)' in ordinary
    assert '.onDisappear{closeComputerList(session.id)}' in ordinary, 'actual inner-sheet lifecycle is not fenced'
    assert 'let displayedSession=selectionSurface.presented?.id' in ordinary
    assert 'if next == nil,let displayedSession{closeComputerList(displayedSession)}' in ordinary, 'gesture binding has no captured-session cancellation'
    assert 'selectionSurface.dismiss(sessionID)' in ordinary
    assert 'if selectionSurface.complete(token){dismiss()}' in ordinary, 'success must finish its token before programmatic dismissal'
    assert 'if let attempt=appModel.crewdWindow.attempt' in ordinary
    assert 'ProgressView(attempt.phase.label)' in ordinary
    for identifier in ['selection-progress', 'cancel-selection', 'cancel-read', 'back']:
        assert 'pendingcrew.saved-computers.'+identifier in ordinary, 'ordinary UI control missing: '+identifier
    assert 'appModel.cancelCrewdAuthoritySelection()' in ordinary
    assert 'appModel.crewdWindow.cancelComputerListObservation(if:lease)' in ordinary
    assert '}.disabled(appModel.crewdWindow.credentialBusy)' not in ordinary
    assert '.disabled(!computer.paired && appModel.crewdWindow.credentialBusy)' in ordinary
    assert ordinary.count('selectionSurface.isCurrent(token)') >= 2
    assert 'workspace.originalJournalReady' in ordinary
    return ordinary


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--products', required=True, type=pathlib.Path)
    p.add_argument('--source-manifest', required=True, type=pathlib.Path)
    p.add_argument('--library', required=True, type=pathlib.Path)
    p.add_argument('--module-name', default='PendingCrew', choices=['PendingCrew', 'PendingCrewHotReload'])
    p.add_argument('--module-map', type=pathlib.Path)
    p.add_argument('--output', type=pathlib.Path)
    p.add_argument('--scenarios', nargs='+', choices=CASES)
    a = p.parse_args()
    root = pathlib.Path(__file__).resolve().parents[2]
    packet = json.loads(a.source_manifest.read_text())
    for entry in packet['files']:
        assert sha(root/entry['path']) == entry['afterSha256'], 'source packet mismatch: '+entry['path']
    out = a.output or pathlib.Path(tempfile.mkdtemp(prefix='pcw-bounded-selection-runtime-', dir='/private/tmp'))
    out.mkdir(exist_ok=True, parents=True)
    ios = (root/'Sources/Views/IOSRemotePairingView.swift').read_text()
    ipad = (root/'Sources/Views/IPadShell.swift').read_text()
    ordinary = check_ordinary_ios_entry(ios, ipad)
    # Retained legacy controls must never satisfy coverage of the real form.
    mutations = [ios.split('struct IOSRemotePairingView:View', 1)[0]+'struct IOSRemotePairingView:View { }',
                 ios.replace('pendingcrew.saved-computers.cancel-selection', 'legacy-only.cancel-selection'),
                 ios.replace('.onDisappear{closeComputerList(session.id)}', ''),
                 ios.replace('if next == nil,let displayedSession{closeComputerList(displayedSession)}', 'if next == nil {}'),
                 ios.replace('if selectionSurface.complete(token){dismiss()}', 'dismiss()'),
                 ios.replace('selectionSurface.dismiss(sessionID)', 'selectionSurface.dismiss(selectionSurface.presented!.id)')]
    for changed in mutations:
        rejected = False
        try: check_ordinary_ios_entry(changed, ipad)
        except AssertionError: rejected = True
        assert rejected, 'ordinary-entry source negative control was not detected'
    (out/'ordinary-ios-entry-source.json').write_text(json.dumps({
        'layer': 'actual IPadShell→ordinary IOSRemotePairingView callsite and source-only control coverage; not UI runtime',
        'iPadShellSHA256': sha(root/'Sources/Views/IPadShell.swift'),
        'ordinaryFormSHA256': sha(root/'Sources/Views/IOSRemotePairingView.swift'),
        'positive': True, 'negativeControls': len(mutations), 'iOSRuntimeClaim': False}, indent=2)+'\n')
    owner_sources = [root/'Sources/CrewdRemote'/name for name in ['CrewdRemotePairing.swift', 'CrewdRemoteWindow.swift', 'CrewdRemoteTransport.swift']]
    for source in owner_sources:
        assert 'CrewdRemotePairing.credentialRevision' not in source.read_text(), 'global credential clock survived: '+str(source)
    assert 'nonisolated let credentialRevision' in owner_sources[0].read_text()
    assert 'owner: pairing' in owner_sources[2].read_text()
    fixture = out/'fixture.swift'
    original = root/'scripts/dev/fixtures/crewd-bounded-selection.swift'
    fixture.write_text(original.read_text().replace('@testable import PendingCrew\n', '@testable import '+a.module_name+'\n'))
    proof = out/'proof'
    command = ['/usr/bin/xcrun', 'swiftc', '-swift-version', '5', '-parse-as-library', '-I', str(a.products),
               '-module-cache-path', str(out/'module-cache')]
    if a.module_map: command += ['-Xcc', '-fmodule-map-file='+str(a.module_map)]
    command += [str(fixture), str(a.library), '-Xlinker', '-rpath', '-Xlinker', str(a.library.parent),
                '-Xlinker', '-rpath', '-Xlinker', str(a.library.parent.parent/'Frameworks'), '-o', str(proof)]
    evidence = {'layer': 'source-prepared only until Root runs this driver against exactly matching freshly built App',
                'appLibrary': {'path': str(a.library), 'sha256': sha(a.library)},
                'sources': {s: sha(root/s) for s in SOURCES}, 'fixture': sha(original),
                'sourceManifest': {'path': str(a.source_manifest), 'sha256': sha(a.source_manifest)},
                'unchangedProductionDependencies': {s: sha(root/s) for s in [
                    'Sources/Mac/Crewd/CrewdClient.swift', 'Sources/Mac/Crewd/CrewdWorkspace.swift',
                    'Sources/Mac/Crewd/CrewdPresentationJournal.swift', 'Sources/CrewdRemote/CrewdRemoteVault.swift',
                    'Sources/CrewdRemote/CrewdRemoteCertificate.swift', 'Sources/CrewdRemote/CrewdRemoteProtocol.swift']},
                'appModule': {str(v): sha(v) for v in (a.products/(a.module_name+'.swiftmodule')).glob('*') if v.is_file()},
                'argv': command}
    (out/'inputs.json').write_text(json.dumps(evidence, indent=2)+'\n')
    with (out/'compile.log').open('w') as log:
        subprocess.run(command, check=True, stdout=log, stderr=subprocess.STDOUT, timeout=120)
    results = []
    for index, case in enumerate(a.scenarios or CASES):
        case_root = out/("c%02d" % index); case_root.mkdir(mode=0o700)
        local = case_root/'local'; local.mkdir(mode=0o700)
        second_local = case_root/'second-local'; second_local.mkdir(mode=0o700)
        calls, errors, connections, threads = [], [], [], []
        stopping = threading.Event()
        listeners = []
        assert len(str(local/'crewd.sock').encode()) < 104, 'use a shorter private output directory for Unix sockets'
        def serve(conn, path):
            name = path.name if path.parent == case_root else path.parent.name+'-'+path.name
            try:
                with conn, conn.makefile('r') as stream:
                    for line in stream:
                        req = json.loads(line); method = req['method']; calls.append({'socket': name, 'method': method})
                        binding = {'authorityId': '9d5628f3-58c9-4dfe-89ba-cd5ca0f388d8', 'dbId': 'bounded-local',
                                   'grantId': '9d5628f3-58c9-4dfe-89ba-cd5ca0f388d9', 'grantRevision': '1'}
                        if name.startswith('remote-'):
                            binding = json.loads((case_root/'public-bindings.json').read_text())[name]
                        if method == 'system.hello':
                            hold, release = case_root/(name+'.hold'), case_root/(name+'.release')
                            while hold.exists() and not release.exists() and not stopping.wait(.005): pass
                            result = {'server': 'crewd', 'apiVersion': 3, 'minApiVersion': 1, 'version': 'offline-selection-peer',
                                      'capabilities': ['session.respondRunnerRequest'], 'remoteBinding': binding,
                                      'runtimeIdentity': {'dbId': binding['dbId'], 'runtimeBindingId': '6e3fbe22-07a5-463f-a7da-36dde68dff9f'}}
                        elif method in ['crew.list', 'ask.list']: result = []
                        elif method in ['events.subscribe', 'restart.pending']: result = {}
                        elif method == 'lifecycle.status': result = {'role': 'observer', 'ownerPresent': False, 'previousExit': 'clean'}
                        elif method == 'remote.status':
                            hold, release = case_root/(name+'.history-hold'), case_root/(name+'.history-release')
                            while hold.exists() and not release.exists() and not stopping.wait(.005): pass
                            result = {'identity': binding}
                        else: raise AssertionError('Unexpected method (no business requests allowed): '+method)
                        conn.sendall((json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': result})+'\n').encode())
            except (BrokenPipeError, ConnectionResetError, OSError):
                # Cancellation retires a real candidate socket. Its delayed peer
                # is released and joined below; there is no server-side replay.
                pass
            except BaseException as e: errors.append(repr(e))
        def accept(listener, path):
            while not stopping.is_set():
                try: conn, _ = listener.accept()
                except socket.timeout: continue
                except OSError: return
                connections.append(conn)
                task = threading.Thread(target=serve, args=(conn, path)); threads.append(task); task.start()
        for path in [local/'crewd.sock', second_local/'crewd.sock', case_root/'remote-a.sock', case_root/'remote-b.sock']:
            listener = socket.socket(socket.AF_UNIX); listener.settimeout(.1); listener.bind(str(path)); listener.listen(8)
            listeners.append(listener)
            task = threading.Thread(target=accept, args=(listener, path)); threads.append(task); task.start()
        try:
            with (out/(case+'.log')).open('w') as log:
                completed = subprocess.run([str(proof), str(case_root), case], stdout=log, stderr=subprocess.STDOUT, timeout=20)
            results.append({'case': case, 'exit': completed.returncode, 'calls': calls, 'serverErrors': errors})
        finally:
            stopping.set()
            for name in ['remote-a.sock', 'remote-b.sock']: (case_root/(name+'.release')).touch()
            (case_root/'second-local-crewd.sock.history-release').touch()
            for conn in connections:
                try: conn.shutdown(socket.SHUT_RDWR)
                except OSError: pass
                conn.close()
            for listener in listeners: listener.close()
            for task in threads: task.join(timeout=2)
        assert all(not task.is_alive() for task in threads), 'private socket test thread failed to drain'
        (out/'results.json').write_text(json.dumps({'results': results}, indent=2)+'\n')
        if completed.returncode or errors: raise SystemExit('FAILED '+case+'; see '+str(out))
    print('PASS '+str(len(results))+' actual App bounded-selection scenarios; '+str(out))

if __name__ == '__main__': main()
