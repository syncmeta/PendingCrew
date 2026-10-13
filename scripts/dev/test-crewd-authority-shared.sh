#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
evidence=$(mktemp -d /private/tmp/pcw-M6-authority.XXXXXX)
echo "M6 authority evidence: $evidence"
mkdir "$evidence/source" "$evidence/A" "$evidence/B"
git -C "$root" archive ed67cb32 Sources/Mac/Crewd Sources/Mac/Design/CrewColorHash.swift | tar -x -C "$evidence/source"
cp "$root/scripts/dev/fixtures/crewd-authority-shared.swift" "$evidence/fixture.swift"
cp "$root/scripts/dev/fixtures/crewd-authority-server.py" "$evidence/server.py"
find "$evidence/source" -type f -exec shasum -a 256 {} \; > "$evidence/source-sha256.txt"
shasum -a 256 "$evidence/fixture.swift" "$evidence/server.py" >> "$evidence/source-sha256.txt"
xcrun swiftc -module-cache-path "$evidence/cache" -parse-as-library "$evidence/source"/Sources/Mac/Crewd/*.swift "$evidence/source/Sources/Mac/Design/CrewColorHash.swift" "$evidence/fixture.swift" -o "$evidence/test"
python3 "$evidence/server.py" "$evidence/A" A > "$evidence/A/server.log" 2>&1 &
a=$!
python3 "$evidence/server.py" "$evidence/B" B > "$evidence/B/server.log" 2>&1 &
b=$!
trap 'kill "$a" "$b" 2>/dev/null || true' EXIT
attempt=0
while [ ! -f "$evidence/A/ready" ] || [ ! -f "$evidence/B/ready" ]; do
 attempt=$((attempt+1)); [ "$attempt" -lt 250 ] && kill -0 "$a" "$b" 2>/dev/null || { echo "Synthetic authority servers did not become ready" >&2; exit 1; }; sleep 0.02
done
"$evidence/test" "$evidence/A/s" "$evidence/B/s"
python3 - "$evidence" <<'ASSERT'
import pathlib,sys
r=pathlib.Path(sys.argv[1])
for name in ('A','B'):
 methods=(r/name/'requests').read_text().splitlines()
 assert not any(x in methods for x in ('work.update','todo.create','ask.answer')),methods
(r/'RESULTS').write_text('PASS: authority transport records contain read-only calls; retired write never sent\n')
ASSERT
