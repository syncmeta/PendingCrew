#!/bin/sh
# Offline group protocol/cache/draft acceptance; no app/model/Cargo.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-channel-client-qa.XXXXXX)
echo "Channel client evidence: $fixture"
mkdir -p "$fixture/source"
for file in CrewdClient.swift CrewdWorkStore.swift CrewdPlanActions.swift CrewdRequirementDrafts.swift CrewdChannelWorkStore.swift; do cp "$root/Sources/Mac/Crewd/$file" "$fixture/source/$file"; done
cp "$root/scripts/dev/fixtures/crewd-channel-work.swift" "$fixture/source/fixture.swift"
shasum -a 256 "$fixture"/source/*.swift > "$fixture/input-sha256.txt"
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library "$fixture"/source/*.swift -o "$fixture/test"
"$fixture/test"
