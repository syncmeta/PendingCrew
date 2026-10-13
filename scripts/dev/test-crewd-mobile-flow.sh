#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
evidence=$(mktemp -d /private/tmp/pcw-M6-mobile-flow.XXXXXX)
echo "Mobile operation evidence: $evidence"
mkdir "$evidence/source"
git -C "$root" rev-parse "${PENDINGCREW_QA_REF:-HEAD}" > "$evidence/source-ref.txt"
git -C "$root" archive "${PENDINGCREW_QA_REF:-HEAD}" Sources/Mac/Crewd Sources/Mac/Design/CrewColorHash.swift | tar -x -C "$evidence/source"
cp "$root/scripts/dev/fixtures/crewd-mobile-flow.swift" "$evidence/fixture.swift"
cp "$root/scripts/dev/fixtures/crewd-mobile-flow-server.py" "$evidence/server.py"
find "$evidence/source" -type f -exec shasum -a 256 {} \; > "$evidence/source-sha256.txt"
shasum -a 256 "$evidence/fixture.swift" "$evidence/server.py" >> "$evidence/source-sha256.txt"
textflag=
[ "${PENDINGCREW_QA_GREEN_TEXT:-0}" = 1 ] && textflag="-D QA_TEXT_SUBMISSION"
xcrun swiftc $textflag -module-cache-path "$evidence/cache" -parse-as-library "$evidence/source"/Sources/Mac/Crewd/*.swift "$evidence/source/Sources/Mac/Design/CrewColorHash.swift" "$evidence/fixture.swift" -o "$evidence/test"
failed=0
for mode in ${PENDINGCREW_QA_MODES:-late-directory same-crew-directory sync-directory sync-crews late-start late-create unknown}; do
 mkdir "$evidence/$mode"
 printf '%s\n' "$mode" > "$evidence/$mode/qa-mode"
 python3 "$evidence/server.py" "$evidence/$mode" > "$evidence/$mode/server.log" 2>&1 &
 server=$!
 trap 'kill "$server" 2>/dev/null || true' EXIT
 count=0
 while [ ! -f "$evidence/$mode/ready" ]; do count=$((count+1)); [ "$count" -lt 250 ] && kill -0 "$server" 2>/dev/null || exit 1; sleep .02; done
 if "$evidence/test" "$evidence/$mode" "$mode" > "$evidence/$mode/result.log" 2>&1; then echo "PASS $mode"; else echo "FAIL $mode"; failed=1; fi
 kill "$server" 2>/dev/null || true
 wait "$server" 2>/dev/null || true
 done
exit "$failed"
