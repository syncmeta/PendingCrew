#!/usr/bin/env python3
"""Read-only consistency check for the simplified SQLite Mac candidate."""
import argparse, hashlib, json, pathlib, plistlib, subprocess

def sha(path): return hashlib.sha256(path.read_bytes()).hexdigest()
def check(root, expected_ref=None):
    root = pathlib.Path(root)
    facts = json.loads((root/'candidate.json').read_text())
    if expected_ref is not None and facts['sourceRef'] != expected_ref:
        raise ValueError('Source ref mismatch')
    app = pathlib.Path(facts['app'])
    if app.parent != root or not app.is_dir() or app.is_symlink():
        raise ValueError('Candidate application path mismatch')
    info = plistlib.loads((app/'Contents/Info.plist').read_bytes())
    ordinary = facts['configuration'] in ['Debug', 'Release']
    if ordinary and (info.get('CFBundleIdentifier') != 'com.pendingname.pendingcrew' or any(info.get(k) for k in ['PendingCrewKnowledgeValidation','PendingCrewDeliveryValidation','PendingCrewNativeCandidate','SUFeedURL','SUPublicEDKey'])):
        raise ValueError('Ordinary product contains candidate or old update configuration')
    if info.get('PendingCrewSourceRef') != facts['sourceRef'] or info.get('BuildStampCommit') != facts['sourceRef'] or info.get('PendingCrewLocalMac' if ordinary else 'PendingCrewKnowledgeValidation') is not True:
        raise ValueError('Actual application source/config differs')
    if (app/'Contents/Helpers/crewd-data-worker').exists(): raise ValueError('Retired worker present')
    if sha(app/'Contents/Helpers/crewd') != facts['backendSHA256']:
        raise ValueError('Backend hash differs')
    for relative, expected in facts['binaryHashes'].items():
        path = app/relative
        if not path.resolve().is_relative_to(app.resolve()) or sha(path) != expected:
            raise ValueError('Executable differs: '+relative)
    if sha(root/'build.log') != facts['buildLogSHA256']: raise ValueError('Build log differs')
    subprocess.run(['/usr/bin/codesign','--verify','--deep','--strict',str(app)], check=True)
    return facts

if __name__ == '__main__':
    parser=argparse.ArgumentParser(description=__doc__); parser.add_argument('root'); parser.add_argument('--ref')
    args=parser.parse_args(); check(args.root,args.ref); print('PASS candidate source/config/binary hashes and basic signature')
