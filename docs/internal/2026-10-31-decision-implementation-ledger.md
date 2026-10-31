# 决策能力实施台账（2026-10-31）

基线：66d92bb0fa889787fb6e2c7c86a83419e2381af8。本表是当前源码与裁决的只读审计，不是新功能验收。并行作者后续变更须另记实际结果。本次未调用模型、读取秘密或更改日用配置。

## 裁决优先级

- 原始业务目标见 `docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md` §5–8；其中 §24 等追记优先于已划掉的旧稿。
- 2026-10-15 `docs/internal/2026-10-15-jev-first-production-unit.md` 明确批准三业务及 TypeSafe 单一 provider，取代此前笼统“Jev 待讨论”。路由收件人不等于指定 Work owner，归属不改回复或 Ask。
- 2026-10-31 用户要求所有尚未落地能力逐项扎实实现；并行建设不省略权限、持久化与恢复。
- 原 §8.1 明确行政管辖（顶层机组或全组织）留 M6 重定。本次搜索 internal 与 specs 未发现后续明确扩权裁决。组织账本存在不等于 admin 获得全组织权限；`collab/src/work.rs` 按实际对象机组授角色。

## 当前逐项事实

| 能力 | 有效范围与现状 | 完整交付仍欠什么 |
|---|---|---|
| 需求接收路由 | 已有生产 batch → provider → 权威重验 → 持久结果/唤醒；仅既有合法成员。server/lib.rs 调用 judgments::Runtime::start | 三业务作者继续核失败、并发与恢复；实际配置和真实 provider 使用验收独立记账 |
| 路由后新成员、新机组 | 原 §7 候选含 new_session/new_crew；10-15 首轮刻意限既有成员。创建工具存在，但判断 batch 不含这些选项 | 后续业务合同：扩编依据、并发上限、创建幂等、机长命名/章程职责、创建失败处置；不得把工具存在算自动扩编 |
| 消息唤醒 | batch 对合法收件人成员构造 Predicate，原生概率阈值选集合；可见性仍由群成员关系决定 | 核空集合、部分答案、明确 @ 与自动判断边界、Stop/配置改变的晚到结果、真实交付消费 |
| 消息归属 | batch 对已有对象构造 Choice，独立 typed link；不重写 inReplyTo/mentions/Ask | 当前对象候选完整性与权限、关闭/修订后的失效处理、用户查阅与纠正体验；不得说每条消息已有丰富语义分类 |
| 成员退场 | 原 §8.3 有效目标；当前 judgments batch 无此题型业务 | 可退场事实（活项/在途交付/恢复）、判断、可撤销退出效果、重启恢复、人工覆盖；不得用空闲直接替代可退场 |
| 真进展/原地踏步、卡死 | 原 §8.3 有效；检查点和停下检查已接生产，但只是确定性事实 | 有界报告上下文、两种判断结果、证据记录、低置信处置、协调执行与旧判断失效；不能把定时唤醒说成理解进展 |
| 失联重派 | 当前 followups 持久识别 owner_lost 并提醒机长；work_item_reassign_lost 工具已存在，权限核实失联 | 尚无 Jev 选新负责人并执行的生产链；需排除旧负责人、重验恢复失败/工作修订、保证唯一转移和告知 |
| 停下无改善升级 | stop_checks::accept 将预算升级消息持久写入管理机组人类群，category=work_stop_escalation，@Human | 这是已存在的交接渠道。没有找到该类别专门自动行政消费；不能说“没有升级”，也不能说“行政已接手”；后续复用原记录，不造平行账本 |
| 过期/重复/完成识别与关闭 | 原 §8.3 档案目标；现有工作账本/链接与状态工具是基础；当前 batch 无此判断 | 疑似识别、行政核实证据、权限内关闭、重复/取代链接、撤销/恢复。模型判断本身不能将任务关闭 |
| 闲置群归档 | 原 §17 补充有效；纯 work::Patrol 能投影候选，但 collab 公共巡检明确标 archivable_channels 不可用 | 当前群消息/活项/成员证据、候选分页、归档前重验、合法执行与解档；不可仅凭工作全部终结直接归档 |
| 相近需求/人的待办整理与简报 | 原 §8.3 秘书目标；等待时长、提醒与默认方案已有生产确定性链 | 语义合并候选、保留原始需求与人工问答身份、不擅自回答/丢弃、简报生成与去重；当前 batch 无此判断 |
| 行政巡检触发 | work::watch::patrol_due 提供 10 条/30 分钟纯函数；全源码检索仅定义及本模块测试，生产未调用 | 事件计数与持久游标、定时/手动入口、并发合并、重启与漏事件恢复、预算与暂停 |
| 行政 fan-out 与临时会话 | work::Patrol::needs_admin_session 仅纯函数及测试；当前 Runtime 只有 route/wake/attribute | 管辖合同、完整事实输入、判断持久记录、仅有事启动、唯一会话/任务、结果回收与退出、失败恢复 |
| 侧栏紧急度排序 | §24/§7 明确取消；旧 jev decisions 的 Urgency 代码/旧实测不构成待实现需求 | 无待办；不恢复该功能，不将通用 Score 接口误记为排序业务 |

## 已落地的底座与明确边界

`server/src/orchestra/core.rs` tick 实际调用 `apply_due_defaults` 与 `enqueue_work_checkpoints`；db 实现调用 collab 的 defaults、checkpoints、work_followups。followups 文件明确只提供确定性事实、没有模型判断或重派，持久观察与唤醒是有效基础。其完成不代表上述所有行政业务完成。

`collab/src/work_patrol.rs` 是显式、有界、只读投影，输出无主项、失联、停止升级、长等待；返回 unavailable_categories 为 unclaimed_entries、needs_ruling、archivable_channels。`work/src/watch.rs` 的全量纯算法不能替代这些生产投影。补齐投影可与三业务链并行，不能擅自扩展调用者权限。

三业务有 settings get/set/inspect、Mac 设置页及持久恢复机制；这说明实现已经进入生产调用路径，不等于日用已经启用。本台账未重新查询日用 DB，不将此前配置缺失的观察伪装成本轮实测。

## 推荐实施顺序及每项完成定义

1. 现有三业务闭合；同时补齐授权范围内的巡检事实和用户检查入口。
2. 确立行政作用域、一次巡检的持久身份与触发/失效/恢复合同，接通定时和手动调度；复用现有停止升级与工作账本。
3. 接进展/卡住、失联协调与成员退场，逐项落实观察→判断→合法执行→结果确认。先完成一个模块的全部故障分支再扩下一项。
4. 接归档/重复识别与秘书整理；保留原始对象、证据与可纠正路径。
5. 再完成扩编路由以及各客户端真实使用验收。

每项验收分别记录：代码调用链、针对性正常/异常/竞争/重启证据、真实供应商结果、安装版本和用户可见行为。没有启用/真实验收的项仍注明欠项。不得将一次演示、测试数量或发布版本替代能力交付。

## 本批巡检事实实现（取代上表巡检投影缺口状态）

本批新增 `work.patrol` 的 `scan=entries/channels`，默认 `work` 保持原 W 分页；同一 `after/nextAfter` 按扫描类型代表独立源游标。源页先限额，返回没有发现的源对象，不能用工作页尾代替入口或群扫描完成。群游标允许源对象已删除；分页不承诺快照。

入口页使用经验证组织账本的真实 EntryView，补齐待认领与待裁定事实。群页覆盖历史工作关联与当前 channel_work_links，不依赖存在活跃 W；只有关联工作且全部终结才成为待进一步核实候选。活项、源机组移除、非工作关联、无关联、单群超过 200 个关联分别用闭集 blockedBy 说明，不把未完整扫描当无事。候选不授权自动归档，后续仍需核消息活动与当时权限/状态。

同页只恢复组织权威一次，历史群索引只收本页群、源机组移除状态缓存复用。固有组织账本恢复及 EntryView 成本仍存在，不声称恒定时间。缺失/损坏权威返回错误。

实际验收：`cargo test --manifest-path crewd/Cargo.toml -p crewd-collab --lib work_patrol::tests -- --nocapture` 11 项通过（chunk d40733）；`cargo test --manifest-path crewd/Cargo.toml -p crewd-server --test work_patrol -- --nocapture` 1 项真实隔离 socket 测试通过（chunk f8e941），含新扫描 RPC 与同数据库关闭重开只读检查。默认 target 为 crewd/target。原新增 scan 红回归因 unknown field 失败（0747d6）；首个受限 socket 运行连接超时（f49fab）保留，允许本机隔离 socket 后成功，不外推真实模型/日用验收。格式化使用已安装 Rust 1.94.0；公共 schema 与组合检查交根整合。

因此本批状态为三种巡检扫描源码及上述回归已接通；定时触发、客户端手动巡检、行政会话判断和执行仍未完成。新增事实接口不是完整行政交付。

## 本批判断输入与客户端接线

路由/唤醒新增成员职责和来源范围内工作样本，内部快照纳入职责及工作修订；工作变动后晚到结果拒绝。模型不接收内部工作ID、版本、目标或证据。为保留原64KiB请求上限，新增成员上下文总预算16KiB，模型投影最多一条工作摘要并显式标记截断；这是样本，不是完整负载。最大中文30成员及关联对象压力回归先红后绿。最终server判断11项通过；collab判断8项通过，包含来源群隔离、截断、版本失效。

Mac原生日用消息菜单及两处需求详情已接只读判断记录，支持无记录、失败、等待、历史和失效状态；成员名称复用现有资料。连接/对象变化作废旧结果。根执行整合后的WorkStore验证通过，日志 `/private/tmp/pcw-decision-work-store-integrated-20261031.log`；完整Mac Debug编译通过，日志 `/private/tmp/pcw-decisions-swift-build-20261031.log`。该构建使用已有安装Core仅满足嵌入检查，不能视为本批后台与App同源安装包，也没有安装或GUI实测。

本批未启用日用Jev、未取用凭据、未调用真实供应商、未发布。后续仍须行政触发/执行建设及对应真实使用验收。

根整合：公共API schema已由当前源码重新生成；受影响三crate的lib/bins严格clippy通过（Rust 1.94.0）。all-targets未通过：既有集成测试需testkit feature，默认目标缺testkit/serve_with_test_hooks等；保留失败日志，不称全目标检查通过。当前改动尚未提交、安装或发布。

续轮核验：当前源码生成与已保存公共schema一致，实际 `crewd --test e2e committed_schema_matches_generated` 通过，日志 `/private/tmp/pcw-decisions-schema-test.log`。后续新增运行调度与手动界面由独立作者继续，旧批状态不外推。

手动巡检续批：原生日用进展页已新增当前机组三类事实扫描入口。整合WorkStore runner新增patrol fixture，实际通过三扫描、跨机组拒绝、旧连接失效及7类畸形页拒绝；日志 `/private/tmp/pcw-patrol-client-final.log`。Mac普通Debug完整编译通过 `/private/tmp/pcw-patrol-ui-build.log`；仍使用已有Core输入仅作编译验证，无本批同源安装或GUI验收。后台持久巡检在实施中，未以此标完成。

后台状态界面整合：新增后台巡检状态只读展示，实际WorkStore整合测试通过 `/private/tmp/pcw-patrol-status-client-final.log`，Mac完整编译通过 `/private/tmp/pcw-patrol-status-ui-build.log`。公共schema重新生成并包含work.patrolStatus。后台错误元数据损坏导致轮转回滚的独审发现仍在修复，未将运行模块标为完成。

巡检运行本批收口：10条非系统消息/30分钟触发、每拍一机组一页、持久游标、运行中新消息留后轮、恢复epoch失效、50条发现明细上限及独立incomplete_channels均已接线。坏run/error记录保留并显示错误，不阻塞其它机组；错误退避绑定恢复epoch。五项collab、真实数据库关闭重开/socket以及无执行器准入Core tick回归通过。独审发现的坏error游标回滚已红绿修复并复审。生产lib/bins严格clippy通过 `/private/tmp/pcw-patrol-final-clippy.log`。仍未进行日用安装/真实GUI验收；尚未实现行政模型判断与协调执行，扫描完成只代表本轮源页结束。

## 下一完整模块：进展判断与协调请求（实施合同）

按当前有效用户授权继续实现。事实读取 organization_work_events 的 typed Reported 事件，并核当前 issued W、crew、core identity、报告作者与当前owner；work.history只含owner_history，明确不用于进展比较。比较最近两条真实同owner报告，缺少依据时记录insufficient_reports，不宣称卡死。当前工作事实仍来自唯一WorkBook。

复用现有判断模型配置、凭据绑定、恢复epoch、撤销和绝对期限。meaningful_progress与needs_coordination独立判断；仅合格原生结果达到阈值时生成唯一协调请求。过程记录、请求与唤醒入队同事务；负责人、工作revision/终态、captain、Stop/config/restore版本在模型前后及实际投递前重验。使用已有System wake及经过校验的来源，不新建重复Todo、不扩行政跨机组权限。未知派发重启后不重发。

只读状态接口与原计划详情接线，分别显示判断、协调请求、投递状态；通知不等于协调完成，实际换人/拆项仍用原Actor/CAS工具及工作证据。后台作者负责生产链，客户端等proto合同确定后接两处实际详情入口，独审核事实、权限、竞争、恢复和消费。此节为实施合同，未标代码或验收完成。

## Coordination integration evidence

Implemented: typed two-report evidence -> existing provider -> durable decision -> unique captain wake -> production Core/DbPort delivery -> typed ACK. Durable report cursor scans at most 64 source events per page; first/restore baseline avoids historical dispatch. Lost hub events recover, Prepared retries, Dispatched restart becomes Unknown without replay. Capacity pressure retains Prepared. Work revision, Stop, restore and configuration are rechecked. Client plan details expose decision and delivery separately; delivery does not complete work.

Final checks: server judgments 17 passed (`/private/tmp/pcw-coordination-final-runtime2.log`), collab coordination 5 passed (`/private/tmp/pcw-coordination-final-collab.log`), strict production lib/bins clippy passed (`/private/tmp/pcw-coordination-final-clippy3.log`). Initial sandbox run failed on local socket/process-birth permissions; retained failure log, then authorized isolated rerun passed. Independent socket, production actor StandIn ACK and missed-hub recovery tests passed. API schema regenerated.

Swift integrated fixtures passed and full Mac Debug build succeeded (`/private/tmp/pcw-coordination-swift-integrated.log`, `/private/tmp/pcw-coordination-ui-build.log`). Final review corrected display mapping to protocol pending/inflight. Build embeds existing installed Core only for compilation; no same-source installed package, real provider, GUI acceptance or publication claimed. Lost-owner coordination/reassignment, administrative sessions, semantic archive and secretary organization remain outstanding.

## Lost-owner reassignment: implementation contract

Authoritative scope: original backend design sections 8.3 / 420 / 424 require verified loss -> Jev chooses a replacement excluding the old owner -> actual reassignment -> captain notification. A suggestion alone does not satisfy this requirement. Runtime System is allowed by the pure work permission table, but generic collab Sender::System stays forbidden. A dedicated internal entry must reuse WorkBook::reassign_lost without impersonating a captain or expanding public RPC permissions.

Use an independent durable reassignment record because progress observations and ownership transfers have different triggers and completion facts. Snapshot current work revision/owner, genuine loss, eligible same-crew candidates, config/restore/Stop versions. Bound candidate summaries; never interpret empty work samples as idle. Persist Prepared before dispatch; keep capacity-deferred work pending; recover Dispatched as Unknown without replay. Scan at most 64 source work references per tick with a durable cursor so time-based loss needs no hub event.

Settlement must revalidate all authority, then atomically change the original work owner, preserve owner history, enqueue replacement and captain notifications, and record the actual result. Any failure rolls back. Invalid, unknown, unconfigured, low-confidence or no-candidate judgments leave ownership unchanged and retain deterministic follow-up. Replacement/captain delivery must revalidate current facts and require actual typed acknowledgement. Do not equate model selection, committed ownership transfer, notification delivery, or work completion.

This contract is in implementation, not accepted or installed. Backend, runtime, client, and independent delivery/recovery tests have separate writers; root owns final integration.

### Lost-owner reassignment: integrated source acceptance

The above contract is now connected in production source. The original W is transferred by the dedicated internal System entry, with owner history and two durable notifications in one transaction. No public Sender::System privilege was opened. Prepared retries, Dispatched becomes Unknown after restart; a 64-source scan catches time-driven Silent loss and genuine resume failure. Empty/low-confidence/unknown results leave ownership unchanged. Post-transfer witnesses validate the resulting revision, owner, Stop/config/restore state rather than comparing against the pre-transfer owner.

Captain-as-lost-owner is supported: a healthy worker can take over; the failed captain's notification remains Pending. DbPort filters that notification from delivery and Claim rechecks the failure in the transaction. It neither retries failed recovery nor clears its evidence. Human Stop still drops it. After explicit recovery it is visible, renderable and claimable; that special case has Port/Claim evidence, not real external recovery/ACK evidence.

Verified final evidence: 24 server judgment regressions passed (`/private/tmp/pcw-reassignment-all-judgments2.log`), including seven independent reassignment runtime cases. Normal two-notification delivery uses production Core/DbPort plus StandIn and typed ACK, preserving the original W and not marking Done. The fixture needed a real Working(true)->Working(false) edge for the second Auto-priority notice; initial timeouts and fixture-schema failures are retained, not presented as production defects. One actual socket status test passed (`/private/tmp/pcw-reassignment-socket-review.log`). Two model-selection unit tests and 27 wake-lifecycle regressions passed. Backend tests cover transactional enqueue failure, stale replies, silent-loss checkpoint receipts, source pagination/cursor rollback, Stop, restore, and failed-captain handling.

Strict production lib/bins clippy passed (`/private/tmp/pcw-reassignment-final-clippy.log`); regenerated API schema matches source (`/private/tmp/pcw-reassignment-schema-test.log`). Client integrated fixtures passed (`/private/tmp/pcw-reassignment-client2.log`) and full Mac Debug compilation succeeded (`/private/tmp/pcw-reassignment-ui-build2.log`). First restricted build failed on cache permissions and was rerun in the authorized environment. Existing installed Core is embedded for compiler verification only; this is not a same-source installed build. No real provider invocation, real executor recovery-failure acceptance, GUI acceptance, installation or publication is claimed.

## Administrative implementation audit after reassignment

Administrative scope remains explicitly undecided in the original spec (section 8.1, line 162) and program plan line 21. A user clarification is pending: top-level subtree administration versus organization-wide administration. Do not silently expand existing same-crew Admin authority to either scope. Independent member-retirement work can proceed meanwhile.

Current source gaps verified: stop_checks emits management Human messages but no administrative consumer; patrol completion means three factual scans ended. The public session start path compresses role into a captain boolean, so Admin becomes Worker at the DbPort creation boundary. Admin worldview/catalog largely follow Worker; channel archive authorizes owner/captain/Human, not administrative scope. Generic work.update maps dropped to business NotNeeded, and done with a note attempts a report first; these cannot stand in for evidence-based administrative duplicate/superseded closure. A full module therefore needs scoped role creation, scoped read/action capabilities, durable fan-out judgments, only-needed temporary session admission, validated factual actions, explicit results and retirement. Notification delivery alone proves none of those business effects.

Member retirement audit: existing Arbiter only chooses wake recipients. No can_exit/canExit production judgment was found. Core halt preserves desired state unlike Human stop, but lacks an individual durable retired marker and actual-exit receipt; using it alone would make boot interpret retirement as interrupted Running work. Implementation must bind provider answers to actor generation and current persistent facts, recheck active work and all pending inputs, preserve Human authority and resume identity, and confirm exit from the real actor event.

## Individual session retirement: implementation contract

Independent of the pending administrative-scope decision, implement the original personnel retirement judgment. This retires only the current idle process while retaining the same resumable session, work history, desired state and Human Stop semantics. It must not delete the session, mark work complete or treat an idle event as model approval.

The actor supplies an internal witness containing startup incarnation, process generation and activity version. Persistent preparation verifies no outstanding owned work, asks, approvals or wakes; current message frontier, resume identity, Stop/config/restore state are fingerprinted. The configured native provider answers can_retire under the existing absolute deadline. Model approval creates DecisionReady only. Actor retirement rechecks its live witness and pending writes, delivery, decisions and controls; the final database transaction rechecks persistent facts and records ExitRequested, followed without another await by input fencing and process stop. Later input is retained and follows existing admission/resume rules rather than being consumed by retirement.

Only a real matching process-exit receipt confirms Parked/adopted. Stream loss, unknown stop outcome and restart during ExitRequested remain Unknown; Dispatched is not resent. DecisionReady from an older actor incarnation is stale. Confirmed idle retirement must not be mistaken for an interrupted Running session on daemon restart, while new input must invalidate the parked marker and preserve legitimate resume behavior. Each scan considers at most 64 source sessions fairly. This section records the implementation contract, not completed runtime acceptance.

### Individual retirement: integrated source acceptance

The provider/runtime, durable decision, exact actor witness, process stop, actual exit receipt, boot projection and client inspection are connected. Bounded recent readable messages provide semantic evidence with explicit sample/truncation markers; unrelated unreadable messages do not affect the snapshot or reach the provider. Outstanding transfers, current work, open asks, pending wakes, native decisions, public controls, process activity and missing resumable identity prevent retirement. A post-commit input remains pending and uses the existing admission path. Human Stop records and desired state are preserved.

A confirmed Parked process is excluded only from the boot interrupted-session list; Unknown/ExitRequested retain ordinary recovery handling. Before another physical launch, valid retirement state must be durably invalidated; write failure prevents launch. Corrupt optional metadata is preserved and shown as an error, but does not permanently prohibit an otherwise-authorized Human resume. Exit receipt write failures retain the exact receipt for retry rather than inventing completion. No new administrative scope was granted.

Final evidence: 34 combined server judgment tests passed (`/private/tmp/pcw-retirement-all-judgments.log`), including ten independent retirement runtime tests; 28 existing boot/recovery tests passed (`/private/tmp/pcw-retirement-boot-regression.log`). Seven collab retirement tests passed (`/private/tmp/pendingcrew-retirement-collab.log`), plus the native-probability selection test and actual socket test (`/private/tmp/pcw-retirement-socket-review2.log`). The independent file-DB close/reopen test confirms no unsolicited launch for Parked and same-session recovery after a legal wake. The StandIn fixture explicitly persists its synthetic agent ID through the existing DAO to represent the real adapter's separate persistence path. It also acknowledges actual nudge deliveries before awaiting the command reply; the earlier fixture hang was diagnosed and its exact test process stopped, not patched around in production.

Strict production lib/bins clippy passed (`/private/tmp/pcw-retirement-final-clippy.log`); public schema matches generated source (`/private/tmp/pcw-retirement-schema-test.log`). Client decoder fixtures passed and the new view is in both original session-control surfaces; full Mac Debug compilation passed (`/private/tmp/pcw-retirement-ui-build.log`). As before, embedded installed Core only supports compiler verification. Real provider/PTY retirement, installed same-source behavior and GUI acceptance remain unverified; no installation or publication occurred. Administrative scope still awaits the user's requested choice, and administrative execution remains a separate unfinished module.

Final capacity review corrected a starvation defect: scanning 64 source sessions with only 32 shared slots could repeatedly omit the latter half. Runtime now reserves actual permits before asking the actor for a source page of that size, then pairs every returned candidate with a reserved permit. Zero capacity does not advance the cursor. The production pagination function regression covers 65 sources in 32+32+1 pages, wraparound and zero-capacity behavior (`/private/tmp/pendingcrew-retirement-paging.log`); it is not a claim of running 65 real executors. Strict clippy passed after this correction (`/private/tmp/pcw-retirement-capacity-clippy.log`).
