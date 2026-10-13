#!/bin/sh
# Explicit old/new gated binaries, isolated synthetic stores; never imports daily data.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
: "${PENDINGCREW_TEST_OLD_CREWD_PROGRAM:?Explicit v9 binary required}"
: "${PENDINGCREW_TEST_CREWD_PROGRAM:?Explicit v10 binary required}"
: "${FAKE_CODEX_PROGRAM:?Explicit fake executor required}"
fixture=$(mktemp -d /private/tmp/pcw-resources-migration.XXXXXX)
echo "Migration evidence: $fixture"
mkdir -p "$fixture/bin"
cp "$PENDINGCREW_TEST_OLD_CREWD_PROGRAM" "$fixture/bin/crewd-v9"
cp "$PENDINGCREW_TEST_CREWD_PROGRAM" "$fixture/bin/crewd-v10"
cp "$FAKE_CODEX_PROGRAM" "$fixture/bin/fake_codex"
cp "$root/scripts/dev/fixtures/crewd-resources-migration.py" "$fixture/fixture.py"
shasum -a 256 "$fixture"/bin/* "$fixture/fixture.py" > "$fixture/input-sha256.txt"
python3 "$fixture/fixture.py" "$fixture" "$fixture/bin/crewd-v9" "$fixture/bin/crewd-v10" "$fixture/bin/fake_codex" "$root/crewd/crates/legacy/tests/fixtures/basic"
