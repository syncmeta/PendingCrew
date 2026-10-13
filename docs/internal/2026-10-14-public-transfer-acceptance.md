# Public two-party work transfer — isolated candidate acceptance

The existing organization work book remains the sole state authority. Public `work.transferRequest`, `work.transferAccept`, and `work.transferWithdraw` and authenticated MCP equivalents expose the pure two-party rules. Request preserves owner; target acceptance records real owner history and original note. Human can cancel, but cannot impersonate either participant. Attached direct ordinary RPC rejects all three writes; unattached local Human uses the existing local trust boundary, not OS isolation.

## Stable offer and bounded reads

Full work detail retains transfer note and genuine organization request-event sequence. Compact MCP summary includes only target and optional requestSeq, with no long note. Migrated pending offers without an organization request event remain unknown; no event or automatic wake is invented. Newly queued offers use `work-transfer:<public W>:<requestSeq>`. Report revision changes preserve a valid offer; accept, withdraw, reoffer, ownership change, crew freeze and later human stop invalidate old hints at both rendering and reservation claim. Lost request journals fail closed rather than silently pretending to be legacy.

## Evidence

- Independent real Unix socket/authenticated MCP and actual fake-runner actor: six cases passed, `/private/tmp/pcw-transfer-workflow-02.log`. Includes same-payload reoffer identity, full-note history, foreign authorization denial, stale CAS, SQL rollback, delivery and render/claim race guards.
- Own true-v8 legacy migration/journal-loss/stop-and-event-fault tests: three passed, `/private/tmp/pcw-transfer-collab-02.log`. Legacy fixture correction added its genuinely issued old alias; production migration validation was retained.
- Full workspace: **2517 passed, 0 failed, 8 ignored**, `/private/tmp/pcw-transfer-workspace-01.log`.
- Strict workspace all-target clippy and formatting passed: `/private/tmp/pcw-transfer-clippy-01.log`, `/private/tmp/pcw-transfer-fmt-01.log`.
- Generated public schema and bilingual tool surface reflect the three methods. Actual CLI coverage calls all three routes; role catalogs retain authenticated identity and compact reads.

Initial logs remain: package checks caught projection inference and constructor naming; a package-only all-target check required its explicit testkit feature. Independent workflow01 failed only its note whitespace fixture against the existing trim rule. First full collab run had one obsolete pending-summary expectation, corrected to the bounded public hint and verified before full workspace.

This is isolated source/protocol/transaction/actor proof. Real account/model use, daily-data migration, main integration, installation and public release are not claimed. Verified lost reassignment and complete documents/view-folder work remain subsequent engineering units.
