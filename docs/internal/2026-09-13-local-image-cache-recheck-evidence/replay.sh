#!/bin/sh
set -eu
cd /tmp/crew-cache-91-recheck-5f02cb2
EVIDENCE=/tmp/crew-cache-91-recheck-evidence
CACHE=Sources/Mac/Support/CrewLocalImageCache.swift
TESTS=Tests/PendingCrewTests/CrewLocalImageCacheTests.swift
cp "$CACHE" "$EVIDENCE/cache-original.swift"
cp "$TESTS" "$EVIDENCE/tests-original.swift"
trap 'cp "$EVIDENCE/cache-original.swift" "$CACHE"; cp "$EVIDENCE/tests-original.swift" "$TESTS"' EXIT
run_tests() {
  rc=0
  xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination 'platform=macOS' -derivedDataPath .test-archive/dd -only-testing:PendingCrewTests/CrewLocalImageCacheTests test > "$EVIDENCE/$1.log" 2>&1 || rc=$?
  echo "$1 exit=$rc"
  rg 'Executed [0-9]+ tests|TEST SUCCEEDED|TEST FAILED|error: -\[PendingCrewTests.CrewLocalImageCacheTests' "$EVIDENCE/$1.log" | tail -9
  [ "$rc" -eq "$2" ]
}
git show 5ed062d^:Tests/PendingCrewTests/CrewLocalImageCacheTests.swift > "$TESTS"
python3 - <<'PY'
from pathlib import Path
p=Path('Tests/PendingCrewTests/CrewLocalImageCacheTests.swift')
s=p.read_text().replace('CrewLocalImageCache()', 'CrewLocalImageCache(storage: ImmediateEvictionStorage())')
s=s.replace('    // MARK: - fixtures', '''    private final class ImmediateEvictionStorage: CrewLocalImageStorage {
        func object(forKey key: NSString) -> NSImage? { nil }
        func setObject(_ image: NSImage, forKey key: NSString, cost: Int) {}
        func removeAllObjects() {}
    }

    // MARK: - fixtures''')
p.write_text(s)
PY
git diff > "$EVIDENCE/01-old-assertions.patch"
run_tests 01-old-assertions-red 65
cp "$EVIDENCE/tests-original.swift" "$TESTS"
python3 - <<'PY'
from pathlib import Path
p=Path('Sources/Mac/Support/CrewLocalImageCache.swift')
s=p.read_text()
assert s.count('cache = storage ?? CrewLocalNSImageStorage') == 1
p.write_text(s.replace('cache = storage ?? CrewLocalNSImageStorage', 'cache = CrewLocalNSImageStorage'))
PY
git diff > "$EVIDENCE/02-ignore-injection.patch"
run_tests 02-ignore-injection-red 65
cp "$EVIDENCE/cache-original.swift" "$CACHE"
git diff --exit-code
run_tests 03-restored-green 0
git status --short
