#!/bin/sh
# Pure offline Swift draft semantics; no server, GUI, account, or model.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-drafts.XXXXXX)
echo "Offline draft evidence: $fixture"
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library \
    "$root/Sources/Mac/Crewd/CrewdDrafts.swift" \
    "$root/scripts/dev/fixtures/crewd-drafts-test.swift" -o "$fixture/draft-test"
"$fixture/draft-test" "$@"
