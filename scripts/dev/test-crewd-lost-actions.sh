#!/bin/sh
# Offline exact typed transport/cache contract. No daemon/model/account/app.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-lost-client.XXXXXX)
echo "Lost client evidence: $fixture"
mkdir -p "$fixture/source"
for file in CrewdClient.swift CrewdWorkStore.swift CrewdPlanActions.swift; do cp "$root/Sources/Mac/Crewd/$file" "$fixture/source/$file"; done
cp "$root/scripts/dev/fixtures/crewd-lost-actions.swift" "$fixture/source/fixture.swift"
shasum -a 256 "$fixture"/source/*.swift > "$fixture/source-sha256.txt"
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library "$fixture"/source/*.swift -o "$fixture/test"
"$fixture/test"
