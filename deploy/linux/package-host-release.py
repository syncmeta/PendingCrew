#!/usr/bin/env python3
"""Assemble a reviewed Linux Core into an immutable host package; never publish."""
import argparse, ast, hashlib, io, json, os, pathlib, re, stat, subprocess, tarfile, urllib.parse


def sha(data): return hashlib.sha256(data).hexdigest()
def canonical(value): return json.dumps(value, sort_keys=True, separators=(',', ':')).encode() + b'\n'
def url(value, source):
    parsed = urllib.parse.urlsplit(value)
    if parsed.scheme != 'https' or not parsed.hostname or parsed.username or parsed.password or parsed.fragment or parsed.query or source not in parsed.path:
        raise ValueError('immutable source-qualified HTTPS URL required')
    return value


def verify_component_sources(here, package_source, core_source):
    root = here.parents[1]
    def git(*args):
        return subprocess.check_output(['git', '-C', str(root), *args], stderr=subprocess.DEVNULL)
    for ref in [package_source, core_source]:
        if git('rev-parse', '--verify', ref + '^{commit}').decode().strip() != ref:
            raise ValueError('exact component commit required')
    original = git('ls-tree', '-rz', core_source, '--', 'crewd')
    if not original or original != git('ls-tree', '-rz', package_source, '--', 'crewd'):
        raise ValueError('Core source trees differ; binary reuse refused')
    # The identical two Git trees bind both sets of Rust blobs. Also require
    # the actual source files beside this assembler to match those blobs.
    for row in original.split(b'\0'):
        if not row: continue
        metadata, raw_name = row.split(b'\t', 1)
        mode, kind, blob = metadata.decode().split()
        path = root / raw_name.decode()
        info = path.lstat()
        if kind != 'blob' or mode not in ['100644', '100755', '120000']:
            raise ValueError('ordinary component source blobs required')
        if mode == '120000':
            if not stat.S_ISLNK(info.st_mode): raise ValueError('Core source type changed')
            actual = os.fsencode(os.readlink(path))
        else:
            if not stat.S_ISREG(info.st_mode) or bool(info.st_mode & stat.S_IXUSR) != (mode == '100755'):
                raise ValueError('Core source type/mode changed')
            actual = path.read_bytes()
        if actual != git('cat-file', 'blob', blob):
            raise ValueError('actual Core source differs from the two committed components')
    for name in ['package-host-release.py', 'pendingcrew-host.py', 'managed-host.py']:
        path = here / name
        if path.is_symlink() or not path.is_file() or path.read_bytes() != git('show', package_source + ':deploy/linux/' + name):
            raise ValueError('actual package component differs from its frozen source')


def package(args):
    if not re.fullmatch('[0-9a-f]{40}', args.source): raise ValueError('full frozen source commit required')
    core_source = getattr(args, 'core_source', None) or args.source
    if not re.fullmatch('[0-9a-f]{40}', core_source): raise ValueError('full Core source commit required')
    artifact_url = url(args.artifact_url, args.source)
    manifest_url = url(args.manifest_url, args.source)
    core = pathlib.Path(args.core).read_bytes()
    machine = {'x86_64':62, 'aarch64':183}[args.architecture]
    if len(core) < 20 or core[:6] != b'\x7fELF\x02\x01' or int.from_bytes(core[18:20], 'little') != machine:
        raise ValueError('exact ordinary Linux ELF architecture required; no old Mac/R2 substitution')
    proof_bytes = pathlib.Path(args.proof).read_bytes(); proof = json.loads(proof_bytes)
    if proof.get('sourceCommit') != core_source or proof.get('binarySha256') != sha(core) or proof.get('sourceClean') is not True or proof.get('defaultFeatures') is not True or proof.get('features') != [] or proof.get('test') is not False:
        raise ValueError('normal frozen-source build proof does not match this ELF')
    here = pathlib.Path(__file__).resolve().parent
    if core_source != args.source: verify_component_sources(here, args.source, core_source)
    installer = (here / 'pendingcrew-host.py').read_bytes()
    policy_nodes = [
        node.value for node in ast.parse(installer).body
        if isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id == 'PROVIDER_POLICY'
            for target in node.targets)
    ]
    if len(policy_nodes) != 1: raise ValueError('one literal installer prerequisite policy required')
    provider_policy = ast.literal_eval(policy_nodes[0])
    files = {'bin/crewd':core, 'bin/pendingcrew-host.py':installer, 'bin/managed-host.py':(here/'managed-host.py').read_bytes(), 'build-proof.json':proof_bytes}
    release = {'format':1,'sourceCommit':args.source,'architecture':args.architecture,'storeVersion':21,'apiVersion':3,'files':{name:sha(data) for name,data in files.items()}}
    if core_source != args.source: release['coreSourceCommit'] = core_source
    files['release.json'] = canonical(release)
    archive = io.BytesIO()
    with tarfile.open(fileobj=archive,mode='w:gz',format=tarfile.USTAR_FORMAT) as bundle:
        for name, data in sorted(files.items()):
            info = tarfile.TarInfo(name); info.size=len(data); info.mode=0o555 if name.startswith('bin/') else 0o444
            info.mtime=0; info.uid=0; info.gid=0; info.uname=''; info.gname=''
            bundle.addfile(info,io.BytesIO(data))
    payload = archive.getvalue()
    artifact = {'url':artifact_url,'sha256':sha(payload),'sourceCommit':args.source,'architecture':args.architecture}
    if core_source != args.source: artifact['coreSourceCommit'] = core_source
    manifest = canonical({'format':1,'providerPrerequisites':provider_policy,'artifact':artifact})
    entry = installer.decode().replace('DEFAULT_MANIFEST_URL = None', 'DEFAULT_MANIFEST_URL = ' + repr(manifest_url),1).replace('DEFAULT_MANIFEST_SHA256 = None','DEFAULT_MANIFEST_SHA256 = ' + repr(sha(manifest)),1).encode()
    out = pathlib.Path(args.output); out.mkdir(mode=0o700,parents=False,exist_ok=False)
    for name,data in [('host.tar.gz',payload),('manifest.json',manifest),('install.py',entry)]:
        with (out/name).open('xb') as stream: stream.write(data)
    print(json.dumps({'sourceCommit':args.source,'published':False,'files':{p.name:sha(p.read_bytes()) for p in sorted(out.iterdir())}},indent=2))


def main():
    p=argparse.ArgumentParser(description=__doc__)
    for name in ['core','proof','source','artifact-url','manifest-url','output']: p.add_argument('--'+name,required=True)
    p.add_argument('--core-source', help='original compiled Core commit; defaults to the package source')
    p.add_argument('--architecture',choices=['x86_64','aarch64'],required=True)
    package(p.parse_args())


if __name__ == '__main__': main()
