#!/usr/bin/env python3
"""Verify a Linux candidate against its full recorded Rust build; no execution."""
import hashlib,importlib.util,json,pathlib,struct,sys
# Verification is read-only even when imported proof modules have no cached bytecode.
sys.dont_write_bytecode = True

def main():
 root=pathlib.Path(sys.argv[1]).resolve();manifest=json.loads((root/'candidate.json').read_text())
 if manifest.get('format')!=1 or manifest.get('platform')!='linux-x86_64' or manifest.get('distribution')!='development-candidate' or manifest.get('installed') is not False or manifest.get('published') is not False:raise ValueError('candidate distribution scope missing')
 if manifest.get('binary')!='bin/crewd' or manifest.get('witness')!='normal-build-witness.json':raise ValueError('unexpected candidate evidence paths')
 binary=root/'bin/crewd';witness=root/'normal-build-witness.json'
 if binary.is_symlink() or witness.is_symlink() or hashlib.sha256(binary.read_bytes()).hexdigest()!=manifest['binarySHA256'] or hashlib.sha256(witness.read_bytes()).hexdigest()!=manifest['witnessSHA256']:raise ValueError('candidate binary/witness mismatch')
 elf=binary.read_bytes()[:20]
 if elf[:6]!=b'\x7fELF\x02\x01' or struct.unpack('<H',elf[18:20])[0]!=62:raise ValueError('actual Linux x86_64 ELF required')
 spec=importlib.util.spec_from_file_location('proof',pathlib.Path(__file__).with_name('write-codex-first-provenance.py'));p=importlib.util.module_from_spec(spec);spec.loader.exec_module(p)
 value=json.loads(witness.read_text());cfg=p.complete_configuration(value['buildConfiguration'])
 if value['sourceRef']!=manifest['sourceRef'] or cfg.get('target')!='x86_64-unknown-linux-gnu' or cfg.get('profile')!='release' or cfg.get('defaultFeatures') is not True or cfg.get('allFeatures') is not False or cfg.get('features')!=[] or cfg.get('artifactProfile',{}).get('test') is not False:raise ValueError('normal Linux build configuration mismatch')
 expected={k:cfg[k] for k in ['toolchain','cargoVersion','target','profile','defaultFeatures','features','allFeatures']}
 p.verify_witness(witness,binary,value['rustInputDigest'],expected)
 tier=p.verify_fresh_source(witness.parent/value['evidenceDirectory'],value)
 if manifest.get('sourceProofTier') and manifest['sourceProofTier']!=tier:raise ValueError('Linux backend source-proof tier mismatch')
 print('PASS actual Linux ELF/default release/full Rust input/log/binary provenance; source tier: '+tier)
try:main()
except (ValueError,OSError,KeyError) as e:raise SystemExit('FAIL: '+str(e))
