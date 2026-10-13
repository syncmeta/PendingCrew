#!/bin/sh
# Frozen offline Human draft/receipt evidence; no daemon/account/app/Cargo.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-human-actions.XXXXXX)
echo "Human evidence: $fixture"
ref=$(git -C "$root" rev-parse --verify "${PENDINGCREW_CLIENT_REF:-HEAD}^{commit}")
mkdir -p "$fixture/source"
echo "$ref" > "$fixture/client-source-ref.txt"
git -C "$root" ls-tree -r --name-only "$ref" -- Sources/Mac/Crewd > "$fixture/source-paths.txt"
while IFS= read -r source_file; do
 case "$source_file" in *.swift) git -C "$root" show "$ref:$source_file" > "$fixture/source/$(basename "$source_file")";; esac
done < "$fixture/source-paths.txt"
git -C "$root" show "$ref:Sources/Mac/Design/CrewColorHash.swift" > "$fixture/source/CrewColorHash.swift"
cp "$root/scripts/dev/fixtures/crewd-human-actions.swift" "$fixture/source/fixture.swift"
shasum -a 256 "$fixture/source/"*.swift > "$fixture/client-source-sha256.txt"
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library "$fixture/source/"*.swift -o "$fixture/test"
"$fixture/test"
