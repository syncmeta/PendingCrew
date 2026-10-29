#!/usr/bin/env python3
"""Compile actual Claude store/wire sources against synthetic read/perform closures.
Unrelated personal/Codex input validators fail closed; no App, provider or credentials.
"""
import pathlib, subprocess, tempfile
root=pathlib.Path(__file__).resolve().parents[2]
out=pathlib.Path(tempfile.mkdtemp(prefix='pcw-claude-recovery-',dir='/private/tmp'))
client=(root/'Sources/Mac/Crewd/CrewdClient.swift').read_text()
(out/'Value.swift').write_text('import Foundation\n'+client[client.index('indirect enum CrewdValue:'):client.index('/// The same bounded NDJSON')])
(out/'Unrelated.swift').write_text('''import Foundation
enum CrewPersonalCoreWire {
static func validateResult(_ value:CrewdValue,command:CrewdValue)throws {throw CrewdPublicWire.invalid()}
static func canonical(_ method:String,_ value:CrewdValue)throws->CrewdValue {throw CrewdPublicWire.invalid()}
}
''')
files=[out/'Value.swift',out/'Unrelated.swift']+[root/'Sources/Mac/Crewd'/n for n in ['CrewdPresentationJournal.swift','CrewdPublicCommandWire.swift','CrewdCodexWire.swift','CrewdClaudeHistoryStore.swift']]+[root/'scripts/dev/fixtures/crewd-claude-recovery.swift']
with (out/'compile.log').open('w') as log:
 result=subprocess.run(['xcrun','swiftc','-parse-as-library','-module-cache-path',str(out/'cache'),*map(str,files),'-o',str(out/'probe')],stdout=log,stderr=subprocess.STDOUT)
if result.returncode:
 print((out/'compile.log').read_text());raise SystemExit(result.returncode)
result=subprocess.run([str(out/'probe')],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
(out/'result.log').write_text(result.stdout)
print(result.stdout[:5000]);print('Evidence:',out)
raise SystemExit(result.returncode)
