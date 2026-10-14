#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
evidence=$(mktemp -d /private/tmp/pcw-M6-wire-cancel.XXXXXX)
echo "M6 cancellation evidence: $evidence"
cp "$root/Sources/Mac/Crewd/CrewdClient.swift" "$evidence/Client.swift"
cp "$root/scripts/dev/fixtures/crewd-wire-cancel.swift" "$evidence/fixture.swift"
cp "$root/scripts/dev/fixtures/crewd-wire-cancel-server.py" "$evidence/server.py"
shasum -a 256 "$evidence"/*.swift "$evidence/server.py" > "$evidence/source-sha256.txt"
xcrun swiftc -module-cache-path "$evidence/cache" -parse-as-library "$evidence/Client.swift" "$evidence/fixture.swift" -o "$evidence/test"
python3 "$evidence/server.py" "$evidence" > "$evidence/server.log" 2>&1 &
server=$!
trap 'kill "$server" 2>/dev/null || true' EXIT
attempt=0
while [ ! -f "$evidence/ready" ]; do
 attempt=$((attempt+1)); [ "$attempt" -lt 250 ] && kill -0 "$server" 2>/dev/null || { echo "Synthetic server did not become ready" >&2; exit 1; }; sleep 0.02
done
"$evidence/test" "$evidence"
wait "$server"
