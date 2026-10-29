# Claude managed PTY: PCW-owned history and exact manual resume

## Current addition: historical resume disposition (source c62a85bd, 2026-10-29)

This section specifies the implemented recovery addition in `crewd/crates/proto/src/claude.rs`, `crewd/crates/server/src/claude_recovery.rs`, `orchestra/public_control.rs`, and the Swift Claude history/public-command/Workspace consumers at c62a85bd. It supersedes the older snapshot's statements that no new capability or command exists **for this addition only**. The three original history reads and strict `session.resume` keep their own semantics. This source contract is not evidence of successful real-Claude lifecycle, native complete history, device use or deployment of a particular binary.

Support requires the advertised `session.claude.recovery` capability. Clients without it retain the original Unknown blocker; do not infer support from a schema version or executor availability. All requests use existing authenticated authority and principal scope. Recovery inspection requires `session.inspect` authorization, rechecked inside the DB owner; disposition requires `session.resume` authorization, rechecked in its transaction. It is not an anonymous, Agent, Cloud or offline authority grant.

### Read: `session.claude.recoveryInspect`

Params: `{sessionId}`. The normal RPC result is the following direct object, with explicit nullable fields:

```text
{
  sessionId,
  originalAgentSessionId: canonicalUUID | null,
  target: CommandRuntimeTarget,
  eligibility,
  evidenceToken: lowercaseSHA256Hex | null,
  unresolved: [{requestId, runtimeBindingId, phase:"unknown", historical:boolean}],
  dispositions: [{requestId, originalRequestIds:[canonicalUUID,...]}]
}
```

`CommandRuntimeTarget` is the existing `{sessionId,generation,controlRevision,turnId:null|string}`. PCW session IDs are opaque; request/runtime-binding/native IDs are distinct identities. `dispositions[].requestId` identifies the new committed disposition command, not the original resume. `unresolved` lists only the requesting principal's eligible-to-dispose Unknown rows; an empty list alone does not prove that the session has no blocker.

Eligibility is exactly `running|removedCrew|missingWorkdir|missingOriginalId|notConfigured|previousBirthLiveOrUnknown|evidenceUnavailable|noUnresolvedRequests|eligible`, evaluated in that order. `running` includes current/start-in-progress state at the serialized control seam. Previous births must be physically gone: missing PID or changed birth identity qualifies; unreadable or same live birth does not. Directory, original native identity and qualified program must still exist. Neither stopped metadata nor a closed socket proves physical exit.

The bounded query considers at most 257 candidate rows in request-ID order: unresolved `session.resume` phases `reserved|admitted|dispatched|unknown` for the session, plus the requesting principal's current, committed dispositions for the same original native ID. This is a **256 total candidate-row ceiling**, not 256 returned Unknowns or 256 per page; there is no recovery pagination. More than 256 candidates makes evidence unavailable once the earlier eligibility gates pass. Foreign-principal, non-Unknown, or restore-marked historical resume candidates also make evidence unavailable without disclosing their identities. Do not use partial arrays from such a response as a complete recovery set or bypass the limit by requesting subsets.

`historical:true` on a returned unresolved row can mean its runtime binding differs from the current Core after an ordinary restart. That is distinct from a database restore marking the stored receipt historical: restore-marked candidates are not disposition-eligible. A normal restart does not by itself invalidate a previously committed disposition. Restore marks committed disposition receipts historical and they cease to provide coverage; remaining old resume evidence therefore stays blocked (`evidenceUnavailable` when earlier eligibility gates pass).

Only `eligible` supplies an opaque 64-character lowercase SHA256 `evidenceToken`. It binds the current Core binding, complete target, native ID, work directory, original receipt evidence, physical births and dispositions. It is a freshness witness, not a bearer credential or a claim that the original resume failed. Inspecting, refreshing, reopening or reading receipts performs no disposition, process launch, input delivery, original-journal acknowledgement or deletion.

### Explicit command: `session.claude.recoveryResolve`

There is no direct standalone write RPC. Submit the new operation through the existing `command.submit` envelope and a **new disposition request UUID**, preserving that UUID for subsequent lookup:

```text
operation: "session.claude.recoveryResolve"
params: {target, originalAgentSessionId, evidenceToken, originalRequestIds}
```

`originalRequestIds` must reproduce **all** `recoveryInspect.unresolved[].requestId` values, in the returned order. Omission, subset, duplicate, changed evidence/native ID, stale target or wrong current runtime binding refuses. The actor checks current target and running/starting state, then the DB transaction rechecks authority and evidence before reserving and committing the disposition. An existing same-UUID/same-command/same-binding receipt is returned idempotently before freshness checks; this does not authorize a client to resend an uncertain operation.

Success is an ordinary public receipt with phase `committed` and exact result:

```text
{dispositionRecorded:true, sessionId, originalRequestIds,
 originalOutcome:"unknown", effectsDispatched:false}
```

It records that current evidence permits releasing this specific historical resume blocker. It **does not** mark the original operation failed/successful, edit its fields, acknowledge/delete its frontend journal, start/stop a provider, grant terminal control, deliver a message or replay input. The new disposition is stored in the existing public command receipt ledger; no second recovery ledger is introduced. Evidence refusal uses `CONFLICT` with `refusal:"claude.recovery_evidence_unavailable"`, before a new disposition or execution effect is committed. Other authorization/transport failures retain their ordinary meanings.

### Client and subsequent resume boundary

The UI obtains fresh inspection, asks for explicit disposition confirmation, and rechecks source and the complete inspection before submit. An uncertain disposition only exposes lookup of its original disposition UUID; no new UUID or automatic resend is permitted. A committed result is validated against session, original IDs, `originalOutcome:"unknown"` and `effectsDispatched:false` before acceptance.

Only fresh, validated server `dispositions` can cover an old `session.resume` blocker. The client requires `eligibility:"noUnresolvedRequests"` and an empty unresolved array, and rechecks selected authority/connection/source and the complete current target. It only excludes matching original public-command UUIDs for the exact session; an unidentifiable legacy journal entry remains blocking. Coverage never acknowledges or deletes that old journal. Server execution independently checks uncovered Unknown resumes, including legacy/wake-driven launch paths. The existing `session.resume` remains a **separate explicitly confirmed action**, with its own fresh target and strict original-ID/physical-birth/program checks. A successful disposition is not a resume authorization or real-provider restoration result.

## Original frozen history contract (historical context)

> **Historical frozen snapshot, not current progress (maintenance note, 2026-10-26).** The early branch/candidate, method-count/version and unfinished-backend statements below are retained as history; they must not trigger duplicate implementation or acceptance work. Use the [current whole-programme plan](../../superpowers/plans/2026-10-13-program-delivery-milestones.md), [current formal schema](../../../crewd/schema/crewd-api.schema.json), [API version source](../../../crewd/crates/proto/src/lib.rs) and [Store version source](../../../crewd/crates/store/src/schema.rs) for current scope (at this note: API3, 149 methods, Store21).
>
> The completed managed fresh-Human recovery module is recorded in its [whole-module QA](../../private/preserved-delivery-artifacts/claude-fresh-human-resume-20261026/INDEPENDENT-FINAL-MODULE-QA.json); compiled Core218 has [normal-component QA](../../private/preserved-delivery-artifacts/normal-core-218f6c15-independent-qa-20261026.json), and the e13 deployment package retains that original Core/proof with [exact artifact QA](../../private/preserved-delivery-artifacts/linux-persistent-host-e13ff2c9-20261026/INDEPENDENT-ACTUAL-PACKAGE-QA.json). These layers do not establish real-Claude complete lifecycle or complete native history acceptance. The original `fullHistoryLoad:false`/non-ACP and real-provider boundaries below remain in force.

This is the complete public consumer contract for this independent Claude A module, based on accepted source S6c5fdb30. It is a DTO/behavior freeze for parallel client implementation, not acceptance of the unfinished backend/fault qualification, real Claude execution, native complete history, ACP or a shipped binary. Current installed/candidate S does not contain these methods. API version remains3; generated checked methods become144 (the original141 methods/definitions are unchanged). Store remains20; no Native16 or SDK/transcript reader is activated.

## Identity and permissions

No new system.hello capability is added. Offer the existing Claude entry, explicitly probe inspect, and enable this contract only after a successful typed result; methodNotFound or an unavailable source is honest unavailability, not inferred support from a Store version/canStart.

Existing executor profile stays `id=claude`, `executor=claude`, `backend=existingClaudePty`; no second Claude profile or fake ACP capability. `features.fullHistoryLoad=false` remains unchanged. These methods read only PCW-owned managed tasks/observations across the original session's runtime births. They do not read private provider transcripts, initialize a provider, import history, start a runtime, grant a lease or retry a task. History is available without a running provider.

All three methods return their declared object directly in the normal RPC `result`, with no Known/result staging wrapper. They require the original authorized Local Human or a currently valid paired mTLS grant, original connection scope and current crew/source visibility. Attached Agents and untrusted/helper lineage are refused. Authorization is checked before dispatch, inside the original DB owner and before disclosure. Session must exist and have executor `claude`; cross-session request IDs cannot disclose text. Existing Human crew semantics remain: removal prevents execution; historical rows remain readable where the original crew/session reads permit them. No capability URL or cloud copy confers permission.

RuntimeId, requestId, native agentSessionId and promptId remain distinct. PCW sessionId is the existing opaque session identity; do not derive or replace it with a provider ID. All decimal counters/revisions/cuts in the new results are strings; byte limit declarations and request limit are JSON integers. Nullable fields are explicitly present as null; no missing-field default may imply success.

Canonical DTO source: `crewd/crates/proto/src/claude.rs`; formal schema: `crewd/schema/crewd-api.schema.json`. Exact generated result shapes/field spelling are authoritative. No new write RPC or new resume command is added.

## `session.claude.inspect`

Params: `{sessionId}`. Result `ClaudeInspect`:

- `sessionId`, `crewId`, `executorProfileId:"claude"`, `backend:"existingClaudePty"`.
- `historyScope:"pcwManagedTasks"`, `fullHistoryLoad:false`, `nativeAcp:false`.
- `manualResume:{eligibility,originalAgentSessionId,rechecksProgramAndSourceOnLaunch:true}`.
- `currentRuntimeId`: currently qualified managed birth ID or null; historical runtime metadata is not live authority.
- Limits: `taskTextLimitBytes:1048576`, `responseObservationLimitBytes:262144`, `retainedObservationsPerTask:4`.

`eligibility` is exactly `eligible|running|notConfigured|missingOriginalId|previousBirthLiveOrUnknown|removedCrew|missingWorkdir`. This is a current PCW admission observation, not proof the provider can restore its conversation. `originalAgentSessionId` is the canonical original recorded non-nil UUID, or null when absent/noncanonical. It is never generated or trimmed by inspection. `eligible` allows offering the existing explicit Human resume action; it does not promise native history completeness or silently waive the launch-time exact2.1.301 qualification/source guard. Client must not infer resume eligibility from canStart/profile.available or retain the old wording “refusal may create a new conversation.” A failed strict resume never falls back to a new ID.

## `session.claude.history`

Params: `{sessionId,cursor:null|opaqueString,limit:1..64}`; cursor defaults null, limit defaults32. Result `ClaudeHistoryPage`:

`{sessionId,tasks:[ClaudeTaskSummary],nextCursor:null|opaqueString,membershipCut:decimalString,observedAt:timestamp}`.

`ClaudeTaskSummary` is `{requestId,runtimeId,createdAt,updatedAt,phase,receiptRevision,promptId,observationCount,omittedObservations,latestObservedAt,waitingObserved,currentRuntime}`.

`phase` is the exact original receipt phase: `prepared|content_accepted|running|cancel_requested|raw_revoked|unknown|completed|aborted|failed`. In this managed route `completed` with null native turn means the genuine response reached the observed waiting seam; it is neither business Done nor proof of a full native turn/history. Stop alone stays provisional. `waitingObserved` requires the current managed runtime/birth, current original completed receipt and latest matched waiting observation. It can become false after later activity, raw control, stop, disconnect, reopen or restore. Never use this read field to authorize a task/tool or infer business success.

Order is newest receipt rowid first across original runtime births, not UUID text order. First page fixes the maximum original receipt rowid; subsequent pages contain only members under that cut. Cursor binds original DB/session/restore context and exact last row/request; invalid, foreign, over-bound or restored cursor refuses. Client may explicitly refresh from a null cursor after that refusal, without any execution. The cut only fixes membership/order: phase, counts and observations are current bounded facts at that read, not a frozen atomic historical state snapshot. `observedAt` makes this distinction explicit. New tasks do not enter the old pages.

## `session.claude.taskGet`

Params: `{sessionId,requestId}`; requestId is the original canonical managed request UUID. Result `ClaudeTaskDetail`:

`{sessionId,task:ClaudeTaskSummary,originalTask,observations:[ClaudeResponseObservation],latestObservation:null|ClaudeResponseObservation,waitingObservationNo:null|decimalString,observedAt}`.

`originalTask:{text:null|string,managedEnvelope:null|string,fidelity:"authorizedTask"|"exactManagedEnvelope"}`. Newly retained original task is returned exactly, including whitespace/delimiters, with envelope null. Historical/retention-limited task returns only the exact immutable original managed envelope with its fidelity label; client must not split markers to guess an original prompt. At most one of the two bodies is returned. No new model input is created by viewing/copying it.

`ClaudeResponseObservation` is `{observationNo,promptId,observedAt,text,sourceBytes,capturedBytes,sourceSha256,availability}`. `availability` is `observed|missing|truncated|retentionLimit|observationLimit`. `sourceBytes` and `sourceSha256` are null if the Stop field was absent. An explicitly supplied empty string is `text:""`, sourceBytes/capturedBytes `"0"`, distinct from missing. Full observed SHA256/byte count describe the actual received Stop string; captured text is a UTF8-boundary prefix, never a reconstructed or alleged final answer. It is not a provider transcript or an instruction to execute.

The retained `observations` array holds at most4 immutable bodies per request, each at most256KiB. The optional newly retained task/observation plaintext budgets are64MiB/session and256MiB/Core (the original immutable input ledger is neither deleted nor reinterpreted by this quota); reaching a limit reports retentionLimit without overwriting old text. `latestObservation` is a bounded head fact (its text is always null; locate a retained body by observationNo); later omitted observations still update head facts/count. `capturedBytes` describes the retained capture for that observation, not the head's null-text serialization. More than4 observations increment omittedObservations and report observationLimit. Previous bodies remain available after later activity retracts waiting eligibility. `waitingObservationNo` is an observed original waiting stamp; it does not independently grant current readiness—use task.waitingObserved for the current read, and the original command/lease for all writes.

## Existing `session.resume`: one strict Human action

Use the existing `session.controlTarget` and `command.submit` SessionResume envelope/CAS, preserving the original request UUID and `command.lookup` behavior. No new DTO/command shape. Human explicitly chooses resume; inspecting/history/reopen never triggers it. Uncertain submission only queries its original receipt; no auto retry, new UUID, replacement session or Unknown task re-delivery.

Strict managed resume requires the exact canonical recorded native UUID and a private Core-owned manual-resume intent. Missing, whitespace, invalid or mismatched identity refuses before a native process, runtime files or provider metadata probe. The driver bypasses legacy invalid-ID/fresh fallback; production managed refusal never respawns a new conversation. All known previous original births must be physically gone, not merely have a closed socket/revoked row; unreadable birth refuses. Fresh runtime registration mints a new seal only after the original custody/registration checks. The actual same native session must emit SessionStart source `resume`; new sessions require source `startup`. Wrong ID/source, refusal, timeout or disconnect does not become native-history success. Exact program/source qualification is still checked at launch, without changing global CLI config.

No held resume note, brief, old Unknown or original pending body is attached. At each explicit strict resume, the existing actor/admission order fixes the maximum pending wake ID and the global monotonic message sequence in one DB job, binding both cuts to the new physical generation. Every original automatic delivery entry (first scan, tick, Working) excludes <=cut; those rows, UUIDs, attempts and Unknown are preserved, never cancelled/delivered by the cut. The private, nonserialized PreparedSnapshot, original causal tuple recapture and renderer use the same message floor, so a new automatic task cannot smuggle a pre-resume message body through unread context. No read cursor is changed by resume or by this projection. Only a new task actually Delivered may advance its real end through the original normal ACK. Old messages remain readable as history; explicit Human quoting or newly written text is not censored. New wake IDs after the cut and a fresh Human nudge remain normal independent requests. Failed launch creates no new live cut and does not affect another session. Next explicit resume takes a new cut; reopening alone starts zero births. Native internal restore behavior is not represented as interception of every provider tool.

The public resume receipt records the original control's physical/dispatch outcome, not full native-history load or business success. Existing terminal/manual control and structured decision surfaces are reused; client must not add a duplicate automatic recovery route or auto-send copied history. PCW working/observed waiting remains separate from work completion and notification business events.

## Evidence and delivery boundary

This DTO checkpoint is consumable by the sole Swift author while backend Core/restore/fault tests continue. The original strict-ID and Stop-body RED logs remain preserved. No real provider/SDK/model, private transcript, public endpoint, OS service or deployed package is covered by a source/fixture pass. The complete backend freeze/independent quality and final combined candidate provenance remain separate gates.
