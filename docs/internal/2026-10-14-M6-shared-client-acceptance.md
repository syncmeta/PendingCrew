# M6 shared client independent acceptance — 2026-10-14

## Executed scope

Independent QA against the M6 worktree, not the paused a9 visible candidate. Shared source freeze `3e14d12c` plus exact UInt64 repair `ed67cb32`. No Cargo, paid model, app launch, daily data, installation, remote pairing, or device operation. Synthetic Unix sockets exercise the real macOS `CrewdClient` and the same shared data/draft implementation that the author separately typechecked for iOS. The authorityRecordID is only a cache/draft namespace, not authentication evidence.

## Preserved RED and GREEN

- `/private/tmp/pcw-M6-wire-u64-red-01.log`: immutable committed 3e14 Client, actual byte-fragment decode → JSONEncoder → UInt64 decoder loses UInt64.max via Double and exits 133. Raw test/source hashes and binary remain `/private/tmp/pcw-M6-wire-qa.2FgmD1`.
- `/private/tmp/pcw-M6-wire-u64-green-01.log`: same fixture against the exact signed/unsigned integer repair exits 0. Evidence `/private/tmp/pcw-M6-wire-qa.AnHP0b`. Full UInt64.max and 2^63+1, split UTF8, unknown value/zero/null, high integer not forged into an Int RPC identifier, exact frame bound, excessive frame rejection, incomplete EOF rejection, and first complete response delivered before a later malformed sibling.
- `/private/tmp/pcw-M6-wire-cancel-01.log`: initial real Unix cancellation controls pass; retained original evidence, not replaced.
- `/private/tmp/pcw-M6-wire-cancel-02.log`: extended real Unix controls exit 0; evidence `/private/tmp/pcw-M6-wire-cancel.6tQBVF`. Pre-send cancellation is definitely not submitted and emits no request; cancellation after the server's actual held-request receipt remains uncertain. Only that pending request is retired, while another response and event succeed. A late canceled response is ignored. Structured CAS data retains the string revision. An event callback retires the original connection while its receive reaches EOF; replacement hello/subscription and probe succeed. Server method records prove no mutation replay.
- `/private/tmp/pcw-M6-authority-shared-01.log`: exact ed67 committed archive, all Crew client sources and ColorHash compile to one executable, then two real synthetic Unix authority connectors. Evidence `/private/tmp/pcw-M6-authority.Rett2t`. The same W1/#1/H1 have distinct authoritative server values in the two actual WorkStore caches. Request UUIDs and original unknown requirement/plan/Human draft inputs stay independent. Retiring A prevents reads, writes and reconnect while B continues reading its own W1. A controlled delayed connector completing after retirement cannot resurrect the old instance. Server records contain read-only calls; no retired write is delivered.

The server fixtures deliberately expose only the local synthetic transport. They do not validate remote authentication, pairing or TLS. Folder/document behavior remains covered by the previous M2 dedicated acceptance, rather than duplicating those suites here.

## Compatibility adjustment and remaining delivery boundaries

The old local smoke fixture now explicitly unwraps the Mac local workspace's known nonnil data directory before asserting that the forbidden legacy directory remains absent. This narrow source compatibility change is not claimed as a fresh execution of the old whole-model smoke chain.

Author's macOS/iOS SDK typechecks are distinct evidence from these independently executed macOS transport tests. The new shared SwiftUI forms are still an in-progress author unit. No iOS remote connection entry, actual mobile UI, paired backend authority, or device lifecycle has been accepted here. The a9 visible window remains paused with zero actual clicks, as documented in the M3 provenance record; this unit does not change that status.
