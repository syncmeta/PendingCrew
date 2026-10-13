#!/bin/sh
# Actual AppKit/SwiftUI offscreen render; synthetic state only, no daemon or model.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-native-render.XXXXXX)
echo "Native render evidence: $fixture"
# Snapshot a committed source version so parallel UI edits cannot change this render.
ref=$(git -C "$root" rev-parse --verify "${PENDINGCREW_RENDER_REF:-HEAD}^{commit}")
mkdir -p "$fixture/source"
echo "$ref" > "$fixture/source-ref.txt"
git -C "$root" ls-tree -r --name-only "$ref" -- Sources/Mac/Crewd > "$fixture/source-paths.txt"
while IFS= read -r source_file; do
 case "$source_file" in *.swift) git -C "$root" show "$ref:$source_file" > "$fixture/source/$(basename "$source_file")";; esac
done < "$fixture/source-paths.txt"
git -C "$root" show "$ref:Sources/Mac/Design/CrewColorHash.swift" > "$fixture/source/CrewColorHash.swift"
cp "$root/scripts/dev/fixtures/crewd-native-render.swift" "$fixture/source/fixture.swift"
shasum -a 256 "$fixture"/source/*.swift > "$fixture/source-sha256.txt"
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library "$fixture"/source/*.swift -o "$fixture/render"
"$fixture/render" "$fixture"
