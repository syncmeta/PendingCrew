#!/bin/sh
# Local, ad-hoc Codex-first candidate. Does not install, launch, import, or release.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
out=${1:-/private/tmp/pendingcrew-codex-first}
mkdir -p "$out"
out=$(CDPATH= cd -- "$out" && pwd)
app="$out/PendingCrew Codex First.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Helpers"
cat > "$out/CandidateMain.swift" <<'SWIFT'
import SwiftUI
@main struct CandidateMain {
    static func main() { PendingCrewNextApp.main() }
}
SWIFT
candidate_arch=$(uname -m)
xcrun swiftc -O -parse-as-library -target "$candidate_arch-apple-macos14.0" \
    "$root/Sources/Mac/Crewd/CrewdClient.swift" \
    "$root/Sources/Mac/Crewd/CrewdDrafts.swift" \
    "$root/Sources/Mac/Crewd/CrewdWorkStore.swift" "$root/Sources/Mac/Crewd/CrewdNavigation.swift" "$root/Sources/Mac/Crewd/CrewdRequirementDrafts.swift" "$root/Sources/Mac/Crewd/CrewdPlanActions.swift" "$root/Sources/Mac/Crewd/CrewdWorkspace.swift" \
    "$root/Sources/Mac/Design/CrewColorHash.swift" "$root/Sources/Mac/Crewd/CrewdWorkPanels.swift" "$root/Sources/Mac/Crewd/CrewdWorkspaceView.swift" \
    "$out/CandidateMain.swift" -o "$app/Contents/MacOS/PendingCrewCodexFirst"
(cd "$root/crewd" && cargo build --locked --release -p crewd)
cargo_target=${CARGO_TARGET_DIR:-$root/crewd/target}
cp "$cargo_target/release/crewd" "$app/Contents/Helpers/crewd"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.pendingname.pendingcrew.codex-first</string>
<key>CFBundleName</key><string>PendingCrew Codex First</string>
<key>CFBundleDisplayName</key><string>PendingCrew · Codex 首版本机候选</string>
<key>CFBundleExecutable</key><string>PendingCrewCodexFirst</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.1.43</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --sign - "$app/Contents/Helpers/crewd"
codesign --force --sign - "$app"
codesign --verify --strict "$app"
git -C "$root" rev-parse HEAD > "$out/source-head.txt"
git -C "$root" diff --binary > "$out/source-changes.patch"
shasum -a 256 "$app/Contents/MacOS/PendingCrewCodexFirst" "$app/Contents/Helpers/crewd" > "$out/artifact-sha256.txt"
printf '%s\n' "$app"
