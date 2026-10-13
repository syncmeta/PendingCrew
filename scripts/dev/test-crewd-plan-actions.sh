#!/bin/sh
# Typed plan-command and window draft regression; no GUI, daemon, model or Cargo.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-plan-actions.XXXXXX)
echo "Plan UI evidence: $fixture"
client="$root/Sources/Mac/Crewd"
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library "$client/CrewdClient.swift" "$client/CrewdWorkStore.swift" "$client/CrewdPlanActions.swift" "$root/scripts/dev/fixtures/crewd-plan-actions.swift" -o "$fixture/test"
"$fixture/test"
