#!/usr/bin/env python3
"""Actual consumer import/CLI boundary control; no compiler or provider."""
import argparse,contextlib,importlib.util,io,pathlib,subprocess,sys,tempfile

def main():
 parser=argparse.ArgumentParser();parser.add_argument('--source',type=pathlib.Path,default=pathlib.Path(__file__).with_name('write-codex-first-provenance.py'));args=parser.parse_args()
 source=args.source.resolve()
 with tempfile.TemporaryDirectory(prefix='pcw-proof-import-') as root:
  previous=sys.argv;sys.argv=['-', 'unexpected-action',root]
  try:
   spec=importlib.util.spec_from_file_location('pcw_actual_proof',source);module=importlib.util.module_from_spec(spec)
   output=io.StringIO()
   with contextlib.redirect_stdout(output),contextlib.redirect_stderr(output):spec.loader.exec_module(module)
   assert output.getvalue()=='','import dispatched the CLI'
   assert callable(module.verify_witness) and callable(module.verify_fresh_source)
  finally:sys.argv=previous
  result=subprocess.run([sys.executable,str(source),'unexpected-action',root],capture_output=True,text=True)
  assert result.returncode==1 and 'Unsupported action' in result.stderr,(result.returncode,result.stdout,result.stderr)
 print('PASS actual import with unrelated argv is silent; fresh verifier remains callable; explicit invalid CLI still refuses')
if __name__=='__main__':main()
