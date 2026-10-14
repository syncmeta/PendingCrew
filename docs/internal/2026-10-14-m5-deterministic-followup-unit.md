# M5 deterministic followup delivery unit

Status: the root approved schema13 and the existing originating-crew captain routing contract. This is the next complete backend unit; Core receipt/recovery freeze `d3601449` is preserved separately. No M5 product code, schema13 migration or model probe has been started.

The unit closes authoritative fact → durable observation/merge → scope/stop/budget admission → actual reminder delivery → restart/prune idempotency. It uses the existing organization WorkBook, globally issued object references, real session recovery facts, checkpoint delivery accounting, work causes and the existing wake actor. Jev, memory, new administrative sessions, automatic reassignment and Human-only decisions are outside the unit.

## Existing behavior to preserve and reuse

`collab::work_patrol` already reads bounded authoritative work facts and exposes the unavailable categories instead of inventing empty ones. Resume-failure timestamps and actually confirmed missed checkpoint counts establish owner loss. `collab::checkpoints::enqueue_due` merges by owner, validates current revision/deadline and marks misses only inside the real successful-delivery transaction. `wakes::automatic_work_paused` and `human_stop_floor` record actual Human stop intent. System and self wakes share the existing target LoopBreaker budget. None of this requires rewriting accepted M1 work or repeating its full gate.

The missing durable cross-prune observation is bookkeeping, not a second work ledger. Current queue dedup survives delivered/dropped rows only until prune. The new observation survives prune, restart and suppressed/unknown outcomes, so no unchanged fact can create a fresh reminder just because an old queue row disappeared.

## Proposed version13 contract

Version12 DDL stays unchanged. Version13 installs `work_followup_facts` atomically, with the same pre-upgrade backup/failure rollback/old-version refusal protections.

A fact row records only its issued Plan reference, originating crew, fixed factual category, canonical semantic cause, recipient snapshot/current nullable session FK, captured Human stop floor, bounded machine metadata, effect disposition and nullable existing wake ID. The semantic uniqueness includes the actual cause and intended recipient, but excludes elapsed display time, polling time, unrelated item revision/note and random retry UUIDs. The queue link may become null when a terminal wake is pruned; the observation remains, preserving dedup. No prompt transcript/body, forged role or model decision is stored in this table.

Possible categories are existing checkpoint due, verified owner loss, and actionable stalled blocking. A checkpoint identity uses the actual next-check deadline and owner; new confirmed delivery may advance that deadline/accounting, which is a real new fact. Owner-loss identity uses the actual failed-resume timestamp or real missed-check count together with the bound owner. Blocking uses currently open Human asks at an existing 24h/72h wait tier, or an actually lost dependency; a dependency merely being unfinished is not proof that a model reminder can help. The actual blocker set/occurrence is part of identity. A clock ticking, another polling round, the same status label or unrelated progress note is not a new cause.

Routing uses current ownership for ordinary checkpoint reminders and the existing originating-crew captain for actionable loss/blocking escalations. It does not create parent/subtree administrative jurisdiction or perform a forced reassignment. Missing/failed/stopped recipients and archived sources have an explicit suppressed observation. A new captain/owner is a new routing fact, but resuming/unarchiving alone does not reopen an old observed fact.

## Core transaction and delivery interfaces

The collector runs through the existing `crewd-db` transaction. It validates authority/issued IDs and captures eligible facts, merges at most50 facts for one target per batch, and atomically links them to one ordinary System wake. Duplicate keys confirm an old observation rather than spending a new wake. The existing checkpoint admission/accounting remains the authority for checkpoint side effects; its persistent fact dedup must join the same transaction rather than wrapping it in a separate actor.

There is no new public mutation RPC. `work.patrol` remains the explicit human view. `work::followup_note` adds a validated internal source branch that resolves fact IDs to their actual linked wake/session and reconstructs current facts from WorkBook/session/ask authority before preparing a message. Machine context exposes bounded work IDs and actionable reasons, not copied private item bodies or a new permission. Unknown/corrupt keys and missing authority fail closed and visibly; a forged source prefix is never enough.

Admission/render recheck current originating crew/captain/owner, item liveness, factual cause/blockers, archived state, captured stop floor and actual Human stop. Existing system target budgets and causal admission still apply, with no Human-priority bypass. Stop after queue creation invalidates that old fact's admission; no unchanged fact is automatically revived. Unknown delivery is resolved by the existing cause/wake/receipt lifecycle, never by generating a new semantic fact or replaying input.

## Complete acceptance boundary

Freeze one source after migration, authoritative collection, durable merge, existing admission/render and real fake-provider delivery are connected. Validate real normal12→13 migration/backups/rollback/old12 refusal; actual public-socket scope and typed denial; table-level transaction faults; multi-work same-target merge; owner/captain changes; Human stop and later resume; stale/resolved blockers; prune/restart/same-fact no re-wake; fresh genuine cause becoming eligible; budget exhaustion; actual delivery, delayed/unknown result and no manufactured second input. Use temporary Core/fake-provider data only. The independent quality line verifies this as a full unit, and asks authors to repair concrete defects directly.

The root has approved the version13 bookkeeping shape and existing-crew routing contract before implementation. Public schema/build files remain owned by the backend line. Only source/fake readiness is promised; real provider SDK/ACP/history/approval/Stop behavior stays behind its separate M4 evidence boundary.

## Recovery limit and approved baseline

A restored/imported snapshot may roll bookkeeping back to before an unknown or delivered effect. PK dedup alone cannot provide exactly-once across that unobservable rollback. Establish a conservative observed/suppressed baseline for historical facts after restore/import, reusing existing recovery and Human stop admission. Existing failed-resume/missed-check causes must not launch another automatic reminder merely because bookkeeping disappeared. Only a genuinely new authoritative cause or explicitly authorized Human recovery may progress. Preserve original wake attribution separately when pruning clears its live FK. Existing Held budget remains pending until Human recovery; elapsed time never resets it.
