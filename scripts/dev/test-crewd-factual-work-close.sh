#!/bin/sh
# Offline factual Work close and original receipt regression.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
evidence=$(mktemp -d /private/tmp/pcw-factual-work-close-fixture.XXXXXX)
echo "Factual Work close evidence: $evidence"
cd "$root"
set -- \
    Sources/Mac/Crewd/CrewdClient.swift \
    Sources/Mac/Crewd/CrewdLocalIPC.swift \
    Sources/Mac/Crewd/CrewdPublicCommandWire.swift \
    Sources/Mac/Crewd/CrewdCodexWire.swift \
    Sources/Mac/Crewd/CrewdClaudeHistoryStore.swift \
    Sources/Mac/Crewd/CrewdPresentationJournal.swift \
    Sources/Mac/Crewd/CrewdWorkStore.swift \
    Sources/Mac/Crewd/CrewdChannelWorkStore.swift \
    Sources/Mac/Crewd/CrewdPublicCommands.swift \
    Sources/PersonalSync/CrewPersonalCoreWire.swift \
    Sources/PersonalSync/AppleBranchStore.swift \
    Sources/Chat/Adapter/CrewComposerDraftValue.swift \
    Sources/Chat/Adapter/CrewComposerDrafts.swift
xcrun swiftc -disable-sandbox -module-cache-path "$evidence/cache" -parse-as-library "$@" \
    scripts/dev/fixtures/crewd-factual-work-close.swift -o "$evidence/test"
"$evidence/test"
