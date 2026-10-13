#!/bin/sh
# Read-only bundle/metadata checks; never starts the application or backend.
set -eu
fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
[ "$#" -eq 1 ] || fail 'usage: check-codex-first-artifact.sh /absolute/path/Name.app'
app=$1
[ -d "$app/Contents" ] || fail 'application bundle is missing'
app=$(CDPATH= cd -- "$app" && pwd)
meta=$(dirname "$app")
plist="$app/Contents/Info.plist"
[ -f "$plist" ] || fail 'Info.plist is missing'
exe=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$plist") || fail 'CFBundleExecutable is missing'
case "$exe" in ''|*/*|*..*) fail 'invalid CFBundleExecutable' ;; esac
binary="$app/Contents/MacOS/$exe"
helper="$app/Contents/Helpers/crewd"
[ -f "$binary" ] && [ -x "$binary" ] || fail 'GUI executable is missing or not executable'
[ -f "$helper" ] && [ -x "$helper" ] || fail 'embedded crewd is missing or not executable'
[ -f "$meta/source-head.txt" ] || fail 'source-head.txt is missing'
[ "$(wc -l < "$meta/source-head.txt" | tr -d ' ')" -eq 1 ] || fail 'source-head.txt must contain exactly one line'
LC_ALL=C /usr/bin/grep -Eq '^[0-9a-f]{40}$' "$meta/source-head.txt" || fail 'source-head.txt must contain one Git SHA'
[ -f "$meta/source-changes.patch" ] || fail 'source-changes.patch is missing'
[ ! -s "$meta/source-changes.patch" ] || fail 'source-changes.patch is not empty; source is not frozen'
manifest="$meta/artifact-sha256.txt"
[ -f "$manifest" ] || fail 'artifact-sha256.txt is missing'
count=0
seen_gui=0
seen_helper=0
while IFS= read -r line || [ -n "$line" ]; do
    count=$((count + 1))
    expected=${line%% *}
    printf '%s\n' "$expected" | LC_ALL=C /usr/bin/grep -Eq '^[0-9a-f]{64}$' || fail 'invalid digest in manifest'
    case "$line" in
        "$expected  "*"/Contents/MacOS/$exe")
            [ "$seen_gui" -eq 0 ] || fail 'duplicate GUI digest'
            seen_gui=1; file=$binary ;;
        "$expected  "*'/Contents/Helpers/crewd')
            [ "$seen_helper" -eq 0 ] || fail 'duplicate helper digest'
            seen_helper=1; file=$helper ;;
        *) fail 'manifest must name only the GUI executable and embedded crewd' ;;
    esac
    actual=$(/usr/bin/shasum -a 256 "$file")
    [ "${actual%% *}" = "$expected" ] || fail "digest mismatch: $file"
done < "$manifest"
[ "$count" -eq 2 ] && [ "$seen_gui" -eq 1 ] && [ "$seen_helper" -eq 1 ] || fail 'manifest must contain exactly two required digests'
checker_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
/usr/bin/python3 "$checker_root/check-codex-first-provenance.py" "$app" || fail 'GUI/backend source proof does not establish a paired frozen candidate'
/usr/bin/codesign --verify --strict "$helper" || fail 'embedded crewd signature is invalid'
/usr/bin/codesign --verify --strict "$app" || fail 'application signature is invalid'
printf 'PASS: paired frozen artifact, source proof, signatures and recorded hashes match\nSource: '
cat "$meta/source-head.txt"
printf 'Application: %s\n' "$app"
