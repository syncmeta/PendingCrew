#!/bin/sh
# Offline SwiftUI typecheck and pure structural snapshots. No GUI, model or Cargo.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
fixture=$(mktemp -d /private/tmp/pcw-navigation.XXXXXX)
echo "Navigation evidence: $fixture"
client="$root/Sources/Mac/Crewd"
xcrun swiftc -module-cache-path "$fixture/cache" -parse-as-library "$client/CrewdClient.swift" "$client/CrewdWorkStore.swift" "$client/CrewdNavigation.swift" "$root/scripts/dev/fixtures/crewd-navigation.swift" -o "$fixture/fixture"
"$fixture/fixture" "$fixture/navigation-structure.json"
xcrun swiftc -module-cache-path "$fixture/cache" -typecheck -parse-as-library "$client/CrewdClient.swift" "$client/CrewdDrafts.swift" "$client/CrewdWorkStore.swift" "$client/CrewdNavigation.swift" "$root/Sources/Mac/Design/CrewColorHash.swift" "$client/CrewdHandoffView.swift" "$client/CrewdWorkPanels.swift" "$client/CrewdRequirementDrafts.swift" "$client/CrewdPlanActions.swift" "$client/CrewdHumanActions.swift" "$client/CrewdResources.swift" "$client/CrewdResourceDrafts.swift" "$client/CrewdDocumentsView.swift" "$client/CrewdFolderControls.swift" "$client/CrewdWorkspace.swift" "$client/CrewdWorkspaceView.swift"
python3 - "$client/CrewdWorkspaceView.swift" <<'PYCODE'
from pathlib import Path
import sys
s=Path(sys.argv[1]).read_text()
# Verify existing mutation/recovery routes remain in the actual renderer, not a mock layout.
for token in ['message.post','session.start','session.interrupt','session.stop','session.resume','session.respondRunnerRequest','session.answer','restart.answer','ask.answer','clearIfUnchanged','CrewdNavigation.Sidebar','CrewdNavigation.Tab','selectConversation','本群 Todo','本群成员']:
 assert token in s, token
assert '总览' not in s and '早上好' not in s
print('PASS: SwiftUI typecheck and retained action/recovery/draft routes')
PYCODE
