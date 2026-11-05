#!/bin/sh
# Offline Swift inbox, Ask compatibility and original receipt regression.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
evidence=$(mktemp -d /private/tmp/pcw-human-inbox-fixture.XXXXXX)
echo "Human inbox evidence: $evidence"
cd "$root"
set -- \
    Sources/Mac/Crewd/CrewdClient.swift \
    Sources/Mac/Crewd/CrewdLocalIPC.swift \
    Sources/Mac/Crewd/CrewdPublicCommandWire.swift \
    Sources/Mac/Crewd/CrewdCodexWire.swift \
    Sources/Mac/Crewd/CrewdClaudeHistoryStore.swift \
    Sources/Mac/Crewd/CrewdPresentationJournal.swift \
    Sources/Mac/Crewd/CrewdWorkStore.swift \
    Sources/Mac/Crewd/CrewdPublicCommands.swift \
    Sources/PersonalSync/CrewPersonalCoreWire.swift \
    Sources/PersonalSync/AppleBranchStore.swift \
    Sources/Chat/Adapter/CrewComposerDraftValue.swift \
    Sources/Chat/Adapter/CrewComposerDrafts.swift
for fixture in crewd-human-inbox crewd-ask-resolution; do
    xcrun swiftc -module-cache-path "$evidence/cache" -parse-as-library "$@" \
        "scripts/dev/fixtures/$fixture.swift" -o "$evidence/$fixture"
    "$evidence/$fixture"
done
