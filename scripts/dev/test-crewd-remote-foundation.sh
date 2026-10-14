#!/bin/sh
# Caller must hold the project's shared Cargo window. No second cold target.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
evidence=$(mktemp -d /private/tmp/pcw-M6-remote-foundation.XXXXXX)
echo "Remote foundation evidence: $evidence"
ref=$(git -C "$root" rev-parse "${PENDINGCREW_QA_REF:-HEAD}")
printf '%s\n' "$ref" > "$evidence/source-ref.txt"
mkdir "$evidence/source"
git -C "$root" archive "$ref" crewd | tar -x -C "$evidence/source"
cp "$root/crewd/crates/store/tests/remote_restore.rs" "$evidence/source/crewd/crates/store/tests/remote_restore.rs"
mkdir -p "$evidence/source/crewd/crates/remote/tests"
cp "$root/crewd/crates/remote/tests/tls_workflow.rs" "$evidence/source/crewd/crates/remote/tests/tls_workflow.rs"
find "$evidence/source" -type f -exec shasum -a 256 {} \; > "$evidence/source-sha256.txt"
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-/private/tmp/pcw-decision-target}
export CARGO_INCREMENTAL=0
cd "$evidence/source/crewd"
if cargo test -p crewd-store --locked --offline --test remote_restore > "$evidence/restore.log" 2>&1; then cat "$evidence/restore.log"; else result=$?; cat "$evidence/restore.log"; exit "$result"; fi
if cargo test -p crewd-remote --locked --offline --test tls_workflow > "$evidence/tls.log" 2>&1; then cat "$evidence/tls.log"; else result=$?; cat "$evidence/tls.log"; exit "$result"; fi
echo "PASS Store restore and actual loopback mutual TLS; production pairing/RPC/device acceptance remain separate"
