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
evidence=$(mktemp -d /private/tmp/pcw-followup-migration.XXXXXX)
printf 'Followup formal migration evidence: %s\n' "$evidence"
cp "$root/scripts/dev/fixtures/crewd-followup-migration.py" "$evidence/fixture.py"
printf '%s\n%s\n' "$PENDINGCREW_TEST_OLD_CREWD_PROGRAM" "$PENDINGCREW_TEST_CREWD_PROGRAM" > "$evidence/input-binaries.txt"
shasum -a 256 "$PENDINGCREW_TEST_OLD_CREWD_PROGRAM" "$PENDINGCREW_TEST_CREWD_PROGRAM" "$evidence/fixture.py" > "$evidence/input-sha256.txt"
python3 "$evidence/fixture.py" "$evidence" "$PENDINGCREW_TEST_OLD_CREWD_PROGRAM" "$PENDINGCREW_TEST_CREWD_PROGRAM"
[ "$(shasum -a 256 "$PENDINGCREW_TEST_OLD_CREWD_PROGRAM" | cut -d ' ' -f 1)" = "$old_actual" ]
[ "$(shasum -a 256 "$PENDINGCREW_TEST_CREWD_PROGRAM" | cut -d ' ' -f 1)" = "$new_actual" ]
