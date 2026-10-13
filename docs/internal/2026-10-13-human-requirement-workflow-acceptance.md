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


## 客户端整合与读竞态验收

- 数据接口13b34a21、UI接线c59b23d2、专用独验c62db744；后续根修补保证需求读取双向提交水位：旧详情不能覆盖新列表，旧列表不能覆盖新详情。整笔刷新在提交前拒绝已被详情提交取代的快照，不用自动重试写入来恢复。
- 独审首次RED（7缺口）：/private/tmp/pcw-requirement-store-review-red-01.log；逆向RED（3缺口）：/private/tmp/pcw-requirement-store-review-reverse-red-01.log；最终独立GREEN（15检查）：/private/tmp/pcw-requirement-store-review-reverse-green-independent-01.log。包括closed理由不可丢、假close回执拒绝、create精确原文字节、trim理由和只读后续刷新。
- 最终原布局/实际UI源码SwiftUI编译与纯结构：/private/tmp/pcw-requirement-client-final-01.log，全部通过。UI按crew保留草稿UUID；未知提交保留旧snapshot并提供显式确认原提交入口，不自动重试、不在编辑时无声遗失不确定任务。当前未知传输错误保守提示，不把发送前未执行与可能已执行精准区分。
- 最终正常daemon + Swift：/private/tmp/pcw-requirement-swift-backend-final-01.log，数据 /private/tmp/pcw-work-integration.Y2gJ7b。实际公开create→同UUID复用→get原文→冲突拒绝→Human关闭理由（刷新保留），然后既有跨层计划阻塞/H答复/完成/CAS/只读重连，全部通过；无真实模型/账户或fake runner流量。01/02及早期编译日志全部保留。
- 整套Rust workspace门禁仍在进行，已发现旧agent工具数和每方法RPC覆盖夹具需要补新todo_get及三个公开方法；当前不得称最终workspace全绿。待定向修补/复验与strict clippy/fmt后再冻结交付检查点。

边界仍为隔离源码、真实本地通信与合成数据。未执行原生窗口/像素验收、正式安装、日常数据迁移、main整合或正式发布；文档/独立视图文件夹、完整交接、多厂商真实PTY等仍在工程路线内。
