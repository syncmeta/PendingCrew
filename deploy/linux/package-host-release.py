#!/usr/bin/env python3
"""Assemble a reviewed Linux Core into an immutable host package; never publish."""
import argparse, hashlib, io, json, pathlib, re, tarfile, urllib.parse


def sha(data): return hashlib.sha256(data).hexdigest()
def canonical(value): return json.dumps(value, sort_keys=True, separators=(',', ':')).encode() + b'\n'
def url(value, source):
    parsed = urllib.parse.urlsplit(value)
    if parsed.scheme != 'https' or not parsed.hostname or parsed.username or parsed.password or parsed.fragment or parsed.query or source not in parsed.path:
        raise ValueError('immutable source-qualified HTTPS URL required')
    return value


def package(args):
    if not re.fullmatch('[0-9a-f]{40}', args.source): raise ValueError('full frozen source commit required')
    artifact_url = url(args.artifact_url, args.source)
    manifest_url = url(args.manifest_url, args.source)
    core = pathlib.Path(args.core).read_bytes()
    machine = {'x86_64':62, 'aarch64':183}[args.architecture]
    if len(core) < 20 or core[:6] != b'\x7fELF\x02\x01' or int.from_bytes(core[18:20], 'little') != machine:
        raise ValueError('exact ordinary Linux ELF architecture required; no old Mac/R2 substitution')
    proof_bytes = pathlib.Path(args.proof).read_bytes(); proof = json.loads(proof_bytes)
    if proof.get('sourceCommit') != args.source or proof.get('binarySha256') != sha(core) or proof.get('sourceClean') is not True or proof.get('defaultFeatures') is not True or proof.get('features') != [] or proof.get('test') is not False:
        raise ValueError('normal frozen-source build proof does not match this ELF')
    here = pathlib.Path(__file__).resolve().parent
    installer = (here / 'pendingcrew-host.py').read_bytes()
    files = {'bin/crewd':core, 'bin/pendingcrew-host.py':installer, 'bin/managed-host.py':(here/'managed-host.py').read_bytes(), 'build-proof.json':proof_bytes}
    release = {'format':1,'sourceCommit':args.source,'architecture':args.architecture,'storeVersion':18,'apiVersion':3,'files':{name:sha(data) for name,data in files.items()}}
    files['release.json'] = canonical(release)
    archive = io.BytesIO()
    with tarfile.open(fileobj=archive,mode='w:gz',format=tarfile.USTAR_FORMAT) as bundle:
        for name, data in sorted(files.items()):
            info = tarfile.TarInfo(name); info.size=len(data); info.mode=0o555 if name.startswith('bin/') else 0o444
            info.mtime=0; info.uid=0; info.gid=0; info.uname=''; info.gname=''
            bundle.addfile(info,io.BytesIO(data))
    payload = archive.getvalue()
    manifest = canonical({'format':1,'providerQualification':{'codex':'0.163.0','claude':'2.1.301'},'artifact':{'url':artifact_url,'sha256':sha(payload),'sourceCommit':args.source,'architecture':args.architecture}})
    entry = installer.decode().replace('DEFAULT_MANIFEST_URL = None', 'DEFAULT_MANIFEST_URL = ' + repr(manifest_url),1).replace('DEFAULT_MANIFEST_SHA256 = None','DEFAULT_MANIFEST_SHA256 = ' + repr(sha(manifest)),1).encode()
    out = pathlib.Path(args.output); out.mkdir(mode=0o700,parents=False,exist_ok=False)
    for name,data in [('host.tar.gz',payload),('manifest.json',manifest),('install.py',entry)]:
        with (out/name).open('xb') as stream: stream.write(data)
    print(json.dumps({'sourceCommit':args.source,'published':False,'files':{p.name:sha(p.read_bytes()) for p in sorted(out.iterdir())}},indent=2))


def main():
    p=argparse.ArgumentParser(description=__doc__)
    for name in ['core','proof','source','artifact-url','manifest-url','output']: p.add_argument('--'+name,required=True)
    p.add_argument('--architecture',choices=['x86_64','aarch64'],required=True)
    package(p.parse_args())


if __name__ == '__main__': main()
