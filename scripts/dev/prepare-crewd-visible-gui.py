#!/usr/bin/python3
"""Prepare a reviewable fake-only visible GUI session; never launch by default."""
import hashlib,json,os,pathlib,plistlib,shutil,subprocess,sys,tempfile

def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def main():
 app=pathlib.Path(sys.argv[1]).resolve();fake=pathlib.Path(sys.argv[2]).resolve()
 if app.suffix!='.app' or not app.is_dir():raise ValueError('Explicit complete candidate app required')
 if fake.name!='fake_codex' or not fake.is_file() or not os.access(fake,os.X_OK):raise ValueError('Explicit executable fake_codex required; no user Codex fallback')
 artifact=app.parent;proof=json.loads((artifact/'build-provenance.json').read_text());info=plistlib.loads((app/'Contents/Info.plist').read_bytes())
 gui=app/'Contents/MacOS/PendingCrewCodexFirst';helper=app/'Contents/Helpers/crewd'
 if info['PendingCrewSourceRef']!=proof['sourceRef'] or sha(gui)!=proof['packaged']['guiSHA256'] or sha(helper)!=proof['packaged']['helperSHA256']:raise ValueError('Candidate source/package hashes differ from accepted evidence')
 base=pathlib.Path(tempfile.mkdtemp(prefix='pcw-visible-gui.',dir='/private/tmp'))
 for name in ['home','work','data','legacy-preserved','bin']:(base/name).mkdir(mode=0o700)
 saved=base/'bin/fake_codex';shutil.copy2(fake,saved);saved.chmod(0o500)
 if sha(saved)!=sha(fake):raise ValueError('Fake copy hash differs')
 (base/'work/README.md').write_text('# Isolated visible PendingCrew acceptance\nSynthetic work only; no user project or account.\n')
 subprocess.run(['/usr/bin/git','init','-q','--initial-branch=codex/gui-acceptance',str(base/'work')],check=True)
 environment={'PATH':'/usr/bin:/bin:/usr/sbin:/sbin','HOME':str(base/'home'),'SHELL':'/usr/bin/false','PENDINGCREW_CREWD_DATA_DIR':str(base/'data'),'PENDINGCREW_DATA_DIR':str(base/'legacy-preserved'),'PENDINGCREW_CODEX_PROGRAM':str(saved)}
 # Deliberately absent CREWD_PROGRAM: the actual packaged helper is selected.
 manifest={'format':1,'app':str(app),'sourceRef':proof['sourceRef'],'guiSHA256':sha(gui),'helperSHA256':sha(helper),'fakeCodex':str(saved),'fakeSHA256':sha(saved),'dataRoot':str(base/'data'),'workRoot':str(base/'work'),'environment':environment,'launched':False}
 (base/'session.json').write_text(json.dumps(manifest,indent=2)+'\n')
 (base/'launch-after-authorization.sh').write_text('#!/bin/sh\n# Explicit visible test only after root authorizes this prepared fake-only session.\nset -eu\npython3 - '+repr(str(base/'session.json'))+" <<'PYRUN'\nimport hashlib,json,os,pathlib,subprocess,sys\nm=json.loads(pathlib.Path(sys.argv[1]).read_text());app=pathlib.Path(m['app']);fake=pathlib.Path(m['fakeCodex']);gui=app/'Contents/MacOS/PendingCrewCodexFirst';helper=app/'Contents/Helpers/crewd'\nfor file,key in [(gui,'guiSHA256'),(helper,'helperSHA256'),(fake,'fakeSHA256')]:\n assert file.is_file() and os.access(file,os.X_OK) and hashlib.sha256(file.read_bytes()).hexdigest()==m[key], 'Prepared input changed; refuse launch'\nassert 'PENDINGCREW_CREWD_PROGRAM' not in m['environment']\nlog=open(pathlib.Path(m['dataRoot']).parent/'visible-app.log','w')\np=subprocess.Popen([str(gui)],env=m['environment'],stdout=log,stderr=log)\nm['launched']=True;m['pid']=p.pid;pathlib.Path(sys.argv[1]).write_text(json.dumps(m,indent=2)+chr(10));print('Visible synthetic candidate PID:',p.pid)\nPYRUN\n")
 (base/'launch-after-authorization.sh').chmod(0o700)
 print('PREPARED, not launched:',base)
 print('Candidate title must contain source:',proof['sourceRef'][:8])
 print('Packaged helper / explicit fake / fresh data root recorded in session.json; never installs or reads daily data')
try:main()
except (ValueError,OSError,KeyError,IndexError,subprocess.CalledProcessError) as error:print('FAIL:',error,file=sys.stderr);sys.exit(1)
