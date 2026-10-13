#!/bin/sh
# Isolated ad-hoc candidate only. Never installs, launches, imports, or releases.
# Cargo requires an explicit coordination flag; a supplied helper bypasses Cargo.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
frozen_ref=$(git -C "$root" rev-parse --verify "${PENDINGCREW_BUILD_REF:-HEAD}^{commit}")
out=${1:-$(mktemp -d /private/tmp/pendingcrew-codex-first.XXXXXX)}
mkdir -p "$out"
out=$(CDPATH= cd -- "$out" && pwd)
python3 - "$out" <<'PY'
import pathlib,sys
if any(pathlib.Path(sys.argv[1]).iterdir()):raise SystemExit('Build output must be empty; preserve earlier artifacts and choose a new directory.')
PY
helper=${PENDINGCREW_BUILD_CREWD_PROGRAM:-}
witness=${PENDINGCREW_BUILD_CREWD_WITNESS:-}
if [ -n "$helper" ]; then
    case "$helper" in /*) ;; *) echo 'Prebuilt helper must have an absolute path.' >&2; exit 2;; esac
    [ -x "$helper" ] || { echo 'Prebuilt helper is not executable.' >&2; exit 2; }
    mode=declared-unverified
    if [ -n "$witness" ]; then mode=verified-prebuilt; fi
else
    [ "${PENDINGCREW_BUILD_ALLOW_CARGO:-0}" = 1 ] || { echo 'Coordinate the sole Cargo target first, then set PENDINGCREW_BUILD_ALLOW_CARGO=1, or supply an existing helper.' >&2; exit 2; }
    mode=built-from-snapshot
fi
snapshot=$(mktemp -d "$out/source-inputs.XXXXXX")
archive="$out/source-inputs.tar"
git -C "$root" archive --format=tar --output="$archive" "$frozen_ref" Sources/Mac/Crewd Sources/Mac/Design/CrewColorHash.swift crewd scripts/dev/build-codex-first-macos.sh scripts/dev/write-codex-first-provenance.py
/usr/bin/tar -xf "$archive" -C "$snapshot"
# The runner itself must match the selected committed recipe.
cmp "$root/scripts/dev/build-codex-first-macos.sh" "$snapshot/scripts/dev/build-codex-first-macos.sh" || { echo 'Builder differs from the frozen commit; commit the recipe or select its matching ref.' >&2; exit 2; }
recipe="$snapshot/scripts/dev/write-codex-first-provenance.py"
cat > "$snapshot/CandidateMain.swift" <<'SWIFT'
import SwiftUI
@main struct CandidateMain {
    static func main() { PendingCrewNextApp.main() }
}
SWIFT
count=$(git -C "$root" rev-list --count "$frozen_ref")
version="0.0.$count"
python3 "$recipe" inputs "$out" "$snapshot" "$frozen_ref" "$count"
if [ "$mode" = verified-prebuilt ]; then python3 "$recipe" preflight-witness "$out" "$witness" "$helper"; fi
if [ -n "$helper" ]; then cp "$helper" "$out/crewd.pre-sign"; if [ "$mode" = verified-prebuilt ]; then python3 "$recipe" preflight-witness "$out" "$witness" "$out/crewd.pre-sign"; fi; fi
printf '%s\n' "$frozen_ref" > "$out/source-head.txt"
: > "$out/source-changes.patch"
app="$out/PendingCrew Codex First.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Helpers"
candidate_arch=$(uname -m)
python3 "$recipe" log "$out" gui gui-build.log xcrun swiftc -O -parse-as-library -target "$candidate_arch-apple-macos14.0" "$snapshot"/Sources/Mac/Crewd/*.swift "$snapshot/Sources/Mac/Design/CrewColorHash.swift" "$snapshot/CandidateMain.swift" -o "$out/gui.pre-sign"
xcrun swiftc -O -parse-as-library -target "$candidate_arch-apple-macos14.0" "$snapshot"/Sources/Mac/Crewd/*.swift "$snapshot/Sources/Mac/Design/CrewColorHash.swift" "$snapshot/CandidateMain.swift" -o "$out/gui.pre-sign" >> "$out/gui-build.log" 2>&1
printf 'Exit: 0\n' >> "$out/gui-build.log"
if [ "$mode" = built-from-snapshot ]; then
    cargo_target=${CARGO_TARGET_DIR:-$root/crewd/target}
    case "$cargo_target" in /*) ;; *) echo 'CARGO_TARGET_DIR must be absolute.' >&2; exit 2;; esac
    case "$cargo_target/" in "$snapshot/"*) echo 'Cargo target cannot modify the immutable input snapshot.' >&2; exit 2;; esac
    profile=${PENDINGCREW_BUILD_BACKEND_PROFILE:-release}
    set -- cargo build --manifest-path "$snapshot/crewd/Cargo.toml" --locked -p crewd --message-format=json
    if [ "$profile" = release ]; then set -- "$@" --release; fi
    if [ "${PENDINGCREW_BUILD_BACKEND_DEFAULT_FEATURES:-1}" = 0 ]; then set -- "$@" --no-default-features; fi
    if [ -n "${PENDINGCREW_BUILD_BACKEND_FEATURES:-}" ]; then set -- "$@" --features "$PENDINGCREW_BUILD_BACKEND_FEATURES"; fi
    if [ "${PENDINGCREW_BUILD_BACKEND_ALL_FEATURES:-0}" = 1 ]; then set -- "$@" --all-features; fi
    backend_target=${PENDINGCREW_BUILD_BACKEND_TARGET:-${CARGO_BUILD_TARGET:-}}
    if [ -n "$backend_target" ]; then set -- "$@" --target "$backend_target"; fi
    python3 "$recipe" log "$out" rust backend-build.log env "CREWD_GIT_SHA=$frozen_ref" CREWD_GIT_DIRTY=0 "CARGO_TARGET_DIR=$cargo_target" "$@"
    (cd "$snapshot/crewd" && CREWD_GIT_SHA="$frozen_ref" CREWD_GIT_DIRTY=0 CARGO_TARGET_DIR="$cargo_target" "$@") >> "$out/backend-build.log" 2>&1
    printf 'Exit: 0\n' >> "$out/backend-build.log"
    cargo_profile=$profile
    if [ -n "$backend_target" ]; then helper="$cargo_target/$backend_target/$cargo_profile/crewd"; else helper="$cargo_target/$cargo_profile/crewd"; fi
fi
if [ "$mode" = built-from-snapshot ]; then cp "$helper" "$out/crewd.pre-sign"; fi
cp "$out/gui.pre-sign" "$app/Contents/MacOS/PendingCrewCodexFirst"
cp "$out/crewd.pre-sign" "$app/Contents/Helpers/crewd"
python3 - "$app/Contents/Info.plist" "$frozen_ref" "$version" "$count" <<'PY'
import pathlib,plistlib,sys
path,ref,version,count=sys.argv[1:]
value={'CFBundleIdentifier':'com.pendingname.pendingcrew.codex-first','CFBundleName':'PendingCrew Codex First','CFBundleDisplayName':'PendingCrew · 开发候选 '+ref[:8],'CFBundleExecutable':'PendingCrewCodexFirst','CFBundlePackageType':'APPL','CFBundleShortVersionString':version,'CFBundleVersion':count,'PendingCrewSourceRef':ref,'LSMinimumSystemVersion':'14.0','NSHighResolutionCapable':True}
pathlib.Path(path).write_bytes(plistlib.dumps(value))
PY
codesign --force --sign - "$app/Contents/Helpers/crewd"
codesign --force --sign - "$app"
codesign --verify --strict "$app/Contents/Helpers/crewd"
codesign --verify --strict "$app"
python3 "$recipe" final "$out" "$mode" "$app" "$witness"
shasum -a 256 "$app/Contents/MacOS/PendingCrewCodexFirst" "$app/Contents/Helpers/crewd" > "$out/artifact-sha256.txt"
printf 'Backend source pairing: %s\nApplication: %s\n' "$mode" "$app"
