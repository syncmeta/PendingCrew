#!/bin/sh
# Explicit normal executables and their frozen hashes; no model/App/daily data.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
: "${PENDINGCREW_TEST_OLD_CREWD_PROGRAM:?Explicit normal schema12 helper required}"
: "${PENDINGCREW_TEST_CREWD_PROGRAM:?Explicit normal schema13 helper required}"
: "${PENDINGCREW_TEST_OLD_CREWD_SHA256:?Old immutable helper hash required}"
: "${PENDINGCREW_TEST_CREWD_SHA256:?New immutable helper hash required}"
case "$PENDINGCREW_TEST_OLD_CREWD_PROGRAM" in /*) ;; *) exit 2;; esac
case "$PENDINGCREW_TEST_CREWD_PROGRAM" in /*) ;; *) exit 2;; esac
[ -x "$PENDINGCREW_TEST_OLD_CREWD_PROGRAM" ] && [ -x "$PENDINGCREW_TEST_CREWD_PROGRAM" ]
old_actual=$(shasum -a 256 "$PENDINGCREW_TEST_OLD_CREWD_PROGRAM" | cut -d ' ' -f 1)
new_actual=$(shasum -a 256 "$PENDINGCREW_TEST_CREWD_PROGRAM" | cut -d ' ' -f 1)
[ "$old_actual" = "$PENDINGCREW_TEST_OLD_CREWD_SHA256" ] && [ "$new_actual" = "$PENDINGCREW_TEST_CREWD_SHA256" ]
# Default /tmp is short and portable; CI may provide an existing private temp
# parent, then uploads that parent with its newly generated evidence child.
evidence=$(python3 - <<'ROOT'
import os,pathlib,stat,tempfile
value=os.environ.get('PENDINGCREW_TEST_MIGRATION_EVIDENCE_ROOT')
if value:
 raw=pathlib.Path(value)
 if not raw.is_absolute() or raw.is_symlink() or not raw.is_dir():raise SystemExit('explicit existing absolute private migration evidence parent required')
 parent=raw.resolve()
 st=parent.stat()
 if st.st_uid!=os.geteuid() or stat.S_IMODE(st.st_mode)&0o077:raise SystemExit('explicit migration evidence parent must be owner-private')
else:
 parent=pathlib.Path('/tmp').resolve()
if any(chr(c) in str(parent) for c in [0,10,13]):raise SystemExit('migration evidence parent contains control characters')
# tempfile's random suffix is eight ASCII characters. Check the longest actual
# served subroot before allocating; check its actual resolved path afterward.
probe=parent/'pcw-followup.XXXXXXXX'/'retry-after-collision'/'crewd.sock'
if len(str(probe).encode('utf-8'))>103:raise SystemExit('migration longest Unix socket path exceeds portable 103 bytes; choose a shorter temp parent')
actual=pathlib.Path(tempfile.mkdtemp(prefix='pcw-followup.',dir=parent)).resolve()
for name in ['data','backup-failure','mid-ddl-failure','retry-after-collision']:
 if len(str(actual/name/'crewd.sock').encode('utf-8'))>103:raise SystemExit('actual resolved migration socket exceeds 103 bytes; fresh evidence retained')
print(actual)
ROOT
)
printf 'Followup formal migration evidence: %s\n' "$evidence"
cp "$root/scripts/dev/fixtures/crewd-followup-migration.py" "$evidence/fixture.py"
printf '%s\n%s\n' "$PENDINGCREW_TEST_OLD_CREWD_PROGRAM" "$PENDINGCREW_TEST_CREWD_PROGRAM" > "$evidence/input-binaries.txt"
shasum -a 256 "$PENDINGCREW_TEST_OLD_CREWD_PROGRAM" "$PENDINGCREW_TEST_CREWD_PROGRAM" "$evidence/fixture.py" > "$evidence/input-sha256.txt"
python3 "$evidence/fixture.py" "$evidence" "$PENDINGCREW_TEST_OLD_CREWD_PROGRAM" "$PENDINGCREW_TEST_CREWD_PROGRAM"
[ "$(shasum -a 256 "$PENDINGCREW_TEST_OLD_CREWD_PROGRAM" | cut -d ' ' -f 1)" = "$old_actual" ]
[ "$(shasum -a 256 "$PENDINGCREW_TEST_CREWD_PROGRAM" | cut -d ' ' -f 1)" = "$new_actual" ]
