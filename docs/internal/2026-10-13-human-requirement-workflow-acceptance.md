# Human original requirement workflow — isolated candidate acceptance

Baseline: `d47a9e74` (organization schema 9 unit). This extends the candidate, not main or daily data. No real model, app installation or real-data migration was run for this unit.

## Public contract

- `todo.create {requestId, crewId, text}` returns the existing Todo fields plus `closure`. Ledger is agent; `requirementId` is the immutable public organization `#` reference. The raw `id` and local `number` retain their source-fact meaning.
- UUID spellings normalize to one request identity. Same UUID/crew/original text returns the current derived view without allocating or notifying again. A changed payload or an existing source without the human-request marker conflicts. Extra stores only the UUID marker, not a second copy of the original text.
- `todo.get {id: "#N"}` reads a single original source and organization-derived status. `todo_get` is the corresponding all-agent read-only MCP tool.
- `todo.close {id: "#N", expectedStatus, note}` is Human-only, checks the current derived state, and uses the pure core closure rule. Live associated plans still refuse closure. Its real note/time are returned as `closure`; it does not rewrite the original text or bulk-drop plans.
- `work.update` / `work_item_update` now accepts `dropped`, a business `NotNeeded` decision with a nonblank note and existing revision/owner/scoped-permission checks. It does not supply factual evidence or silently release live dependencies. All-dropped requirements become `needs_ruling` until an explicit human closure.

Human-only means both collab's trusted sender boundary and rejection of direct create/close RPC on a successfully agent-attached connection. Unattached local client connections retain the existing trusted-Human model; this is not an OS-level separation claim. Read RPC/MCP remain organization-wide, including frozen objects.

## Durable notification and cost

The original source, pure `EntryRegistered` event, organization state, public identity, `WorkChanged` and actual current-captain wake commit together. No captain leaves an unclaimed source without inventing an owner. A queue/event/storage failure rolls the transaction back.

The notification is `requirement-new:#N`, System kind with Human priority. Rendering and reservation claim both verify that the source remains unclaimed/unclosed, its crew is active, its current captain still matches, and the human-stop watermark has not changed. The normal executor Spawned transition from Stopped to Running remains valid; a subsequent real human stop cancels the older hint. Stale hints never reach model send/claim. The rendered note contains only the public requirement reference and crew; the original text is fetched explicitly with `todo_get`, avoiding repeated full-body injection.

## Observed evidence

| Layer | Result | Original evidence |
| --- | --- | --- |
| Server production compile | PASS | `/private/tmp/pcw-requirement-check-02.log` |
| Own real collab transaction/guard regression | 5 PASS | `/private/tmp/pcw-requirement-collab-02.log` |
| Independent actual Unix RPC, authenticated MCP and actor regression | 5 PASS | `/private/tmp/pcw-requirement-workflow-04.log`, test commit `ee59ac9c` |
| MCP strict catalog / both locales | 12 PASS | `/private/tmp/pcw-requirement-catalog-01.log` |
| Intentional MCP surface generation / fingerprint | 2 PASS | `/private/tmp/pcw-requirement-surface-01.log` |
| API schema and normal daemon binary generation | PASS | `/private/tmp/pcw-requirement-schema-generate-01.log` |

The independent public lifecycle covers creation, UUID retry/conflict, exact original text through get/MCP, cross-crew plan sources and completion-derived state, public dropped→needs_ruling→human close, attached direct-write refusal without durable or hub events, frozen writes, queue-trigger rollback, actual fake-executor delivery/receipt, and changes between real render and claim producing dropped/attempts-zero without send.

Own regressions additionally cover UUID uppercase normalization, unknown legacy-source marker rejection, changed blank retry payload conflict, latest closed view on retry, no captain, event-SQL rollback, stale status, live-source closure refusal, and render guard changes for claimed/closed/frozen/captain/human-stop plus a permitted Spawned transition.

Earlier evidence remains: check-01 caught an adapter signature compile error; workflow-02 actor cases failed because the test fixture attempted a second captain in an occupied crew. The independent author corrected only the synthetic crew fixture, retaining the failure log. These are not claimed as product-behavior red baselines.

Whole-workspace/lint and real daemon+Swift integration are the next integration gates, owned by root. Passing this unit does not complete the entire PendingCrew engineering project.
