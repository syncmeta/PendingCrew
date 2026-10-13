#!/bin/sh
# Offline deterministic requirement store evidence; no daemon/model/app.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-requirement-store.XXXXXX)
echo "Evidence: $fixture"
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library \
 "$root/Sources/Mac/Crewd/CrewdClient.swift" "$root/Sources/Mac/Crewd/CrewdWorkStore.swift" \
 "$root/scripts/dev/fixtures/crewd-requirement-store.swift" -o "$fixture/test"
"$fixture/test"
