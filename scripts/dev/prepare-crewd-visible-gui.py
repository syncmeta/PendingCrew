#!/usr/bin/python3
"""Prepare a reviewable fake-only visible GUI session; never launch by default."""
import hashlib,json,os,pathlib,plistlib,shutil,subprocess,sys,tempfile

def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def main():
 app=pathlib.Path(sys.argv[1]).resolve();fake=pathlib.Path(sys.argv[2]).resolve()
 if app.suffix!='.app' or not app.is_dir():raise ValueError('Explicit complete candidate app required')
 if fake.name!='fake_codex' or not fake.is_file() or not os.access(fake,os.X_OK):raise ValueError('Explicit executable fake_codex required; no user Codex fallback')
 artifact=app.parent;proof=json.loads((artifact/'build-provenance.json').read_text());info=plistlib.loads((app/'Contents/Info.plist').read_bytes())
 executable=info['CFBundleExecutable']
 if pathlib.Path(executable).name!=executable:raise ValueError('Candidate executable must be a basename')
 gui=app/'Contents/MacOS'/executable;helper=app/'Contents/Helpers/crewd'
 gui_hash='nativeSHA256' if proof.get('format')==2 else 'guiSHA256'
 subprocess.run([sys.executable,str(pathlib.Path(__file__).with_name('check-codex-first-provenance.py')),str(app)],check=True)
 if info['PendingCrewSourceRef']!=proof['sourceRef'] or sha(gui)!=proof['packaged'][gui_hash] or sha(helper)!=proof['packaged']['helperSHA256']:raise ValueError('Candidate source/package hashes differ from accepted evidence')
 base=pathlib.Path(tempfile.mkdtemp(prefix='pcw-visible-gui.',dir='/private/tmp'))
 for name in ['home','work','data','legacy-preserved','bin']:(base/name).mkdir(mode=0o700)
 saved=base/'bin/fake_codex';shutil.copy2(fake,saved);saved.chmod(0o500)
 if sha(saved)!=sha(fake):raise ValueError('Fake copy hash differs')
 (base/'work/README.md').write_text('# Isolated visible PendingCrew acceptance\nSynthetic work only; no user project or account.\n')
 git_env={'PATH':'/usr/bin:/bin','HOME':str(base/'home'),'GIT_CONFIG_GLOBAL':'/dev/null','GIT_CONFIG_NOSYSTEM':'1'}
 subprocess.run(['/usr/bin/git','init','-q','--template=','--initial-branch=codex/gui-acceptance',str(base/'work')],env=git_env,check=True)
 subprocess.run(['/usr/bin/git','-C',str(base/'work'),'add','README.md'],env=git_env,check=True)
 subprocess.run(['/usr/bin/git','-C',str(base/'work'),'-c','user.name=PendingCrew Fixture','-c','user.email=fixture@pendingcrew.invalid','-c','commit.gpgsign=false','-c','core.hooksPath=/dev/null','commit','-q','-m','test: initialize isolated GUI workspace'],env=git_env,check=True)
 work_head=subprocess.check_output(['/usr/bin/git','-C',str(base/'work'),'rev-parse','HEAD'],env=git_env,text=True).strip()
 environment={'PATH':'/usr/bin:/bin:/usr/sbin:/sbin','HOME':str(base/'home'),'SHELL':'/usr/bin/false','PENDINGCREW_CREWD_DATA_DIR':str(base/'data'),'PENDINGCREW_DATA_DIR':str(base/'legacy-preserved'),'PENDINGCREW_CODEX_PROGRAM':str(saved)}
 # Deliberately absent CREWD_PROGRAM: the actual packaged helper is selected.
 manifest={'format':1,'app':str(app),'sourceRef':proof['sourceRef'],'executableName':executable,'guiSHA256':sha(gui),'helperSHA256':sha(helper),'fakeCodex':str(saved),'fakeSHA256':sha(saved),'dataRoot':str(base/'data'),'workRoot':str(base/'work'),'workHead':work_head,'environment':environment,'launched':False}
 (base/'session.json').write_text(json.dumps(manifest,indent=2)+'\n')
 (base/'launch-after-authorization.sh').write_text('#!/bin/sh\n# Explicit visible test only after root authorizes this prepared fake-only session.\nset -eu\npython3 - '+repr(str(base/'session.json'))+" <<'PYRUN'\nimport hashlib,json,os,pathlib,subprocess,sys\nm=json.loads(pathlib.Path(sys.argv[1]).read_text());app=pathlib.Path(m['app']);fake=pathlib.Path(m['fakeCodex']);gui=app/'Contents/MacOS'/m['executableName'];helper=app/'Contents/Helpers/crewd'\nfor file,key in [(gui,'guiSHA256'),(helper,'helperSHA256'),(fake,'fakeSHA256')]:\n assert file.is_file() and os.access(file,os.X_OK) and hashlib.sha256(file.read_bytes()).hexdigest()==m[key], 'Prepared input changed; refuse launch'\nassert 'PENDINGCREW_CREWD_PROGRAM' not in m['environment']\nlog=open(pathlib.Path(m['dataRoot']).parent/'visible-app.log','w')\nm['launched']=True;m['pid']=os.getpid();pathlib.Path(sys.argv[1]).write_text(json.dumps(m,indent=2)+chr(10));print('Visible synthetic candidate PID:',m['pid'],flush=True)\n# exec preserves the controlled process until the caller explicitly closes it.\nos.dup2(log.fileno(),1);os.dup2(log.fileno(),2);os.execve(str(gui),[str(gui)],m['environment'])\nPYRUN\n")
 (base/'launch-after-authorization.sh').chmod(0o700)
 print('PREPARED, not launched:',base)
 print('Candidate title must contain source:',proof['sourceRef'][:8])
 print('Packaged helper / explicit fake / fresh data root recorded in session.json; never installs or reads daily data')
try:main()
except (ValueError,OSError,KeyError,IndexError,subprocess.CalledProcessError) as error:print('FAIL:',error,file=sys.stderr);sys.exit(1)
