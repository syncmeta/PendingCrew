#!/bin/sh
# Offline SwiftUI typecheck and pure structural snapshots. No GUI, model or Cargo.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-navigation.XXXXXX)
echo "Navigation evidence: $fixture"
client="$root/Sources/Mac/Crewd"
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library "$client/CrewdClient.swift" "$client/CrewdWorkStore.swift" "$client/CrewdNavigation.swift" "$root/scripts/dev/fixtures/crewd-navigation.swift" -o "$fixture/fixture"
"$fixture/fixture" "$fixture/navigation-structure.json"
# Register the complete frozen shared workspace source set, including channel
# projection and conditional mobile view. This remains compile/structural proof.
xcrun swiftc -module-cache-path "$fixture/cache" -typecheck -parse-as-library "$client"/*.swift "$root/Sources/Mac/Design/CrewColorHash.swift"
ios_sdk=$(xcrun --sdk iphonesimulator --show-sdk-path)
xcrun swiftc -module-cache-path "$fixture/cache-ios" -target arm64-apple-ios17.0-simulator -sdk "$ios_sdk" -typecheck -parse-as-library "$client"/*.swift "$root/Sources/Mac/Design/CrewColorHash.swift"
python3 - "$client/CrewdWorkspaceView.swift" "$client/CrewdHumanActions.swift" "$client/CrewdMobileWorkspaceView.swift" "$client/CrewdWorkStore.swift" <<'PYCODE'
from pathlib import Path
import sys
s="\n".join(Path(path).read_text() for path in sys.argv[1:])
# Source-presence guard across the actual shared renderer/typed command routes.
# This does not assert actual GUI interaction or runtime delivery.
for token in ['message.post','session.start','session.interrupt','session.stop','session.resume','session.respondRunnerRequest','session.answer','restart.answer','ask.answer','clearIfUnchanged','CrewdNavigation.Sidebar','CrewdNavigation.Tab','selectConversation','本群 Todo','本群成员']:
 assert token in s, token
assert '总览' not in s and '早上好' not in s
print('PASS: actual macOS/iOS source typecheck, pure hierarchy snapshots, and retained shared action/recovery/draft route source')
PYCODE
