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

## Retired connector: strengthened pre-transport boundary

The original authority control proved that a late connector could not ultimately reactivate a retired Workspace or alter the new authority's cache. It did not prove zero late handshakes. A new independent method-trace assertion against exact committed `2347928d` exposed two actual late calls: system.hello and events.subscribe after the old connector's hold was released. `/private/tmp/pcw-M6-authority-retired-transport-red-01.log` exits 1; raw A methods are preserved in `/private/tmp/pcw-M6-authority.pMrp8F/A/requests`. All original final-isolation assertions still passed, so those results are not relabeled as zero-network evidence.

Source `118e0764` adds permanent CrewdClient retirement, called by Workspace retirement: connect/call reject definitely-not-submitted before creating another connection or sending. QA personally ran the identical strengthened control against that committed source: `/private/tmp/pcw-M6-authority-retired-transport-green-01.log` exits 0, evidence `/private/tmp/pcw-M6-authority.mEXDN7`. Each A/B server sees exactly one original hello and one original subscription; releasing the retired late connector adds neither handshake nor business request. B remains healthy and both authorities retain their independent cached values and unknown original draft UUID/payloads.

The dedicated script accepts PENDINGCREW_QA_REF and now defaults to committed HEAD, never a partly edited UI source tree; the exact source ref and SHA256 inventory are retained with each execution. This closes the strengthened early-transport seam only. New shared mobile forms/workspace layout remain a separate pending UI unit, and no remote pairing or device operation was performed.


## Shared mobile operation flow and directory publication

QA uses the committed archive of the shared mobile workspace implementation, compiling the same actual client/model/form sources on macOS with real synthetic Unix transport. This proves runtime data and draft seams, not iOS taps, device rendering, remote pairing or production authorization.

`/private/tmp/pcw-M6-mobile-operation-red-01.log` preserves the exact `0dd409ed` archive at `/private/tmp/pcw-M6-mobile-flow.0RSLbn`: three genuine failures exposed A/B/A late directory publication, late session-start receipt selecting A's returned session inside B, and late crew-create stealing the user's newer B selection. Unknown receipt and original draft preservation controls passed. No producer error explains these failures.

`c22ef439` addressed selection and per-manual-read query gates. QA personally executed five controls in `/private/tmp/pcw-M6-mobile-operation-green-02.log` (source `/private/tmp/pcw-M6-mobile-flow.85muId`): scope-switch and same-crew reversed directory reads, late start/create, missing-ID uncertainty and structured server refusal. Accepted old writes return their valid receipt to the original caller without clearing newly edited full inputs or adopting across the newer selection. No automatic mutation retry is introduced.

A separate actual writer-crossing counterexample exposed the remaining stale publication: a held manual session-directory read completed after normal Workspace refresh had already published newer facts. `/private/tmp/pcw-M6-directory-sync-publication-red-01.log` preserves exact `c22ef439` source/hash at `/private/tmp/pcw-M6-mobile-flow.B8WOtM`. Manual nonce alone was insufficient because normal refresh did not share it.

Source `055c397b` shares each crew/session directory query gate between normal refresh and manual reads, including a second session publication check after awaiting channel.list. QA personally executed all six operation cases against that immutable source: `/private/tmp/pcw-M6-mobile-operation-green-03.log`, evidence `/private/tmp/pcw-M6-mobile-flow.zNhJx8`. The additional crew.list counterpart is independently GREEN in `/private/tmp/pcw-M6-directory-crew-publication-green-01.log`, evidence `/private/tmp/pcw-M6-mobile-flow.9Kb8xb`. Both real server streams prove that a late manual response cannot overwrite the newer normal refresh. Request records retain the exact immutable create/start parameters; no provider runs or writes are replayed.

The navigation entry now registers the actual full Crew client source set, including ChannelWorkStore/ChannelTodoView/MobileWorkspaceView, and checks moved approval routes in their current shared product files. `/private/tmp/pcw-M6-navigation-shared-01.log` is the separately executed Mac compilation, iOS SDK typecheck and original pure navigation structure control; source evidence `/private/tmp/pcw-navigation.pmmLAA`. Text-presence guards are explicitly not runtime click evidence. The prior M2 business suites were not repeated.

Remaining M6 acceptance is the authenticated remote service, real Store restore trust invalidation, online revoke, method allowlist, complete target-project builds and device/window lifecycle. The author separately owns target-project build evidence. These shared-operation GREEN results do not advertise a connected mobile product.


## Remote trust restore foundation — preserved RED, not TLS acceptance

The separately owned remote foundation `61f4f607` corrects an earlier checkpoint's return-type compile error without adding restore invalidation. QA froze all committed Rust inputs under `/private/tmp/pcw-M6-restore-trust-red-01/crewd` before execution. The new `store/tests/remote_restore.rs` invokes actual schema11 Store open_data_dir, official remote identity/grant persistence functions, production backup_db, grant revoke, production backup::restore, and Store reopen. Its inert certificate strings are explicitly Store-fact witnesses, never real cryptographic certificates or successful TLS principals.

The initial actual RED `/private/tmp/pcw-M6-restore-trust-red-01.log` compiled successfully and failed because the revoked grant became active again after restoring the prior active snapshot. Its tempfile cleanup removed the database on panic, so a second narrow execution changed only evidence retention and preserves the raw database/backup/replaced files: `/private/tmp/pcw-M6-restore-trust-red-02.log`, directory `/var/folders/ls/spzx50wd3qg9jp_nfl41swp80000gn/T/.tmpJNV5rG`. Both exit101 on the same business invariant. Before that failure the exact backup bytes, unchanged dbId, authority UUID, server SPKI and certificate witness assertions all passed. Denial cannot be explained by a swapped server identity. The entire test holds exclusive ownership after dropping its live Store; no serve process is present.

The author has the actual RED and owns the staged-copy restore fix. This remains an open acceptance item until the same test passes against the repaired source and the real remote TLS/grant evaluator is separately exercised. The original backup must remain byte-for-byte immutable. No network, user certificate, daily library or paid model is involved.
