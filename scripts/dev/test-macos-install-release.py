#!/usr/bin/env python3
"""Offline execution of install.sh's exact release-selection block; no download/install."""
import json,pathlib,subprocess,tempfile
source=pathlib.Path(__file__).resolve().parents[1]/'install.sh'
text=source.read_text();block=text[text.index('# 只认 .dmg'):text.index('say "→ 最新版本：$tag"')]
def asset(name,digest):return {'name':name,'browser_download_url':'https://github.com/syncmeta/PendingCrew/releases/download/v1/'+name,'digest':digest}
a=asset('PendingCrew.zip','sha256:'+'a'*64);b=asset('PendingCrew.dmg','sha256:'+'b'*64)
cases=[('zip-first',[a,b],True),('dmg-first',[b,a],True),('missing-digest',[a,asset('PendingCrew.dmg',None)],False),('ambiguous',[b,asset('Other.dmg','sha256:'+'c'*64)],False),('no-dmg',[a],False),('bad-digest',[asset('PendingCrew.dmg','sha256:abc')],False)]
with tempfile.TemporaryDirectory(prefix='pcw-dmg-binding-') as root:
 p=pathlib.Path(root)/'release.json'
 for name,assets,ok in cases:
  for compact in (False,True):
   p.write_text(json.dumps({'tag_name':'v1','assets':assets},indent=None if compact else 2))
   script='set -eu\ndie(){ echo "$*" >&2; exit 1; }\nmeta=$(cat "$1")\n'+block+'\nprintf "%s\\n%s\\n" "$url" "$want"\n'
   r=subprocess.run(['/bin/sh','-c',script,'test',str(p)],text=True,capture_output=True)
   if ok:assert r.returncode==0 and r.stdout.splitlines()==[b['browser_download_url'],'b'*64],(name,compact,r.returncode,r.stdout,r.stderr)
   else:assert r.returncode!=0,(name,compact,'invalid metadata accepted',r.stdout)
  print('PASS',name)
