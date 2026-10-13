# Verified lost-owner reassignment — isolated candidate acceptance

`work.reassignLost` uses the existing WorkAssignParams and returns full WorkPlan; `work_item_reassign_lost` returns compact WorkSummary without report text. It reads the current authoritative owner and actual persisted recovery-failure record inside the owning transaction. Alternatively the existing pure silence rule requires real delivered checkpoint counts and the next check due. No caller-controlled lost flag is accepted. Human/scoped authorized captains use existing pure permissions; attached ordinary RPC cannot borrow Human identity. Healthy, unknown or merely stopped owners do not authorize takeover.

Owner change, actual lost history, stable object identity, WorkChanged and eligible new-owner notification commit atomically. SQL/queue failures roll them back. The guarded hint is work-lost:<public W>:<revision>, with bounded assignedWork metadata; render and claim recheck current owner/revision/nonterminal state, source/target crew freeze and Human stop intent.

## Human stopping and subsequent work scheduling

A real Human-stop meta record plus desiredStopped pauses automatic work, including a zero watermark recorded before the first wake. Reassignment preserves owner/history but does not launch a previously stopped recipient. Due checkpoint enqueue/render/claim and automatic work completion/assignment/transfer hints share this qualification. Existing stop checks already require desiredRunning. Silence during an explicit Human stop is not newly classified as lost; genuine recorded recovery failure remains a distinct fact. Actual Human resume restores eligibility without dropping work or history.

## Evidence

- Independent five actual socket/actor/transaction-clock cases plus six transfer regressions: PASS, `/private/tmp/pcw-lost-workflow-04.log`, test commit `7693dda5`.
- Actual old-owner resume attempts launch and fails, production DbPort persists resume_failed_at, public Human reassignment succeeds, new owner's real fake-runner Delivery settles Delivered. Attached plain RPC refusal, worker denial, SQL rollback, render/claim completion race and full owner-history readback are included.
- Prior-human-stop real RED preserved `/private/tmp/pcw-lost-prior-stop-red-01.log`: initial implementation queued a work wake despite the prior explicit stop. GREEN verifies an active report/checkpoint before loss, prior target h.stop, actual recovery failure, public reassignment, twenty actor ticks with no automatic relaunch, then actual Human resume permits the second launch.
- Silent evidence is deliberately layered: two genuine production owning-Tx enqueue/claim/ack Delivered receipts, duplicate ack Stale with unchanged count, actual public wallclock RPC refusal before grace, then the same durable facts through existing production Tx.run with dueTs permit reassignment. This final timing boundary is transaction clock control, not a fifteen-minute real-wallclock socket/runner trial.
- Own transaction/zero-watermark regressions 3 PASS; affected checkpoint 8 and stop-check 9 PASS (`pcw-lost-{collab-04,checkpoints-01,stopchecks-01}.log`). SQL seeded recovery flags in own tests are explicitly transaction fixtures, never claimed as runtime evidence.
- Generated schema/tool catalog and actual CLI all-method coverage 9 PASS. Hello advertises work.reassignLost independently of orchestra, allowing older clients to gate the new capability.
- Full workspace **2525 passed, 0 failed, 8 ignored** (`/private/tmp/pcw-lost-workspace-01.log`), strict workspace all-target clippy and fmt PASS (`pcw-lost-{clippy,fmt}-01.log`).

Earlier fixture/command failures remain: collab01 used the wrong synthetic wake table; schema01 used a nonexistent CLI option, corrected using the actual schema subcommand with an atomic output replacement. Neither is described as a product red. No real account/model/app, daily-data migration, main merge, installation or release was run. Documents and view-folder contracts are frozen for parallel client work; their backend implementation remains next.

## Client and daemon combination accepted

Independent QA commit `82188069` uses frozen client `c7651580` and a private copy of the backend `716712fe` binary. `/private/tmp/pcw-lost-daemon-qa-02.log` reports PASS for healthy-owner rejection, actual fake-runner thread-resume handshake failure, persisted `session.get.resumeFailedAt`, typed `reassignLost`, exact owner/Lost history, and read-only reconnect without command replay. The fixture waits for actual process termination after Human stop before attempting resume. The first fixture failure (`AlreadyRunning` caused by assuming asynchronous stop was already settled) remains retained and is not represented as a product failure.

This closes the isolated typed-client/daemon M1 combination; visible-window interaction, real models, final packaged helper, installation and daily-data migration remain M3 gates. M2 documents/folders are not claimed by these results.
