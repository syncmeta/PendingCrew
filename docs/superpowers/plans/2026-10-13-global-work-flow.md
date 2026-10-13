# 全组织工作跟进实施计划

批准范围：在隔离树完成跨机组拆分、全局 parent/dependency/source、人/项解阻与可靠唤醒；不是仅开放API参数。基线 eaa300e4。单元未完成前只能报在途，不push/main、运行模型、安装应用或迁移真实数据。

## 输入与不变条件

- spec §4–6 / §24 全组织一本权威工作账，原始需求/计划/待人类处理是三种对象；最多3层只约束parent深度。
- 沿当前父→子、项→依赖/卡点图方向：parent依赖自己的child仍合法，child依赖ancestor构成环；不得把所有边统一方向。
- 沿当前owner完成证据可选规则，不借迁移加新门禁。普通机长权限按对象crew作用域；组织一本账不等于全员机长。
- 外部W/H/#、旧aliases、真实事件、已有pending/inflight/settled wakes与stop预算保留。旧状态超3层明确报诊断，拒绝切换，不能截断。

## 依赖顺序与接口

1. 纯core：组织对象crew归属、作用域Actor权限、组织唯一ID、全图/parent≤3不变量及纯命令。旧单crew状态恢复兼容只用于迁移输入；新组织状态显式启用3层限制。独立target只运行work crate。
2. 身份与迁移：组织账唯一持久行；object_refs添加core identity映射作为身份索引，无工作状态。显式旧(crew,type,local)→组织core ID映射，现有external ID与source alias不改。迁移同步改全部引用和持久wake目标，旧books只保留不可写证据。
3. 权威事务：with_org_book load→CAS/权限→纯命令/全图校验→唯一保存/真实事件/身份→受影响owner合并durable wake。任一步失败整个Tx回滚。保留完整targets与render/claim时新owner/终态/冻结重检。
4. 对外API/MCP和context/checkpoint/idle/patrol/ask全部接唯一权威账，旧局部句柄从identity映射读回。读回旧alias不等于继续写旧book。
5. Store迁移/事务故障、实际socket+MCP、actual actor+fake executor四层验收；完整单元才接受。

## 迁移版本与公共文件

usage正在占schema8，本单元组织迁移拟schema9；本树基线暂不修改CURRENT_SCHEMA_VERSION直到usage接口落定、主代理安排串行移植。不得直接在schema7后抢占schema8。先实现纯core和离线迁移函数/fixture，生产schema接线跟随最终usage版本。

共享接线冲突：store/schema.rs/lib.rs/object_refs.rs、proto/lib.rs、server/dispatch.rs、orchestra/{core,port,db}.rs、生成schema。不同树隔离修改，由主代理串行整合与全门禁。保留usage session/thread事实关联；不把usage实现变成本用例前置。

## 迁移诊断与证据

每个旧BookState先按原有规则restore，再用explicit身份映射合并，检查对象归属、权限、全图、计数器、parent深度。缺对象/身份、旧超深或损坏状态均返回可定位crew+legacy handle，失败不写组织状态。不能根据不存在的历史补Created事件。保留旧事件原seq；组织新事件序列独立加来源映射，避免不同crew seq1撞。

计时用max各旧clock；retain revision/ownerHistory/missedChecks和globalstop预算。迁移幂等与库备份/restore通过合成数据证明；真实数据授权另计。

## 验收索引（实现时填入唯一日志）

- pure：跨crew有向环拒绝、父依赖子合法、4层parent拒绝但长dependency链合法；普通机长不跨crew、全局机长可管；部分解阻/最后依赖/全局source推导。
- store：两crew同W1/H1/#1保留外部读回；migration幂等/损坏fail/超深诊断；queue故障回滚源done、受影响项、事件、ID。
- socket/MCP：A派B工作，B负责人与A负责人各按权限读写，B完成使A解除并保留其他卡点。
- actor：busy pending→idle投递、崩溃恢复、重连不重放、render后reassign/terminal不投旧正文，runner确认才Delivered。

当前状态：仅启动实施，未达四层验收；完整范围与问题定位见 /private/tmp/pcw-global-work-flow-design-20261013.md。

### Pure core checkpoint (2026-10-13)

- Single organization state now carries exact object-to-crew attribution and three-level parent restriction. Trusted `ScopedSession` resolves captain/admin roles against the object crew; unscoped legacy session roles fall back to worker in organization mode.
- Cross-crew parent/dependency/source and blocker operations use the same full graph. Splits/questions inherit the source object's crew even when the owner's home changes. Parent-waits-on-child remains legal; child-waits-on-ancestor and fourth parent level reject without mutation.
- `merge_legacy_books` is a pure, deterministic conversion with explicit legacy W/H/# mapping. It preserves issued identity holes, original revisions/history/transfers/blockers/checks and clock; stop budgets merge by maximum rather than summing. It fabricates no events and retains original event sequence provenance separately. Persistence must consume this result in its one transaction; this is not a second authoritative ledger.
- Pure gate: 147 tests passed (130 unit + 2 fuzz + 12 organization + 3 scenario), `/private/tmp/pcw-global-work-core-final-01.log`; all-target work clippy `/private/tmp/pcw-global-work-core-clippy-02.log`. Earlier compile/clippy failure logs remain preserved.
- Still in flight: store schema 9 migration/identity lookup/wake rewrite, unique organization transaction, public RPC/MCP/context/checkpoint/idle/patrol/ask switch, and socket/actor end-to-end evidence. No production migration, model run, app installation, main merge or push.

Core self-review follow-up: organization work creation cannot default to the synthetic organization identifier; standalone system asks have explicit `open_ask_in` attribution and reject crew mismatch with their associated work; repeated entry registration cannot silently change its crew. Final pure core gate is 149 tests (14 organization scenarios), `/private/tmp/pcw-global-work-core-final-03.log`, plus all-target clippy `/private/tmp/pcw-global-work-core-clippy-04.log`.

Store schema-9 module is drafted in `store/work_organization.rs`, but not connected to version chain or production operations and not yet compiled. It stages the sole organization state, identity mapping, immutable legacy archive, genuine organization event stream and transactional save/rollback tests. Existing durable wake/stop manifests already contain stable public IDs, which must remain unchanged; only reads into the new core authority translate through the identity registry. Integration and four-layer acceptance remain open.

### Store migration checkpoint (2026-10-13)

The schema-9 `work_organization` module is now compiled and tested but remains deliberately unactivated in the production schema chain. Existing usage schema-8 code is still separate. Six organization store tests cover collision-safe published identity/alias retention, immutable original states, no fabricated events, idempotent retry, missing authority refusal, injected mid-migration schema+identity rollback, new-object state/identity/event single-transaction persistence, dropped transaction rollback, and pre-migration SQLite snapshot restoration.

Store full gate: 139 tests passed, 2 environment/cost tests ignored, `/private/tmp/pcw-global-work-store-final-01.log`; warnings-denied all-target clippy `/private/tmp/pcw-global-work-store-clippy-02.log`. Used the shared warm target only after usage worker explicitly released it; no cold workspace build. Earlier clippy failure evidence retained. No current build process remains.

The next required integration is atomic cutover: resolve every external W/H/# and legacy alias through `object_refs.core_id`; construct trusted scoped actor from actual crew hierarchy; replace every per-crew mutable work/ask/context/checkpoint/idle/patrol operation with the sole authority transaction; preserve external wake/receipt identity, check render/claim against translated nodes, and prove queue failure rollback. Only after those paths are complete may schema 9 be activated and socket/actor acceptance begin. The module and its tests do not yet prove this full product use case.

### Production cutover checkpoint — independent socket evidence (2026-10-13)

This checkpoint supersedes the earlier statement that usage schema 8 remains separate: usage f990bf71 is now integrated into this worktree. Production schema remains version 8; organization migration is explicitly initialized only in synthetic fixtures. No daily data has been migrated.

- Store migration/raw-source gate: 9 passing tests, `/private/tmp/pcw-global-work-raw-source-store-01.log`. Previously registered raw Agent requirements reuse their exact public identity; unregistered requirements receive one organization entry; raw text and local numbers remain source facts, while status derives from organization plans. Failed registration rolls back the raw source too.
- Collaboration transaction gate: 6 passing tests, `/private/tmp/pcw-global-work-transaction-02.log`. Actual owner completion preserves the other blocker and refreshes the affected crew; queue refusal rolls back source, target, events and revision. Purge allows a crew's internal subgraph but refuses foreign references/current owners; SQL failure restores the complete transaction.
- Independently authored Unix-socket/MCP gate: commit `64788d1e`, `/private/tmp/pcw-global-workflow-02.log`. Attached identities create cross-crew parent/source/dependency relationships. B owner reports and finishes B; two A items retain their shared H until the last answer, then become active with checkpoints and one owner-grouped durable wake. Unrelated captain mutation is refused; affected events carry the actual object crew. Initial fixture checkpoint-time failure is preserved in the 01 log and was corrected using current time plus a valid interval.

The socket test seeds initial blockers directly in the canonical organization fixture. It proves public creation relationships, owner completion and human-answer handling, but does not yet prove user creation of those blockers or actual runner receipt. The public atomic `work.block` / MCP `work_item_block` entry is approved for implementation using the existing pure BlockOn/Continuation rules, revision CAS, scoped identity and one transaction.

Remaining gates before activation and delivery:

1. Finish and test the public blocker/new-human/split action, including atomic rejection and real announcement/wake paths.
2. Resolve independent production review findings: frozen-crew history must stay readable; public H identity must not silently fall back to a different local H. Verify raw requirement allocation high-water behavior after purge.
3. Prove actual actor delivery with fake execution, including render/claim refusal after owner/revision/terminal changes. A queue row is not a delivery receipt.
4. Switch schema 9 through the serialized migration/backup chain; run version-8 upgrade, reopen/idempotence, failure rollback and restoration fixtures. Run the final combined lint/workspace gate on a single source worktree.
5. Integrate with original-style client and packaged runtime. Main integration, release, installation and real-account/day-data acceptance remain separate delivery gates.

### Actual actor and staged version-marker checkpoint (2026-10-13)

Independent actor tests commit `2efde806`, `/private/tmp/pcw-global-work-actor-01.log`, pass both scenarios. Last H answer leaves one owner-grouped message containing public H and both W identities; fake execution receives it, then an explicit success receipt transitions Inflight to durable Delivered. Completing both organization items after actual render but before claim drops the obsolete wake with attempts=0, no send and no extra executor launch. This is actual actor/fake-executor evidence, not a real model/account task.

Root-owned staged schema wrapper/tests pass three cases, `/private/tmp/pcw-global-schema-staged-03.log`: conversion and marker 9 commit together; injected identity UPDATE refusal rolls back the schema/identities and marker to 8 and permits a clean retry; a pre-conversion SQLite snapshot retains true historical v8 state and can upgrade again. Fixture creates V1→V8 through the historical migrations rather than falsifying a new database's marker. Retry proof creates a real new organization item/event while legacy archive remains unchanged and verifies the new state/event survives another conversion call. This is staged transaction evidence: production SCHEMA_VERSION remains 8, and public Store::open/reopen/backup-refusal through the activated v9 chain still needs final validation.

The next remaining product gate is public block/new-human/split creation plus review regression checks, followed by schema-chain activation and combined workspace validation. Earlier initial-blocker fixture boundaries remain explicit.
