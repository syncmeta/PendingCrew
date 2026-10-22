#!/bin/sh
# Offline bounded resource authority projections; source hashes retained, no daemon/model/app.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-resources-qa.XXXXXX)
echo "Resources evidence: $fixture"
mkdir -p "$fixture/source"
for file in CrewdClient.swift CrewdLocalIPC.swift CrewdWorkStore.swift CrewdResources.swift CrewdResourceDrafts.swift; do cp "$root/Sources/Mac/Crewd/$file" "$fixture/source/$file"; done
cp "$root/scripts/dev/fixtures/crewd-resources.swift" "$fixture/source/fixture.swift"
shasum -a 256 "$fixture"/source/*.swift > "$fixture/source-sha256.txt"
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library "$fixture"/source/*.swift -o "$fixture/test"
"$fixture/test"
