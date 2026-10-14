# Native plan operations: one complete authority contract

This is the joint client/backend contract for the actual visible-plan workflow; a field-level fixture or one successful submission is not acceptance of this module. The original `fd19551d` startup gate mismatch was found through the real normal helper hello. `5c8d7794` fixes the orchestra gate and basic owner gate, and its visible window reaches actual fake-member start and message GOT. Its unassigned open report with a default checkpoint still fails, so that package remains incomplete for progress. Original sources/packages/logs/data are retained.

## Result-state matrix

| Result state | Owner | Result checkpoint | Rules |
| --- | --- | --- | --- |
| open | absent | absent | Nonblank note is legal; do not silently assign an owner. |
| open | present | absent or present | A supplied checkpoint must satisfy authoritative time policy. Open does not prohibit checkpoints when owned. |
| active | present | present | A supplied checkpoint may replace the existing one; omitting it can retain an actual existing checkpoint. Dependencies must be done. |
| active | absent | any | Formal refusal, no durable side effects. |
| active | present | absent | Formal refusal, no durable side effects. |
| blocked | present | absent | Nonempty valid blockers and continuation decision; block clears the old checkpoint. |
| blocked | absent | any | Formal refusal before ask/split allocation, no durable side effects. |
| done/dropped | optional | absent | The authority clears checkpoints/blockers/transfers and validates actual permissions and completion/drop rules. |

Assign/reassign/transfer use the authority's owner history and current role/lost facts. Assigning an active plan sets its authoritative checkpoint to now to notify the new owner. Client code does not imitate this transition or automatically assign anyone. Frozen/terminal scope, organization roles, dependencies, check policy and CAS remain server decisions; a loaded member list is not an authorization proof.

## Shared client behavior

- A new unassigned form defaults to open without a checkpoint. Assigned open plans may set one. Any active/block/new-check command requires the currently read owner. An existing invalid draft can explicitly turn off its old checkpoint; its original unknown snapshot remains intact.
- Active note-only reports may retain a checkpoint from the latest actual detail. A plan with no existing checkpoint must supply one. The client does not invent a check policy limit or copy a ledger.
- Before submission, read fresh detail and compare its revision with the user-reviewed revision. Validate owner/check against that actual detail, then submit the original typed payload. Changed revision requires another explicit review; role changes are adjudicated by the server. Failures preserve the draft; only an accepted matching version clears it. No automatic write replay.
- Real hello's `orchestra` group covers session.start. The shared start gate requires connection and this actual group, never a guessed per-method advertisement. Real normal-hello positive and absent-group/bogus-method/offline negatives are retained in `/private/tmp/pcw-session-start-capability.Eo6Ehp`.

The affected typed-plan fixture covers owner/check combinations, active retention, exact revisions and old/new/other-scope draft ownership. `/private/tmp/pcw-M6-full-plan-contract-build.log` and `pcw-M6-full-plan-contract-result.log` pass. This is not actual socket/role or visible-window proof. The backend author is freezing the complete public-socket result-state/owner/check/CAS/role matrix with all-table zero-effects on refused commands. QA then rechecks the same complete visible workflow. Only after that full contract freeze is there a new actual normal witness and one new affected candidate build; no partial source is relabeled over `5c`/old helpers.

## Parallel Linux installation unit

`install-crewd-linux-candidate.py` prepares explicit absolute prefixes with immutable retained releases/full proof, durable UUID/payload records, writer and controlled-runtime locks, compare-and-swap on the reviewed activation, atomic unique activation and read-only unknown-result confirmation. It does not touch a database or manage a service. Process identity ambiguity fails closed; one scan is only a bounded observation. Rollback switches binary pointers while preserving the current DB; old binaries must refuse newer schemas until the normal Core backup/restore mechanism is explicitly used.

The complete `smoke-crewd-linux-install-upgrade.py` prepares actual isolated install11 → running-runtime refusal → CAS refusal → real prepared-pointer obstruction → dropped receipt/readback → same-request/no-replay → payload conflict → actual normal11→12 → binary rollback without DB writes → old11 refusal → formal Core restore11/restart → restore12/restart. It requires two different real Linux binaries/source refs and schemas 11/12, preserves backups/replaced DBs and all original request identities. Only syntax has been checked so far; no system or temporary install experiment has run yet. The accepted original `7b1849d9` tar remains the old input, never a fabricated second version. Actual execution waits for the backend's complete schema12 delivery input and independently frozen Linux package.
