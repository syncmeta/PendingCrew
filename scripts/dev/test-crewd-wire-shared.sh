#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-M6-wire-qa.XXXXXX)
echo "M6 wire evidence: $fixture"
if [ -n "${PENDINGCREW_QA_REF:-}" ]; then git -C "$root" show "$PENDINGCREW_QA_REF:Sources/Mac/Crewd/CrewdClient.swift" > "$fixture/Client.swift"; else cp "$root/Sources/Mac/Crewd/CrewdClient.swift" "$fixture/Client.swift"; fi
cp "$root/scripts/dev/fixtures/crewd-wire-shared.swift" "$fixture/fixture.swift"
shasum -a 256 "$fixture"/*.swift > "$fixture/source-sha256.txt"
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library "$fixture/Client.swift" "$fixture/fixture.swift" -o "$fixture/test"
"$fixture/test"
