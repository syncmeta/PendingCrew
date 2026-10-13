#!/bin/sh
# Pure window-input separation; no GUI, daemon, Cargo or model.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-resource-domain.XXXXXX)
echo "Resource domain evidence: $fixture"
mkdir -p "$fixture/source"
for file in CrewdClient.swift CrewdWorkStore.swift CrewdResources.swift CrewdResourceDrafts.swift; do cp "$root/Sources/Mac/Crewd/$file" "$fixture/source/$file"; done
cp "$root/scripts/dev/fixtures/crewd-resource-domain.swift" "$fixture/source/fixture.swift"
shasum -a 256 "$fixture"/source/*.swift > "$fixture/source-sha256.txt"
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library "$fixture"/source/*.swift -o "$fixture/test"
"$fixture/test"
