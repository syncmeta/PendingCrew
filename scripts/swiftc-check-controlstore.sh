#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
TMP=$(mktemp -d)
# LocalCrewControlStore 依赖 LocalWhiteboardStore.defaultDirectory；测试用注入目录，桩掉即可。
cat > "$TMP/stub.swift" <<'EOF'
import Foundation
enum LocalWhiteboardStore { static var defaultDirectory: URL { FileManager.default.temporaryDirectory } }
EOF
swiftc -o "$TMP/run" \
  Sources/Stores/LocalCrewControlStore.swift "$TMP/stub.swift" \
  scripts/LocalCrewControlStoreMain.swift 2>&1
"$TMP/run"
