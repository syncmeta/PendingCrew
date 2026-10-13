#!/usr/bin/python3
"""Read-only self-contained build-input pairing checks. Never executes bundle code."""
import hashlib,json,pathlib,plistlib,re,sys

def fail(message):raise ValueError(message)
def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def digest(items):return hashlib.sha256(''.join(p+'\t'+h+'\n' for p,h in sorted(items)).encode()).hexdigest()
def hexsha(value,length=64):
 if not isinstance(value,str) or not re.fullmatch('[0-9a-f]{'+str(length)+'}',value):fail('invalid SHA')
 return value
def basename(value):
 if not isinstance(value,str) or not value or value in ['.','..'] or pathlib.PurePosixPath(value).name!=value or '\\' in value:fail('invalid relative basename')
 return value
def regular(root,name):
 p=root/basename(name)
 if p.is_symlink() or not p.is_file():fail('missing/nonregular evidence file: '+str(p))
 return p
def load(path):
 with path.open() as f:v=json.load(f)
 if not isinstance(v,dict) or type(v.get('format')) is not int or v.get('format')!=1:fail('unsupported proof format')
 return v
def files(root,rows):
 if root.is_symlink() or not root.is_dir() or not isinstance(rows,list):fail('missing input snapshot')
 out=[];seen=set()
 for row in rows:
  path=row.get('path') if isinstance(row,dict) else None
  if not isinstance(path,str) or not path or '\\' in path or path.startswith('/') or any(p in ['.','..',''] for p in path.split('/')) or path in seen:fail('invalid/duplicate snapshot path')
  seen.add(path);p=root
  for part in path.split('/'):
   p=p/part
   if p.is_symlink():fail('snapshot symlink forbidden')
  if not p.is_file() or sha(p)!=hexsha(row.get('sha256')):fail('snapshot file/hash mismatch: '+path)
  out.append((path,row['sha256']))
 actual=set()
 for p in root.rglob('*'):
  if p.is_symlink():fail('snapshot symlink forbidden')
  if p.is_file():actual.add(p.relative_to(root).as_posix())
 if actual!=seen:fail('snapshot inventory does not equal manifest')
 return out
def build_configuration(value):
 if not isinstance(value,dict):fail('actual build configuration missing')
 for field in ['toolchain','cargoVersion','target']:
  if not isinstance(value.get(field),str) or not value[field]:fail('build configuration '+field+' missing')
 if value.get('profile') not in ['debug','release'] or type(value.get('defaultFeatures')) is not bool or type(value.get('allFeatures')) is not bool:fail('invalid profile/default-feature configuration')
 for field in ['features','artifactFeatures']:
  v=value.get(field)
  if not isinstance(v,list) or not all(isinstance(x,str) for x in v) or sorted(set(v))!=v:fail('invalid feature configuration')
 if not isinstance(value.get('artifactProfile'),dict) or value['artifactProfile'].get('test') is not False:fail('candidate is not a normal non-test compiler artifact')
 packages=value.get('workspaceFeatures')
 if not isinstance(packages,dict) or not packages:fail('actual workspace feature evidence missing')
 for name,v in packages.items():
  if not isinstance(name,str) or not isinstance(v,list) or not all(isinstance(x,str) for x in v) or sorted(set(v))!=v:fail('workspace feature configuration invalid')
 return value

def log(base,item,snapshot,input_digest,kind):
 p=regular(base,item.get('buildLog'));expected=hexsha(item.get('buildLogSHA256'))
 if sha(p)!=expected:fail('build log hash mismatch')
 text=p.read_text();lines=text.splitlines()
 for field,want in [('Snapshot',snapshot),('InputDigest',input_digest),('Exit','0')]:
  found=[x[len(field)+2:] for x in lines if x.startswith(field+': ')]
  if found!=[want]:fail('build log '+field+' mismatch')
 commands=[x[9:] for x in lines if x.startswith('Command: ')]
 if len(commands)!=1 or not commands[0].strip():fail('actual build command is missing')
 if kind=='gui' and ('swiftc' not in commands[0] or 'Sources/Mac/Crewd/' not in commands[0]):fail('GUI command does not name actual Swift inputs')
 if kind=='rust' and ('cargo' not in commands[0] or 'crewd' not in commands[0]):fail('backend command does not name actual Rust input cwd')
 if kind=='rust':
  configs=[json.loads(x[len('BuildConfiguration: '):]) for x in lines if x.startswith('BuildConfiguration: ')]
  if configs!=[build_configuration(item.get('buildConfiguration'))]:fail('actual logged build configuration mismatch')

def main():
 app=pathlib.Path(sys.argv[1]).resolve();base=app.parent
 manifest_path=regular(base,'build-inputs.json');manifest=load(manifest_path);proof=load(regular(base,'build-provenance.json'))
 source=hexsha(manifest.get('sourceRef'),40)
 if manifest.get('sourceSelection')!='committed-archive' or proof.get('sourceRef')!=source:fail('GUI/backend source selection mismatch')
 if (base/'source-head.txt').read_text()!=source+'\n':fail('source-head does not bind frozen input')
 if proof.get('inputManifestSHA256')!=sha(manifest_path):fail('input manifest hash mismatch')
 snapshot=basename(manifest.get('snapshotDirectory'));entries=files(base/snapshot,manifest.get('files'))
 gui_entries=[(p,h) for p,h in entries if (p.startswith('Sources/Mac/Crewd/') and p.endswith('.swift')) or p in ['Sources/Mac/Design/CrewColorHash.swift','scripts/dev/build-codex-first-macos.sh','CandidateMain.swift']]
 gui_paths={p for p,h in gui_entries}
 if not {'Sources/Mac/Crewd/CrewdClient.swift','Sources/Mac/Crewd/CrewdWorkspace.swift','Sources/Mac/Design/CrewColorHash.swift','scripts/dev/build-codex-first-macos.sh','CandidateMain.swift'}.issubset(gui_paths):fail('GUI snapshot lacks required real input files')
 rust_entries=[(p,h) for p,h in entries if p.startswith('crewd/')]
 if not {'crewd/Cargo.toml','crewd/Cargo.lock'}.issubset({p for p,h in rust_entries}):fail('Rust snapshot lacks locked source inputs')
 gd,rd=digest(gui_entries),digest(rust_entries)
 if manifest.get('guiInputDigest')!=gd or manifest.get('rustInputDigest')!=rd:fail('canonical input digests mismatch')
 gui,backend=proof.get('gui'),proof.get('backend')
 expected_configuration=build_configuration(manifest.get('backendBuildConfiguration'))
 if not isinstance(backend,dict) or build_configuration(backend.get('buildConfiguration'))!=expected_configuration:fail('candidate build configuration differs from requested backend inputs')
 if not isinstance(gui,dict) or not isinstance(backend,dict) or gui.get('inputDigest')!=gd or backend.get('inputDigest')!=rd:fail('compiled GUI/backend inputs do not match snapshot')
 for item in [gui,backend]:
  if sha(regular(base,item.get('preSignFile')))!=hexsha(item.get('preSignSHA256')):fail('actual pre-sign binary does not match evidence')
 log(base,gui,snapshot,gd,'gui')
 mode=backend.get('mode')
 if mode=='built-from-snapshot':log(base,backend,snapshot,rd,'rust')
 elif mode=='verified-prebuilt':
  witness_path=regular(base,backend.get('witnessFile'))
  if sha(witness_path)!=hexsha(backend.get('witnessSHA256')):fail('prebuilt witness hash mismatch')
  witness=load(witness_path);hexsha(witness.get('sourceRef'),40)
  if build_configuration(witness.get('buildConfiguration'))!=expected_configuration:fail('prebuilt toolchain/target/profile/features differ')
  witness_base=base/basename(witness.get('evidenceDirectory'))
  if witness_base.is_symlink() or not witness_base.is_dir():fail('prebuilt evidence directory missing')
  snapshot_name=basename(witness.get('snapshotDirectory'))
  witness_entries=files(witness_base/snapshot_name,witness.get('files'))
  if not all(p.startswith('crewd/') for p,h in witness_entries) or digest(witness_entries)!=rd or witness.get('rustInputDigest')!=rd:fail('prebuilt full Rust inputs differ')
  if sha(regular(witness_base,witness.get('binaryFile')))!=hexsha(witness.get('binarySHA256')) or witness['binarySHA256']!=backend['preSignSHA256']:fail('prebuilt binary is not the packaged input')
  log(witness_base,witness,snapshot_name,rd,'rust')
 else:fail('backend origin is declared/unverified or unsupported')
 with (app/'Contents/Info.plist').open('rb') as f:plist=plistlib.load(f)
 version=manifest.get('candidateVersion')
 if not isinstance(version,str) or not re.fullmatch(r'0\.0\.[0-9]+',version) or plist.get('CFBundleShortVersionString')!=version or plist.get('PendingCrewSourceRef')!=source:fail('plist version/source does not bind frozen input')
 executable=basename(plist.get('CFBundleExecutable'))
 packaged=proof.get('packaged',{})
 if packaged.get('guiSHA256')!=sha(app/'Contents/MacOS'/executable) or packaged.get('helperSHA256')!=sha(app/'Contents/Helpers/crewd'):fail('packaged binary hashes differ from build proof')
 print('PASS: immutable full GUI/Rust inputs, actual pre-sign evidence, logs and packaged digests pair')
try:main()
except (ValueError,KeyError,TypeError,OSError,json.JSONDecodeError) as e:
 print('FAIL: '+str(e),file=sys.stderr);sys.exit(1)
