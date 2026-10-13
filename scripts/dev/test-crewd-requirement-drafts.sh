#!/bin/sh
# Pure UI submission state: no GUI, daemon, account, model or Cargo.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-requirement-drafts.XXXXXX)
echo "Requirement UI evidence: $fixture"
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library "$root/Sources/Mac/Crewd/CrewdRequirementDrafts.swift" "$root/scripts/dev/fixtures/crewd-requirement-drafts.swift" -o "$fixture/test"
"$fixture/test"
