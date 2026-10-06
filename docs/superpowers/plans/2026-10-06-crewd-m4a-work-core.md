# crewd M4a 工作跟进核心（sans-IO）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新建 crate `crewd-work`：把 spec §4–§6 的两层 Todo（入口 #N + 工作项 W<n> 的 DAG）、负责人与转交、检查点、卡住只卡那一件（人类待确认 H<n> + 拆分 + 默认方案）、停下即检查、等人升级、§8 行政巡检的代码判定，做成一个纯计算的领域核心：每条命令在草稿上改、整份过不变量才提交并产出事件；推导视图（入口状态、就绪、我的工作项、到期检查点、失联候选、人的待确认清单、巡检）全是只读函数。M4b 只需要把它落库、接 MCP 工具与唤醒。

**Architecture:** 一本账 = 一个机组（`WorkBook`，内含可序列化的 `BookState`）。三层：**数据**（`ids` 编号与时间、`model` 字段表、`events` 变更事件、`perm` 权限表）；**规则**（`graph` 找环、`invariants` 整份校验）；**命令与视图**（`book` 事务外壳 + 认领 / 建项 / 关联群，`block` 卡住与人回复，`progress` 回报 / 完成 / 放弃 / 改链接 / 入口裁定，`owner` 转交 / 指派 / 失联重派 / 检查点记账，`views` 推导视图，`watch` 停下即检查 / 等人升级 / 巡检）。事务 = 克隆整份状态 → 改 → 自动维护 revision / updated_at / missedChecks → `invariants::check` → 换上并按序号发事件；被拒 = 什么都不动。持久化边界是「整份 `BookState`（serde）+ 事件流」，不定义仓储 trait（裁定 3）。

**Tech Stack:** Rust 1.94（edition 2024，workspace 已有的 `rust-toolchain.toml`）、serde 1（derive）；dev：serde_json 1。两者都已在 `Cargo.lock` 里（proto / store 在用），不新增任何第三方 crate。不用 tokio、chrono、rand、proptest。

**Spec:** `docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`（§2 现状、§4 两层 Todo、§5 负责人与跟进引擎、§6 卡住只卡那一件、§8 行政处（只取代码判定）、§9 MCP 工具语义、§10 迁移、§12 测试、§13 Q2/Q4/Q5/Q9、§17 机组内多群）。

> 写计划时已把本计划的全部代码，在一个只含本 crate 的副本 workspace（同一份 `crewd/Cargo.toml`、`rust-toolchain.toml`、`Cargo.lock`）里按任务顺序逐个重放：每个任务结束时 `cargo fmt --check`、`cargo clippy --all-targets -D warnings`、`cargo test` 全绿。crewd-work 单测数依次 6 / 18 / 38 / 53 / 64 / 74 / 81 / 89 / 100（Task 1–9），Task 10 后单测 100 + 集成 `fuzz` 2 + `scenario` 3。三个「验这把尺子会红」的变异步骤（Task 4、Task 7、Task 10）都在副本上真红过，红哪几条写在步骤里。**没做过的事**：没在真实 `crewd/` workspace 里跑过 `./scripts/check.sh`（其它 crate 的状态以执行时为准）；没在 Linux 上编译（本 crate 无平台相关代码）。执行时数字对不上就停下查，别改期望凑数。

## Global Constraints

- 所有命令在 `crewd/` 目录下跑；Rust 工具链在 `/opt/homebrew/opt/rustup/bin`（找不到 `cargo` 先 `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`）。
- 依赖一律 `cargo add --offline`（只用 `Cargo.lock` 里已有的版本），**不要手写版本号**；整个 M4a 不往 `Cargo.lock` 加第三方 crate，只多出 `crewd-work` 自己那一条。`Cargo.lock` 要提交。
- `crewd-work` **不依赖** proto / store / server / exec / term，也不被它们依赖（M4b 才接）。不用 tokio，不做 IO：不读写文件、不开数据库、不调 `SystemTime::now()` / `Instant::now()`——时间一律是调用方传入的 `Ts`（Unix 毫秒），同 M2a 裁定 3，不引入 `Clock` trait。
- **不设计 SQLite schema**（M2b 正在加 schema v2；落库在 M4b）。本 crate 只给出可序列化的 `BookState` 与事件。
- 不变量在**每一次**修改后整份校验（spec §4.2「crewd 写入时强制，违反即拒」）；被拒的命令不改任何状态、不消耗任何号码、不产出事件。
- 编号：工作项 `W<n>`、人类待确认 `H<n>`、入口 `#N`，三个命名空间分开（spec §4.2 [推]；记忆：两本账号码会撞）；号码只增不复用。字符串形式是唯一规范写法（`W3`，不是 `W03`）。
- 给 LLM 看的文字不写进代码（记忆：prompt 不内联、Langfuse 是唯一事实源）：本 crate 只产出结构化数据（视图、事件），「你名下的工作项」怎么写成一段话归 M4b 的模板。
- 测试只用合成数据，**不读真实账本**（spec §12：仓库公开）。性质测试用测试文件里自带的确定性生成器（xorshift64\*，固定种子），不引 proptest / rand。
- 本机 shell 是 zsh：手动跑变异步骤时照抄给出的命令，别把参数塞进变量再展开。变异还原用 `cp` 备份，**不要用 `git checkout -- <file>`**（会丢掉全部未提交实现，且看起来像「这刀没红」）。
- 每个任务结束跑 `./scripts/check.sh`（fmt + clippy -D warnings + test，全 workspace）全绿再提交。提交只 `git add` 该任务列出的文件（路径相对 `crewd/`；Task 10 的 spec 路径相对仓库根写成 `../docs/...`），提交信息末尾加 `Co-Authored-By: Claude <noreply@anthropic.com>`（不带模型名）。
- 在分支 `crewd-m4a` 上做（执行时用 worktree）。本计划不依赖 M2b–M3 的任何代码，可以与它们并行；合并时只会在 `Cargo.lock`、`README.md`、spec 末尾追加段落三处相遇。

## 范围

做：`crewd/crates/work`——编号与时间（`W<n>` / `H<n>` / `#N` 解析与 serde、`Ts`）；字段表（spec §4.2 全部字段，外加 `channel`（§17）、`lastReport`（§4.3 的「下一步」）、`dropReason`）；权限表（负责人 / 机长 / 行政处 / 人 / 调度层）；DAG 找环（父项、依赖、卡点三种边的并图）与整份不变量；命令：登记入口、认领入口（批量建项，批内可互相引用）、建项（含 `request_help` 的无主项）、关联群、回报进度 + 下次检查、卡住（新问 / 已有问 / 等另一项，必答「其余能否先推进」，可拆分）、人回复、默认方案到点、带证据完成、放弃（事实性 vs 业务决定）、改链接、入口裁定关闭、转交请求 / 接受 / 撤回、指派、失联重派、检查点叫醒记账、升级通知记账；视图：入口状态（由第二层推导）与「→ W3 W4（1/2）」数据、就绪项、待认领、负载、我的工作项、待接受的转交、到期检查点（按负责人合并）、失联候选、停下即检查（含防死循环）、人的待确认清单与 24h / 72h 升级、默认方案到点、巡检汇总与触发判定；事件日志；spec §1 验收判据的纯逻辑场景测试；随机命令序列的性质测试；spec 交付记录。

不做（归 M4b 及以后）：SQLite 表与读写、重启后的到期唤醒持久队列（M4b 用 `due_checkpoints` + 落库的 `BookState` 实现「重启不丢」）；MCP 工具本体与参数校验文案（§9，接线对照见下）；唤醒投递、准入限流、负责人忙时排队（M3 / M4b）；「我的工作项」注入文字与 hooks（§4.3，M4b）；旧 plan.json / Todo 的迁移导入（§10，M4b——本 crate 只提供它要调的命令，见对照表）；Jev 决策（M5）；行政 session 的启动与 Jev fan-out（M6）；跨机组 DAG 边（§13 Q5：v1 不做，一本账 = 一个机组在结构上就排除了它）；`Rejected` 的人类可读文案（M4b 的 MCP 层映射）。

## 写计划时定下的裁定（执行时照做，有异议先回报）

1. **crate 叫 `crates/work`**（包 `crewd-work`，库 `crewd_work`）。只依赖 serde；与 store / proto 互不依赖，M4b 在 server 里把两边接起来（同 M2b 裁定 11「账本是 trait、执行器不依赖存储」的方向）。
2. **一本账 = 一个机组**。`W<n>` / `H<n>` 按机组编号，和 `#N`（M1 的 todos 表里按机组的账内编号）同一口径；`crewId` 是账本级字段，不在每个工作项上重复存（M4b 落库时每行带上）。v1 依赖只限同机组（spec §4.2 [推]、§13 Q5）由此**在结构上**成立，不需要额外检查。
3. **持久化边界 = 整份 `BookState`（serde）+ 事件流，不定义 `WorkRepo` trait**。理由：schema 还没定，先定 trait 会让存储形状反过来约束领域；一个机组的账本是几百项量级，整份读写够用；事件里带着每次改了哪些 id，M4b 要增量落库可以照事件做。`WorkBook::restore` 读回时整份跑不变量，不合法就拒绝、不做修补。
4. **图的边方向**：父 → 子（父项要等子项）、项 → 它依赖的项、项 → 卡住它的项（`blockedOn` 里的工作项也进图）。spec 说「parent 与 dependsOn 的并图无环」，没说方向；按这个方向，**父项可以依赖自己的子项**（「子项做完父项才能动」很自然），**子项不能依赖自己的祖先**（父等子、子又等父 = 死锁）；两项互相卡住同样被拒。
5. **补的不变量**（spec §4.2 之外，都是「不这样就会出现永远推进不了 / 账对不上的状态」）：active 或 blocked ⇒ 有负责人；nextCheckAt 只许出现在有负责人的 open / active 项上（open 项可以排「什么时候开工」）；blocked ⇔ blockedOn 非空；blockedOn 不指向已结束的问或已终结的项（清卡点时同步删掉）；活着的项不挂在已终结的父项下（所以 **done 要求子项全部终结**）、不依赖已 dropped 的项、不来自已裁定关闭的入口；待接受的转交只挂在活着、有主、且转给别人的项上；标题非空；引用不重复、不悬空；号码计数器不落后于已发出的号。
6. **入口状态只看直接关联的项**（`sources` 含 #N）；卡住时拆出的子项**不继承**入口——否则「→ W3 W4（1/2）」会被拆分噪音撑大；「父项 done 要求子项终结」让完成自然传递。没有活项时：至少一项 done → 完成（混着 dropped 也算）；全部 dropped → 交机长 / 人裁定（spec §4.1：不自动关）。
7. **权限表**（`perm::allowed`，一张表一个函数）：负责人改自己项的一切；机长 = 业务权力（改状态、强制指派、业务放弃、裁定入口），不替人接收转交、不回答人类待确认；**人 = 机长之上**，且只有人能回答 H<n>；行政处只能事实性关闭（带证据完成、重复 / 被取代的放弃、入口事实关闭）、改链接、关联群、失联重派——不能强制指派、不能以「不值得做」关闭（spec §8.2）；调度层（`Actor::System`）只做派活、失联重派、默认方案到点、检查点 / 升级记账。worker 只能把**无主**项领给自己，不能替别人指定负责人。机长 / 行政处是 session 的角色，由 crewd 按机组认定后传进来。
8. **清卡点**：人回复 H<n> / 默认方案到点 / 卡它的项完成 → 从所有项的 blockedOn 里删掉它，每个受影响项产出一条 `BlockerCleared { owner, .. }`——**不论这一项是否还卡在别处**，负责人都要被直接叫醒（spec §5「直接唤醒 blockedOn 指向它的所有工作项的负责人」）。卡点清空时：依赖已齐 → active 且 nextCheckAt = now（下一轮检查点立刻叫醒）；依赖没齐 → open（spec 说「回到 active」，但不变量不许依赖未齐的项 active）。负责人自己用 report 把 blocked 改回 open / active = 不等了：清空 blockedOn，问题本身保留（人仍可回答）；**人的待确认清单只列此刻仍卡着活项的问**，没人等的问不打扰人。
9. **missedChecks 记账**：crewd 每投递一次检查点唤醒就调 `record_checkpoint_wake`：该项 +1，排了检查时间的项顺延到 now + `recheck_after_ms`（可配，不超过检查上限；默认值执行时按人拍板改为 15 分钟，见文末 Q3——下面 Task 2 / 7 / 8 / 10 代码里的 `30 * MINUTE` 复查间隔在仓库里已相应改成 15）。**负责人对自己名下项的任何一次成功操作都算回报**，在事务外壳里统一清零（别人替它回报不算）。missed ≥ K（默认 2）的项不再进到期检查，顺延时间也过了就进失联候选；会话不可恢复的 session（crewd 先试 `--resume`，失败才算，spec §13 Q4）名下所有活项（**含 blocked**）都是失联候选。
10. **时间**：`Ts` 是 Unix 毫秒，调用方传入。账本记住见过的最晚时间 `clock`，命令一律用 `max(传入的 now, clock)`：墙钟往回跳（NTP 校时、睡眠唤醒）时，若按传入时间算，「检查时间 ≤ now + 2h」会对已有的项突然不成立，整本账的每条命令都会被拒——这是账本被卡死，不是数据坏了。
11. **停下即检查的三种「没着落」**：被检查点叫醒过却没回报（Unanswered）、open / active 项检查时间已过（Overdue）、名下 open 项依赖已齐却没排检查（ReadyUnscheduled）。spec 写的「active 项缺 nextCheckAt」在裁定 5 下不可能出现，Overdue 是它的真实对应。blocked 项永不触发（spec §6）。防死循环：违反集合与上次**完全相同** = 没改善；连续 3 次没改善 → `Escalate`（交行政处），之后 `Suppressed` 保持安静，集合一变就重新计数。走 idle 边沿重新唤醒，不是 Stop hook 阻塞（spec §2）。连续计数是软状态：存在 `BookState` 里，但不产出事件；丢了最多多叫 3 次。
12. **视图只出结构化数据**，不拼给 agent 看的句子（见 Global Constraints）。
13. **事务 = 克隆整份状态**：O(项数)/命令。几百项的账本在 debug 下性质测试跑 19200 条命令约 1.2s；真到瓶颈再换增量（M4b 有数据再说，不预先优化）。revision / updatedAt 由事务外壳比较前后自动维护，命令代码不手动碰。
14. **检查时间的下限**：新建项的 nextCheckAt 可以等于 now（派活占位 / 迁移占位「现在就查」，spec §10）；回报里的 nextCheckAt 必须**严格晚于** now（「下次检查」写成现在 = 没写）。上限由不变量统一管（≤ now + 2h）。
15. **巡检只判「要不要起行政 session」**：只有「停下即检查交过来的 session」和「关联项全部终结的群」需要行政 session；失联重派、待认领、入口派活由调度层直接做，入口裁定归机长 / 人（spec §8.4「只有标出需要动手的事才启动行政 session」）。触发：从没巡过 / 攒够 10 条消息 / 距上次 ≥ 30 分钟。
16. **H<n> 由工作账本按机组发**（卡住时新开的问）。若 M3 的人类待确认账本已有自己的 H 号，M4b 必须把两边合成一个命名空间——见文末「给人的待决问题」Q1。
17. **必填文字**：回报 note、交接说明（转交 / 指派 / 失联重派）、放弃理由、入口裁定说明、问题、答案、标题。只有空白算空。

## Review Focus

1. 墙钟往回跳（NTP 校时、合盖睡眠后唤醒）→ 账本照常接受命令，不会因为「已有项的检查时间超过 now + 上限」而整本卡死。（Task 4 `clock_going_backwards_does_not_wedge_the_book`）
2. 批量命令在第 N 条才出错（认领入口时第三项引用错了、拆分里一项标题为空、检查点记账里有一项无主）→ 整批不生效：账本、W / H 号、事件日志原样不动，不会出现跳号或半截拆分。（Task 4 `forward_batch_reference_is_rejected_and_consumes_no_number`、Task 5 `block_input_is_validated_without_consuming_numbers`、Task 7 `checkpoint_wake_rejects_items_that_are_not_checkpointed`；Task 10 性质测试逐条核「被拒 = 原样不动」）
3. 一个 session 再也恢复不了，名下有卡在人身上的项 → 那一项也要换负责人，否则人回复了没人被叫醒。（Task 8 `unrecoverable_session_loses_every_live_item_including_blocked`）
4. 负责人绕过了问题（不等了）或项被放弃 → 人的待确认清单不再挂着没人等的问，也不再给它升级提醒。（Task 9 `asks_nobody_waits_on_any_more_leave_the_human_list`）
5. 机长或别人替负责人回报 → 不清掉负责人的未回报计数，否则失联永远判不出来。（Task 7 `checkpoint_wakes_count_until_the_owner_reports`）

## 文件结构

```
crewd/crates/work/
  Cargo.toml              cargo new + cargo add 生成                                          (T1)
  src/lib.rs              模块声明（每个任务给出整份）；Task 10 加公开 re-export                  (T1–T10)
  src/ids.rs              Ts、MINUTE/HOUR、WorkId/AskId/EntryNo（前缀解析 + serde 字符串）、SessionId/CrewId/ChannelId (T1)
  src/model.rs            字段表：Status、Body、Evidence、Blocker、Actor/Role、OwnerChange、WorkItem、EntryTodo、
                          HumanAsk、WaitTier、StopViolation/StopStreak、BookState、Config、NewItem/ItemRef      (T2)
  src/events.rs           Event、EventKind                                                     (T2)
  src/perm.rs             Action、Ctx、allowed（权限表）                                         (T2)
  src/graph.rs            adjacency、find_cycle（迭代 DFS）+ 随机图性质测试                        (T3)
  src/invariants.rs       Violation、check（整份校验）                                          (T3)
  src/book.rs             Rejected、Applied、WorkBook（new/restore/读取/事务）、Draft、register_entry、
                          claim_entry、create_item、link_channel                               (T4)
  src/testkit.rs          单测夹具（cfg(test)）                                                (T4，T6 加 activate)
  src/block.rs            BlockOn、Continuation、block、answer_ask、apply_default、Draft::release  (T5)
  src/progress.rs         Progress、Report、report、mark_done、drop_item、relink、close_entry       (T6)
  src/owner.rs            request/accept/withdraw_transfer、assign、reassign_lost、record_checkpoint_wake (T7)
  src/views.rs            EntryStatus/EntryView、ready/unowned/load/my_items/incoming、due_checkpoints、owner_lost (T8)
  src/watch.rs            StopDecision、stop_violations、on_idle、HumanWait、escalations、defaults_due、Patrol (T9)
  tests/scenario.rs       spec §1 验收判据的纯逻辑版                                            (T10)
  tests/fuzz.rs           随机命令序列的性质测试                                                 (T10)
crewd/Cargo.lock          多一条 crewd-work                                                    (T1)
crewd/README.md           一行                                                                (T10)
docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md   追加 M4a 交付记录        (T10)
```

## 公开 API 一览（M4b 只用这些）

```rust
// 账本
WorkBook::new(crew: CrewId, cfg: Config) -> WorkBook
WorkBook::restore(state: BookState, cfg: Config, now: Ts) -> Result<WorkBook, Violation>
fn state(&self) -> &BookState;   fn log(&self) -> &[Event];   fn take_log(&mut self) -> Vec<Event>;
fn item(&self, WorkId) -> Option<&WorkItem>;  fn ask(&self, AskId) -> Option<&HumanAsk>;  fn entry(&self, EntryNo) -> Option<&EntryTodo>;

// 命令：全部 -> Result<Applied<T>, Rejected>，Applied { value: T, events: Vec<Event> }
fn register_entry(&mut self, now: Ts, no: EntryNo)                                         // T = bool（新登记？）
fn claim_entry(&mut self, by: &Actor, now: Ts, entry: EntryNo, items: Vec<NewItem>)          // T = Vec<WorkId>
fn create_item(&mut self, by: &Actor, now: Ts, item: NewItem)                                // T = WorkId
fn link_channel(&mut self, by: &Actor, now: Ts, id: WorkId, channel: Option<ChannelId>)
fn block(&mut self, by: &Actor, now: Ts, id: WorkId, on: Vec<BlockOn>, rest: Continuation)   // T = BlockResult { asks, split }
fn answer_ask(&mut self, by: &Actor, now: Ts, ask: AskId, text: &str)                       // T = Vec<WorkId>（卡点被清的项）
fn apply_default(&mut self, by: &Actor, now: Ts, ask: AskId)                                // T = Vec<WorkId>
fn report(&mut self, by: &Actor, now: Ts, id: WorkId, r: Report)
fn mark_done(&mut self, by: &Actor, now: Ts, id: WorkId, evidence: Evidence)                // T = Vec<WorkId>（被放行的项）
fn drop_item(&mut self, by: &Actor, now: Ts, id: WorkId, reason: DropReason)
fn relink(&mut self, by: &Actor, now: Ts, id: WorkId, parent: Option<WorkId>, depends_on: Vec<WorkId>)
fn close_entry(&mut self, by: &Actor, now: Ts, no: EntryNo, note: &str, evidence: Option<Evidence>)
fn request_transfer(&mut self, by: &Actor, now: Ts, id: WorkId, to: SessionId, note: &str)
fn accept_transfer(&mut self, by: &Actor, now: Ts, id: WorkId)
fn withdraw_transfer(&mut self, by: &Actor, now: Ts, id: WorkId)
fn assign(&mut self, by: &Actor, now: Ts, id: WorkId, to: SessionId, note: &str)
fn reassign_lost(&mut self, by: &Actor, now: Ts, id: WorkId, to: SessionId, note: &str)
fn record_checkpoint_wake(&mut self, by: &Actor, now: Ts, ids: &[WorkId])
fn note_escalation(&mut self, by: &Actor, now: Ts, ask: AskId, tier: WaitTier)
fn on_idle(&mut self, session: &SessionId, now: Ts) -> StopDecision                          // 只动软状态，不产出事件

// 视图（只读）
fn entry_view(&self, EntryNo) -> Option<EntryView>;   fn entry_views(&self) -> Vec<EntryView>;
fn ready_items(&self) -> Vec<WorkId>;   fn unowned_items(&self) -> Vec<WorkId>;   fn load_of(&self, &SessionId) -> usize;
fn my_items(&self, &SessionId) -> Vec<MyItem>;   fn incoming_transfers(&self, &SessionId) -> Vec<WorkId>;
fn due_checkpoints(&self, now: Ts) -> Vec<CheckpointDue>;   fn owner_lost(&self, now: Ts, unrecoverable: &BTreeSet<SessionId>) -> Vec<LostOwner>;
fn stop_violations(&self, &SessionId, now: Ts) -> Vec<StopViolation>;
fn human_waits(&self, now: Ts) -> Vec<HumanWait>;   fn escalations_due(&self, now: Ts) -> Vec<(AskId, WaitTier)>;   fn defaults_due(&self, now: Ts) -> Vec<AskId>;
fn patrol(&self, now: Ts, unrecoverable: &BTreeSet<SessionId>) -> Patrol;   // Patrol::needs_admin_session()
fn patrol_due(messages_since_last: u32, last_patrol_at: Option<Ts>, now: Ts, cfg: &Config) -> bool;
```

## M4b 接线对照（spec §9 / §10 → 本 crate）

| spec | 调用 |
|---|---|
| `claim_todo(#N, items[])` | `claim_entry(session, now, #N, items)`（缺省负责人 = 调用的 session） |
| `work_item_create` | `create_item(session, now, item)` |
| `request_help(text)` | `create_item(session, now, NewItem { owner: None, parent: 自己的项?, .. })` → 无主项，调度层用 `assign(System, ..)` 派 |
| `work_item_update(id, status?, next_check_at?, evidence?, note)` | status = open / active → `report`；evidence → `mark_done`；放弃 → `drop_item` |
| `work_item_block(id, on, can_continue_split?)` | `block(.., on, Continuation::Nothing / Split(items))`（「其余能否先推进」必答） |
| `work_item_transfer_request/accept` | `request_transfer` / `accept_transfer`（另有 `withdraw_transfer`：撤回 / 拒绝） |
| `work_item_assign`（机长） | `assign` |
| `my_work_items` 与每轮注入 | `my_items(session)` + `incoming_transfers(session)` |
| 人回复 H<n> | `answer_ask(Human, ..)` → 事件里每条 `BlockerCleared.owner` 直接叫醒 |
| 检查点计时器 | 每轮 `due_checkpoints(now)` → 给每个 owner 投一条唤醒 → `record_checkpoint_wake(System, now, items)` |
| 负责人失联 | `owner_lost(now, 恢复失败的 session)` → Jev 选人（排除原负责人）→ `reassign_lost(System, ..)` + 告知机长 |
| idle 边沿 | `on_idle(session, now)` → Rewake 就叫醒并列出项；Escalate 交行政处 |
| 等人升级 / 默认方案 | `escalations_due` → 通知人 → `note_escalation`；`defaults_due` → `apply_default(System, ..)` → 叫醒负责人「按默认办，照常走审批，触发审批框就转回人」（§13 Q2） |
| 新入口 Todo | `register_entry(now, #N)`（重复登记无副作用，重启对账用） |
| §10 迁移：plan.json 每条 | `create_item(System, now, NewItem::titled(..).owned_by(机长))` |
| §10 迁移：未结入口、最后 in_progress 回复者可恢复 | `claim_entry(System, now, #N, [NewItem::titled(..).owned_by(那个 session).check_at(now)])` |
| §10 迁移：其余未结入口 | 只 `register_entry`，留待认领（巡检 `unclaimed_entries` 会捞） |
| §17 换负责人拉进关联群 | `OwnerChanged.channel` |
| §17 归档闲置群 | `patrol(..).archivable_channels`（「闲置」再看消息） |

---

### Task 1: crate 骨架 + 编号与时间（`ids`）

三个编号空间分开且字符串形式唯一：`W3` / `H1` / `#7`，前缀不分大小写（agent 会写 `w3`），数字不带前导 0。线上 JSON 就用这个字符串形式（serde `try_from` / `into`），这样 M4b 的 MCP 参数和事件 JSON 直接可读。

**Files:**
- Create: `crewd/crates/work/Cargo.toml`（`cargo new` + `cargo add`）、`crewd/crates/work/src/lib.rs`、`crewd/crates/work/src/ids.rs`
- Modify: `crewd/Cargo.lock`

**Interfaces:**
- Consumes: 无。
- Produces（`crewd_work::ids`）：`pub struct Ts(pub i64)`（`plus(self, ms: i64) -> Ts` 饱和加、`since(self, earlier: Ts) -> i64`）；`pub const MINUTE: i64`、`pub const HOUR: i64`；`pub struct WorkId(pub u32)`、`pub struct AskId(pub u32)`、`pub struct EntryNo(pub i64)`（`Display` / `FromStr` / serde 字符串、按数值排序）；`pub struct SessionId(pub String)`、`CrewId`、`ChannelId`（`new(impl Into<String>)`、`as_str()`、serde 透明）；`pub struct ParseIdError { pub input: String, pub expected: &'static str }`。

- [ ] **Step 1: 建 crate、加依赖**

```bash
cargo new --lib crates/work --name crewd-work --vcs none
cargo add --offline -p crewd-work serde --features derive
cargo add --offline -p crewd-work --dev serde_json
cat crates/work/Cargo.toml
```

Expected（workspace 继承的字段 cargo 会自动写；若不是，手工改成这样）：

```toml
[package]
name = "crewd-work"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
serde = { version = "1.0.229", features = ["derive"] }

[dev-dependencies]
serde_json = "1.0.151"
```

- [ ] **Step 2: 写失败的测试**

`crates/work/src/lib.rs`（整个文件，覆盖 `cargo new` 生成的内容）：

```rust
//! crewd 的工作跟进核心（M4a）：两层 Todo（入口 #N + 工作项 W<n> 的 DAG）、负责人、检查点、
//! 卡住只卡那一件、停下即检查、等人升级、行政巡检的纯判定（spec §4–§6、§8、§9）。
//! sans-IO：不读时钟、不碰文件 / 数据库 / tokio；时间一律由调用方传入（Unix 毫秒）。持久化与 MCP 接线归 M4b。
pub mod ids;
```

`crates/work/src/ids.rs`：先只放测试模块（下一步把实现写在它**上面**）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_print_with_their_prefix() {
        assert_eq!(WorkId(3).to_string(), "W3");
        assert_eq!(AskId(12).to_string(), "H12");
        assert_eq!(EntryNo(7).to_string(), "#7");
    }

    #[test]
    fn ids_parse_canonical_and_lowercase_prefix() {
        assert_eq!("W3".parse::<WorkId>(), Ok(WorkId(3)));
        assert_eq!("w3".parse::<WorkId>(), Ok(WorkId(3)));
        assert_eq!("h1".parse::<AskId>(), Ok(AskId(1)));
        assert_eq!("#42".parse::<EntryNo>(), Ok(EntryNo(42)));
    }

    #[test]
    fn wrong_prefix_or_number_shape_is_rejected() {
        for bad in [
            "", "W", "3", "#3", "H3", "W03", "W0", "W-1", "W+1", "W 3", "W3 ", "W3a",
        ] {
            assert!(bad.parse::<WorkId>().is_err(), "{bad:?} 应被拒");
        }
        assert!(
            "W3".parse::<EntryNo>().is_err(),
            "W 号不能当 #N 用（两本账分开）"
        );
        assert!("#3".parse::<WorkId>().is_err());
        assert!("W4294967296".parse::<WorkId>().is_err(), "超出 u32");
        let err = "X1".parse::<WorkId>().unwrap_err();
        assert_eq!(err.to_string(), "`X1` is not a valid work item id (W<n>)");
    }

    #[test]
    fn ids_serialize_as_their_printed_form() {
        assert_eq!(serde_json::to_string(&WorkId(5)).unwrap(), "\"W5\"");
        assert_eq!(serde_json::from_str::<WorkId>("\"W5\"").unwrap(), WorkId(5));
        assert!(serde_json::from_str::<WorkId>("\"#5\"").is_err());
        assert!(serde_json::from_str::<WorkId>("5").is_err());
        assert_eq!(serde_json::to_string(&EntryNo(9)).unwrap(), "\"#9\"");
        assert_eq!(
            serde_json::to_string(&SessionId::new("s-1")).unwrap(),
            "\"s-1\""
        );
        assert_eq!(serde_json::to_string(&Ts(1_000)).unwrap(), "1000");
    }

    #[test]
    fn ids_order_numerically_not_lexically() {
        let mut v = vec![WorkId(10), WorkId(2), WorkId(1)];
        v.sort();
        assert_eq!(v, [WorkId(1), WorkId(2), WorkId(10)]);
    }

    #[test]
    fn ts_arithmetic_saturates() {
        assert_eq!(Ts(10).plus(5), Ts(15));
        assert_eq!(Ts(i64::MAX).plus(1), Ts(i64::MAX));
        assert_eq!(Ts(10).since(Ts(4)), 6);
        assert_eq!(Ts(4).since(Ts(10)), -6);
        assert_eq!(HOUR, 3_600_000);
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-work`
Expected: 编译失败，`cannot find type WorkId` / `cannot find function ...` 一类（实现还没写）。

- [ ] **Step 4: 实现**

把下面整段放在 `crates/work/src/ids.rs` 测试模块的**上面**：

```rust
//! 编号与时间。全是值类型；时间是调用方传进来的 Unix 毫秒，本 crate 从不读时钟。
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

pub const MINUTE: i64 = 60_000;
pub const HOUR: i64 = 60 * MINUTE;

/// Unix 毫秒。只比较、只加减，不做日历运算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Ts(pub i64);

impl Ts {
    pub fn plus(self, ms: i64) -> Ts {
        Ts(self.0.saturating_add(ms))
    }

    /// `self - earlier`，毫秒；`earlier` 在后面时为负。
    pub fn since(self, earlier: Ts) -> i64 {
        self.0.saturating_sub(earlier.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseIdError {
    pub input: String,
    pub expected: &'static str,
}

impl fmt::Display for ParseIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` is not a valid {}", self.input, self.expected)
    }
}

impl std::error::Error for ParseIdError {}

/// `<前缀><正整数>`，前缀不分大小写，数字不带符号、不带前导 0（规范写法唯一）。
fn parse_prefixed(s: &str, prefix: char, expected: &'static str) -> Result<u64, ParseIdError> {
    let err = || ParseIdError {
        input: s.to_string(),
        expected,
    };
    let mut chars = s.chars();
    let first = chars.next().ok_or_else(err)?;
    if !first.eq_ignore_ascii_case(&prefix) {
        return Err(err());
    }
    let digits = chars.as_str();
    if digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(err());
    }
    digits.parse::<u64>().map_err(|_| err())
}

macro_rules! numbered_id {
    ($name:ident, $int:ty, $prefix:literal, $expected:literal) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(pub $int);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", $prefix, self.0)
            }
        }

        impl FromStr for $name {
            type Err = ParseIdError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let n = parse_prefixed(s, $prefix, $expected)?;
                <$int>::try_from(n).map($name).map_err(|_| ParseIdError {
                    input: s.to_string(),
                    expected: $expected,
                })
            }
        }

        impl TryFrom<String> for $name {
            type Error = ParseIdError;
            fn try_from(s: String) -> Result<Self, Self::Error> {
                s.parse()
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.to_string()
            }
        }
    };
}

numbered_id!(WorkId, u32, 'W', "work item id (W<n>)");
numbered_id!(AskId, u32, 'H', "human ask id (H<n>)");
numbered_id!(EntryNo, i64, '#', "entry todo number (#<n>)");

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                $name(s.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

opaque_id!(SessionId);
opaque_id!(CrewId);
opaque_id!(ChannelId);
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-work`
Expected: `test result: ok. 6 passed; 0 failed`

- [ ] **Step 6: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/work/Cargo.toml crates/work/src/lib.rs crates/work/src/ids.rs Cargo.lock
git diff --cached --stat
git commit -m "crewd-work: crate skeleton, W/H/# ids with canonical string form, injected time

Co-Authored-By: Claude <noreply@anthropic.com>"
```

`git diff --cached --stat` 应该正好 4 个文件。

---

### Task 2: 字段表、事件、权限表（`model`、`events`、`perm`）

spec §4.2 的字段表落成类型；另加三个字段：`channel`（§17 工作项可关联群）、`lastReport`（§4.3 注入要的「下一步」）、`dropReason`（放弃要留理由，事实性放弃要带证据）。`BookState` 是一个机组的全部状态，`clock` 见裁定 10。`Config` 的默认值逐条来自 spec（测试钉住）。权限表是裁定 7 的唯一实现，测试把每个角色的「能做的操作集合」整张列出来——表一改，测试就要跟着改，不会悄悄放宽。

**Files:**
- Create: `crewd/crates/work/src/model.rs`、`crewd/crates/work/src/events.rs`、`crewd/crates/work/src/perm.rs`
- Modify: `crewd/crates/work/src/lib.rs`

**Interfaces:**
- Consumes: Task 1 的 `ids::*`。
- Produces（`crewd_work::model`）：`Status { Open, Active, Blocked, Done, Dropped }`（`is_terminal`、`is_live`）；`DoneLevel { Local, Public, Production }`；`Body { goal, level, acceptance }`；`Evidence { Commit(String), Url(String), Text(String) }`（`is_blank`）；`Blocker { Human(AskId), Item(WorkId) }`；`Role { Worker, Captain, Admin }`；`Actor { Session { id: SessionId, role: Role }, Human, System }`（`worker(&str)` / `captain(&str)` / `admin(&str)`、`session(&self) -> Option<&SessionId>`）；`HowChanged { Claim, Transfer, Assign, Lost }`；`OwnerChange { from, to, by, how, note, at }`；`PendingTransfer { to, requested_by, note, at }`；`Note { text, at }`；`DropKind { Duplicate, Superseded, NotNeeded }`（`is_factual`）；`DropReason { kind, note, evidence: Option<Evidence> }`；`WorkItem`（字段：`id, title, body, parent, depends_on, sources, owner, owner_history, status, blocked_on, next_check_at, missed_checks, evidence, drop_reason, pending_transfer, channel, last_report, revision, created_at, updated_at`）；`EntryClosure { by, note, evidence, at }`；`EntryTodo { no, registered_at, closed: Option<EntryClosure> }`；`DefaultOption { text, after_ms: i64 }`；`AskState { Open, Answered { text, at }, Defaulted { at } }`；`WaitTier { Fresh, Pinned, StillWanted }`（有序）；`HumanAsk { id, item, asked_by, question, default, asked_at, state, escalated }`；`StopKind { Unanswered, Overdue, ReadyUnscheduled }`；`StopViolation { item, kind }`；`StopStreak { last, unimproved, escalated }`；`BookState { crew, items: BTreeMap<WorkId, WorkItem>, entries: BTreeMap<EntryNo, EntryTodo>, asks: BTreeMap<AskId, HumanAsk>, last_item: u32, last_ask: u32, last_seq: u64, clock: Ts, stop_streaks: BTreeMap<SessionId, StopStreak> }`（`empty(CrewId)`）；`Config { max_check_horizon_ms, recheck_after_ms, missed_checks_limit, stop_check_limit, pin_after_ms, still_wanted_after_ms, patrol_every_messages, patrol_interval_ms }`（`Default`）；`ItemRef { Id(WorkId), Batch(usize) }`；`NewItem { title, body, parent: Option<ItemRef>, depends_on: Vec<ItemRef>, sources: Vec<EntryNo>, owner: Option<SessionId>, channel: Option<ChannelId>, next_check_at: Option<Ts> }`（`titled(&str)`、`owned_by(&str)`、`under(ItemRef)`、`after(ItemRef)`、`from_entry(EntryNo)`、`check_at(Ts)`）。
- Produces（`crewd_work::events`）：`Event { seq: u64, at: Ts, by: Actor, kind: EventKind }`；`EventKind` 的变体：`EntryRegistered`、`EntryClaimed`、`EntryClosed`、`ItemCreated`、`Reported`、`Blocked`、`AskOpened`、`AskAnswered`、`AskDefaulted`、`BlockerCleared { id, owner, blocker, status }`、`Done`、`Dropped`、`Relinked`、`ChannelLinked`、`TransferRequested`、`TransferWithdrawn`、`OwnerChanged { id, from, to, how, note, channel }`、`CheckpointWake { id, owner, missed_checks, next_check_at }`、`EscalationNoted`（serde：`{"type": "<snake_case>", ...}`）。
- Produces（`crewd_work::perm`）：`Action`（21 个变体，`Action::ALL`）；`Ctx<'a> { owner: Option<&'a SessionId>, transfer_to: Option<&'a SessionId>, assignee: Option<&'a SessionId> }`（`Default`）；`fn allowed(actor: &Actor, action: Action, ctx: Ctx<'_>) -> bool`。

- [ ] **Step 1: 写失败的测试**

`crates/work/src/lib.rs`（整个文件）：

```rust
//! crewd 的工作跟进核心（M4a）：两层 Todo（入口 #N + 工作项 W<n> 的 DAG）、负责人、检查点、
//! 卡住只卡那一件、停下即检查、等人升级、行政巡检的纯判定（spec §4–§6、§8、§9）。
//! sans-IO：不读时钟、不碰文件 / 数据库 / tokio；时间一律由调用方传入（Unix 毫秒）。持久化与 MCP 接线归 M4b。
pub mod events;
pub mod ids;
pub mod model;
pub mod perm;
```

`crates/work/src/model.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_statuses() {
        let live: Vec<Status> = [
            Status::Open,
            Status::Active,
            Status::Blocked,
            Status::Done,
            Status::Dropped,
        ]
        .into_iter()
        .filter(|s| s.is_live())
        .collect();
        assert_eq!(live, [Status::Open, Status::Active, Status::Blocked]);
    }

    #[test]
    fn blank_evidence_is_detected() {
        assert!(Evidence::Text("  \n".into()).is_blank());
        assert!(Evidence::Commit(String::new()).is_blank());
        assert!(!Evidence::Url("https://x".into()).is_blank());
    }

    #[test]
    fn wait_tiers_are_ordered() {
        assert!(WaitTier::Fresh < WaitTier::Pinned);
        assert!(WaitTier::Pinned < WaitTier::StillWanted);
    }

    #[test]
    fn spec_defaults() {
        let c = Config::default();
        assert_eq!(c.max_check_horizon_ms, 2 * HOUR);
        assert_eq!(c.missed_checks_limit, 2);
        assert_eq!(c.stop_check_limit, 3);
        assert_eq!(c.pin_after_ms, 24 * HOUR);
        assert_eq!(c.still_wanted_after_ms, 72 * HOUR);
        assert_eq!(c.patrol_every_messages, 10);
        assert_eq!(c.patrol_interval_ms, 30 * MINUTE);
    }

    #[test]
    fn wire_shapes_are_stable() {
        let b = serde_json::to_value(Blocker::Human(AskId(2))).unwrap();
        assert_eq!(b, serde_json::json!({"kind": "human", "id": "H2"}));
        let e = serde_json::to_value(Evidence::Commit("abc".into())).unwrap();
        assert_eq!(e, serde_json::json!({"kind": "commit", "value": "abc"}));
        let a = serde_json::to_value(Actor::captain("s1")).unwrap();
        assert_eq!(
            a,
            serde_json::json!({"kind": "session", "id": "s1", "role": "captain"})
        );
        assert_eq!(serde_json::to_value(Status::Blocked).unwrap(), "blocked");
        assert_eq!(
            serde_json::to_value(WaitTier::StillWanted).unwrap(),
            "still_wanted"
        );
    }
}
```

`crates/work/src/perm.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use Action::*;

    fn sid(s: &str) -> SessionId {
        SessionId::new(s)
    }

    /// 某个操作者在「它是负责人 / 不是负责人」两种上下文下能做的操作集合。
    fn grants(actor: &Actor, ctx: Ctx<'_>) -> Vec<Action> {
        Action::ALL
            .into_iter()
            .filter(|a| allowed(actor, *a, ctx))
            .collect()
    }

    #[test]
    fn worker_acts_only_on_what_it_owns() {
        let me = sid("w");
        let other = sid("x");
        let owner_ctx = Ctx {
            owner: Some(&me),
            ..Ctx::default()
        };
        assert_eq!(
            grants(&Actor::worker("w"), owner_ctx),
            [
                ClaimEntry,
                CreateItem,
                CreateChild,
                Report,
                Block,
                Done,
                DropDecision,
                DropFactual,
                Relink,
                LinkChannel,
                TransferRequest,
                TransferWithdraw
            ]
        );
        let foreign = Ctx {
            owner: Some(&other),
            ..Ctx::default()
        };
        assert_eq!(
            grants(&Actor::worker("w"), foreign),
            [ClaimEntry, CreateItem]
        );
    }

    #[test]
    fn worker_may_assign_only_an_unowned_item_to_itself() {
        let me = sid("w");
        let other = sid("x");
        let w = Actor::worker("w");
        let self_claim = Ctx {
            assignee: Some(&me),
            ..Ctx::default()
        };
        assert!(allowed(&w, Assign, self_claim));
        let to_other = Ctx {
            assignee: Some(&other),
            ..Ctx::default()
        };
        assert!(!allowed(&w, Assign, to_other));
        let owned = Ctx {
            owner: Some(&other),
            assignee: Some(&me),
            ..Ctx::default()
        };
        assert!(!allowed(&w, Assign, owned), "抢别人的项不行");
    }

    #[test]
    fn transfer_target_may_accept_or_decline() {
        let me = sid("w");
        let other = sid("x");
        let ctx = Ctx {
            owner: Some(&other),
            transfer_to: Some(&me),
            ..Ctx::default()
        };
        assert_eq!(
            grants(&Actor::worker("w"), ctx),
            [ClaimEntry, CreateItem, TransferAccept, TransferWithdraw]
        );
    }

    #[test]
    fn captain_has_business_authority_but_not_system_duties() {
        let other = sid("x");
        let ctx = Ctx {
            owner: Some(&other),
            ..Ctx::default()
        };
        let denied: Vec<Action> = Action::ALL
            .into_iter()
            .filter(|a| !allowed(&Actor::captain("c"), *a, ctx))
            .collect();
        assert_eq!(
            denied,
            [
                TransferRequest,
                TransferAccept,
                AnswerAsk,
                ApplyDefault,
                RecordWake,
                NoteEscalation
            ]
        );
    }

    #[test]
    fn admin_closes_only_on_facts() {
        let other = sid("x");
        let ctx = Ctx {
            owner: Some(&other),
            ..Ctx::default()
        };
        assert_eq!(
            grants(&Actor::admin("a"), ctx),
            [
                Done,
                DropFactual,
                Relink,
                LinkChannel,
                ReassignLost,
                CloseEntryFactual
            ]
        );
        assert!(
            !allowed(&Actor::admin("a"), Assign, ctx),
            "强制指派是机长权力"
        );
        assert!(
            !allowed(&Actor::admin("a"), DropDecision, ctx),
            "不能以「不值得做」关闭"
        );
    }

    #[test]
    fn human_is_above_captain_and_is_the_only_one_who_answers() {
        let denied: Vec<Action> = Action::ALL
            .into_iter()
            .filter(|a| !allowed(&Actor::Human, *a, Ctx::default()))
            .collect();
        assert_eq!(
            denied,
            [
                TransferRequest,
                TransferAccept,
                ApplyDefault,
                RecordWake,
                NoteEscalation
            ]
        );
        let answerers: Vec<Actor> = [
            Actor::worker("w"),
            Actor::captain("c"),
            Actor::admin("a"),
            Actor::Human,
            Actor::System,
        ]
        .into_iter()
        .filter(|a| allowed(a, AnswerAsk, Ctx::default()))
        .collect();
        assert_eq!(answerers, [Actor::Human]);
    }

    #[test]
    fn system_only_dispatches_and_keeps_time() {
        assert_eq!(
            grants(&Actor::System, Ctx::default()),
            [
                ClaimEntry,
                CreateItem,
                CreateChild,
                Assign,
                ReassignLost,
                ApplyDefault,
                RecordWake,
                NoteEscalation
            ]
        );
    }
}
```

`crates/work/src/events.rs`：本步先建一个空文件（`touch crates/work/src/events.rs`），Step 3 填内容。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-work`
Expected: 编译失败，找不到 `Status` / `Evidence` / `Action` / `allowed` 等。

- [ ] **Step 3: 实现**

`crates/work/src/model.rs` 测试模块**上面**：

```rust
//! 数据形状（spec §4.2 字段表）。只有数据与极小的取值助手；规则在 `invariants`、命令在 `book` 及其兄弟模块。
use crate::ids::{AskId, ChannelId, CrewId, EntryNo, HOUR, MINUTE, SessionId, Ts, WorkId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Open,
    Active,
    Blocked,
    Done,
    Dropped,
}

impl Status {
    pub fn is_terminal(self) -> bool {
        matches!(self, Status::Done | Status::Dropped)
    }
    pub fn is_live(self) -> bool {
        !self.is_terminal()
    }
}

/// 完成层级（spec §4.2 body 里的 local|public|production）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DoneLevel {
    #[default]
    Local,
    Public,
    Production,
}

/// body = 目标 + 完成层级 + 验收标准。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Body {
    pub goal: String,
    pub level: DoneLevel,
    pub acceptance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum Evidence {
    Commit(String),
    Url(String),
    Text(String),
}

impl Evidence {
    pub fn is_blank(&self) -> bool {
        match self {
            Evidence::Commit(s) | Evidence::Url(s) | Evidence::Text(s) => s.trim().is_empty(),
        }
    }
}

/// blockedOn 的一项：一条人类待确认，或另一个工作项。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "lowercase")]
pub enum Blocker {
    Human(AskId),
    Item(WorkId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Worker,
    Captain,
    Admin,
}

/// 谁在做这次修改。机长 / 行政处是 session 的角色，由调用方（crewd）按机组认定后传进来。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Actor {
    Session {
        id: SessionId,
        role: Role,
    },
    Human,
    /// crewd 自己（调度层 Jev、计时器、迁移）。
    System,
}

impl Actor {
    pub fn worker(id: &str) -> Actor {
        Actor::Session {
            id: SessionId::new(id),
            role: Role::Worker,
        }
    }
    pub fn captain(id: &str) -> Actor {
        Actor::Session {
            id: SessionId::new(id),
            role: Role::Captain,
        }
    }
    pub fn admin(id: &str) -> Actor {
        Actor::Session {
            id: SessionId::new(id),
            role: Role::Admin,
        }
    }
    pub fn session(&self) -> Option<&SessionId> {
        match self {
            Actor::Session { id, .. } => Some(id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HowChanged {
    /// 自己认领（认领入口、建项时自己当负责人、领走无主项）。
    Claim,
    /// 原负责人发起、新负责人接受。
    Transfer,
    /// 机长 / 人 / 调度层指派。
    Assign,
    /// 负责人失联后重派。
    Lost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerChange {
    pub from: Option<SessionId>,
    pub to: SessionId,
    pub by: Actor,
    pub how: HowChanged,
    pub note: String,
    pub at: Ts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingTransfer {
    pub to: SessionId,
    pub requested_by: SessionId,
    pub note: String,
    pub at: Ts,
}

/// 最近一次回报（spec §4.3 注入里的「下一步」）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub text: String,
    pub at: Ts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DropKind {
    /// 事实性关闭（行政处也能做，必须带证据）。
    Duplicate,
    Superseded,
    /// 业务决定「不做了」（只有负责人 / 机长 / 人）。
    NotNeeded,
}

impl DropKind {
    pub fn is_factual(self) -> bool {
        matches!(self, DropKind::Duplicate | DropKind::Superseded)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DropReason {
    pub kind: DropKind,
    pub note: String,
    pub evidence: Option<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkItem {
    pub id: WorkId,
    pub title: String,
    pub body: Body,
    pub parent: Option<WorkId>,
    pub depends_on: Vec<WorkId>,
    pub sources: Vec<EntryNo>,
    pub owner: Option<SessionId>,
    pub owner_history: Vec<OwnerChange>,
    pub status: Status,
    pub blocked_on: Vec<Blocker>,
    pub next_check_at: Option<Ts>,
    pub missed_checks: u32,
    pub evidence: Option<Evidence>,
    pub drop_reason: Option<DropReason>,
    pub pending_transfer: Option<PendingTransfer>,
    pub channel: Option<ChannelId>,
    pub last_report: Option<Note>,
    pub revision: u64,
    pub created_at: Ts,
    pub updated_at: Ts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryClosure {
    pub by: Actor,
    pub note: String,
    pub evidence: Option<Evidence>,
    pub at: Ts,
}

/// 第一层入口（#N）。原文在 M1 的 todos 表里，这里只记工作跟进需要的部分。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryTodo {
    pub no: EntryNo,
    pub registered_at: Ts,
    /// 机长 / 人的裁定关闭（或行政处带证据的事实关闭）。状态推导见 `views::EntryStatus`。
    pub closed: Option<EntryClosure>,
}

/// 「X 毫秒没回复就按 A 办」（spec §6；到点后照常走审批，§13 Q2）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultOption {
    pub text: String,
    pub after_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum AskState {
    Open,
    Answered { text: String, at: Ts },
    Defaulted { at: Ts },
}

/// 等人的升级档（spec §6：24h 置顶、72h 问「还要吗」）。
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WaitTier {
    #[default]
    Fresh,
    Pinned,
    StillWanted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanAsk {
    pub id: AskId,
    /// 提问时被它卡住的那一项（后来别的项也可以挂到同一条问上）。
    pub item: WorkId,
    pub asked_by: Actor,
    pub question: String,
    pub default: Option<DefaultOption>,
    pub asked_at: Ts,
    pub state: AskState,
    /// 已经通知过的最高升级档。
    pub escalated: WaitTier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopKind {
    /// 被检查点叫醒过、之后没回报。
    Unanswered,
    /// active 项的检查时间已过、没改。
    Overdue,
    /// 名下 open 项依赖已齐，却没排下一次检查。
    ReadyUnscheduled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StopViolation {
    pub item: WorkId,
    pub kind: StopKind,
}

/// 一个 session 连续几次 idle 都没改善（spec §5 防死循环）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopStreak {
    pub last: Vec<StopViolation>,
    pub unimproved: u32,
    pub escalated: bool,
}

/// 一个机组的全部工作跟进状态。M4b 把它落库 / 读回（`WorkBook::restore` 会整份校验）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookState {
    pub crew: CrewId,
    pub items: BTreeMap<WorkId, WorkItem>,
    pub entries: BTreeMap<EntryNo, EntryTodo>,
    pub asks: BTreeMap<AskId, HumanAsk>,
    /// 最后发出的 W 号 / H 号 / 事件序号；号码只增不复用。
    pub last_item: u32,
    pub last_ask: u32,
    pub last_seq: u64,
    /// 账本见过的最晚时间。命令用 max(传入的 now, clock)：墙钟往回跳不会让账本卡死（裁定 10）。
    pub clock: Ts,
    pub stop_streaks: BTreeMap<SessionId, StopStreak>,
}

impl BookState {
    pub fn empty(crew: CrewId) -> BookState {
        BookState {
            crew,
            items: BTreeMap::new(),
            entries: BTreeMap::new(),
            asks: BTreeMap::new(),
            last_item: 0,
            last_ask: 0,
            last_seq: 0,
            clock: Ts(0),
            stop_streaks: BTreeMap::new(),
        }
    }
}

/// 可配的阈值。默认值来自 spec：检查上限 2h（§4.2）、失联 K=2（§5）、停下即检查 3 次（§5）、
/// 24h / 72h（§6）、巡检每 10 条消息 / 30min（§8.4）。`recheck_after_ms`（叫醒后没回报、多久再叫）是本计划定的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub max_check_horizon_ms: i64,
    pub recheck_after_ms: i64,
    pub missed_checks_limit: u32,
    pub stop_check_limit: u32,
    pub pin_after_ms: i64,
    pub still_wanted_after_ms: i64,
    pub patrol_every_messages: u32,
    pub patrol_interval_ms: i64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_check_horizon_ms: 2 * HOUR,
            recheck_after_ms: 30 * MINUTE,
            missed_checks_limit: 2,
            stop_check_limit: 3,
            pin_after_ms: 24 * HOUR,
            still_wanted_after_ms: 72 * HOUR,
            patrol_every_messages: 10,
            patrol_interval_ms: 30 * MINUTE,
        }
    }
}

/// 新建项引用别的项：已存在的 W 号，或同一批里排在它前面的第几条。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum ItemRef {
    Id(WorkId),
    Batch(usize),
}

/// 建项参数（认领入口、建项、卡住拆分共用）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewItem {
    pub title: String,
    pub body: Body,
    pub parent: Option<ItemRef>,
    pub depends_on: Vec<ItemRef>,
    pub sources: Vec<EntryNo>,
    pub owner: Option<SessionId>,
    pub channel: Option<ChannelId>,
    pub next_check_at: Option<Ts>,
}

impl NewItem {
    pub fn titled(title: &str) -> NewItem {
        NewItem {
            title: title.to_string(),
            ..NewItem::default()
        }
    }
    pub fn owned_by(mut self, owner: &str) -> NewItem {
        self.owner = Some(SessionId::new(owner));
        self
    }
    pub fn under(mut self, parent: ItemRef) -> NewItem {
        self.parent = Some(parent);
        self
    }
    pub fn after(mut self, dep: ItemRef) -> NewItem {
        self.depends_on.push(dep);
        self
    }
    pub fn from_entry(mut self, no: EntryNo) -> NewItem {
        self.sources.push(no);
        self
    }
    pub fn check_at(mut self, at: Ts) -> NewItem {
        self.next_check_at = Some(at);
        self
    }
}
```

`crates/work/src/events.rs`（整个文件）：

```rust
//! 变更事件。每条成功的命令产出一串事件（同一个时间、同一个操作者、连续的序号），
//! M4b 照着它落库、推给订阅者、决定叫醒谁；被拒的命令不产出任何事件。
use crate::ids::{AskId, ChannelId, EntryNo, SessionId, Ts, WorkId};
use crate::model::{
    Actor, Blocker, DefaultOption, DropReason, Evidence, HowChanged, Status, WaitTier,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub at: Ts,
    pub by: Actor,
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    EntryRegistered {
        entry: EntryNo,
    },
    EntryClaimed {
        entry: EntryNo,
        items: Vec<WorkId>,
    },
    EntryClosed {
        entry: EntryNo,
        note: String,
    },
    ItemCreated {
        id: WorkId,
        title: String,
        parent: Option<WorkId>,
        depends_on: Vec<WorkId>,
        sources: Vec<EntryNo>,
        owner: Option<SessionId>,
    },
    Reported {
        id: WorkId,
        from: Status,
        to: Status,
        next_check_at: Option<Ts>,
        note: String,
    },
    Blocked {
        id: WorkId,
        on: Vec<Blocker>,
    },
    AskOpened {
        ask: AskId,
        item: WorkId,
        question: String,
        default: Option<DefaultOption>,
    },
    AskAnswered {
        ask: AskId,
        text: String,
    },
    AskDefaulted {
        ask: AskId,
        text: String,
    },
    /// 某项的一个卡点消失了（人回复了 / 默认方案到点 / 卡它的项完成了）。
    /// `owner` 要被直接叫醒（spec §5「人回复」），不论这一项是否还卡在别处。
    BlockerCleared {
        id: WorkId,
        owner: Option<SessionId>,
        blocker: Blocker,
        status: Status,
    },
    Done {
        id: WorkId,
        evidence: Evidence,
    },
    Dropped {
        id: WorkId,
        reason: DropReason,
    },
    Relinked {
        id: WorkId,
        parent: Option<WorkId>,
        depends_on: Vec<WorkId>,
    },
    ChannelLinked {
        id: WorkId,
        channel: Option<ChannelId>,
    },
    TransferRequested {
        id: WorkId,
        to: SessionId,
        note: String,
    },
    TransferWithdrawn {
        id: WorkId,
    },
    /// 换了负责人。`channel` 是该项关联的群：M4b 要把新负责人拉进去（spec §17）。
    OwnerChanged {
        id: WorkId,
        from: Option<SessionId>,
        to: SessionId,
        how: HowChanged,
        note: String,
        channel: Option<ChannelId>,
    },
    CheckpointWake {
        id: WorkId,
        owner: SessionId,
        missed_checks: u32,
        next_check_at: Option<Ts>,
    },
    EscalationNoted {
        ask: AskId,
        tier: WaitTier,
    },
}
```

`crates/work/src/perm.rs` 测试模块**上面**：

```rust
//! 谁能做什么（spec §4.2「只有负责人、机长、行政处能改状态」、§5 换负责人、§8.2 行政无业务决策权）。
//! 一张表、一个函数；命令先问它，再做别的检查。
use crate::ids::SessionId;
use crate::model::{Actor, Role};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    ClaimEntry,
    CreateItem,
    /// 在已存在的项下面建子项（ctx.owner = 父项负责人）。
    CreateChild,
    Report,
    Block,
    Done,
    /// 业务上的「不做了」。
    DropDecision,
    /// 重复 / 被取代，带证据。
    DropFactual,
    Relink,
    LinkChannel,
    TransferRequest,
    TransferAccept,
    TransferWithdraw,
    /// 指派负责人（ctx.assignee = 被指派的人）。
    Assign,
    ReassignLost,
    AnswerAsk,
    ApplyDefault,
    RecordWake,
    NoteEscalation,
    CloseEntryDecision,
    CloseEntryFactual,
}

impl Action {
    pub const ALL: [Action; 21] = [
        Action::ClaimEntry,
        Action::CreateItem,
        Action::CreateChild,
        Action::Report,
        Action::Block,
        Action::Done,
        Action::DropDecision,
        Action::DropFactual,
        Action::Relink,
        Action::LinkChannel,
        Action::TransferRequest,
        Action::TransferAccept,
        Action::TransferWithdraw,
        Action::Assign,
        Action::ReassignLost,
        Action::AnswerAsk,
        Action::ApplyDefault,
        Action::RecordWake,
        Action::NoteEscalation,
        Action::CloseEntryDecision,
        Action::CloseEntryFactual,
    ];
}

/// 判定需要的上下文：被操作项当前的负责人、待接受的转交对象、被指派的人。
#[derive(Debug, Clone, Copy, Default)]
pub struct Ctx<'a> {
    pub owner: Option<&'a SessionId>,
    pub transfer_to: Option<&'a SessionId>,
    pub assignee: Option<&'a SessionId>,
}

pub fn allowed(actor: &Actor, action: Action, ctx: Ctx<'_>) -> bool {
    use Action::*;
    match actor {
        Actor::System => matches!(
            action,
            ClaimEntry
                | CreateItem
                | CreateChild
                | Assign
                | ReassignLost
                | ApplyDefault
                | RecordWake
                | NoteEscalation
        ),
        Actor::Human => !matches!(
            action,
            TransferRequest | TransferAccept | ApplyDefault | RecordWake | NoteEscalation
        ),
        Actor::Session { id, role } => {
            let owns = ctx.owner == Some(id);
            match action {
                TransferRequest => owns,
                TransferAccept => ctx.transfer_to == Some(id),
                TransferWithdraw => owns || ctx.transfer_to == Some(id) || *role == Role::Captain,
                AnswerAsk | ApplyDefault | RecordWake | NoteEscalation => false,
                _ => match role {
                    Role::Captain => true,
                    Role::Admin => matches!(
                        action,
                        Done | DropFactual
                            | Relink
                            | LinkChannel
                            | ReassignLost
                            | CloseEntryFactual
                    ),
                    Role::Worker => match action {
                        ClaimEntry | CreateItem => true,
                        CreateChild | Report | Block | Done | DropDecision | DropFactual
                        | Relink | LinkChannel => owns,
                        Assign => ctx.owner.is_none() && ctx.assignee == Some(id),
                        _ => false,
                    },
                },
            }
        }
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-work`
Expected: `test result: ok. 18 passed; 0 failed`

- [ ] **Step 5: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/work/src/lib.rs crates/work/src/model.rs crates/work/src/events.rs crates/work/src/perm.rs
git diff --cached --stat
git commit -m "crewd-work: work item field table, change events, permission table

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: 找环与整份不变量（`graph`、`invariants`）

找环是迭代 DFS（5 万项的链也不爆栈），按 `BTreeMap` 顺序走、结果确定，返回环上节点（按边的方向）。性质测试两条：随机图上与「能不能沿边走回自己」的暴力答案逐个对照、且返回的确实是一个环；按随机拓扑序只连前向边的图永远无环。生成器自带（xorshift64\*），并断言它真的造出了足够多的环（尺子先证明会红）。

`check` 返回第一条违反（按 W 号顺序，确定），每条不变量各有一个最小反例测试。

**Files:**
- Create: `crewd/crates/work/src/graph.rs`、`crewd/crates/work/src/invariants.rs`
- Modify: `crewd/crates/work/src/lib.rs`

**Interfaces:**
- Consumes: Task 1 `ids`、Task 2 `model::{BookState, WorkItem, Blocker, Status, AskState, Config}`。
- Produces：`graph::adjacency(items: &BTreeMap<WorkId, WorkItem>) -> BTreeMap<WorkId, Vec<WorkId>>`（裁定 4 的三种边，悬空边丢掉）；`graph::find_cycle(adj: &BTreeMap<WorkId, Vec<WorkId>>) -> Option<Vec<WorkId>>`；`invariants::Violation`（`KeyMismatch(WorkId)`、`CounterBehind`、`BlankTitle(WorkId)`、`DuplicateRef(WorkId)`、`UnknownItemRef { item, missing }`、`UnknownEntryRef { item, entry }`、`UnknownAskRef { item, ask }`、`AskForUnknownItem { ask, item }`、`OwnerMissing(WorkId)`、`ActiveWithoutCheck(WorkId)`、`CheckTooFar { item, at, limit }`、`CheckNotAllowed(WorkId)`、`DoneWithoutEvidence(WorkId)`、`ActiveBeforeDeps { item, dep }`、`BlockedWithoutBlocker(WorkId)`、`BlockerOnUnblocked(WorkId)`、`StaleBlocker { item, blocker }`、`LiveUnderTerminal { item, parent }`、`LiveOnDropped { item, dep }`、`LiveOnClosedEntry { item, entry }`、`StrayTransfer(WorkId)`、`Cycle(Vec<WorkId>)`）；`invariants::check(s: &BookState, cfg: &Config, now: Ts) -> Result<(), Violation>`。

- [ ] **Step 1: 写失败的测试**

`crates/work/src/lib.rs`（整个文件）：

```rust
//! crewd 的工作跟进核心（M4a）：两层 Todo（入口 #N + 工作项 W<n> 的 DAG）、负责人、检查点、
//! 卡住只卡那一件、停下即检查、等人升级、行政巡检的纯判定（spec §4–§6、§8、§9）。
//! sans-IO：不读时钟、不碰文件 / 数据库 / tokio；时间一律由调用方传入（Unix 毫秒）。持久化与 MCP 接线归 M4b。
pub mod events;
pub mod graph;
pub mod ids;
pub mod invariants;
pub mod model;
pub mod perm;
```

`crates/work/src/graph.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// 确定性的伪随机数（xorshift64*）。测试专用，不引第三方 crate。
    struct Rng(u64);

    impl Rng {
        fn new(seed: u64) -> Rng {
            Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
        }
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    fn graph(edges: &[(u32, u32)], nodes: u32) -> BTreeMap<WorkId, Vec<WorkId>> {
        let mut adj: BTreeMap<WorkId, Vec<WorkId>> =
            (1..=nodes).map(|n| (WorkId(n), Vec::new())).collect();
        for &(a, b) in edges {
            adj.get_mut(&WorkId(a)).unwrap().push(WorkId(b));
        }
        adj
    }

    /// 对照答案：有没有一个节点能沿 ≥1 条边走回自己。
    fn has_cycle_oracle(adj: &BTreeMap<WorkId, Vec<WorkId>>) -> bool {
        adj.keys().any(|&start| {
            let mut seen = BTreeSet::new();
            let mut stack: Vec<WorkId> = adj[&start].clone();
            while let Some(n) = stack.pop() {
                if n == start {
                    return true;
                }
                if seen.insert(n)
                    && let Some(next) = adj.get(&n)
                {
                    stack.extend(next.iter().copied());
                }
            }
            false
        })
    }

    fn assert_is_cycle(adj: &BTreeMap<WorkId, Vec<WorkId>>, cycle: &[WorkId]) {
        assert!(!cycle.is_empty());
        let distinct: BTreeSet<WorkId> = cycle.iter().copied().collect();
        assert_eq!(distinct.len(), cycle.len(), "环上节点不重复: {cycle:?}");
        for (i, a) in cycle.iter().enumerate() {
            let b = cycle[(i + 1) % cycle.len()];
            assert!(adj[a].contains(&b), "{a} → {b} 不是一条边: {cycle:?}");
        }
    }

    #[test]
    fn dag_has_no_cycle() {
        let adj = graph(&[(1, 2), (1, 3), (2, 4), (3, 4)], 4);
        assert_eq!(find_cycle(&adj), None);
    }

    #[test]
    fn self_loop_is_a_cycle() {
        let adj = graph(&[(2, 2)], 3);
        assert_eq!(find_cycle(&adj), Some(vec![WorkId(2)]));
    }

    #[test]
    fn long_cycle_is_reported_in_edge_order() {
        let adj = graph(&[(1, 2), (2, 3), (3, 4), (4, 2)], 4);
        assert_eq!(
            find_cycle(&adj),
            Some(vec![WorkId(2), WorkId(3), WorkId(4)])
        );
    }

    #[test]
    fn dangling_targets_are_ignored() {
        let mut adj = graph(&[(1, 2)], 2);
        adj.get_mut(&WorkId(2)).unwrap().push(WorkId(99));
        assert_eq!(find_cycle(&adj), None);
    }

    #[test]
    fn deep_chain_does_not_overflow_the_stack() {
        let n = 50_000u32;
        let edges: Vec<(u32, u32)> = (1..n).map(|i| (i, i + 1)).collect();
        let mut adj = graph(&edges, n);
        assert_eq!(find_cycle(&adj), None);
        adj.get_mut(&WorkId(n)).unwrap().push(WorkId(1));
        assert_eq!(find_cycle(&adj).map(|c| c.len()), Some(n as usize));
    }

    #[test]
    fn property_random_graphs_agree_with_the_oracle() {
        let mut rng = Rng::new(0xC0FFEE);
        let mut with_cycle = 0;
        for _ in 0..3_000 {
            let nodes = 1 + rng.below(9) as u32;
            let edge_count = rng.below(u64::from(nodes) * 2);
            let edges: Vec<(u32, u32)> = (0..edge_count)
                .map(|_| {
                    (
                        1 + rng.below(u64::from(nodes)) as u32,
                        1 + rng.below(u64::from(nodes)) as u32,
                    )
                })
                .collect();
            let adj = graph(&edges, nodes);
            let found = find_cycle(&adj);
            assert_eq!(found.is_some(), has_cycle_oracle(&adj), "{edges:?}");
            if let Some(c) = found {
                with_cycle += 1;
                assert_is_cycle(&adj, &c);
            }
        }
        assert!(with_cycle > 500, "生成器要真的造出环来（{with_cycle}）");
    }

    #[test]
    fn property_topologically_ordered_graphs_never_cycle() {
        let mut rng = Rng::new(7);
        for _ in 0..2_000 {
            let nodes = 2 + rng.below(10) as u32;
            // 随机排列给出一个拓扑序；只连「序靠前 → 序靠后」的边。
            let mut order: Vec<u32> = (1..=nodes).collect();
            for i in (1..order.len()).rev() {
                order.swap(i, rng.below(i as u64 + 1) as usize);
            }
            let mut edges = Vec::new();
            for i in 0..order.len() {
                for j in i + 1..order.len() {
                    if rng.below(3) == 0 {
                        edges.push((order[i], order[j]));
                    }
                }
            }
            assert_eq!(find_cycle(&graph(&edges, nodes)), None, "{edges:?}");
        }
    }
}
```

`crates/work/src/invariants.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CrewId, SessionId};
    use crate::model::Actor;
    use crate::model::{
        Body, EntryClosure, EntryTodo, Evidence, HumanAsk, PendingTransfer, WaitTier, WorkItem,
    };

    const NOW: Ts = Ts(1_000_000);

    fn item(n: u32) -> WorkItem {
        WorkItem {
            id: WorkId(n),
            title: format!("item {n}"),
            body: Body::default(),
            parent: None,
            depends_on: vec![],
            sources: vec![],
            owner: None,
            owner_history: vec![],
            status: Status::Open,
            blocked_on: vec![],
            next_check_at: None,
            missed_checks: 0,
            evidence: None,
            drop_reason: None,
            pending_transfer: None,
            channel: None,
            last_report: None,
            revision: 1,
            created_at: NOW,
            updated_at: NOW,
        }
    }

    fn owned(n: u32, status: Status) -> WorkItem {
        let mut it = item(n);
        it.owner = Some(SessionId::new("s1"));
        it.status = status;
        if status == Status::Active {
            it.next_check_at = Some(NOW.plus(1_000));
        }
        if status == Status::Done {
            it.evidence = Some(Evidence::Commit("abc".into()));
        }
        it
    }

    fn book(items: Vec<WorkItem>) -> BookState {
        let mut s = BookState::empty(CrewId::new("c"));
        for it in items {
            s.last_item = s.last_item.max(it.id.0);
            s.items.insert(it.id, it);
        }
        s
    }

    fn ask(s: &mut BookState, n: u32, item: u32, state: AskState) {
        s.last_ask = s.last_ask.max(n);
        s.asks.insert(
            AskId(n),
            HumanAsk {
                id: AskId(n),
                item: WorkId(item),
                asked_by: Actor::worker("s1"),
                question: "q".into(),
                default: None,
                asked_at: NOW,
                state,
                escalated: WaitTier::Fresh,
            },
        );
    }

    fn verdict(s: &BookState) -> Result<(), Violation> {
        check(s, &Config::default(), NOW)
    }

    #[test]
    fn a_well_formed_book_passes() {
        let mut child = owned(2, Status::Active);
        child.parent = Some(WorkId(1));
        let mut parent = owned(1, Status::Open);
        parent.depends_on = vec![WorkId(3)];
        let dep = owned(3, Status::Done);
        let mut s = book(vec![parent, child, dep]);
        let mut blocked = owned(4, Status::Blocked);
        blocked.blocked_on = vec![Blocker::Human(AskId(1)), Blocker::Item(WorkId(2))];
        s.items.insert(WorkId(4), blocked);
        s.last_item = 4;
        ask(&mut s, 1, 4, AskState::Open);
        assert_eq!(verdict(&s), Ok(()));
    }

    #[test]
    fn cycle_through_parent_and_dependency_is_rejected() {
        // 父 → 子 是一条边：子项依赖自己的父项就成环（父要等子，子又等父）。
        let mut child = item(2);
        child.parent = Some(WorkId(1));
        child.depends_on = vec![WorkId(1)];
        let s = book(vec![item(1), child]);
        assert_eq!(
            verdict(&s),
            Err(Violation::Cycle(vec![WorkId(1), WorkId(2)]))
        );
    }

    #[test]
    fn parent_may_depend_on_its_own_child() {
        let mut parent = item(1);
        parent.depends_on = vec![WorkId(2)];
        let mut child = item(2);
        child.parent = Some(WorkId(1));
        assert_eq!(verdict(&book(vec![parent, child])), Ok(()));
    }

    #[test]
    fn blocked_on_item_edges_count_toward_cycles() {
        let mut a = owned(1, Status::Blocked);
        a.blocked_on = vec![Blocker::Item(WorkId(2))];
        let mut b = owned(2, Status::Blocked);
        b.blocked_on = vec![Blocker::Item(WorkId(1))];
        assert_eq!(
            verdict(&book(vec![a, b])),
            Err(Violation::Cycle(vec![WorkId(1), WorkId(2)]))
        );
    }

    #[test]
    fn active_needs_owner_check_and_finished_deps() {
        let mut no_owner = owned(1, Status::Active);
        no_owner.owner = None;
        assert_eq!(
            verdict(&book(vec![no_owner])),
            Err(Violation::OwnerMissing(WorkId(1)))
        );

        let mut no_check = owned(1, Status::Active);
        no_check.next_check_at = None;
        assert_eq!(
            verdict(&book(vec![no_check])),
            Err(Violation::ActiveWithoutCheck(WorkId(1)))
        );

        let mut far = owned(1, Status::Active);
        far.next_check_at = Some(NOW.plus(2 * crate::ids::HOUR + 1));
        assert_eq!(
            verdict(&book(vec![far])),
            Err(Violation::CheckTooFar {
                item: WorkId(1),
                at: NOW.plus(2 * crate::ids::HOUR + 1),
                limit: NOW.plus(2 * crate::ids::HOUR),
            })
        );

        let mut edge = owned(1, Status::Active);
        edge.next_check_at = Some(NOW.plus(2 * crate::ids::HOUR));
        assert_eq!(verdict(&book(vec![edge])), Ok(()), "上限本身可以");

        let mut early = owned(1, Status::Active);
        early.depends_on = vec![WorkId(2)];
        assert_eq!(
            verdict(&book(vec![early, owned(2, Status::Active)])),
            Err(Violation::ActiveBeforeDeps {
                item: WorkId(1),
                dep: WorkId(2)
            })
        );
    }

    #[test]
    fn overdue_check_is_still_valid() {
        // 检查时间已过（还没被处理）不是违反：违反的只是「排得太远」。
        let mut it = owned(1, Status::Active);
        it.next_check_at = Some(Ts(0));
        assert_eq!(verdict(&book(vec![it])), Ok(()));
    }

    #[test]
    fn check_time_only_on_owned_open_or_active() {
        let mut unowned = item(1);
        unowned.next_check_at = Some(NOW);
        assert_eq!(
            verdict(&book(vec![unowned])),
            Err(Violation::CheckNotAllowed(WorkId(1)))
        );
        let mut done = owned(1, Status::Done);
        done.next_check_at = Some(NOW);
        assert_eq!(
            verdict(&book(vec![done])),
            Err(Violation::CheckNotAllowed(WorkId(1)))
        );
        let mut open = owned(1, Status::Open);
        open.next_check_at = Some(NOW);
        assert_eq!(verdict(&book(vec![open])), Ok(()));
    }

    #[test]
    fn done_needs_non_blank_evidence() {
        let mut d = owned(1, Status::Done);
        d.evidence = None;
        assert_eq!(
            verdict(&book(vec![d])),
            Err(Violation::DoneWithoutEvidence(WorkId(1)))
        );
        let mut blank = owned(1, Status::Done);
        blank.evidence = Some(Evidence::Text(" ".into()));
        assert_eq!(
            verdict(&book(vec![blank])),
            Err(Violation::DoneWithoutEvidence(WorkId(1)))
        );
    }

    #[test]
    fn blocked_iff_blockers() {
        assert_eq!(
            verdict(&book(vec![owned(1, Status::Blocked)])),
            Err(Violation::BlockedWithoutBlocker(WorkId(1)))
        );
        let mut open = owned(1, Status::Open);
        open.blocked_on = vec![Blocker::Item(WorkId(2))];
        assert_eq!(
            verdict(&book(vec![open, item(2)])),
            Err(Violation::BlockerOnUnblocked(WorkId(1)))
        );
    }

    #[test]
    fn stale_and_unknown_blockers() {
        let mut b = owned(1, Status::Blocked);
        b.blocked_on = vec![Blocker::Human(AskId(1))];
        let mut s = book(vec![b.clone()]);
        assert_eq!(
            verdict(&s),
            Err(Violation::UnknownAskRef {
                item: WorkId(1),
                ask: AskId(1)
            })
        );
        ask(
            &mut s,
            1,
            1,
            AskState::Answered {
                text: "ok".into(),
                at: NOW,
            },
        );
        assert_eq!(
            verdict(&s),
            Err(Violation::StaleBlocker {
                item: WorkId(1),
                blocker: Blocker::Human(AskId(1))
            })
        );
        b.blocked_on = vec![Blocker::Item(WorkId(2))];
        assert_eq!(
            verdict(&book(vec![b, owned(2, Status::Done)])),
            Err(Violation::StaleBlocker {
                item: WorkId(1),
                blocker: Blocker::Item(WorkId(2))
            })
        );
    }

    #[test]
    fn live_items_cannot_hang_on_terminal_things() {
        let mut child = item(2);
        child.parent = Some(WorkId(1));
        assert_eq!(
            verdict(&book(vec![owned(1, Status::Done), child])),
            Err(Violation::LiveUnderTerminal {
                item: WorkId(2),
                parent: WorkId(1)
            })
        );
        let mut waiting = item(2);
        waiting.depends_on = vec![WorkId(1)];
        let mut dropped = item(1);
        dropped.status = Status::Dropped;
        assert_eq!(
            verdict(&book(vec![dropped, waiting])),
            Err(Violation::LiveOnDropped {
                item: WorkId(2),
                dep: WorkId(1)
            })
        );
        let mut from_entry = item(1);
        from_entry.sources = vec![EntryNo(5)];
        let mut s = book(vec![from_entry]);
        assert_eq!(
            verdict(&s),
            Err(Violation::UnknownEntryRef {
                item: WorkId(1),
                entry: EntryNo(5)
            })
        );
        s.entries.insert(
            EntryNo(5),
            EntryTodo {
                no: EntryNo(5),
                registered_at: NOW,
                closed: Some(EntryClosure {
                    by: Actor::Human,
                    note: "no".into(),
                    evidence: None,
                    at: NOW,
                }),
            },
        );
        assert_eq!(
            verdict(&s),
            Err(Violation::LiveOnClosedEntry {
                item: WorkId(1),
                entry: EntryNo(5)
            })
        );
    }

    #[test]
    fn transfers_need_a_live_owned_item_and_a_different_target() {
        let t = |to: &str| PendingTransfer {
            to: SessionId::new(to),
            requested_by: SessionId::new("s1"),
            note: "n".into(),
            at: NOW,
        };
        let mut ok = owned(1, Status::Open);
        ok.pending_transfer = Some(t("s2"));
        assert_eq!(verdict(&book(vec![ok])), Ok(()));
        let mut to_self = owned(1, Status::Open);
        to_self.pending_transfer = Some(t("s1"));
        assert_eq!(
            verdict(&book(vec![to_self])),
            Err(Violation::StrayTransfer(WorkId(1)))
        );
        let mut done = owned(1, Status::Done);
        done.pending_transfer = Some(t("s2"));
        assert_eq!(
            verdict(&book(vec![done])),
            Err(Violation::StrayTransfer(WorkId(1)))
        );
    }

    #[test]
    fn bookkeeping_shape_is_checked() {
        let mut s = book(vec![item(1)]);
        s.last_item = 0;
        assert_eq!(verdict(&s), Err(Violation::CounterBehind));
        let mut s = book(vec![item(1)]);
        let moved = s.items.remove(&WorkId(1)).unwrap();
        s.items.insert(WorkId(9), moved);
        s.last_item = 9;
        assert_eq!(verdict(&s), Err(Violation::KeyMismatch(WorkId(9))));
        let mut blank = item(1);
        blank.title = "   ".into();
        assert_eq!(
            verdict(&book(vec![blank])),
            Err(Violation::BlankTitle(WorkId(1)))
        );
        let mut dup = item(1);
        dup.depends_on = vec![WorkId(2), WorkId(2)];
        assert_eq!(
            verdict(&book(vec![dup, item(2)])),
            Err(Violation::DuplicateRef(WorkId(1)))
        );
        let mut dangling = item(1);
        dangling.parent = Some(WorkId(7));
        let mut s = book(vec![dangling]);
        s.last_item = 7;
        assert_eq!(
            verdict(&s),
            Err(Violation::UnknownItemRef {
                item: WorkId(1),
                missing: WorkId(7)
            })
        );
        let mut s = book(vec![item(1)]);
        ask(&mut s, 1, 4, AskState::Open);
        assert_eq!(
            verdict(&s),
            Err(Violation::AskForUnknownItem {
                ask: AskId(1),
                item: WorkId(4)
            })
        );
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-work`
Expected: 编译失败，找不到 `find_cycle` / `check` / `Violation`。

- [ ] **Step 3: 实现**

`crates/work/src/graph.rs` 测试模块**上面**：

```rust
//! 有向图找环。边的方向（裁定 4）：父 → 子（父项等子项）、项 → 它依赖的项、项 → 卡住它的项。
use crate::ids::WorkId;
use crate::model::{Blocker, WorkItem};
use std::collections::BTreeMap;

/// 所有项的出边。指向不存在的项的边丢掉（悬空引用由 `invariants` 单独报）。
pub fn adjacency(items: &BTreeMap<WorkId, WorkItem>) -> BTreeMap<WorkId, Vec<WorkId>> {
    let mut adj: BTreeMap<WorkId, Vec<WorkId>> = items.keys().map(|&k| (k, Vec::new())).collect();
    for item in items.values() {
        let out = adj.get_mut(&item.id).expect("key exists");
        for dep in &item.depends_on {
            if items.contains_key(dep) {
                out.push(*dep);
            }
        }
        for b in &item.blocked_on {
            if let Blocker::Item(w) = b
                && items.contains_key(w)
            {
                out.push(*w);
            }
        }
        if let Some(p) = item.parent
            && let Some(children) = adj.get_mut(&p)
        {
            children.push(item.id);
        }
    }
    adj
}

/// 找一个环：返回环上的节点，按边的方向排列（最后一个指回第一个）；无环返回 `None`。
/// 迭代 DFS（深图不爆栈），按 `BTreeMap` 顺序走，结果确定。
pub fn find_cycle(adj: &BTreeMap<WorkId, Vec<WorkId>>) -> Option<Vec<WorkId>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        New,
        OnPath,
        Finished,
    }
    let mut mark: BTreeMap<WorkId, Mark> = adj.keys().map(|&k| (k, Mark::New)).collect();
    for &start in adj.keys() {
        if mark[&start] != Mark::New {
            continue;
        }
        mark.insert(start, Mark::OnPath);
        let mut path: Vec<WorkId> = vec![start];
        let mut next_edge: Vec<usize> = vec![0];
        while let Some(&node) = path.last() {
            let i = *next_edge.last().expect("parallel to path");
            let succ = &adj[&node];
            if i < succ.len() {
                *next_edge.last_mut().expect("parallel to path") += 1;
                let to = succ[i];
                match mark.get(&to) {
                    None | Some(Mark::Finished) => {}
                    Some(Mark::OnPath) => {
                        let at = path.iter().position(|&n| n == to).expect("on path");
                        return Some(path[at..].to_vec());
                    }
                    Some(Mark::New) => {
                        mark.insert(to, Mark::OnPath);
                        path.push(to);
                        next_edge.push(0);
                    }
                }
            } else {
                mark.insert(node, Mark::Finished);
                path.pop();
                next_edge.pop();
            }
        }
    }
    None
}
```

`crates/work/src/invariants.rs` 测试模块**上面**：

```rust
//! 不变量（spec §4.2「crewd 写入时强制，违反即拒」，加上本计划补的几条，见计划「裁定」）。
//! 每条命令在草稿上改完后整份跑一遍；`WorkBook::restore` 读回时也跑一遍。返回第一条违反（按 W 号顺序，确定）。
use crate::graph::{adjacency, find_cycle};
use crate::ids::{AskId, EntryNo, Ts, WorkId};
use crate::model::{AskState, Blocker, BookState, Config, Status};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// 表里的键和项自己的 id 不一致 / 号码计数器落后于已发出的号。
    KeyMismatch(WorkId),
    CounterBehind,
    BlankTitle(WorkId),
    DuplicateRef(WorkId),
    UnknownItemRef {
        item: WorkId,
        missing: WorkId,
    },
    UnknownEntryRef {
        item: WorkId,
        entry: EntryNo,
    },
    UnknownAskRef {
        item: WorkId,
        ask: AskId,
    },
    AskForUnknownItem {
        ask: AskId,
        item: WorkId,
    },
    /// active / blocked 必须有负责人。
    OwnerMissing(WorkId),
    ActiveWithoutCheck(WorkId),
    CheckTooFar {
        item: WorkId,
        at: Ts,
        limit: Ts,
    },
    /// nextCheckAt 只许出现在有负责人的 open / active 项上。
    CheckNotAllowed(WorkId),
    DoneWithoutEvidence(WorkId),
    ActiveBeforeDeps {
        item: WorkId,
        dep: WorkId,
    },
    BlockedWithoutBlocker(WorkId),
    BlockerOnUnblocked(WorkId),
    /// 卡点指向已结束的问 / 已终结的项（应当已被清掉）。
    StaleBlocker {
        item: WorkId,
        blocker: Blocker,
    },
    /// 活着的项挂在已终结的父项下。
    LiveUnderTerminal {
        item: WorkId,
        parent: WorkId,
    },
    /// 活着的项依赖一个已 dropped 的项（永远等不到）。
    LiveOnDropped {
        item: WorkId,
        dep: WorkId,
    },
    /// 活着的项来自一条已裁定关闭的入口。
    LiveOnClosedEntry {
        item: WorkId,
        entry: EntryNo,
    },
    /// 待接受的转交挂在终结项 / 无主项上，或转给现负责人自己。
    StrayTransfer(WorkId),
    Cycle(Vec<WorkId>),
}

pub fn check(s: &BookState, cfg: &Config, now: Ts) -> Result<(), Violation> {
    if s.items
        .keys()
        .next_back()
        .is_some_and(|w| w.0 > s.last_item)
        || s.asks.keys().next_back().is_some_and(|h| h.0 > s.last_ask)
    {
        return Err(Violation::CounterBehind);
    }
    let horizon = now.plus(cfg.max_check_horizon_ms);
    for (key, it) in &s.items {
        let id = it.id;
        if *key != id {
            return Err(Violation::KeyMismatch(*key));
        }
        if it.title.trim().is_empty() {
            return Err(Violation::BlankTitle(id));
        }
        let unique_deps: BTreeSet<_> = it.depends_on.iter().collect();
        let unique_sources: BTreeSet<_> = it.sources.iter().collect();
        let unique_blockers: BTreeSet<_> = it.blocked_on.iter().collect();
        if unique_deps.len() != it.depends_on.len()
            || unique_sources.len() != it.sources.len()
            || unique_blockers.len() != it.blocked_on.len()
        {
            return Err(Violation::DuplicateRef(id));
        }
        for w in it.parent.iter().chain(&it.depends_on) {
            if !s.items.contains_key(w) {
                return Err(Violation::UnknownItemRef {
                    item: id,
                    missing: *w,
                });
            }
        }
        for e in &it.sources {
            if !s.entries.contains_key(e) {
                return Err(Violation::UnknownEntryRef {
                    item: id,
                    entry: *e,
                });
            }
        }
        for b in &it.blocked_on {
            match b {
                Blocker::Item(w) if !s.items.contains_key(w) => {
                    return Err(Violation::UnknownItemRef {
                        item: id,
                        missing: *w,
                    });
                }
                Blocker::Human(h) if !s.asks.contains_key(h) => {
                    return Err(Violation::UnknownAskRef { item: id, ask: *h });
                }
                _ => {}
            }
        }

        let live = it.status.is_live();
        if matches!(it.status, Status::Active | Status::Blocked) && it.owner.is_none() {
            return Err(Violation::OwnerMissing(id));
        }
        match it.next_check_at {
            None if it.status == Status::Active => {
                return Err(Violation::ActiveWithoutCheck(id));
            }
            Some(_)
                if it.owner.is_none() || !matches!(it.status, Status::Open | Status::Active) =>
            {
                return Err(Violation::CheckNotAllowed(id));
            }
            Some(at) if at > horizon => {
                return Err(Violation::CheckTooFar {
                    item: id,
                    at,
                    limit: horizon,
                });
            }
            _ => {}
        }
        if it.status == Status::Done && it.evidence.as_ref().is_none_or(|e| e.is_blank()) {
            return Err(Violation::DoneWithoutEvidence(id));
        }
        if it.status == Status::Active {
            for dep in &it.depends_on {
                if s.items[dep].status != Status::Done {
                    return Err(Violation::ActiveBeforeDeps {
                        item: id,
                        dep: *dep,
                    });
                }
            }
        }
        match (it.status, it.blocked_on.is_empty()) {
            (Status::Blocked, true) => return Err(Violation::BlockedWithoutBlocker(id)),
            (st, false) if st != Status::Blocked => return Err(Violation::BlockerOnUnblocked(id)),
            _ => {}
        }
        for b in &it.blocked_on {
            let stale = match b {
                Blocker::Item(w) => s.items[w].status.is_terminal(),
                Blocker::Human(h) => s.asks[h].state != AskState::Open,
            };
            if stale {
                return Err(Violation::StaleBlocker {
                    item: id,
                    blocker: *b,
                });
            }
        }
        if live {
            if let Some(p) = it.parent
                && s.items[&p].status.is_terminal()
            {
                return Err(Violation::LiveUnderTerminal {
                    item: id,
                    parent: p,
                });
            }
            for dep in &it.depends_on {
                if s.items[dep].status == Status::Dropped {
                    return Err(Violation::LiveOnDropped {
                        item: id,
                        dep: *dep,
                    });
                }
            }
            for e in &it.sources {
                if s.entries[e].closed.is_some() {
                    return Err(Violation::LiveOnClosedEntry {
                        item: id,
                        entry: *e,
                    });
                }
            }
        }
        if let Some(t) = &it.pending_transfer
            && (!live || it.owner.is_none() || it.owner.as_ref() == Some(&t.to))
        {
            return Err(Violation::StrayTransfer(id));
        }
    }
    for (h, ask) in &s.asks {
        if !s.items.contains_key(&ask.item) {
            return Err(Violation::AskForUnknownItem {
                ask: *h,
                item: ask.item,
            });
        }
    }
    match find_cycle(&adjacency(&s.items)) {
        Some(cycle) => Err(Violation::Cycle(cycle)),
        None => Ok(()),
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-work`
Expected: `test result: ok. 38 passed; 0 failed`

- [ ] **Step 5: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/work/src/lib.rs crates/work/src/graph.rs crates/work/src/invariants.rs
git diff --cached --stat
git commit -m "crewd-work: cycle detection over parent/depends/blocked edges, whole-book invariants

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: 账本与事务外壳 + 认领 / 建项 / 关联群（`book`）

`WorkBook` 持有 `BookState` + `Config` + 事件日志。`transact` 是所有命令的唯一入口：用 `max(now, clock)`（裁定 10）→ 克隆状态成草稿 → 命令改草稿 → 外壳统一维护 revision / updatedAt / createdAt 与「负责人操作即回报」（裁定 9）→ 整份 `invariants::check` → 换上、按序号发事件。命令只管自己的业务规则，结构性的约束（环、检查上限、子项未终结……）一律交给不变量。

`insert_batch` 是认领入口、建项、卡住拆分共用的建项器：批内 `ItemRef::Batch(j)` 只许引用排在前面的项；在**已存在**的项下面建子项要有该项负责人（或机长 / 人 / 调度层）的权限；worker 不能替别人指定负责人。`link_channel` 放在这里，是因为它是唯一不改状态的单项命令（spec §17：工作项不自动建群，负责人建好再关联）。

**Files:**
- Create: `crewd/crates/work/src/book.rs`、`crewd/crates/work/src/testkit.rs`
- Modify: `crewd/crates/work/src/lib.rs`

**Interfaces:**
- Consumes: Task 1–3 全部；`perm::{allowed, Action, Ctx}`、`invariants::{check, Violation}`。
- Produces（`crewd_work::book`）：`enum Rejected { Forbidden(Action), UnknownItem(WorkId), UnknownEntry(EntryNo), UnknownAsk(AskId), Terminal(WorkId), EntryClosed(EntryNo), Blank(&'static str), BatchRef(usize), OwnerRequired, NextCheckNotFuture, SameOwner, TransferPending(WorkId), NoPendingTransfer(WorkId), NotOwned(WorkId), AskNotOpen(AskId), NoDefault(AskId), DefaultNotDue(AskId), Invalid(&'static str), Invariant(Violation) }`；`struct Applied<T> { pub value: T, pub events: Vec<Event> }`；`WorkBook::{new, restore, state, config, crew, item, ask, entry, log, take_log, register_entry, claim_entry, create_item, link_channel}`（签名见「公开 API 一览」）。
- Produces（crate 内，后续任务用）：`pub(crate) fn transact<T>(&mut self, by: &Actor, now: Ts, f: impl FnOnce(&mut Draft) -> Result<T, Rejected>) -> Result<Applied<T>, Rejected>`；`pub(crate) struct Draft { pub(crate) s: BookState, pub(crate) now: Ts, pub(crate) by: Actor, .. }`，方法 `emit(EventKind)`、`permit(Action, Ctx) -> Result<(), Rejected>`、`item(WorkId) -> Result<&WorkItem, Rejected>`、`item_mut(WorkId) -> Result<&mut WorkItem, Rejected>`、`live(WorkId) -> Result<&WorkItem, Rejected>`（终结项 → `Terminal`）、`permit_on(Action, WorkId) -> Result<(), Rejected>`（上下文取该项负责人与待接受转交对象）、`insert_batch(Vec<NewItem>, fixed_parent: Option<WorkId>, extra_source: Option<EntryNo>, default_owner: Option<&SessionId>) -> Result<Vec<WorkId>, Rejected>`。**命令闭包里一律用 `d.now`，不用外面传进来的 `now`**（后者可能比账本时间早）。
- Produces（`testkit`，仅测试）：`T0`、`T1`（T0 + 1 分钟）、`sid(&str)`、`book()`（入口 #1 已在 T0 登记）、`claim(&mut WorkBook, who, &[titles]) -> Vec<WorkId>`（worker 在 T1 认领 #1）、`kind_name(&EventKind) -> String`、`kinds(&[Event]) -> Vec<String>`。

- [ ] **Step 1: 写失败的测试**

`crates/work/src/lib.rs`（整个文件）：

```rust
//! crewd 的工作跟进核心（M4a）：两层 Todo（入口 #N + 工作项 W<n> 的 DAG）、负责人、检查点、
//! 卡住只卡那一件、停下即检查、等人升级、行政巡检的纯判定（spec §4–§6、§8、§9）。
//! sans-IO：不读时钟、不碰文件 / 数据库 / tokio；时间一律由调用方传入（Unix 毫秒）。持久化与 MCP 接线归 M4b。
pub mod book;
pub mod events;
pub mod graph;
pub mod ids;
pub mod invariants;
pub mod model;
pub mod perm;
#[cfg(test)]
mod testkit;
```

`crates/work/src/testkit.rs`（整个文件）：

```rust
//! 单测共用的夹具（只在 `cfg(test)` 下编译）。
use crate::book::WorkBook;
use crate::events::{Event, EventKind};
use crate::ids::{CrewId, EntryNo, SessionId, Ts, WorkId};
use crate::model::{Actor, Config, NewItem};

pub(crate) const T0: Ts = Ts(1_700_000_000_000);
/// T0 之后一分钟。
pub(crate) const T1: Ts = Ts(1_700_000_060_000);

pub(crate) fn sid(s: &str) -> SessionId {
    SessionId::new(s)
}

/// 一个机组账本，入口 #1 已在 T0 登记。
pub(crate) fn book() -> WorkBook {
    let mut b = WorkBook::new(CrewId::new("crew-1"), Config::default());
    b.register_entry(T0, EntryNo(1)).unwrap();
    b
}

/// `who`（worker）在 T1 认领 #1，按 `titles` 建项（互不相连），返回 W 号。
pub(crate) fn claim(b: &mut WorkBook, who: &str, titles: &[&str]) -> Vec<WorkId> {
    let items = titles.iter().map(|t| NewItem::titled(t)).collect();
    b.claim_entry(&Actor::worker(who), T1, EntryNo(1), items)
        .unwrap()
        .value
}

pub(crate) fn kind_name(k: &EventKind) -> String {
    serde_json::to_value(k).unwrap()["type"]
        .as_str()
        .unwrap()
        .to_string()
}

pub(crate) fn kinds(events: &[Event]) -> Vec<String> {
    events.iter().map(|e| kind_name(&e.kind)).collect()
}
```

`crates/work/src/book.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::HOUR;
    use crate::model::{EntryClosure, Role};
    use crate::testkit::*;

    #[test]
    fn registering_an_entry_is_idempotent() {
        let mut b = WorkBook::new(CrewId::new("c"), Config::default());
        let first = b.register_entry(T0, EntryNo(1)).unwrap();
        assert!(first.value);
        assert_eq!(
            first.events,
            [Event {
                seq: 1,
                at: T0,
                by: Actor::System,
                kind: EventKind::EntryRegistered { entry: EntryNo(1) }
            }]
        );
        let again = b.register_entry(T0.plus(5), EntryNo(1)).unwrap();
        assert!(!again.value);
        assert!(again.events.is_empty());
        assert_eq!(b.entry(EntryNo(1)).unwrap().registered_at, T0);
        assert_eq!(b.log().len(), 1);
    }

    #[test]
    fn a_session_claim_makes_it_the_owner_of_every_item() {
        let mut b = book();
        let got = b
            .claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![
                    NewItem::titled("  schema  "),
                    NewItem::titled("ui").after(ItemRef::Batch(0)),
                    NewItem::titled("ui tests").under(ItemRef::Batch(1)),
                ],
            )
            .unwrap();
        assert_eq!(got.value, [WorkId(1), WorkId(2), WorkId(3)]);
        let w1 = b.item(WorkId(1)).unwrap();
        assert_eq!(w1.title, "schema", "标题去掉首尾空白");
        assert_eq!(w1.owner, Some(sid("a")));
        assert_eq!(w1.status, Status::Open);
        assert_eq!(w1.sources, [EntryNo(1)]);
        assert_eq!((w1.revision, w1.created_at, w1.updated_at), (1, T1, T1));
        assert_eq!(
            w1.owner_history,
            [OwnerChange {
                from: None,
                to: sid("a"),
                by: Actor::worker("a"),
                how: HowChanged::Claim,
                note: String::new(),
                at: T1
            }]
        );
        assert_eq!(b.item(WorkId(2)).unwrap().depends_on, [WorkId(1)]);
        assert_eq!(b.item(WorkId(3)).unwrap().parent, Some(WorkId(2)));
        assert_eq!(
            kinds(&got.events),
            [
                "item_created",
                "item_created",
                "item_created",
                "entry_claimed"
            ]
        );
        let seqs: Vec<u64> = got.events.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, [2, 3, 4, 5], "接着登记入口那条（seq 1）往下编");
        assert_eq!(
            got.events[3].kind,
            EventKind::EntryClaimed {
                entry: EntryNo(1),
                items: vec![WorkId(1), WorkId(2), WorkId(3)]
            }
        );
    }

    #[test]
    fn forward_batch_reference_is_rejected_and_consumes_no_number() {
        let mut b = book();
        let before = b.state().clone();
        let err = b
            .claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![
                    NewItem::titled("x").after(ItemRef::Batch(1)),
                    NewItem::titled("y"),
                ],
            )
            .unwrap_err();
        assert_eq!(err, Rejected::BatchRef(1));
        assert_eq!(b.state(), &before, "被拒的命令不动账本");
        let ok = b
            .claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![NewItem::titled("y")],
            )
            .unwrap();
        assert_eq!(ok.value, [WorkId(1)], "W 号没被浪费");
    }

    #[test]
    fn claim_needs_a_known_open_entry_and_at_least_one_item() {
        let mut b = book();
        assert_eq!(
            b.claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(9),
                vec![NewItem::titled("x")]
            ),
            Err(Rejected::UnknownEntry(EntryNo(9)))
        );
        assert_eq!(
            b.claim_entry(&Actor::worker("a"), T1, EntryNo(1), vec![]),
            Err(Rejected::Invalid("a claim needs at least one work item"))
        );
        assert_eq!(
            b.claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![NewItem::titled(" ")]
            ),
            Err(Rejected::Blank("title"))
        );
        let mut state = b.state().clone();
        state.entries.get_mut(&EntryNo(1)).unwrap().closed = Some(EntryClosure {
            by: Actor::Human,
            note: "不做了".into(),
            evidence: None,
            at: T0,
        });
        let mut closed = WorkBook::restore(state, Config::default(), T1).unwrap();
        assert_eq!(
            closed.claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![NewItem::titled("x")]
            ),
            Err(Rejected::EntryClosed(EntryNo(1)))
        );
    }

    #[test]
    fn dispatch_on_behalf_must_name_owners() {
        let mut b = book();
        assert_eq!(
            b.claim_entry(&Actor::System, T1, EntryNo(1), vec![NewItem::titled("x")]),
            Err(Rejected::OwnerRequired)
        );
        let got = b
            .claim_entry(
                &Actor::System,
                T1,
                EntryNo(1),
                vec![NewItem::titled("x").owned_by("a").check_at(T1)],
            )
            .unwrap();
        let w = b.item(got.value[0]).unwrap();
        assert_eq!(w.owner, Some(sid("a")));
        assert_eq!(w.next_check_at, Some(T1), "派活占位项可以「现在就查」");
        assert_eq!(w.owner_history[0].how, HowChanged::Assign);
        assert_eq!(w.owner_history[0].by, Actor::System);
    }

    #[test]
    fn workers_cannot_hand_work_to_others_and_admins_cannot_take_work() {
        let mut b = book();
        assert_eq!(
            b.claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![NewItem::titled("x").owned_by("b")]
            ),
            Err(Rejected::Forbidden(Action::Assign))
        );
        assert_eq!(
            b.claim_entry(
                &Actor::admin("d"),
                T1,
                EntryNo(1),
                vec![NewItem::titled("x")]
            ),
            Err(Rejected::Forbidden(Action::ClaimEntry))
        );
        let ok = b
            .claim_entry(
                &Actor::captain("c"),
                T1,
                EntryNo(1),
                vec![NewItem::titled("x").owned_by("b"), NewItem::titled("y")],
            )
            .unwrap();
        assert_eq!(b.item(ok.value[0]).unwrap().owner, Some(sid("b")));
        assert_eq!(
            b.item(ok.value[1]).unwrap().owner,
            Some(sid("c")),
            "机长也能接活（§13 Q9）"
        );
    }

    #[test]
    fn request_help_creates_an_unowned_item() {
        let mut b = book();
        let w = b
            .create_item(&Actor::worker("a"), T1, NewItem::titled("帮我查日志"))
            .unwrap();
        let it = b.item(w.value).unwrap();
        assert_eq!((it.owner.clone(), it.status), (None, Status::Open));
        assert!(it.owner_history.is_empty());
        assert_eq!(
            b.create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("x").after(ItemRef::Batch(0))
            ),
            Err(Rejected::BatchRef(0))
        );
        assert_eq!(
            b.create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("x").from_entry(EntryNo(4))
            ),
            Err(Rejected::UnknownEntry(EntryNo(4)))
        );
    }

    #[test]
    fn sub_items_need_the_parent_owner_or_the_captain() {
        let mut b = book();
        let parent = claim(&mut b, "a", &["parent"])[0];
        let child = NewItem::titled("child").under(ItemRef::Id(parent));
        assert_eq!(
            b.create_item(&Actor::worker("b"), T1, child.clone()),
            Err(Rejected::Forbidden(Action::CreateChild))
        );
        assert!(
            b.create_item(&Actor::worker("a"), T1, child.clone())
                .is_ok()
        );
        assert!(b.create_item(&Actor::captain("c"), T1, child).is_ok());
        assert_eq!(
            b.create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("x").under(ItemRef::Id(WorkId(99)))
            ),
            Err(Rejected::UnknownItem(WorkId(99)))
        );
    }

    #[test]
    fn child_depending_on_its_parent_is_a_cycle() {
        let mut b = book();
        let parent = claim(&mut b, "a", &["parent"])[0];
        let err = b
            .create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("child")
                    .under(ItemRef::Id(parent))
                    .after(ItemRef::Id(parent)),
            )
            .unwrap_err();
        assert_eq!(
            err,
            Rejected::Invariant(Violation::Cycle(vec![parent, WorkId(2)]))
        );
    }

    #[test]
    fn creation_check_times_are_validated() {
        let mut b = book();
        assert_eq!(
            b.create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("x").owned_by("a").check_at(T0)
            ),
            Err(Rejected::NextCheckNotFuture)
        );
        assert_eq!(
            b.create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("x")
                    .owned_by("a")
                    .check_at(T1.plus(3 * HOUR))
            ),
            Err(Rejected::Invariant(Violation::CheckTooFar {
                item: WorkId(1),
                at: T1.plus(3 * HOUR),
                limit: T1.plus(2 * HOUR)
            }))
        );
        assert_eq!(
            b.create_item(&Actor::worker("a"), T1, NewItem::titled("x").check_at(T1)),
            Err(Rejected::Invariant(Violation::CheckNotAllowed(WorkId(1)))),
            "没负责人的项不排检查"
        );
    }

    #[test]
    fn restore_validates_and_numbering_continues() {
        let mut b = book();
        claim(&mut b, "a", &["x", "y"]);
        let json = serde_json::to_string(b.state()).unwrap();
        let state: BookState = serde_json::from_str(&json).unwrap();
        assert_eq!(&state, b.state(), "JSON 往返不丢字段");
        let mut back = WorkBook::restore(state.clone(), Config::default(), T1).unwrap();
        let next = back
            .create_item(&Actor::worker("a"), T1, NewItem::titled("z"))
            .unwrap();
        assert_eq!(next.value, WorkId(3));
        assert_eq!(
            next.events[0].seq,
            b.state().last_seq + 1,
            "事件序号也接着编"
        );

        let mut broken = state;
        broken.items.get_mut(&WorkId(1)).unwrap().status = Status::Active;
        assert_eq!(
            WorkBook::restore(broken, Config::default(), T1).unwrap_err(),
            Violation::ActiveWithoutCheck(WorkId(1))
        );
    }

    #[test]
    fn channel_link_bumps_revision_and_is_owner_only() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        let ch = Some(ChannelId::new("ch-release"));
        assert_eq!(
            b.link_channel(&Actor::worker("b"), T1, w, ch.clone()),
            Err(Rejected::Forbidden(Action::LinkChannel))
        );
        let later = T1.plus(1_000);
        let got = b
            .link_channel(&Actor::worker("a"), later, w, ch.clone())
            .unwrap();
        assert_eq!(
            got.events[0].kind,
            EventKind::ChannelLinked {
                id: w,
                channel: ch.clone()
            }
        );
        let it = b.item(w).unwrap();
        assert_eq!(
            (it.channel.clone(), it.revision, it.updated_at),
            (ch, 2, later)
        );
        assert_eq!(it.created_at, T1);
        assert_eq!(
            b.link_channel(&Actor::worker("a"), later, WorkId(9), None),
            Err(Rejected::UnknownItem(WorkId(9)))
        );
    }

    #[test]
    fn clock_going_backwards_does_not_wedge_the_book() {
        let mut b = book();
        let far = T1.plus(2 * HOUR);
        b.create_item(
            &Actor::worker("a"),
            T1,
            NewItem::titled("x").owned_by("a").check_at(far),
        )
        .unwrap();
        // 墙钟往回跳了一小时：按传入的 now 算，上面那项的检查时间就「超过上限」了，每条命令都会被拒。
        let back = T1.plus(-HOUR);
        let got = b
            .create_item(&Actor::worker("a"), back, NewItem::titled("y"))
            .unwrap();
        assert_eq!(got.events[0].at, T1, "按账本见过的最晚时间算");
        assert_eq!(b.item(got.value).unwrap().created_at, T1);
        assert_eq!(b.state().clock, T1);
        assert!(WorkBook::restore(b.state().clone(), Config::default(), back).is_ok());
    }

    #[test]
    fn take_log_drains() {
        let mut b = book();
        claim(&mut b, "a", &["x"]);
        let drained = b.take_log();
        assert_eq!(drained.len(), 3);
        assert!(b.log().is_empty());
        assert_eq!(b.state().last_seq, 3);
    }

    #[test]
    fn roles_are_supplied_by_the_caller() {
        // 同一个 session 以 worker 身份不能替别人派活，以机长身份可以：角色由 crewd 认定后传入。
        let mut b = book();
        let as_worker = Actor::Session {
            id: sid("c"),
            role: Role::Worker,
        };
        assert!(
            b.claim_entry(
                &as_worker,
                T1,
                EntryNo(1),
                vec![NewItem::titled("x").owned_by("b")]
            )
            .is_err()
        );
        assert!(
            b.claim_entry(
                &Actor::captain("c"),
                T1,
                EntryNo(1),
                vec![NewItem::titled("x").owned_by("b")]
            )
            .is_ok()
        );
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-work`
Expected: 编译失败，找不到 `WorkBook` / `Rejected` / `Applied`。

- [ ] **Step 3: 实现**

`crates/work/src/book.rs` 测试模块**上面**：

```rust
//! 一个机组的工作账本（`WorkBook`）：命令的事务外壳，加上入口登记、认领入口、建项。
//! 每条命令在整份状态的副本（草稿）上改，改完整份跑不变量，过了才换上并产出事件；
//! 被拒的命令不改任何东西（号码也不消耗）。
use crate::events::{Event, EventKind};
use crate::ids::{AskId, ChannelId, CrewId, EntryNo, SessionId, Ts, WorkId};
use crate::invariants::{self, Violation};
use crate::model::{
    Actor, BookState, Config, EntryTodo, HowChanged, HumanAsk, ItemRef, NewItem, OwnerChange,
    Status, WorkItem,
};
use crate::perm::{self, Action, Ctx};

/// 命令被拒的原因。被拒 = 账本原样不动。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    Forbidden(Action),
    UnknownItem(WorkId),
    UnknownEntry(EntryNo),
    UnknownAsk(AskId),
    /// 该项已 done / dropped。
    Terminal(WorkId),
    EntryClosed(EntryNo),
    /// 必填的文字是空的（字段名）。
    Blank(&'static str),
    /// 同一批里引用了不在它前面的第几条。
    BatchRef(usize),
    /// 认领入口建出的项必须有负责人。
    OwnerRequired,
    NextCheckNotFuture,
    SameOwner,
    TransferPending(WorkId),
    NoPendingTransfer(WorkId),
    NotOwned(WorkId),
    AskNotOpen(AskId),
    NoDefault(AskId),
    DefaultNotDue(AskId),
    Invalid(&'static str),
    Invariant(Violation),
}

/// 成功的命令：返回值 + 这次产出的事件（已追加到账本的事件日志里）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied<T> {
    pub value: T,
    pub events: Vec<Event>,
}

#[derive(Debug, Clone)]
pub struct WorkBook {
    pub(crate) state: BookState,
    pub(crate) cfg: Config,
    log: Vec<Event>,
}

/// 草稿：命令在它上面改；`transact` 负责校验与提交。
pub(crate) struct Draft {
    pub(crate) s: BookState,
    pub(crate) now: Ts,
    pub(crate) by: Actor,
    events: Vec<EventKind>,
}

impl WorkBook {
    pub fn new(crew: CrewId, cfg: Config) -> WorkBook {
        WorkBook {
            state: BookState::empty(crew),
            cfg,
            log: Vec::new(),
        }
    }

    /// 从持久化读回（M4b）。整份跑一遍不变量，不合法就拒绝，不做任何修补。
    pub fn restore(state: BookState, cfg: Config, now: Ts) -> Result<WorkBook, Violation> {
        invariants::check(&state, &cfg, now.max(state.clock))?;
        Ok(WorkBook {
            state,
            cfg,
            log: Vec::new(),
        })
    }

    pub fn state(&self) -> &BookState {
        &self.state
    }
    pub fn config(&self) -> &Config {
        &self.cfg
    }
    pub fn crew(&self) -> &CrewId {
        &self.state.crew
    }
    pub fn item(&self, id: WorkId) -> Option<&WorkItem> {
        self.state.items.get(&id)
    }
    pub fn ask(&self, id: AskId) -> Option<&HumanAsk> {
        self.state.asks.get(&id)
    }
    pub fn entry(&self, no: EntryNo) -> Option<&EntryTodo> {
        self.state.entries.get(&no)
    }
    /// 自上次 `take_log` 以来的全部事件（按序号）。
    pub fn log(&self) -> &[Event] {
        &self.log
    }
    /// 取走事件日志（M4b 落库后调用）。
    pub fn take_log(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.log)
    }

    pub(crate) fn transact<T>(
        &mut self,
        by: &Actor,
        now: Ts,
        f: impl FnOnce(&mut Draft) -> Result<T, Rejected>,
    ) -> Result<Applied<T>, Rejected> {
        // 命令一律用 d.now（= max(now, clock)），不用调用方传进来的原值。
        let now = now.max(self.state.clock);
        let mut d = Draft {
            s: self.state.clone(),
            now,
            by: by.clone(),
            events: Vec::new(),
        };
        d.s.clock = now;
        let value = f(&mut d)?;
        let me = by.session();
        for (id, it) in d.s.items.iter_mut() {
            let old = self.state.items.get(id);
            if old != Some(&*it) && me.is_some() && it.owner.as_ref() == me {
                // 负责人对自己名下项的任何成功操作都算一次回报（裁定 9）。
                it.missed_checks = 0;
            }
            match old {
                Some(old) if old == it => {}
                Some(old) => {
                    it.revision = old.revision + 1;
                    it.updated_at = now;
                }
                None => {
                    it.revision = 1;
                    it.created_at = now;
                    it.updated_at = now;
                }
            }
        }
        invariants::check(&d.s, &self.cfg, now).map_err(Rejected::Invariant)?;
        let mut events = Vec::with_capacity(d.events.len());
        for kind in d.events {
            d.s.last_seq += 1;
            events.push(Event {
                seq: d.s.last_seq,
                at: now,
                by: by.clone(),
                kind,
            });
        }
        self.state = d.s;
        self.log.extend(events.iter().cloned());
        Ok(Applied { value, events })
    }

    /// crewd 收到一条新的入口 Todo（#N）时登记。重复登记不报错、不产出事件（重启后对账用）。
    pub fn register_entry(&mut self, now: Ts, no: EntryNo) -> Result<Applied<bool>, Rejected> {
        self.transact(&Actor::System, now, |d| {
            if d.s.entries.contains_key(&no) {
                return Ok(false);
            }
            d.s.entries.insert(
                no,
                EntryTodo {
                    no,
                    registered_at: d.now,
                    closed: None,
                },
            );
            d.emit(EventKind::EntryRegistered { entry: no });
            Ok(true)
        })
    }

    /// `claim_todo(#N, items[])`：把入口整理成工作项。session 认领时缺省自己当负责人；
    /// 调度层 / 人代为派活时每项都要写明负责人。
    pub fn claim_entry(
        &mut self,
        by: &Actor,
        now: Ts,
        entry: EntryNo,
        items: Vec<NewItem>,
    ) -> Result<Applied<Vec<WorkId>>, Rejected> {
        self.transact(by, now, |d| {
            d.permit(Action::ClaimEntry, Ctx::default())?;
            let e =
                d.s.entries
                    .get(&entry)
                    .ok_or(Rejected::UnknownEntry(entry))?;
            if e.closed.is_some() {
                return Err(Rejected::EntryClosed(entry));
            }
            if items.is_empty() {
                return Err(Rejected::Invalid("a claim needs at least one work item"));
            }
            let me = d.by.session().cloned();
            let ids = d.insert_batch(items, None, Some(entry), me.as_ref())?;
            if ids.iter().any(|w| d.s.items[w].owner.is_none()) {
                return Err(Rejected::OwnerRequired);
            }
            d.emit(EventKind::EntryClaimed {
                entry,
                items: ids.clone(),
            });
            Ok(ids)
        })
    }

    /// `work_item_create` / `request_help`：建一项。不写负责人 = 待认领，交调度层派。
    pub fn create_item(
        &mut self,
        by: &Actor,
        now: Ts,
        item: NewItem,
    ) -> Result<Applied<WorkId>, Rejected> {
        self.transact(by, now, |d| {
            d.permit(Action::CreateItem, Ctx::default())?;
            let ids = d.insert_batch(vec![item], None, None, None)?;
            Ok(ids[0])
        })
    }

    /// 把一个群关联到工作项（spec §17：工作项不自动建群，负责人自己建、自己关联）；`None` = 解除。
    pub fn link_channel(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
        channel: Option<ChannelId>,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            d.permit_on(Action::LinkChannel, id)?;
            d.item_mut(id)?.channel = channel.clone();
            d.emit(EventKind::ChannelLinked { id, channel });
            Ok(())
        })
    }
}

impl Draft {
    pub(crate) fn emit(&mut self, kind: EventKind) {
        self.events.push(kind);
    }

    pub(crate) fn permit(&self, action: Action, ctx: Ctx<'_>) -> Result<(), Rejected> {
        if perm::allowed(&self.by, action, ctx) {
            Ok(())
        } else {
            Err(Rejected::Forbidden(action))
        }
    }

    pub(crate) fn item(&self, id: WorkId) -> Result<&WorkItem, Rejected> {
        self.s.items.get(&id).ok_or(Rejected::UnknownItem(id))
    }

    pub(crate) fn item_mut(&mut self, id: WorkId) -> Result<&mut WorkItem, Rejected> {
        self.s.items.get_mut(&id).ok_or(Rejected::UnknownItem(id))
    }

    /// 取一项，必须还活着（不是 done / dropped）。
    pub(crate) fn live(&self, id: WorkId) -> Result<&WorkItem, Rejected> {
        let it = self.item(id)?;
        if it.status.is_terminal() {
            return Err(Rejected::Terminal(id));
        }
        Ok(it)
    }

    /// 对一项做 `action` 的权限：上下文取该项当前的负责人与待接受的转交对象。
    pub(crate) fn permit_on(&self, action: Action, id: WorkId) -> Result<(), Rejected> {
        let it = self.live(id)?;
        self.permit(
            action,
            Ctx {
                owner: it.owner.as_ref(),
                transfer_to: it.pending_transfer.as_ref().map(|t| &t.to),
                assignee: None,
            },
        )
    }

    /// 按批建项（认领、建项、拆分共用）。`fixed_parent` = 拆分时的原项；`extra_source` = 认领的入口；
    /// `default_owner` = 没写负责人时用谁。
    pub(crate) fn insert_batch(
        &mut self,
        batch: Vec<NewItem>,
        fixed_parent: Option<WorkId>,
        extra_source: Option<EntryNo>,
        default_owner: Option<&SessionId>,
    ) -> Result<Vec<WorkId>, Rejected> {
        let mut ids: Vec<WorkId> = Vec::with_capacity(batch.len());
        for n in batch {
            let title = n.title.trim().to_string();
            if title.is_empty() {
                return Err(Rejected::Blank("title"));
            }
            let parent = match (fixed_parent, n.parent) {
                (Some(_), Some(_)) => {
                    return Err(Rejected::Invalid(
                        "split items always hang under the blocked item",
                    ));
                }
                (Some(p), None) => Some(p),
                (None, Some(r)) => Some(self.resolve(r, &ids)?),
                (None, None) => None,
            };
            if let Some(p) = parent
                && fixed_parent.is_none()
                && !ids.contains(&p)
            {
                let owner = self.item(p)?.owner.clone();
                self.permit(
                    Action::CreateChild,
                    Ctx {
                        owner: owner.as_ref(),
                        ..Ctx::default()
                    },
                )?;
            }
            let mut depends_on = Vec::with_capacity(n.depends_on.len());
            for r in n.depends_on {
                depends_on.push(self.resolve(r, &ids)?);
            }
            depends_on.sort();
            depends_on.dedup();
            let mut sources = n.sources;
            sources.extend(extra_source);
            sources.sort();
            sources.dedup();
            if let Some(missing) = sources.iter().find(|e| !self.s.entries.contains_key(e)) {
                return Err(Rejected::UnknownEntry(*missing));
            }
            let owner = n.owner.or_else(|| default_owner.cloned());
            if let Some(o) = &owner
                && self.by.session() != Some(o)
            {
                self.permit(
                    Action::Assign,
                    Ctx {
                        assignee: Some(o),
                        ..Ctx::default()
                    },
                )?;
            }
            if n.next_check_at.is_some_and(|t| t < self.now) {
                return Err(Rejected::NextCheckNotFuture);
            }
            self.s.last_item += 1;
            let id = WorkId(self.s.last_item);
            let how = if self.by.session() == owner.as_ref() {
                HowChanged::Claim
            } else {
                HowChanged::Assign
            };
            let owner_history = owner
                .iter()
                .map(|o| OwnerChange {
                    from: None,
                    to: o.clone(),
                    by: self.by.clone(),
                    how,
                    note: String::new(),
                    at: self.now,
                })
                .collect();
            self.s.items.insert(
                id,
                WorkItem {
                    id,
                    title: title.clone(),
                    body: n.body,
                    parent,
                    depends_on: depends_on.clone(),
                    sources: sources.clone(),
                    owner: owner.clone(),
                    owner_history,
                    status: Status::Open,
                    blocked_on: Vec::new(),
                    next_check_at: n.next_check_at,
                    missed_checks: 0,
                    evidence: None,
                    drop_reason: None,
                    pending_transfer: None,
                    channel: n.channel,
                    last_report: None,
                    revision: 1,
                    created_at: self.now,
                    updated_at: self.now,
                },
            );
            self.emit(EventKind::ItemCreated {
                id,
                title,
                parent,
                depends_on,
                sources,
                owner,
            });
            ids.push(id);
        }
        Ok(ids)
    }

    fn resolve(&self, r: ItemRef, batch: &[WorkId]) -> Result<WorkId, Rejected> {
        match r {
            ItemRef::Id(w) if self.s.items.contains_key(&w) => Ok(w),
            ItemRef::Id(w) => Err(Rejected::UnknownItem(w)),
            ItemRef::Batch(j) => batch.get(j).copied().ok_or(Rejected::BatchRef(j)),
        }
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-work`
Expected: `test result: ok. 53 passed; 0 failed`

- [ ] **Step 5: 验这把尺子会红（Review Focus 1）**

```bash
cp crates/work/src/book.rs crates/work/src/book.rs.bak
sed -i '' 's/        let now = now.max(self.state.clock);/        let now = now;/' crates/work/src/book.rs
grep -c 'let now = now;' crates/work/src/book.rs
cargo test -p crewd-work 2>&1 | grep -E 'FAILED|test result'
cp crates/work/src/book.rs.bak crates/work/src/book.rs
cmp crates/work/src/book.rs crates/work/src/book.rs.bak && rm crates/work/src/book.rs.bak
```

Expected：`grep -c` 输出 `1`（变异确实打上了；是 0 就说明 sed 没匹配，这一步的读数无效）；只有 `book::tests::clock_going_backwards_does_not_wedge_the_book ... FAILED`，汇总 `52 passed; 1 failed`；还原后 `cmp` 无输出、备份删掉。

- [ ] **Step 6: 全量检查并提交**

```bash
./scripts/check.sh
git status --short
git add crates/work/src/lib.rs crates/work/src/book.rs crates/work/src/testkit.rs
git diff --cached --stat
git commit -m "crewd-work: WorkBook transaction shell, entry registration, claim/create/link-channel

Co-Authored-By: Claude <noreply@anthropic.com>"
```

`git status --short` 里不应再有 `book.rs.bak`。

---

### Task 5: 卡住只卡那一件 + 人回复（`block`）

`block` 必须同时给出卡点（新问 / 已有的开着的问 / 另一个活项）与「其余部分能否先推进」（`Continuation` 是必填参数，不是可选——spec §6 ②）；能推进就在同一个事务里拆出子项（parent = 原项、缺省负责人 = 原项负责人），这样 session 立刻有别的活干（spec §1：同一小时内推进别的工作）。新问在这里发 H 号（裁定 16），可附默认方案（延迟必须为正）。`release` 是清卡点的唯一实现（裁定 8），Task 6 的 `mark_done` 也用它。

**Files:**
- Create: `crewd/crates/work/src/block.rs`
- Modify: `crewd/crates/work/src/lib.rs`

**Interfaces:**
- Consumes: Task 4 的 `WorkBook::transact`、`Draft::{permit, permit_on, item, item_mut, insert_batch, emit}`、`testkit`。
- Produces（`crewd_work::block`）：`enum BlockOn { NewAsk { question: String, default: Option<DefaultOption> }, Ask(AskId), Item(WorkId) }`；`enum Continuation { Nothing, Split(Vec<NewItem>) }`；`struct BlockResult { pub asks: Vec<AskId>, pub split: Vec<WorkId> }`；`WorkBook::block`、`WorkBook::answer_ask`、`WorkBook::apply_default`（签名见「公开 API 一览」）；crate 内 `Draft::release(&mut self, blocker: Blocker) -> Vec<WorkId>`。

- [ ] **Step 1: 写失败的测试**

`crates/work/src/lib.rs`（整个文件）：

```rust
//! crewd 的工作跟进核心（M4a）：两层 Todo（入口 #N + 工作项 W<n> 的 DAG）、负责人、检查点、
//! 卡住只卡那一件、停下即检查、等人升级、行政巡检的纯判定（spec §4–§6、§8、§9）。
//! sans-IO：不读时钟、不碰文件 / 数据库 / tokio；时间一律由调用方传入（Unix 毫秒）。持久化与 MCP 接线归 M4b。
pub mod block;
pub mod book;
pub mod events;
pub mod graph;
pub mod ids;
pub mod invariants;
pub mod model;
pub mod perm;
#[cfg(test)]
mod testkit;
```

`crates/work/src/block.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{EntryNo, HOUR};
    use crate::invariants::Violation;
    use crate::model::ItemRef;
    use crate::testkit::*;

    fn ask(q: &str) -> BlockOn {
        BlockOn::NewAsk {
            question: q.into(),
            default: None,
        }
    }

    fn ask_with_default(q: &str, text: &str, after_ms: i64) -> BlockOn {
        BlockOn::NewAsk {
            question: q.into(),
            default: Some(DefaultOption {
                text: text.into(),
                after_ms,
            }),
        }
    }

    #[test]
    fn blocking_on_a_new_ask_opens_h1() {
        let mut b = book();
        let w = claim(&mut b, "a", &["迁移"])[0];
        let got = b
            .block(
                &Actor::worker("a"),
                T1,
                w,
                vec![ask(" 要保留旧格式吗？ ")],
                Continuation::Nothing,
            )
            .unwrap();
        assert_eq!(
            got.value,
            BlockResult {
                asks: vec![AskId(1)],
                split: vec![]
            }
        );
        let it = b.item(w).unwrap();
        assert_eq!(it.status, Status::Blocked);
        assert_eq!(it.blocked_on, [Blocker::Human(AskId(1))]);
        assert_eq!(it.next_check_at, None);
        let h = b.ask(AskId(1)).unwrap();
        assert_eq!(h.question, "要保留旧格式吗？");
        assert_eq!(
            (h.item, h.asked_at, h.state.clone()),
            (w, T1, AskState::Open)
        );
        assert_eq!(kinds(&got.events), ["ask_opened", "blocked"]);
    }

    #[test]
    fn block_input_is_validated_without_consuming_numbers() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        let a = Actor::worker("a");
        assert_eq!(
            b.block(&a, T1, w, vec![], Continuation::Nothing),
            Err(Rejected::Invalid("block needs at least one blocker"))
        );
        assert_eq!(
            b.block(&a, T1, w, vec![ask("  ")], Continuation::Nothing),
            Err(Rejected::Blank("question"))
        );
        assert_eq!(
            b.block(
                &a,
                T1,
                w,
                vec![ask_with_default("q", "A", 0)],
                Continuation::Nothing
            ),
            Err(Rejected::Invalid("a default option needs a positive delay"))
        );
        assert_eq!(
            b.block(
                &a,
                T1,
                w,
                vec![ask_with_default("q", " ", 5)],
                Continuation::Nothing
            ),
            Err(Rejected::Blank("default option"))
        );
        assert_eq!(
            b.block(
                &a,
                T1,
                w,
                vec![ask("q"), BlockOn::Item(WorkId(9))],
                Continuation::Nothing
            ),
            Err(Rejected::UnknownItem(WorkId(9)))
        );
        assert_eq!(
            b.block(
                &a,
                T1,
                w,
                vec![BlockOn::Ask(AskId(4))],
                Continuation::Nothing
            ),
            Err(Rejected::UnknownAsk(AskId(4)))
        );
        assert_eq!(
            b.block(&a, T1, w, vec![ask("q")], Continuation::Split(vec![])),
            Err(Rejected::Invalid("a split needs at least one item"))
        );
        assert_eq!(b.state().last_ask, 0, "被拒的命令不消耗 H 号");
        let ok = b
            .block(&a, T1, w, vec![ask("q")], Continuation::Nothing)
            .unwrap();
        assert_eq!(ok.value.asks, [AskId(1)]);
    }

    #[test]
    fn split_keeps_the_session_busy_on_the_rest() {
        let mut b = book();
        let w = claim(&mut b, "a", &["整件事"])[0];
        let got = b
            .block(
                &Actor::worker("a"),
                T1,
                w,
                vec![ask("q")],
                Continuation::Split(vec![
                    NewItem::titled("不依赖答案的部分"),
                    NewItem::titled("再下一步").after(ItemRef::Batch(0)),
                ]),
            )
            .unwrap();
        assert_eq!(got.value.split, [WorkId(2), WorkId(3)]);
        let part = b.item(WorkId(2)).unwrap();
        assert_eq!(part.parent, Some(w));
        assert_eq!(part.owner, Some(sid("a")), "缺省仍归原负责人");
        assert_eq!(part.status, Status::Open);
        assert!(part.sources.is_empty(), "拆出的子项不继承入口（裁定 6）");
        assert_eq!(b.item(WorkId(3)).unwrap().depends_on, [WorkId(2)]);
        assert_eq!(
            kinds(&got.events),
            ["ask_opened", "item_created", "item_created", "blocked"]
        );
    }

    #[test]
    fn captain_split_may_route_parts_to_someone_else() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        let got = b
            .block(
                &Actor::captain("c"),
                T1,
                w,
                vec![ask("q")],
                Continuation::Split(vec![NewItem::titled("part").owned_by("b")]),
            )
            .unwrap();
        assert_eq!(b.item(got.value.split[0]).unwrap().owner, Some(sid("b")));
        assert_eq!(
            b.block(
                &Actor::worker("a"),
                T1,
                w,
                vec![ask("q")],
                Continuation::Split(vec![NewItem::titled("p").under(ItemRef::Id(w))])
            ),
            Err(Rejected::Invalid(
                "split items always hang under the blocked item"
            ))
        );
    }

    #[test]
    fn only_owner_or_captain_blocks_and_only_owned_items() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        assert_eq!(
            b.block(
                &Actor::worker("b"),
                T1,
                w,
                vec![ask("q")],
                Continuation::Nothing
            ),
            Err(Rejected::Forbidden(Action::Block))
        );
        assert_eq!(
            b.block(
                &Actor::admin("d"),
                T1,
                w,
                vec![ask("q")],
                Continuation::Nothing
            ),
            Err(Rejected::Forbidden(Action::Block))
        );
        let unowned = b
            .create_item(&Actor::worker("a"), T1, NewItem::titled("orphan"))
            .unwrap()
            .value;
        assert_eq!(
            b.block(
                &Actor::captain("c"),
                T1,
                unowned,
                vec![ask("q")],
                Continuation::Nothing
            ),
            Err(Rejected::Invariant(Violation::OwnerMissing(unowned)))
        );
    }

    #[test]
    fn waiting_on_an_ancestor_is_a_cycle_but_on_a_child_is_fine() {
        let mut b = book();
        let parent = claim(&mut b, "a", &["parent"])[0];
        let child = b
            .create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("child")
                    .under(ItemRef::Id(parent))
                    .owned_by("a"),
            )
            .unwrap()
            .value;
        assert_eq!(
            b.block(
                &Actor::worker("a"),
                T1,
                child,
                vec![BlockOn::Item(parent)],
                Continuation::Nothing
            ),
            Err(Rejected::Invariant(Violation::Cycle(vec![parent, child])))
        );
        assert!(
            b.block(
                &Actor::worker("a"),
                T1,
                parent,
                vec![BlockOn::Item(child)],
                Continuation::Nothing
            )
            .is_ok()
        );
    }

    #[test]
    fn human_answer_wakes_the_owner_and_resumes_the_item() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        b.block(
            &Actor::worker("a"),
            T1,
            w,
            vec![ask("q")],
            Continuation::Nothing,
        )
        .unwrap();
        let later = T1.plus(HOUR);
        assert_eq!(
            b.answer_ask(&Actor::captain("c"), later, AskId(1), "yes"),
            Err(Rejected::Forbidden(Action::AnswerAsk)),
            "只有人能回答人类待确认"
        );
        assert_eq!(
            b.answer_ask(&Actor::Human, later, AskId(1), "  "),
            Err(Rejected::Blank("answer"))
        );
        let got = b.answer_ask(&Actor::Human, later, AskId(1), "要").unwrap();
        assert_eq!(got.value, [w]);
        assert_eq!(
            got.events[1].kind,
            EventKind::BlockerCleared {
                id: w,
                owner: Some(sid("a")),
                blocker: Blocker::Human(AskId(1)),
                status: Status::Active
            }
        );
        let it = b.item(w).unwrap();
        assert_eq!((it.status, it.next_check_at), (Status::Active, Some(later)));
        assert!(it.blocked_on.is_empty());
        assert_eq!(
            b.ask(AskId(1)).unwrap().state,
            AskState::Answered {
                text: "要".into(),
                at: later
            }
        );
        assert_eq!(
            b.answer_ask(&Actor::Human, later, AskId(1), "again"),
            Err(Rejected::AskNotOpen(AskId(1)))
        );
        assert_eq!(
            b.answer_ask(&Actor::Human, later, AskId(7), "x"),
            Err(Rejected::UnknownAsk(AskId(7)))
        );
    }

    #[test]
    fn answer_on_one_of_two_blockers_still_wakes_the_owner() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        b.block(
            &Actor::worker("a"),
            T1,
            w,
            vec![ask("q1"), ask("q2")],
            Continuation::Nothing,
        )
        .unwrap();
        let got = b.answer_ask(&Actor::Human, T1, AskId(2), "ok").unwrap();
        assert_eq!(
            got.events[1].kind,
            EventKind::BlockerCleared {
                id: w,
                owner: Some(sid("a")),
                blocker: Blocker::Human(AskId(2)),
                status: Status::Blocked
            }
        );
        assert_eq!(b.item(w).unwrap().blocked_on, [Blocker::Human(AskId(1))]);
    }

    #[test]
    fn unblocked_item_with_unfinished_deps_goes_back_to_open() {
        let mut b = book();
        let ids = b
            .claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![
                    NewItem::titled("dep"),
                    NewItem::titled("x").after(ItemRef::Batch(0)),
                ],
            )
            .unwrap()
            .value;
        b.block(
            &Actor::worker("a"),
            T1,
            ids[1],
            vec![ask("q")],
            Continuation::Nothing,
        )
        .unwrap();
        b.answer_ask(&Actor::Human, T1, AskId(1), "ok").unwrap();
        let it = b.item(ids[1]).unwrap();
        assert_eq!((it.status, it.next_check_at), (Status::Open, None));
    }

    #[test]
    fn one_answer_releases_every_item_waiting_on_it() {
        let mut b = book();
        let ids = claim(&mut b, "a", &["x", "y"]);
        b.block(
            &Actor::worker("a"),
            T1,
            ids[0],
            vec![ask("q")],
            Continuation::Nothing,
        )
        .unwrap();
        b.block(
            &Actor::worker("a"),
            T1,
            ids[1],
            vec![BlockOn::Ask(AskId(1))],
            Continuation::Nothing,
        )
        .unwrap();
        let got = b.answer_ask(&Actor::Human, T1, AskId(1), "ok").unwrap();
        assert_eq!(got.value, ids);
        assert_eq!(
            b.block(
                &Actor::worker("a"),
                T1,
                ids[0],
                vec![BlockOn::Ask(AskId(1))],
                Continuation::Nothing
            ),
            Err(Rejected::AskNotOpen(AskId(1)))
        );
    }

    #[test]
    fn default_option_applies_only_when_due_and_only_by_the_scheduler() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        b.block(
            &Actor::worker("a"),
            T1,
            w,
            vec![
                ask_with_default("用哪个端口？", "用 8080", 4 * HOUR),
                ask("q2"),
            ],
            Continuation::Nothing,
        )
        .unwrap();
        assert_eq!(
            b.apply_default(&Actor::System, T1.plus(4 * HOUR - 1), AskId(1)),
            Err(Rejected::DefaultNotDue(AskId(1)))
        );
        assert_eq!(
            b.apply_default(&Actor::Human, T1.plus(4 * HOUR), AskId(1)),
            Err(Rejected::Forbidden(Action::ApplyDefault))
        );
        assert_eq!(
            b.apply_default(&Actor::System, T1.plus(4 * HOUR), AskId(2)),
            Err(Rejected::NoDefault(AskId(2)))
        );
        let got = b
            .apply_default(&Actor::System, T1.plus(4 * HOUR), AskId(1))
            .unwrap();
        assert_eq!(got.value, [w]);
        assert_eq!(
            got.events[0].kind,
            EventKind::AskDefaulted {
                ask: AskId(1),
                text: "用 8080".into()
            }
        );
        assert_eq!(
            b.ask(AskId(1)).unwrap().state,
            AskState::Defaulted {
                at: T1.plus(4 * HOUR)
            }
        );
        assert_eq!(
            b.answer_ask(&Actor::Human, T1.plus(5 * HOUR), AskId(1), "晚了"),
            Err(Rejected::AskNotOpen(AskId(1)))
        );
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-work`
Expected: 编译失败，找不到 `BlockOn` / `Continuation` / `block`。

- [ ] **Step 3: 实现**

`crates/work/src/block.rs` 测试模块**上面**：

```rust
//! 卡住只卡那一件（spec §6）与人回复（spec §5「人回复」）：标 blocked、开人类待确认、拆出能先推进的部分；
//! 人回复 / 默认方案到点 / 卡它的项完成 → 清掉卡点、叫醒负责人。
use crate::book::{Applied, Draft, Rejected, WorkBook};
use crate::events::EventKind;
use crate::ids::{AskId, Ts, WorkId};
use crate::model::{Actor, AskState, Blocker, DefaultOption, HumanAsk, NewItem, Status, WaitTier};
use crate::perm::{Action, Ctx};

/// 卡在什么上。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockOn {
    /// 新开一条人类待确认（H 号由账本发）。
    NewAsk {
        question: String,
        default: Option<DefaultOption>,
    },
    /// 挂到一条已有的、还开着的待确认上。
    Ask(AskId),
    /// 等另一个工作项。
    Item(WorkId),
}

/// 「其余部分能否先推进」——必答（spec §6 ②）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Continuation {
    Nothing,
    /// 拆出子项（parent = 原项，缺省负责人 = 原项负责人）继续推进。
    Split(Vec<NewItem>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockResult {
    pub asks: Vec<AskId>,
    pub split: Vec<WorkId>,
}

impl WorkBook {
    /// `work_item_block(id, on, can_continue_split?)`。
    pub fn block(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
        on: Vec<BlockOn>,
        rest: Continuation,
    ) -> Result<Applied<BlockResult>, Rejected> {
        self.transact(by, now, |d| {
            d.permit_on(Action::Block, id)?;
            if on.is_empty() {
                return Err(Rejected::Invalid("block needs at least one blocker"));
            }
            let mut blockers = Vec::with_capacity(on.len());
            let mut asks = Vec::new();
            for b in on {
                match b {
                    BlockOn::NewAsk { question, default } => {
                        let question = question.trim().to_string();
                        if question.is_empty() {
                            return Err(Rejected::Blank("question"));
                        }
                        if let Some(opt) = &default {
                            if opt.text.trim().is_empty() {
                                return Err(Rejected::Blank("default option"));
                            }
                            if opt.after_ms <= 0 {
                                return Err(Rejected::Invalid(
                                    "a default option needs a positive delay",
                                ));
                            }
                        }
                        d.s.last_ask += 1;
                        let h = AskId(d.s.last_ask);
                        d.s.asks.insert(
                            h,
                            HumanAsk {
                                id: h,
                                item: id,
                                asked_by: d.by.clone(),
                                question: question.clone(),
                                default: default.clone(),
                                asked_at: d.now,
                                state: AskState::Open,
                                escalated: WaitTier::Fresh,
                            },
                        );
                        d.emit(EventKind::AskOpened {
                            ask: h,
                            item: id,
                            question,
                            default,
                        });
                        blockers.push(Blocker::Human(h));
                        asks.push(h);
                    }
                    BlockOn::Ask(h) => {
                        let ask = d.s.asks.get(&h).ok_or(Rejected::UnknownAsk(h))?;
                        if ask.state != AskState::Open {
                            return Err(Rejected::AskNotOpen(h));
                        }
                        blockers.push(Blocker::Human(h));
                    }
                    BlockOn::Item(w) => {
                        if d.item(w)?.status.is_terminal() {
                            return Err(Rejected::Terminal(w));
                        }
                        blockers.push(Blocker::Item(w));
                    }
                }
            }
            let split = match rest {
                Continuation::Nothing => Vec::new(),
                Continuation::Split(items) => {
                    if items.is_empty() {
                        return Err(Rejected::Invalid("a split needs at least one item"));
                    }
                    let owner = d.item(id)?.owner.clone();
                    d.insert_batch(items, Some(id), None, owner.as_ref())?
                }
            };
            let it = d.item_mut(id)?;
            for b in &blockers {
                if !it.blocked_on.contains(b) {
                    it.blocked_on.push(*b);
                }
            }
            it.blocked_on.sort();
            it.status = Status::Blocked;
            it.next_check_at = None;
            d.emit(EventKind::Blocked { id, on: blockers });
            Ok(BlockResult { asks, split })
        })
    }

    /// 人回复了 H<n>：直接唤醒卡在它上面的所有项的负责人（看事件里的 `BlockerCleared.owner`）。
    /// 返回卡点被清掉的项。
    pub fn answer_ask(
        &mut self,
        by: &Actor,
        now: Ts,
        ask: AskId,
        text: &str,
    ) -> Result<Applied<Vec<WorkId>>, Rejected> {
        self.transact(by, now, |d| {
            let now = d.now;
            d.permit(Action::AnswerAsk, Ctx::default())?;
            let text = text.trim().to_string();
            if text.is_empty() {
                return Err(Rejected::Blank("answer"));
            }
            let a = d.s.asks.get_mut(&ask).ok_or(Rejected::UnknownAsk(ask))?;
            if a.state != AskState::Open {
                return Err(Rejected::AskNotOpen(ask));
            }
            a.state = AskState::Answered {
                text: text.clone(),
                at: now,
            };
            d.emit(EventKind::AskAnswered { ask, text });
            Ok(d.release(Blocker::Human(ask)))
        })
    }

    /// 默认方案到点（调度层调用）：按提问时附的默认方案放行。到点后的动作照常走权限审批，
    /// 触发审批框就不自动执行、转回人（§13 Q2）——那是 M4b 叫醒负责人时写给它的话，不在这里判。
    pub fn apply_default(
        &mut self,
        by: &Actor,
        now: Ts,
        ask: AskId,
    ) -> Result<Applied<Vec<WorkId>>, Rejected> {
        self.transact(by, now, |d| {
            let now = d.now;
            d.permit(Action::ApplyDefault, Ctx::default())?;
            let a = d.s.asks.get_mut(&ask).ok_or(Rejected::UnknownAsk(ask))?;
            if a.state != AskState::Open {
                return Err(Rejected::AskNotOpen(ask));
            }
            let opt = a.default.clone().ok_or(Rejected::NoDefault(ask))?;
            if a.asked_at.plus(opt.after_ms) > now {
                return Err(Rejected::DefaultNotDue(ask));
            }
            a.state = AskState::Defaulted { at: now };
            d.emit(EventKind::AskDefaulted {
                ask,
                text: opt.text,
            });
            Ok(d.release(Blocker::Human(ask)))
        })
    }
}

impl Draft {
    /// 从所有项的 blockedOn 里去掉 `blocker`。卡点清空的项：依赖已齐 → active 且 nextCheckAt = now
    /// （下一轮检查点立刻叫醒负责人）；依赖没齐 → open。每个受影响的项都产出一条 `BlockerCleared`。
    pub(crate) fn release(&mut self, blocker: Blocker) -> Vec<WorkId> {
        let hit: Vec<WorkId> = self
            .s
            .items
            .values()
            .filter(|it| it.blocked_on.contains(&blocker))
            .map(|it| it.id)
            .collect();
        for id in &hit {
            let deps_done = self.s.items[id]
                .depends_on
                .iter()
                .all(|dep| self.s.items[dep].status == Status::Done);
            let now = self.now;
            let it = self.s.items.get_mut(id).expect("collected above");
            it.blocked_on.retain(|b| *b != blocker);
            if it.blocked_on.is_empty() {
                if deps_done {
                    it.status = Status::Active;
                    it.next_check_at = Some(now);
                } else {
                    it.status = Status::Open;
                    it.next_check_at = None;
                }
            }
            let (owner, status) = (it.owner.clone(), it.status);
            self.emit(EventKind::BlockerCleared {
                id: *id,
                owner,
                blocker,
                status,
            });
        }
        hit
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-work`
Expected: `test result: ok. 64 passed; 0 failed`

- [ ] **Step 5: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/work/src/lib.rs crates/work/src/block.rs
git diff --cached --stat
git commit -m "crewd-work: block on human ask or item with mandatory split answer; human reply and default option release blockers

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 6: 回报、完成、放弃、改链接、入口裁定（`progress`）

`report` 是 `work_item_update` 的核心：note 必填、下次检查必须晚于 now（裁定 14）；改到 active 时缺检查时间、依赖未齐、排得太远，都由不变量拒。`mark_done` 证据必填，并放行卡在它上面的项。`drop_item` 区分事实性放弃（重复 / 被取代，必须带证据，行政处可做）和业务放弃（只有负责人 / 机长 / 人）；还有活着的子项、依赖者、等它的项时由不变量拒。`close_entry` 是 spec §4.1「交机长 / 人裁定」的落点；带证据 = 事实关闭（行政处可做）。testkit 加 `activate`，后面的任务都用它。

**Files:**
- Create: `crewd/crates/work/src/progress.rs`
- Modify: `crewd/crates/work/src/lib.rs`、`crewd/crates/work/src/testkit.rs`

**Interfaces:**
- Consumes: Task 4 的事务与 `Draft` 方法；Task 5 的 `Draft::release`、`BlockOn`、`Continuation`。
- Produces（`crewd_work::progress`）：`enum Progress { Open, Active }`；`struct Report { pub status: Option<Progress>, pub next_check_at: Option<Ts>, pub note: String }`；`WorkBook::{report, mark_done, drop_item, relink, close_entry}`（签名见「公开 API 一览」）。
- Produces（`testkit`）：`activate(b: &mut WorkBook, who: &str, id: WorkId, at: Ts)`（worker `who` 在 `at` 把 `id` 改成 active，下次检查 `at + 1h`，note「开工」）。

- [ ] **Step 1: 写失败的测试**

`crates/work/src/lib.rs`（整个文件）：

```rust
//! crewd 的工作跟进核心（M4a）：两层 Todo（入口 #N + 工作项 W<n> 的 DAG）、负责人、检查点、
//! 卡住只卡那一件、停下即检查、等人升级、行政巡检的纯判定（spec §4–§6、§8、§9）。
//! sans-IO：不读时钟、不碰文件 / 数据库 / tokio；时间一律由调用方传入（Unix 毫秒）。持久化与 MCP 接线归 M4b。
pub mod block;
pub mod book;
pub mod events;
pub mod graph;
pub mod ids;
pub mod invariants;
pub mod model;
pub mod perm;
pub mod progress;
#[cfg(test)]
mod testkit;
```

`crates/work/src/testkit.rs`（整个文件，替换 Task 4 的版本：import 多了 `HOUR` 与 `progress::{Progress, Report}`，末尾多了 `activate`）：

```rust
//! 单测共用的夹具（只在 `cfg(test)` 下编译）。
use crate::book::WorkBook;
use crate::events::{Event, EventKind};
use crate::ids::{CrewId, EntryNo, HOUR, SessionId, Ts, WorkId};
use crate::model::{Actor, Config, NewItem};
use crate::progress::{Progress, Report};

pub(crate) const T0: Ts = Ts(1_700_000_000_000);
/// T0 之后一分钟。
pub(crate) const T1: Ts = Ts(1_700_000_060_000);

pub(crate) fn sid(s: &str) -> SessionId {
    SessionId::new(s)
}

/// 一个机组账本，入口 #1 已在 T0 登记。
pub(crate) fn book() -> WorkBook {
    let mut b = WorkBook::new(CrewId::new("crew-1"), Config::default());
    b.register_entry(T0, EntryNo(1)).unwrap();
    b
}

/// `who`（worker）在 T1 认领 #1，按 `titles` 建项（互不相连），返回 W 号。
pub(crate) fn claim(b: &mut WorkBook, who: &str, titles: &[&str]) -> Vec<WorkId> {
    let items = titles.iter().map(|t| NewItem::titled(t)).collect();
    b.claim_entry(&Actor::worker(who), T1, EntryNo(1), items)
        .unwrap()
        .value
}

pub(crate) fn kind_name(k: &EventKind) -> String {
    serde_json::to_value(k).unwrap()["type"]
        .as_str()
        .unwrap()
        .to_string()
}

pub(crate) fn kinds(events: &[Event]) -> Vec<String> {
    events.iter().map(|e| kind_name(&e.kind)).collect()
}

/// `who` 在 `at` 把 `id` 改成 active，下次检查在 `at` 之后一小时。
pub(crate) fn activate(b: &mut WorkBook, who: &str, id: WorkId, at: Ts) {
    b.report(
        &Actor::worker(who),
        at,
        id,
        Report {
            status: Some(Progress::Active),
            next_check_at: Some(at.plus(HOUR)),
            note: "开工".into(),
        },
    )
    .unwrap();
}
```

`crates/work/src/progress.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{BlockOn, Continuation};
    use crate::ids::{AskId, HOUR, MINUTE};
    use crate::invariants::Violation;
    use crate::model::{DropKind, ItemRef, NewItem};
    use crate::testkit::*;

    fn report(status: Option<Progress>, next: Option<Ts>, note: &str) -> Report {
        Report {
            status,
            next_check_at: next,
            note: note.into(),
        }
    }

    fn commit() -> Evidence {
        Evidence::Commit("abc1234".into())
    }

    fn not_needed() -> DropReason {
        DropReason {
            kind: DropKind::NotNeeded,
            note: "需求取消".into(),
            evidence: None,
        }
    }

    #[test]
    fn reporting_starts_work_with_a_next_check() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        let at = T1.plus(MINUTE);
        let got = b
            .report(
                &Actor::worker("a"),
                at,
                w,
                report(Some(Progress::Active), Some(at.plus(HOUR)), "先写 schema"),
            )
            .unwrap();
        assert_eq!(
            got.events[0].kind,
            EventKind::Reported {
                id: w,
                from: Status::Open,
                to: Status::Active,
                next_check_at: Some(at.plus(HOUR)),
                note: "先写 schema".into()
            }
        );
        let it = b.item(w).unwrap();
        assert_eq!(
            (it.status, it.next_check_at),
            (Status::Active, Some(at.plus(HOUR)))
        );
        assert_eq!(
            it.last_report,
            Some(Note {
                text: "先写 schema".into(),
                at
            })
        );
        assert_eq!((it.revision, it.updated_at), (2, at));
    }

    #[test]
    fn report_rules() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        let a = Actor::worker("a");
        assert_eq!(
            b.report(&a, T1, w, report(Some(Progress::Active), None, "go")),
            Err(Rejected::Invariant(Violation::ActiveWithoutCheck(w)))
        );
        assert_eq!(
            b.report(&a, T1, w, report(Some(Progress::Active), Some(T1), "go")),
            Err(Rejected::NextCheckNotFuture)
        );
        assert_eq!(
            b.report(
                &a,
                T1,
                w,
                report(Some(Progress::Active), Some(T1.plus(2 * HOUR + 1)), "go")
            ),
            Err(Rejected::Invariant(Violation::CheckTooFar {
                item: w,
                at: T1.plus(2 * HOUR + 1),
                limit: T1.plus(2 * HOUR)
            }))
        );
        assert_eq!(
            b.report(&a, T1, w, report(None, None, " ")),
            Err(Rejected::Blank("note"))
        );
        assert_eq!(
            b.report(&Actor::worker("b"), T1, w, report(None, None, "x")),
            Err(Rejected::Forbidden(Action::Report))
        );
        assert_eq!(
            b.report(&Actor::admin("d"), T1, w, report(None, None, "x")),
            Err(Rejected::Forbidden(Action::Report)),
            "行政处不改业务状态"
        );
        assert!(
            b.report(
                &Actor::captain("c"),
                T1,
                w,
                report(Some(Progress::Active), Some(T1.plus(HOUR)), "机长代改")
            )
            .is_ok()
        );
    }

    #[test]
    fn active_requires_finished_dependencies() {
        let mut b = book();
        let ids = b
            .claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![
                    NewItem::titled("dep"),
                    NewItem::titled("x").after(ItemRef::Batch(0)),
                ],
            )
            .unwrap()
            .value;
        let go = report(Some(Progress::Active), Some(T1.plus(HOUR)), "go");
        assert_eq!(
            b.report(&Actor::worker("a"), T1, ids[1], go.clone()),
            Err(Rejected::Invariant(Violation::ActiveBeforeDeps {
                item: ids[1],
                dep: ids[0]
            }))
        );
        b.mark_done(&Actor::worker("a"), T1, ids[0], commit())
            .unwrap();
        assert!(b.report(&Actor::worker("a"), T1, ids[1], go).is_ok());
    }

    #[test]
    fn manual_resume_from_blocked_clears_blockers_but_keeps_the_ask() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        b.block(
            &Actor::worker("a"),
            T1,
            w,
            vec![BlockOn::NewAsk {
                question: "q".into(),
                default: None,
            }],
            Continuation::Nothing,
        )
        .unwrap();
        assert_eq!(
            b.report(
                &Actor::worker("a"),
                T1,
                w,
                report(None, Some(T1.plus(HOUR)), "x")
            ),
            Err(Rejected::Invariant(Violation::CheckNotAllowed(w))),
            "blocked 项不排检查"
        );
        b.report(
            &Actor::worker("a"),
            T1,
            w,
            report(Some(Progress::Active), Some(T1.plus(HOUR)), "绕过去了"),
        )
        .unwrap();
        assert!(b.item(w).unwrap().blocked_on.is_empty());
        assert!(b.ask(AskId(1)).is_some());
    }

    #[test]
    fn done_needs_evidence_and_finished_children() {
        let mut b = book();
        let w = claim(&mut b, "a", &["parent"])[0];
        let child = b
            .create_item(
                &Actor::worker("a"),
                T1,
                NewItem::titled("child").under(ItemRef::Id(w)).owned_by("a"),
            )
            .unwrap()
            .value;
        assert_eq!(
            b.mark_done(&Actor::worker("a"), T1, w, Evidence::Text("  ".into())),
            Err(Rejected::Blank("evidence"))
        );
        assert_eq!(
            b.mark_done(&Actor::worker("a"), T1, w, commit()),
            Err(Rejected::Invariant(Violation::LiveUnderTerminal {
                item: child,
                parent: w
            }))
        );
        b.drop_item(&Actor::worker("a"), T1, child, not_needed())
            .unwrap();
        let got = b.mark_done(&Actor::worker("a"), T1, w, commit()).unwrap();
        assert_eq!(kinds(&got.events), ["done"]);
        assert_eq!(b.item(w).unwrap().evidence, Some(commit()));
        assert_eq!(
            b.mark_done(&Actor::worker("a"), T1, w, commit()),
            Err(Rejected::Terminal(w))
        );
    }

    #[test]
    fn finishing_an_item_releases_whoever_waited_on_it() {
        let mut b = book();
        let ids = claim(&mut b, "a", &["blocker", "waiter"]);
        b.block(
            &Actor::worker("a"),
            T1,
            ids[1],
            vec![BlockOn::Item(ids[0])],
            Continuation::Nothing,
        )
        .unwrap();
        let later = T1.plus(HOUR);
        let got = b
            .mark_done(&Actor::worker("a"), later, ids[0], commit())
            .unwrap();
        assert_eq!(got.value, [ids[1]]);
        assert_eq!(kinds(&got.events), ["done", "blocker_cleared"]);
        let it = b.item(ids[1]).unwrap();
        assert_eq!((it.status, it.next_check_at), (Status::Active, Some(later)));
    }

    #[test]
    fn admin_closes_only_with_evidence() {
        let mut b = book();
        let ids = claim(&mut b, "a", &["x", "y"]);
        let admin = Actor::admin("d");
        assert_eq!(
            b.drop_item(&admin, T1, ids[0], not_needed()),
            Err(Rejected::Forbidden(Action::DropDecision))
        );
        let dup = |evidence| DropReason {
            kind: DropKind::Duplicate,
            note: "与 W2 重复".into(),
            evidence,
        };
        assert_eq!(
            b.drop_item(&admin, T1, ids[0], dup(None)),
            Err(Rejected::Blank("evidence"))
        );
        b.drop_item(
            &admin,
            T1,
            ids[0],
            dup(Some(Evidence::Url("https://chat/123".into()))),
        )
        .unwrap();
        assert_eq!(b.item(ids[0]).unwrap().status, Status::Dropped);
        b.mark_done(&admin, T1, ids[1], commit()).unwrap();
        assert_eq!(b.item(ids[1]).unwrap().status, Status::Done);
    }

    #[test]
    fn cannot_drop_what_others_still_need() {
        let mut b = book();
        let ids = b
            .claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![
                    NewItem::titled("dep"),
                    NewItem::titled("x").after(ItemRef::Batch(0)),
                ],
            )
            .unwrap()
            .value;
        assert_eq!(
            b.drop_item(&Actor::worker("a"), T1, ids[0], not_needed()),
            Err(Rejected::Invariant(Violation::LiveOnDropped {
                item: ids[1],
                dep: ids[0]
            }))
        );
        b.relink(&Actor::worker("a"), T1, ids[1], None, vec![])
            .unwrap();
        assert!(
            b.drop_item(&Actor::worker("a"), T1, ids[0], not_needed())
                .is_ok()
        );
    }

    #[test]
    fn relink_is_checked_for_cycles_and_readiness() {
        let mut b = book();
        let ids = claim(&mut b, "a", &["x", "y"]);
        b.relink(&Actor::worker("a"), T1, ids[0], None, vec![ids[1]])
            .unwrap();
        assert_eq!(
            b.relink(&Actor::admin("d"), T1, ids[1], None, vec![ids[0]]),
            Err(Rejected::Invariant(Violation::Cycle(vec![ids[0], ids[1]])))
        );
        assert_eq!(
            b.relink(&Actor::worker("a"), T1, ids[1], Some(WorkId(9)), vec![]),
            Err(Rejected::UnknownItem(WorkId(9)))
        );
        activate(&mut b, "a", ids[1], T1);
        let got = b
            .relink(&Actor::admin("d"), T1, ids[1], Some(ids[0]), vec![])
            .unwrap();
        assert_eq!(
            got.events[0].kind,
            EventKind::Relinked {
                id: ids[1],
                parent: Some(ids[0]),
                depends_on: vec![]
            }
        );
    }

    #[test]
    fn entry_ruling_closes_it_for_good() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        assert_eq!(
            b.close_entry(&Actor::captain("c"), T1, EntryNo(1), "不做了", None),
            Err(Rejected::Invariant(Violation::LiveOnClosedEntry {
                item: w,
                entry: EntryNo(1)
            }))
        );
        b.drop_item(&Actor::worker("a"), T1, w, not_needed())
            .unwrap();
        assert_eq!(
            b.close_entry(&Actor::admin("d"), T1, EntryNo(1), "不做了", None),
            Err(Rejected::Forbidden(Action::CloseEntryDecision))
        );
        assert_eq!(
            b.close_entry(&Actor::captain("c"), T1, EntryNo(1), " ", None),
            Err(Rejected::Blank("note"))
        );
        b.close_entry(&Actor::captain("c"), T1, EntryNo(1), "不做了", None)
            .unwrap();
        assert_eq!(
            b.close_entry(&Actor::Human, T1, EntryNo(1), "again", None),
            Err(Rejected::EntryClosed(EntryNo(1)))
        );
        assert_eq!(
            b.claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![NewItem::titled("y")]
            ),
            Err(Rejected::EntryClosed(EntryNo(1)))
        );
        b.register_entry(T1, EntryNo(2)).unwrap();
        assert!(
            b.close_entry(
                &Actor::admin("d"),
                T1,
                EntryNo(2),
                "已在 #1 做完",
                Some(Evidence::Commit("abc".into()))
            )
            .is_ok()
        );
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-work`
Expected: 编译失败，找不到 `Progress` / `Report` / `report` / `mark_done`。

- [ ] **Step 3: 实现**

`crates/work/src/progress.rs` 测试模块**上面**：

```rust
//! 推进与收尾：回报进度 + 下次检查（`work_item_update`）、带证据完成、放弃、改链接、入口裁定关闭。
use crate::book::{Applied, Rejected, WorkBook};
use crate::events::EventKind;
use crate::ids::{EntryNo, Ts, WorkId};
use crate::model::{Actor, Blocker, DropReason, EntryClosure, Evidence, Note, Status};
use crate::perm::{Action, Ctx};

/// 回报时可以改到的状态（done / dropped / blocked 各有专门的命令）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Open,
    Active,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub status: Option<Progress>,
    /// 必须晚于 now，且不超过 now + 上限（默认 2h）。
    pub next_check_at: Option<Ts>,
    /// 进展 / 下一步（必填；注入「你名下的工作项」时显示）。
    pub note: String,
}

impl WorkBook {
    pub fn report(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
        r: Report,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            let now = d.now;
            d.permit_on(Action::Report, id)?;
            let note = r.note.trim().to_string();
            if note.is_empty() {
                return Err(Rejected::Blank("note"));
            }
            if r.next_check_at.is_some_and(|t| t <= now) {
                return Err(Rejected::NextCheckNotFuture);
            }
            let it = d.item_mut(id)?;
            let from = it.status;
            if let Some(p) = r.status {
                // 从 blocked 手动改回 open / active = 不再等；问题本身留着，人仍可回答。
                it.blocked_on.clear();
                it.status = match p {
                    Progress::Open => Status::Open,
                    Progress::Active => Status::Active,
                };
            }
            if let Some(t) = r.next_check_at {
                it.next_check_at = Some(t);
            }
            it.last_report = Some(Note {
                text: note.clone(),
                at: now,
            });
            let (to, next_check_at) = (it.status, it.next_check_at);
            d.emit(EventKind::Reported {
                id,
                from,
                to,
                next_check_at,
                note,
            });
            Ok(())
        })
    }

    /// 完成，证据必填。卡在这一项上的其它项随之放行；返回被放行的项。
    pub fn mark_done(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
        evidence: Evidence,
    ) -> Result<Applied<Vec<WorkId>>, Rejected> {
        self.transact(by, now, |d| {
            d.permit_on(Action::Done, id)?;
            if evidence.is_blank() {
                return Err(Rejected::Blank("evidence"));
            }
            let it = d.item_mut(id)?;
            it.status = Status::Done;
            it.evidence = Some(evidence.clone());
            it.next_check_at = None;
            it.pending_transfer = None;
            it.blocked_on.clear();
            d.emit(EventKind::Done { id, evidence });
            Ok(d.release(Blocker::Item(id)))
        })
    }

    /// 放弃。重复 / 被取代 = 事实性关闭（行政处可做，必须带证据）；不做了 = 业务决定。
    /// 还有活着的子项、依赖它的项、卡在它上面的项时被拒（不变量）。
    pub fn drop_item(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
        reason: DropReason,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            let action = if reason.kind.is_factual() {
                Action::DropFactual
            } else {
                Action::DropDecision
            };
            d.permit_on(action, id)?;
            if reason.note.trim().is_empty() {
                return Err(Rejected::Blank("note"));
            }
            if reason.kind.is_factual() && reason.evidence.as_ref().is_none_or(|e| e.is_blank()) {
                return Err(Rejected::Blank("evidence"));
            }
            let it = d.item_mut(id)?;
            it.status = Status::Dropped;
            it.drop_reason = Some(reason.clone());
            it.next_check_at = None;
            it.pending_transfer = None;
            it.blocked_on.clear();
            d.emit(EventKind::Dropped { id, reason });
            Ok(())
        })
    }

    /// 改链接（整份替换 parent 与 dependsOn）。成环、依赖未齐却 active 等由不变量拒。
    pub fn relink(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
        parent: Option<WorkId>,
        depends_on: Vec<WorkId>,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            d.permit_on(Action::Relink, id)?;
            for w in parent.iter().chain(&depends_on) {
                d.item(*w)?;
            }
            let mut deps = depends_on;
            deps.sort();
            deps.dedup();
            let it = d.item_mut(id)?;
            it.parent = parent;
            it.depends_on = deps.clone();
            d.emit(EventKind::Relinked {
                id,
                parent,
                depends_on: deps,
            });
            Ok(())
        })
    }

    /// 入口裁定关闭（spec §4.1「全部 dropped → 交机长/人裁定」）。带证据 = 事实关闭（行政处可做）。
    /// 还有活着的关联项时被拒（不变量）。
    pub fn close_entry(
        &mut self,
        by: &Actor,
        now: Ts,
        no: EntryNo,
        note: &str,
        evidence: Option<Evidence>,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            let now = d.now;
            let action = if evidence.is_some() {
                Action::CloseEntryFactual
            } else {
                Action::CloseEntryDecision
            };
            d.permit(action, Ctx::default())?;
            let note = note.trim().to_string();
            if note.is_empty() {
                return Err(Rejected::Blank("note"));
            }
            if evidence.as_ref().is_some_and(|e| e.is_blank()) {
                return Err(Rejected::Blank("evidence"));
            }
            let by = d.by.clone();
            let e = d.s.entries.get_mut(&no).ok_or(Rejected::UnknownEntry(no))?;
            if e.closed.is_some() {
                return Err(Rejected::EntryClosed(no));
            }
            e.closed = Some(EntryClosure {
                by,
                note: note.clone(),
                evidence,
                at: now,
            });
            d.emit(EventKind::EntryClosed { entry: no, note });
            Ok(())
        })
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-work`
Expected: `test result: ok. 74 passed; 0 failed`

- [ ] **Step 5: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/work/src/lib.rs crates/work/src/progress.rs crates/work/src/testkit.rs
git diff --cached --stat
git commit -m "crewd-work: progress report with next check, done with evidence, factual vs decision drop, relink, entry ruling

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 7: 负责人：转交、指派、失联重派、检查点记账（`owner`）

换负责人只有一个内部入口 `change_owner`：写 ownerHistory（交接说明必填）、清待接受的转交与 missedChecks、产出带关联群的 `OwnerChanged`（M4b 据此把新负责人拉进群，spec §17）。强制指派与失联重派把 active 项的检查时间提到 now（新负责人还不知道这件事）；接受转交不提（接受的人此刻醒着）。`record_checkpoint_wake` 是失联判定的依据（裁定 9），整批要么全记、要么全不记。

**Files:**
- Create: `crewd/crates/work/src/owner.rs`
- Modify: `crewd/crates/work/src/lib.rs`

**Interfaces:**
- Consumes: Task 4 事务与 `Draft` 方法、`WorkBook::link_channel`；Task 5 `BlockOn` / `Continuation`；Task 6 `Report`、`testkit::activate`。
- Produces：`WorkBook::{request_transfer, accept_transfer, withdraw_transfer, assign, reassign_lost, record_checkpoint_wake}`（签名见「公开 API 一览」，全部 `-> Result<Applied<()>, Rejected>`）。

- [ ] **Step 1: 写失败的测试**

`crates/work/src/lib.rs`（整个文件）：

```rust
//! crewd 的工作跟进核心（M4a）：两层 Todo（入口 #N + 工作项 W<n> 的 DAG）、负责人、检查点、
//! 卡住只卡那一件、停下即检查、等人升级、行政巡检的纯判定（spec §4–§6、§8、§9）。
//! sans-IO：不读时钟、不碰文件 / 数据库 / tokio；时间一律由调用方传入（Unix 毫秒）。持久化与 MCP 接线归 M4b。
pub mod block;
pub mod book;
pub mod events;
pub mod graph;
pub mod ids;
pub mod invariants;
pub mod model;
pub mod owner;
pub mod perm;
pub mod progress;
#[cfg(test)]
mod testkit;
```

`crates/work/src/owner.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ChannelId, HOUR, MINUTE};
    use crate::model::NewItem;
    use crate::progress::Report;
    use crate::testkit::*;

    #[test]
    fn transfer_is_request_then_accept() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        b.link_channel(&Actor::worker("a"), T1, w, Some(ChannelId::new("ch")))
            .unwrap();
        activate(&mut b, "a", w, T1);
        let a = Actor::worker("a");
        assert_eq!(
            b.request_transfer(&a, T1, w, sid("b"), " "),
            Err(Rejected::Blank("note"))
        );
        assert_eq!(
            b.request_transfer(&a, T1, w, sid("a"), "n"),
            Err(Rejected::SameOwner)
        );
        assert_eq!(
            b.request_transfer(&Actor::worker("b"), T1, w, sid("b"), "给我吧"),
            Err(Rejected::Forbidden(Action::TransferRequest))
        );
        b.request_transfer(&a, T1, w, sid("b"), "我要去修发版")
            .unwrap();
        assert_eq!(
            b.request_transfer(&a, T1, w, sid("c"), "n"),
            Err(Rejected::TransferPending(w))
        );
        assert_eq!(
            b.accept_transfer(&Actor::worker("c"), T1, w),
            Err(Rejected::Forbidden(Action::TransferAccept))
        );
        let later = T1.plus(MINUTE);
        let got = b.accept_transfer(&Actor::worker("b"), later, w).unwrap();
        assert_eq!(
            got.events[0].kind,
            EventKind::OwnerChanged {
                id: w,
                from: Some(sid("a")),
                to: sid("b"),
                how: HowChanged::Transfer,
                note: "我要去修发版".into(),
                channel: Some(ChannelId::new("ch"))
            }
        );
        let it = b.item(w).unwrap();
        assert_eq!(it.owner, Some(sid("b")));
        assert_eq!(it.pending_transfer, None);
        assert_eq!(it.status, Status::Active);
        assert_eq!(
            it.next_check_at,
            Some(T1.plus(HOUR)),
            "接受的人醒着，不提前检查"
        );
        assert_eq!(it.owner_history.last().unwrap().by, Actor::worker("b"));
        assert_eq!(
            b.accept_transfer(&Actor::worker("b"), later, w),
            Err(Rejected::NoPendingTransfer(w))
        );
    }

    #[test]
    fn pending_transfer_can_be_withdrawn_declined_or_cancelled() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        let req = |b: &mut WorkBook| {
            b.request_transfer(&Actor::worker("a"), T1, w, sid("b"), "n")
                .unwrap()
        };
        assert_eq!(
            b.withdraw_transfer(&Actor::worker("a"), T1, w),
            Err(Rejected::NoPendingTransfer(w))
        );
        for who in [Actor::worker("a"), Actor::worker("b"), Actor::captain("c")] {
            req(&mut b);
            let got = b.withdraw_transfer(&who, T1, w).unwrap();
            assert_eq!(kinds(&got.events), ["transfer_withdrawn"]);
            assert_eq!(b.item(w).unwrap().pending_transfer, None);
        }
        req(&mut b);
        assert_eq!(
            b.withdraw_transfer(&Actor::worker("x"), T1, w),
            Err(Rejected::Forbidden(Action::TransferWithdraw))
        );
    }

    #[test]
    fn captain_assignment_wakes_the_new_owner_of_active_work() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        activate(&mut b, "a", w, T1);
        b.request_transfer(&Actor::worker("a"), T1, w, sid("b"), "n")
            .unwrap();
        let later = T1.plus(10 * MINUTE);
        assert_eq!(
            b.assign(&Actor::admin("d"), later, w, sid("c"), "n"),
            Err(Rejected::Forbidden(Action::Assign)),
            "强制指派是机长权力"
        );
        assert_eq!(
            b.assign(&Actor::captain("k"), later, w, sid("a"), "n"),
            Err(Rejected::SameOwner)
        );
        assert_eq!(
            b.assign(&Actor::captain("k"), later, w, sid("c"), ""),
            Err(Rejected::Blank("note"))
        );
        b.assign(&Actor::captain("k"), later, w, sid("c"), "c 更熟这块")
            .unwrap();
        let it = b.item(w).unwrap();
        assert_eq!(it.owner, Some(sid("c")));
        assert_eq!(it.next_check_at, Some(later));
        assert_eq!(it.pending_transfer, None, "强制指派顶掉待接受的转交");
        assert_eq!(it.owner_history.last().unwrap().how, HowChanged::Assign);
    }

    #[test]
    fn workers_may_only_pick_up_unowned_items_for_themselves() {
        let mut b = book();
        let orphan = b
            .create_item(&Actor::worker("a"), T1, NewItem::titled("帮忙"))
            .unwrap()
            .value;
        assert_eq!(
            b.assign(&Actor::worker("a"), T1, orphan, sid("b"), "n"),
            Err(Rejected::Forbidden(Action::Assign))
        );
        b.assign(&Actor::worker("b"), T1, orphan, sid("b"), "我来")
            .unwrap();
        let it = b.item(orphan).unwrap();
        assert_eq!(it.owner, Some(sid("b")));
        assert_eq!(it.owner_history[0].how, HowChanged::Claim);
        assert_eq!(it.owner_history[0].from, None);
        assert_eq!(
            b.assign(&Actor::worker("c"), T1, orphan, sid("c"), "我也要"),
            Err(Rejected::Forbidden(Action::Assign))
        );
        assert!(
            b.assign(&Actor::System, T1, orphan, sid("c"), "调度层改派")
                .is_ok()
        );
    }

    #[test]
    fn lost_owner_is_replaced_by_someone_else() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        activate(&mut b, "a", w, T1);
        let later = T1.plus(3 * HOUR);
        assert_eq!(
            b.reassign_lost(&Actor::System, later, w, sid("a"), "失联"),
            Err(Rejected::SameOwner)
        );
        assert_eq!(
            b.reassign_lost(&Actor::worker("b"), later, w, sid("b"), "失联"),
            Err(Rejected::Forbidden(Action::ReassignLost))
        );
        b.reassign_lost(&Actor::admin("d"), later, w, sid("b"), "a 两次没回报")
            .unwrap();
        let it = b.item(w).unwrap();
        assert_eq!(
            (it.owner.clone(), it.next_check_at),
            (Some(sid("b")), Some(later))
        );
        assert_eq!(it.owner_history.last().unwrap().how, HowChanged::Lost);
        let orphan = b
            .create_item(&Actor::worker("a"), later, NewItem::titled("o"))
            .unwrap()
            .value;
        assert_eq!(
            b.reassign_lost(&Actor::System, later, orphan, sid("b"), "n"),
            Err(Rejected::NotOwned(orphan))
        );
    }

    #[test]
    fn checkpoint_wakes_count_until_the_owner_reports() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        activate(&mut b, "a", w, T1);
        let due = T1.plus(HOUR);
        assert_eq!(
            b.record_checkpoint_wake(&Actor::worker("a"), due, &[w]),
            Err(Rejected::Forbidden(Action::RecordWake))
        );
        let got = b.record_checkpoint_wake(&Actor::System, due, &[w]).unwrap();
        assert_eq!(
            got.events[0].kind,
            EventKind::CheckpointWake {
                id: w,
                owner: sid("a"),
                missed_checks: 1,
                next_check_at: Some(due.plus(30 * MINUTE))
            }
        );
        b.record_checkpoint_wake(&Actor::System, due.plus(30 * MINUTE), &[w])
            .unwrap();
        assert_eq!(b.item(w).unwrap().missed_checks, 2);
        b.report(
            &Actor::captain("c"),
            due.plus(31 * MINUTE),
            w,
            Report {
                status: None,
                next_check_at: None,
                note: "机长问过了".into(),
            },
        )
        .unwrap();
        assert_eq!(b.item(w).unwrap().missed_checks, 2, "别人替它说话不算回报");
        b.report(
            &Actor::worker("a"),
            due.plus(32 * MINUTE),
            w,
            Report {
                status: None,
                next_check_at: None,
                note: "还在跑测试".into(),
            },
        )
        .unwrap();
        assert_eq!(
            b.item(w).unwrap().missed_checks,
            0,
            "负责人的任何操作都清零"
        );
    }

    #[test]
    fn checkpoint_wake_rejects_items_that_are_not_checkpointed() {
        let mut b = book();
        let ids = claim(&mut b, "a", &["x", "y"]);
        let orphan = b
            .create_item(&Actor::worker("a"), T1, NewItem::titled("o"))
            .unwrap()
            .value;
        assert_eq!(
            b.record_checkpoint_wake(&Actor::System, T1, &[ids[0], orphan]),
            Err(Rejected::NotOwned(orphan))
        );
        assert_eq!(
            b.item(ids[0]).unwrap().missed_checks,
            0,
            "整批被拒，前面的也不记"
        );
        b.block(
            &Actor::worker("a"),
            T1,
            ids[1],
            vec![crate::block::BlockOn::Item(ids[0])],
            crate::block::Continuation::Nothing,
        )
        .unwrap();
        assert_eq!(
            b.record_checkpoint_wake(&Actor::System, T1, &[ids[1]]),
            Err(Rejected::Invalid("blocked items are not checkpointed"))
        );
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-work`
Expected: 编译失败，找不到 `request_transfer` / `assign` / `record_checkpoint_wake` 等方法。

- [ ] **Step 3: 实现**

`crates/work/src/owner.rs` 测试模块**上面**：

```rust
//! 负责人（spec §5）：转交（原负责人发起 + 新负责人接受）、机长 / 人 / 调度层指派、失联重派，
//! 以及检查点叫醒的记账（失联判定的依据）。交接说明一律必填，写进 ownerHistory。
use crate::book::{Applied, Draft, Rejected, WorkBook};
use crate::events::EventKind;
use crate::ids::{SessionId, Ts, WorkId};
use crate::model::{Actor, HowChanged, OwnerChange, PendingTransfer, Status};
use crate::perm::{Action, Ctx};

impl WorkBook {
    /// `work_item_transfer_request`：只有现负责人能发起；同一时刻只挂一条。
    pub fn request_transfer(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
        to: SessionId,
        note: &str,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            let now = d.now;
            d.permit_on(Action::TransferRequest, id)?;
            let note = note.trim().to_string();
            if note.is_empty() {
                return Err(Rejected::Blank("note"));
            }
            let it = d.live(id)?;
            if it.owner.as_ref() == Some(&to) {
                return Err(Rejected::SameOwner);
            }
            if it.pending_transfer.is_some() {
                return Err(Rejected::TransferPending(id));
            }
            let requested_by =
                d.by.session()
                    .cloned()
                    .expect("permit: only sessions request");
            d.item_mut(id)?.pending_transfer = Some(PendingTransfer {
                to: to.clone(),
                requested_by,
                note: note.clone(),
                at: now,
            });
            d.emit(EventKind::TransferRequested { id, to, note });
            Ok(())
        })
    }

    /// `work_item_transfer_accept`：只有被转交的人能接受。状态与检查时间照旧（接受的人此刻就醒着）。
    pub fn accept_transfer(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            if d.live(id)?.pending_transfer.is_none() {
                return Err(Rejected::NoPendingTransfer(id));
            }
            d.permit_on(Action::TransferAccept, id)?;
            let t = d
                .item_mut(id)?
                .pending_transfer
                .take()
                .expect("checked above");
            d.change_owner(id, t.to, HowChanged::Transfer, t.note, false)
        })
    }

    /// 撤回（原负责人）/ 拒绝（被转交的人）/ 机长取消一条待接受的转交。
    pub fn withdraw_transfer(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            if d.live(id)?.pending_transfer.is_none() {
                return Err(Rejected::NoPendingTransfer(id));
            }
            d.permit_on(Action::TransferWithdraw, id)?;
            d.item_mut(id)?.pending_transfer = None;
            d.emit(EventKind::TransferWithdrawn { id });
            Ok(())
        })
    }

    /// `work_item_assign`：机长 / 人 / 调度层强制指派；worker 只能把无主项领给自己。
    /// active 项换了人 → nextCheckAt = now（新负责人还不知道，下一轮检查点叫醒它）。
    pub fn assign(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
        to: SessionId,
        note: &str,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            let it = d.live(id)?;
            d.permit(
                Action::Assign,
                Ctx {
                    owner: it.owner.as_ref(),
                    assignee: Some(&to),
                    ..Ctx::default()
                },
            )?;
            if it.owner.as_ref() == Some(&to) {
                return Err(Rejected::SameOwner);
            }
            let how = if d.by.session() == Some(&to) {
                HowChanged::Claim
            } else {
                HowChanged::Assign
            };
            d.change_owner(id, to, how, note.to_string(), true)
        })
    }

    /// 负责人失联后重派（spec §5：排除原负责人）。调度层 / 行政处 / 机长 / 人可做。
    pub fn reassign_lost(
        &mut self,
        by: &Actor,
        now: Ts,
        id: WorkId,
        to: SessionId,
        note: &str,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            if d.live(id)?.owner.is_none() {
                return Err(Rejected::NotOwned(id));
            }
            d.permit_on(Action::ReassignLost, id)?;
            if d.item(id)?.owner.as_ref() == Some(&to) {
                return Err(Rejected::SameOwner);
            }
            d.change_owner(id, to, HowChanged::Lost, note.to_string(), true)
        })
    }

    /// crewd 刚给负责人投递了一次检查点唤醒（列出这些项）：每项 missedChecks + 1，
    /// 排了检查时间的项顺延到 now + recheck_after（不超过检查上限），到时还没回报就再叫 / 判失联。
    pub fn record_checkpoint_wake(
        &mut self,
        by: &Actor,
        now: Ts,
        ids: &[WorkId],
    ) -> Result<Applied<()>, Rejected> {
        let recheck = self.cfg.recheck_after_ms.min(self.cfg.max_check_horizon_ms);
        self.transact(by, now, |d| {
            let now = d.now;
            d.permit(Action::RecordWake, Ctx::default())?;
            for &id in ids {
                let it = d.live(id)?;
                let owner = it.owner.clone().ok_or(Rejected::NotOwned(id))?;
                if it.status == Status::Blocked {
                    return Err(Rejected::Invalid("blocked items are not checkpointed"));
                }
                let it = d.item_mut(id)?;
                it.missed_checks += 1;
                if it.next_check_at.is_some() {
                    it.next_check_at = Some(now.plus(recheck));
                }
                let (missed_checks, next_check_at) = (it.missed_checks, it.next_check_at);
                d.emit(EventKind::CheckpointWake {
                    id,
                    owner,
                    missed_checks,
                    next_check_at,
                });
            }
            Ok(())
        })
    }
}

impl Draft {
    /// 换负责人的唯一入口：写 ownerHistory、清待接受的转交与 missedChecks、产出 `OwnerChanged`
    /// （带关联群，M4b 把新负责人拉进去）。`wake` = active 项把检查时间提到 now。
    fn change_owner(
        &mut self,
        id: WorkId,
        to: SessionId,
        how: HowChanged,
        note: String,
        wake: bool,
    ) -> Result<(), Rejected> {
        let note = note.trim().to_string();
        if note.is_empty() {
            return Err(Rejected::Blank("note"));
        }
        let (by, now) = (self.by.clone(), self.now);
        let it = self.item_mut(id)?;
        let from = it.owner.replace(to.clone());
        it.owner_history.push(OwnerChange {
            from: from.clone(),
            to: to.clone(),
            by,
            how,
            note: note.clone(),
            at: now,
        });
        it.pending_transfer = None;
        it.missed_checks = 0;
        if wake && it.status == Status::Active {
            it.next_check_at = Some(now);
        }
        let channel = it.channel.clone();
        self.emit(EventKind::OwnerChanged {
            id,
            from,
            to,
            how,
            note,
            channel,
        });
        Ok(())
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-work`
Expected: `test result: ok. 81 passed; 0 failed`

- [ ] **Step 5: 验这把尺子会红（Review Focus 5）**

清零规则在 Task 4 的事务外壳里（`book.rs`），这一刀打在那里：

```bash
cp crates/work/src/book.rs crates/work/src/book.rs.bak
sed -i '' 's/                it.missed_checks = 0;/                let _ = 0;/' crates/work/src/book.rs
grep -c 'let _ = 0;' crates/work/src/book.rs
cargo test -p crewd-work 2>&1 | grep -E 'FAILED|test result'
cp crates/work/src/book.rs.bak crates/work/src/book.rs
cmp crates/work/src/book.rs crates/work/src/book.rs.bak && rm crates/work/src/book.rs.bak
```

Expected：`grep -c` 输出 `1`；只有 `owner::tests::checkpoint_wakes_count_until_the_owner_reports ... FAILED`，汇总 `80 passed; 1 failed`；还原后 `cmp` 无输出。

- [ ] **Step 6: 全量检查并提交**

```bash
./scripts/check.sh
git status --short
git add crates/work/src/lib.rs crates/work/src/owner.rs
git diff --cached --stat
git commit -m "crewd-work: transfer request/accept/withdraw, assignment, lost-owner reassignment, checkpoint wake bookkeeping

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 8: 推导视图（`views`）

入口状态由第二层推导（spec §4.1，规则见裁定 6），`EntryView` 给出 UI「→ W3 W4（1/2）」与负责人所需的数据。到期检查点按负责人合并（spec §5：同一 session 同一时刻合并为一条）。失联候选两种原因（裁定 9）。负载不计 blocked（spec §6），供 M5 的 Jev 派活用。全部只读，不改状态。

**Files:**
- Create: `crewd/crates/work/src/views.rs`
- Modify: `crewd/crates/work/src/lib.rs`

**Interfaces:**
- Consumes: Task 4–7 的命令（测试里搭状态）、`testkit::{book, claim, activate, sid, T1}`。
- Produces（`crewd_work::views`）：`enum EntryStatus { Unclaimed, InProgress, Done, NeedsRuling, Closed }`；`struct EntryView { no, status, items: Vec<WorkId>, done: usize, owners: Vec<SessionId> }`；`struct MyItem { id, title, status, next_step: Option<String>, next_check_at: Option<Ts>, blocked_on: Vec<Blocker>, transfer_to: Option<SessionId> }`；`struct CheckpointDue { owner: SessionId, items: Vec<WorkId> }`；`enum LostCause { Silent { missed: u32 }, Unrecoverable }`；`struct LostOwner { item, owner, cause }`；`WorkBook::{entry_view, entry_views, ready_items, unowned_items, load_of, my_items, incoming_transfers, due_checkpoints, owner_lost}`（签名见「公开 API 一览」）。

- [ ] **Step 1: 写失败的测试**

`crates/work/src/lib.rs`（整个文件）：

```rust
//! crewd 的工作跟进核心（M4a）：两层 Todo（入口 #N + 工作项 W<n> 的 DAG）、负责人、检查点、
//! 卡住只卡那一件、停下即检查、等人升级、行政巡检的纯判定（spec §4–§6、§8、§9）。
//! sans-IO：不读时钟、不碰文件 / 数据库 / tokio；时间一律由调用方传入（Unix 毫秒）。持久化与 MCP 接线归 M4b。
pub mod block;
pub mod book;
pub mod events;
pub mod graph;
pub mod ids;
pub mod invariants;
pub mod model;
pub mod owner;
pub mod perm;
pub mod progress;
#[cfg(test)]
mod testkit;
pub mod views;
```

`crates/work/src/views.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{BlockOn, Continuation};
    use crate::ids::{HOUR, MINUTE};
    use crate::model::{Actor, DropKind, DropReason, Evidence, ItemRef, NewItem};
    use crate::progress::{Progress, Report};
    use crate::testkit::*;

    fn wait_on_human(b: &mut WorkBook, who: &str, id: WorkId) {
        b.block(
            &Actor::worker(who),
            T1,
            id,
            vec![BlockOn::NewAsk {
                question: "q".into(),
                default: None,
            }],
            Continuation::Nothing,
        )
        .unwrap();
    }

    fn drop_it(b: &mut WorkBook, who: &str, id: WorkId) {
        b.drop_item(
            &Actor::worker(who),
            T1,
            id,
            DropReason {
                kind: DropKind::NotNeeded,
                note: "n".into(),
                evidence: None,
            },
        )
        .unwrap();
    }

    #[test]
    fn entry_status_is_derived_from_its_items() {
        let mut b = book();
        let view = |b: &WorkBook| b.entry_view(EntryNo(1)).unwrap();
        assert_eq!(view(&b).status, EntryStatus::Unclaimed);
        let ids = claim(&mut b, "a", &["x", "y", "z"]);
        let v = view(&b);
        assert_eq!(
            (v.status, v.items.clone(), v.done, v.owners.clone()),
            (EntryStatus::InProgress, ids.clone(), 0, vec![sid("a")])
        );
        b.mark_done(
            &Actor::worker("a"),
            T1,
            ids[0],
            Evidence::Commit("c1".into()),
        )
        .unwrap();
        drop_it(&mut b, "a", ids[1]);
        let v = view(&b);
        assert_eq!(
            (v.status, v.items.clone(), v.done),
            (EntryStatus::InProgress, vec![ids[0], ids[2]], 1),
            "dropped 的不算进「→ W1 W3（1/2）」"
        );
        b.mark_done(
            &Actor::worker("a"),
            T1,
            ids[2],
            Evidence::Commit("c2".into()),
        )
        .unwrap();
        let v = view(&b);
        assert_eq!(
            (v.status, v.done, v.owners.clone()),
            (EntryStatus::Done, 2, vec![])
        );
        assert_eq!(b.entry_view(EntryNo(7)), None);
    }

    #[test]
    fn all_dropped_needs_a_ruling_then_closes() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        drop_it(&mut b, "a", w);
        assert_eq!(
            b.entry_view(EntryNo(1)).unwrap().status,
            EntryStatus::NeedsRuling
        );
        b.close_entry(&Actor::captain("c"), T1, EntryNo(1), "放弃", None)
            .unwrap();
        assert_eq!(
            b.entry_view(EntryNo(1)).unwrap().status,
            EntryStatus::Closed
        );
        b.register_entry(T1, EntryNo(2)).unwrap();
        let all: Vec<(EntryNo, EntryStatus)> =
            b.entry_views().iter().map(|v| (v.no, v.status)).collect();
        assert_eq!(
            all,
            [
                (EntryNo(1), EntryStatus::Closed),
                (EntryNo(2), EntryStatus::Unclaimed)
            ]
        );
    }

    #[test]
    fn split_children_do_not_change_the_entry_count() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        b.block(
            &Actor::worker("a"),
            T1,
            w,
            vec![BlockOn::NewAsk {
                question: "q".into(),
                default: None,
            }],
            Continuation::Split(vec![NewItem::titled("part")]),
        )
        .unwrap();
        assert_eq!(b.entry_view(EntryNo(1)).unwrap().items, [w]);
    }

    #[test]
    fn ready_and_unowned() {
        let mut b = book();
        let ids = b
            .claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![
                    NewItem::titled("dep"),
                    NewItem::titled("after dep").after(ItemRef::Batch(0)),
                    NewItem::titled("blocked"),
                ],
            )
            .unwrap()
            .value;
        wait_on_human(&mut b, "a", ids[2]);
        let orphan = b
            .create_item(&Actor::worker("a"), T1, NewItem::titled("help"))
            .unwrap()
            .value;
        assert_eq!(b.ready_items(), [ids[0], orphan]);
        assert_eq!(b.unowned_items(), [orphan]);
        b.mark_done(&Actor::worker("a"), T1, ids[0], Evidence::Text("ok".into()))
            .unwrap();
        assert_eq!(b.ready_items(), [ids[1], orphan]);
    }

    #[test]
    fn my_items_lists_active_first_and_excludes_blocked_from_load() {
        let mut b = book();
        let ids = claim(&mut b, "a", &["open", "active", "blocked"]);
        activate(&mut b, "a", ids[1], T1);
        wait_on_human(&mut b, "a", ids[2]);
        b.request_transfer(&Actor::worker("a"), T1, ids[0], sid("b"), "给 b")
            .unwrap();
        let mine = b.my_items(&sid("a"));
        let order: Vec<WorkId> = mine.iter().map(|m| m.id).collect();
        assert_eq!(order, [ids[1], ids[0], ids[2]]);
        assert_eq!(mine[0].next_step.as_deref(), Some("开工"));
        assert_eq!(mine[0].next_check_at, Some(T1.plus(HOUR)));
        assert_eq!(mine[1].transfer_to, Some(sid("b")));
        assert_eq!(mine[2].blocked_on, [Blocker::Human(crate::ids::AskId(1))]);
        assert_eq!(b.load_of(&sid("a")), 2);
        assert_eq!(b.incoming_transfers(&sid("b")), [ids[0]]);
        assert!(b.my_items(&sid("b")).is_empty());
    }

    #[test]
    fn due_checkpoints_merge_per_owner() {
        let mut b = book();
        let a_items = claim(&mut b, "a", &["a1", "a2", "a3"]);
        b.register_entry(T1, EntryNo(2)).unwrap();
        let b_item = b
            .claim_entry(
                &Actor::worker("b"),
                T1,
                EntryNo(2),
                vec![NewItem::titled("b1")],
            )
            .unwrap()
            .value[0];
        activate(&mut b, "a", a_items[0], T1);
        activate(&mut b, "a", a_items[1], T1.plus(MINUTE));
        activate(&mut b, "b", b_item, T1);
        b.report(
            &Actor::worker("a"),
            T1,
            a_items[2],
            Report {
                status: None,
                next_check_at: Some(T1.plus(2 * HOUR)),
                note: "晚点开工".into(),
            },
        )
        .unwrap();
        assert_eq!(b.due_checkpoints(T1.plus(HOUR - 1)), []);
        assert_eq!(
            b.due_checkpoints(T1.plus(HOUR)),
            [
                CheckpointDue {
                    owner: sid("a"),
                    items: vec![a_items[0]]
                },
                CheckpointDue {
                    owner: sid("b"),
                    items: vec![b_item]
                }
            ]
        );
        assert_eq!(
            b.due_checkpoints(T1.plus(2 * HOUR)),
            [
                CheckpointDue {
                    owner: sid("a"),
                    items: a_items.clone()
                },
                CheckpointDue {
                    owner: sid("b"),
                    items: vec![b_item]
                }
            ],
            "同一负责人的到期项合成一条"
        );
    }

    #[test]
    fn silent_owner_is_lost_after_k_unanswered_wakes() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        activate(&mut b, "a", w, T1);
        let none = BTreeSet::new();
        let t = T1.plus(HOUR);
        assert_eq!(b.due_checkpoints(t).len(), 1);
        b.record_checkpoint_wake(&crate::model::Actor::System, t, &[w])
            .unwrap();
        let t = t.plus(30 * MINUTE);
        assert_eq!(b.owner_lost(t, &none), []);
        assert_eq!(b.due_checkpoints(t)[0].items, [w], "第二次叫醒");
        b.record_checkpoint_wake(&crate::model::Actor::System, t, &[w])
            .unwrap();
        assert_eq!(b.owner_lost(t.plus(30 * MINUTE - 1), &none), []);
        let t = t.plus(30 * MINUTE);
        assert_eq!(b.due_checkpoints(t), [], "到失联线就不再叫，改判失联");
        assert_eq!(
            b.owner_lost(t, &none),
            [LostOwner {
                item: w,
                owner: sid("a"),
                cause: LostCause::Silent { missed: 2 }
            }]
        );
        b.report(
            &Actor::worker("a"),
            t,
            w,
            Report {
                status: Some(Progress::Active),
                next_check_at: Some(t.plus(HOUR)),
                note: "回来了".into(),
            },
        )
        .unwrap();
        assert_eq!(b.owner_lost(t, &none), [], "回报一次就解除");
    }

    #[test]
    fn unrecoverable_session_loses_every_live_item_including_blocked() {
        let mut b = book();
        let ids = claim(&mut b, "a", &["x", "y", "z"]);
        wait_on_human(&mut b, "a", ids[1]);
        b.mark_done(&Actor::worker("a"), T1, ids[2], Evidence::Text("ok".into()))
            .unwrap();
        let dead: BTreeSet<SessionId> = [sid("a")].into();
        let lost: Vec<(WorkId, LostCause)> = b
            .owner_lost(T1, &dead)
            .into_iter()
            .map(|l| (l.item, l.cause))
            .collect();
        assert_eq!(
            lost,
            [
                (ids[0], LostCause::Unrecoverable),
                (ids[1], LostCause::Unrecoverable)
            ],
            "blocked 的也要换人，否则人回复了没人接"
        );
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-work`
Expected: 编译失败，找不到 `EntryStatus` / `CheckpointDue` / `entry_view` 等。

- [ ] **Step 3: 实现**

`crates/work/src/views.rs` 测试模块**上面**：

```rust
//! 推导视图（只读）：入口状态（spec §4.1，由第二层推导）、待认领 / 就绪项、「我的工作项」（§4.3 注入用的结构化数据，
//! 不渲染文字——给 agent 看的文案归 M4b 的模板）、到期检查点（§5，同一 session 合并一条）、负责人失联候选（§5）。
use crate::book::WorkBook;
use crate::ids::{EntryNo, SessionId, Ts, WorkId};
use crate::model::{Blocker, Status, WorkItem};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    /// 没有关联工作项 → 待认领。
    Unclaimed,
    InProgress,
    /// 没有活着的关联项，且至少一项 done。
    Done,
    /// 关联项全部 dropped → 交机长 / 人裁定（不自动关）。
    NeedsRuling,
    /// 已裁定关闭。
    Closed,
}

/// 入口行内显示「→ W3 W4（1/2）」与负责人（spec §11）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryView {
    pub no: EntryNo,
    pub status: EntryStatus,
    /// 直接关联（sources 含 #N）、没 dropped 的项。
    pub items: Vec<WorkId>,
    pub done: usize,
    /// 活着的关联项的负责人（去重）。
    pub owners: Vec<SessionId>,
}

/// 「你名下的工作项」一行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyItem {
    pub id: WorkId,
    pub title: String,
    pub status: Status,
    pub next_step: Option<String>,
    pub next_check_at: Option<Ts>,
    pub blocked_on: Vec<Blocker>,
    /// 你发起、对方还没接受的转交。
    pub transfer_to: Option<SessionId>,
}

/// 到期检查点：一个负责人一条，列出它全部到期的项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointDue {
    pub owner: SessionId,
    pub items: Vec<WorkId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LostCause {
    /// 连续 K 次检查点唤醒没有回报，且最后一次之后的复查时间也过了。
    Silent { missed: u32 },
    /// crewd 试过恢复（--resume）没成功。
    Unrecoverable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LostOwner {
    pub item: WorkId,
    pub owner: SessionId,
    pub cause: LostCause,
}

impl WorkBook {
    pub fn entry_view(&self, no: EntryNo) -> Option<EntryView> {
        let entry = self.state.entries.get(&no)?;
        let linked: Vec<&WorkItem> = self
            .state
            .items
            .values()
            .filter(|it| it.sources.contains(&no))
            .collect();
        let any_live = linked.iter().any(|it| it.status.is_live());
        let done = linked.iter().filter(|it| it.status == Status::Done).count();
        let status = if entry.closed.is_some() {
            EntryStatus::Closed
        } else if linked.is_empty() {
            EntryStatus::Unclaimed
        } else if any_live {
            EntryStatus::InProgress
        } else if done > 0 {
            EntryStatus::Done
        } else {
            EntryStatus::NeedsRuling
        };
        let owners: BTreeSet<SessionId> = linked
            .iter()
            .filter(|it| it.status.is_live())
            .filter_map(|it| it.owner.clone())
            .collect();
        Some(EntryView {
            no,
            status,
            items: linked
                .iter()
                .filter(|it| it.status != Status::Dropped)
                .map(|it| it.id)
                .collect(),
            done,
            owners: owners.into_iter().collect(),
        })
    }

    pub fn entry_views(&self) -> Vec<EntryView> {
        self.state
            .entries
            .keys()
            .filter_map(|no| self.entry_view(*no))
            .collect()
    }

    fn deps_done(&self, it: &WorkItem) -> bool {
        it.depends_on
            .iter()
            .all(|d| self.state.items[d].status == Status::Done)
    }

    /// open 且依赖已齐（有没有负责人都算）。
    pub fn ready_items(&self) -> Vec<WorkId> {
        self.state
            .items
            .values()
            .filter(|it| it.status == Status::Open && self.deps_done(it))
            .map(|it| it.id)
            .collect()
    }

    /// 待认领：open 且没有负责人（调度层 / 行政巡检捞这些）。
    pub fn unowned_items(&self) -> Vec<WorkId> {
        self.state
            .items
            .values()
            .filter(|it| it.status == Status::Open && it.owner.is_none())
            .map(|it| it.id)
            .collect()
    }

    /// 该 session 的负载：名下 open + active 项数。blocked 不计（spec §6：卡住的项不算负载）。
    pub fn load_of(&self, session: &SessionId) -> usize {
        self.state
            .items
            .values()
            .filter(|it| it.owner.as_ref() == Some(session))
            .filter(|it| matches!(it.status, Status::Open | Status::Active))
            .count()
    }

    /// 名下活着的项：active 在前，其次 open、blocked；同档按 W 号。
    pub fn my_items(&self, session: &SessionId) -> Vec<MyItem> {
        let rank = |s: Status| match s {
            Status::Active => 0,
            Status::Open => 1,
            _ => 2,
        };
        let mut mine: Vec<&WorkItem> = self
            .state
            .items
            .values()
            .filter(|it| it.status.is_live() && it.owner.as_ref() == Some(session))
            .collect();
        mine.sort_by_key(|it| (rank(it.status), it.id));
        mine.into_iter()
            .map(|it| MyItem {
                id: it.id,
                title: it.title.clone(),
                status: it.status,
                next_step: it.last_report.as_ref().map(|n| n.text.clone()),
                next_check_at: it.next_check_at,
                blocked_on: it.blocked_on.clone(),
                transfer_to: it.pending_transfer.as_ref().map(|t| t.to.clone()),
            })
            .collect()
    }

    /// 别人要转给该 session、等它接受的项。
    pub fn incoming_transfers(&self, session: &SessionId) -> Vec<WorkId> {
        self.state
            .items
            .values()
            .filter(|it| it.pending_transfer.as_ref().map(|t| &t.to) == Some(session))
            .map(|it| it.id)
            .collect()
    }

    /// 到期检查点：有负责人的 open / active 项，nextCheckAt ≤ now，且还没到失联线（missed < K）。
    /// 同一负责人合并成一条（spec §5「同一 session 同一时刻合并为一条」）。
    pub fn due_checkpoints(&self, now: Ts) -> Vec<CheckpointDue> {
        let mut by_owner: BTreeMap<SessionId, Vec<WorkId>> = BTreeMap::new();
        for it in self.state.items.values() {
            let Some(owner) = &it.owner else { continue };
            if matches!(it.status, Status::Open | Status::Active)
                && it.next_check_at.is_some_and(|t| t <= now)
                && it.missed_checks < self.cfg.missed_checks_limit
            {
                by_owner.entry(owner.clone()).or_default().push(it.id);
            }
        }
        by_owner
            .into_iter()
            .map(|(owner, items)| CheckpointDue { owner, items })
            .collect()
    }

    /// 负责人失联候选（spec §5）：`unrecoverable` 是 crewd 试过恢复失败的 session（退出 ≠ 失联，先 resume）。
    /// 由调度层（排除原负责人）重派 + 告知机长。
    pub fn owner_lost(&self, now: Ts, unrecoverable: &BTreeSet<SessionId>) -> Vec<LostOwner> {
        let mut out = Vec::new();
        for it in self.state.items.values() {
            let Some(owner) = &it.owner else { continue };
            if it.status.is_terminal() {
                continue;
            }
            let cause = if unrecoverable.contains(owner) {
                LostCause::Unrecoverable
            } else if matches!(it.status, Status::Open | Status::Active)
                && it.missed_checks >= self.cfg.missed_checks_limit
                && it.next_check_at.is_some_and(|t| t <= now)
            {
                LostCause::Silent {
                    missed: it.missed_checks,
                }
            } else {
                continue;
            };
            out.push(LostOwner {
                item: it.id,
                owner: owner.clone(),
                cause,
            });
        }
        out
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-work`
Expected: `test result: ok. 89 passed; 0 failed`

- [ ] **Step 5: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/work/src/lib.rs crates/work/src/views.rs
git diff --cached --stat
git commit -m "crewd-work: derived views (entry status from items, ready/unowned/load, my items, due checkpoints merged per owner, lost owners)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 9: 督办判定：停下即检查、等人升级、默认方案、巡检（`watch`）

停下即检查的三种违反与防死循环见裁定 11。人的待确认清单只列仍卡着活项的问、等得最久的在前（spec §11）；升级档 24h 置顶、72h 问「还要吗」，每档只通知一次（`note_escalation` 只升不降）。默认方案到点只列出来，执行仍是 `apply_default`（Task 5）。巡检汇总与触发判定见裁定 15。

**Files:**
- Create: `crewd/crates/work/src/watch.rs`
- Modify: `crewd/crates/work/src/lib.rs`

**Interfaces:**
- Consumes: Task 4–8 全部；`views::{EntryStatus, LostOwner}`。
- Produces（`crewd_work::watch`）：`enum StopDecision { Clean, Rewake(Vec<StopViolation>), Escalate(Vec<StopViolation>), Suppressed(Vec<StopViolation>) }`；`struct HumanWait { ask, items: Vec<WorkId>, owners: Vec<SessionId>, waited_ms: i64, tier: WaitTier, default_due_at: Option<Ts> }`；`struct Patrol { unowned_items, unclaimed_entries, needs_ruling, lost, stop_escalated, long_waits, archivable_channels }`（`needs_admin_session(&self) -> bool`）；`fn wait_tier(waited_ms: i64, cfg: &Config) -> WaitTier`；`fn patrol_due(messages_since_last: u32, last_patrol_at: Option<Ts>, now: Ts, cfg: &Config) -> bool`；`WorkBook::{stop_violations, on_idle, human_waits, escalations_due, note_escalation, defaults_due, patrol}`（签名见「公开 API 一览」）。

- [ ] **Step 1: 写失败的测试**

`crates/work/src/lib.rs`（整个文件）：

```rust
//! crewd 的工作跟进核心（M4a）：两层 Todo（入口 #N + 工作项 W<n> 的 DAG）、负责人、检查点、
//! 卡住只卡那一件、停下即检查、等人升级、行政巡检的纯判定（spec §4–§6、§8、§9）。
//! sans-IO：不读时钟、不碰文件 / 数据库 / tokio；时间一律由调用方传入（Unix 毫秒）。持久化与 MCP 接线归 M4b。
pub mod block;
pub mod book;
pub mod events;
pub mod graph;
pub mod ids;
pub mod invariants;
pub mod model;
pub mod owner;
pub mod perm;
pub mod progress;
#[cfg(test)]
mod testkit;
pub mod views;
pub mod watch;
```

`crates/work/src/watch.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{BlockOn, Continuation};
    use crate::ids::{HOUR, MINUTE};
    use crate::model::{DefaultOption, DropKind, DropReason, Evidence, ItemRef, NewItem};
    use crate::progress::{Progress, Report};
    use crate::testkit::*;

    fn v(item: WorkId, kind: StopKind) -> StopViolation {
        StopViolation { item, kind }
    }

    fn block_on_new_ask(
        b: &mut WorkBook,
        who: &str,
        id: WorkId,
        at: Ts,
        default: Option<DefaultOption>,
    ) {
        b.block(
            &Actor::worker(who),
            at,
            id,
            vec![BlockOn::NewAsk {
                question: "q".into(),
                default,
            }],
            Continuation::Nothing,
        )
        .unwrap();
    }

    #[test]
    fn idle_session_with_unscheduled_ready_work_is_rewoken() {
        let mut b = book();
        let ids = b
            .claim_entry(
                &Actor::worker("a"),
                T1,
                EntryNo(1),
                vec![
                    NewItem::titled("ready"),
                    NewItem::titled("waits").after(ItemRef::Batch(0)),
                ],
            )
            .unwrap()
            .value;
        assert_eq!(
            b.stop_violations(&sid("a"), T1),
            [v(ids[0], StopKind::ReadyUnscheduled)],
            "依赖没齐的不算"
        );
        assert_eq!(b.on_idle(&sid("b"), T1), StopDecision::Clean);
        activate(&mut b, "a", ids[0], T1);
        assert_eq!(b.on_idle(&sid("a"), T1), StopDecision::Clean);
    }

    #[test]
    fn overdue_and_unanswered_checks_are_violations() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        activate(&mut b, "a", w, T1);
        let due = T1.plus(HOUR);
        assert_eq!(b.stop_violations(&sid("a"), due), [v(w, StopKind::Overdue)]);
        b.record_checkpoint_wake(&Actor::System, due, &[w]).unwrap();
        assert_eq!(
            b.stop_violations(&sid("a"), due),
            [v(w, StopKind::Unanswered)],
            "叫醒后检查时间顺延了，但它还欠一次回报"
        );
    }

    #[test]
    fn blocked_items_never_trigger_stop_check() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        block_on_new_ask(&mut b, "a", w, T1, None);
        assert_eq!(
            b.on_idle(&sid("a"), T1.plus(10 * HOUR)),
            StopDecision::Clean
        );
    }

    #[test]
    fn three_unimproved_rewakes_then_escalate_then_silence() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        let a = sid("a");
        let expect = vec![v(w, StopKind::ReadyUnscheduled)];
        assert_eq!(b.on_idle(&a, T1), StopDecision::Rewake(expect.clone()));
        assert_eq!(b.on_idle(&a, T1), StopDecision::Rewake(expect.clone()));
        assert_eq!(b.on_idle(&a, T1), StopDecision::Rewake(expect.clone()));
        assert_eq!(b.on_idle(&a, T1), StopDecision::Escalate(expect.clone()));
        assert_eq!(b.on_idle(&a, T1), StopDecision::Suppressed(expect.clone()));
        assert_eq!(
            b.patrol(T1, &BTreeSet::new()).stop_escalated,
            vec![a.clone()]
        );
        // 有变化（新领了一项）= 有动作，重新计数。
        let w2 = b
            .create_item(&Actor::worker("a"), T1, NewItem::titled("y").owned_by("a"))
            .unwrap()
            .value;
        assert_eq!(
            b.on_idle(&a, T1),
            StopDecision::Rewake(vec![
                v(w, StopKind::ReadyUnscheduled),
                v(w2, StopKind::ReadyUnscheduled)
            ])
        );
        activate(&mut b, "a", w, T1);
        activate(&mut b, "a", w2, T1);
        assert_eq!(b.on_idle(&a, T1), StopDecision::Clean);
        assert!(b.state().stop_streaks.is_empty(), "干净了就清掉计数");
    }

    #[test]
    fn human_wait_tiers_at_24h_and_72h() {
        let mut b = book();
        let ids = claim(&mut b, "a", &["x", "y"]);
        block_on_new_ask(&mut b, "a", ids[0], T1, None);
        b.block(
            &Actor::worker("a"),
            T1.plus(HOUR),
            ids[1],
            vec![BlockOn::Ask(crate::ids::AskId(1))],
            Continuation::Nothing,
        )
        .unwrap();
        let at = |h: i64| T1.plus(h * HOUR);
        let waits = b.human_waits(at(23));
        assert_eq!(waits.len(), 1);
        assert_eq!(waits[0].items, ids);
        assert_eq!(waits[0].owners, [sid("a")]);
        assert_eq!(waits[0].tier, WaitTier::Fresh);
        assert_eq!(b.escalations_due(at(23)), []);
        assert_eq!(b.escalations_due(at(24)), [(AskId(1), WaitTier::Pinned)]);
        b.note_escalation(&Actor::System, at(24), AskId(1), WaitTier::Pinned)
            .unwrap();
        assert_eq!(b.escalations_due(at(48)), [], "同一档只通知一次");
        assert_eq!(
            b.note_escalation(&Actor::System, at(48), AskId(1), WaitTier::Pinned),
            Err(Rejected::Invalid("escalation tiers only go up"))
        );
        assert_eq!(
            b.escalations_due(at(72)),
            [(AskId(1), WaitTier::StillWanted)]
        );
        assert_eq!(
            b.note_escalation(&Actor::worker("a"), at(72), AskId(1), WaitTier::StillWanted),
            Err(Rejected::Forbidden(Action::NoteEscalation))
        );
    }

    #[test]
    fn asks_nobody_waits_on_any_more_leave_the_human_list() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        block_on_new_ask(&mut b, "a", w, T1, None);
        b.report(
            &Actor::worker("a"),
            T1,
            w,
            Report {
                status: Some(Progress::Active),
                next_check_at: Some(T1.plus(HOUR)),
                note: "不等了".into(),
            },
        )
        .unwrap();
        assert_eq!(b.human_waits(T1.plus(30 * HOUR)), []);
        assert_eq!(b.escalations_due(T1.plus(30 * HOUR)), []);
    }

    #[test]
    fn longest_wait_comes_first() {
        let mut b = book();
        let ids = claim(&mut b, "a", &["x", "y"]);
        block_on_new_ask(&mut b, "a", ids[0], T1.plus(HOUR), None);
        block_on_new_ask(&mut b, "a", ids[1], T1.plus(2 * HOUR), None);
        let order: Vec<AskId> = b
            .human_waits(T1.plus(3 * HOUR))
            .iter()
            .map(|w| w.ask)
            .collect();
        assert_eq!(order, [AskId(1), AskId(2)]);
    }

    #[test]
    fn default_options_come_due() {
        let mut b = book();
        let w = claim(&mut b, "a", &["x"])[0];
        block_on_new_ask(
            &mut b,
            "a",
            w,
            T1,
            Some(DefaultOption {
                text: "按 A 办".into(),
                after_ms: 4 * HOUR,
            }),
        );
        assert_eq!(b.human_waits(T1)[0].default_due_at, Some(T1.plus(4 * HOUR)));
        assert_eq!(b.defaults_due(T1.plus(4 * HOUR - 1)), []);
        assert_eq!(b.defaults_due(T1.plus(4 * HOUR)), [AskId(1)]);
        b.apply_default(&Actor::System, T1.plus(4 * HOUR), AskId(1))
            .unwrap();
        assert_eq!(b.defaults_due(T1.plus(5 * HOUR)), []);
    }

    #[test]
    fn patrol_collects_what_needs_hands() {
        let mut b = book();
        b.register_entry(T1, EntryNo(2)).unwrap();
        b.register_entry(T1, EntryNo(3)).unwrap();
        let w = claim(&mut b, "a", &["x"])[0];
        let ch = ChannelId::new("ch-x");
        b.link_channel(&Actor::worker("a"), T1, w, Some(ch.clone()))
            .unwrap();
        let orphan = b
            .create_item(&Actor::worker("a"), T1, NewItem::titled("help"))
            .unwrap()
            .value;
        let dropped = b
            .claim_entry(
                &Actor::worker("b"),
                T1,
                EntryNo(3),
                vec![NewItem::titled("z")],
            )
            .unwrap()
            .value[0];
        b.drop_item(
            &Actor::worker("b"),
            T1,
            dropped,
            DropReason {
                kind: DropKind::NotNeeded,
                note: "n".into(),
                evidence: None,
            },
        )
        .unwrap();
        let p = b.patrol(T1, &BTreeSet::new());
        assert_eq!(p.unowned_items, [orphan]);
        assert_eq!(p.unclaimed_entries, [EntryNo(2)]);
        assert_eq!(p.needs_ruling, [EntryNo(3)]);
        assert_eq!(p.archivable_channels, Vec::<ChannelId>::new());
        assert!(!p.needs_admin_session());
        b.mark_done(&Actor::worker("a"), T1, w, Evidence::Text("ok".into()))
            .unwrap();
        let p = b.patrol(T1, &BTreeSet::new());
        assert_eq!(p.archivable_channels, [ch]);
        assert!(p.needs_admin_session());
    }

    #[test]
    fn patrol_cadence() {
        let cfg = Config::default();
        assert!(patrol_due(0, None, T1, &cfg));
        assert!(!patrol_due(9, Some(T1), T1.plus(29 * MINUTE), &cfg));
        assert!(patrol_due(10, Some(T1), T1.plus(MINUTE), &cfg));
        assert!(patrol_due(0, Some(T1), T1.plus(30 * MINUTE), &cfg));
    }

    #[test]
    fn wait_tier_boundaries() {
        let cfg = Config::default();
        assert_eq!(wait_tier(24 * HOUR - 1, &cfg), WaitTier::Fresh);
        assert_eq!(wait_tier(24 * HOUR, &cfg), WaitTier::Pinned);
        assert_eq!(wait_tier(72 * HOUR, &cfg), WaitTier::StillWanted);
        assert_eq!(wait_tier(-5, &cfg), WaitTier::Fresh);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-work`
Expected: 编译失败，找不到 `StopDecision` / `on_idle` / `patrol_due` 等。

- [ ] **Step 3: 实现**

`crates/work/src/watch.rs` 测试模块**上面**：

```rust
//! 督办的纯判定：停下即检查与防死循环（spec §5）、等人超时升级与默认方案到点（§6）、
//! 行政巡检的代码检查部分（§8.4；Jev fan-out 与起行政 session 归 M6）。
use crate::book::{Applied, Rejected, WorkBook};
use crate::events::EventKind;
use crate::ids::{AskId, ChannelId, EntryNo, SessionId, Ts, WorkId};
use crate::model::{Actor, AskState, Blocker, Config, Status, StopKind, StopViolation, WaitTier};
use crate::perm::{Action, Ctx};
use crate::views::{EntryStatus, LostOwner};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", content = "violations", rename_all = "snake_case")]
pub enum StopDecision {
    /// 名下都有着落，不用叫。
    Clean,
    /// 立刻再唤醒一次，要求给这些项写下一步。
    Rewake(Vec<StopViolation>),
    /// 连续 `stop_check_limit` 次没改善：不再叫，交行政处。
    Escalate(Vec<StopViolation>),
    /// 已经交过行政处、仍没改善：保持安静。
    Suppressed(Vec<StopViolation>),
}

/// 人的待确认清单一行（spec §11：按紧急度排，显示已等时长）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanWait {
    pub ask: AskId,
    /// 此刻卡在这条问上的活着的项。
    pub items: Vec<WorkId>,
    pub owners: Vec<SessionId>,
    pub waited_ms: i64,
    pub tier: WaitTier,
    pub default_due_at: Option<Ts>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Patrol {
    pub unowned_items: Vec<WorkId>,
    pub unclaimed_entries: Vec<EntryNo>,
    pub needs_ruling: Vec<EntryNo>,
    pub lost: Vec<LostOwner>,
    pub stop_escalated: Vec<SessionId>,
    /// 等了 ≥ 24h 的问。
    pub long_waits: Vec<AskId>,
    /// 关联的工作项全部终结的群（候选；「闲置」还要看消息，M6 判）。
    pub archivable_channels: Vec<ChannelId>,
}

impl Patrol {
    /// 只有标出需要动手的事才启动行政 session（spec §8.4）：停下即检查交过来的 session、可归档的群。
    /// 失联重派、待认领、入口派活由调度层直接做；入口裁定归机长 / 人。
    pub fn needs_admin_session(&self) -> bool {
        !self.stop_escalated.is_empty() || !self.archivable_channels.is_empty()
    }
}

pub fn wait_tier(waited_ms: i64, cfg: &Config) -> WaitTier {
    if waited_ms >= cfg.still_wanted_after_ms {
        WaitTier::StillWanted
    } else if waited_ms >= cfg.pin_after_ms {
        WaitTier::Pinned
    } else {
        WaitTier::Fresh
    }
}

/// 巡检触发（spec §8.4）：从没巡过、子树内攒够 N 条消息、或距上次超过间隔。人手动按钮不经过这里。
pub fn patrol_due(
    messages_since_last: u32,
    last_patrol_at: Option<Ts>,
    now: Ts,
    cfg: &Config,
) -> bool {
    match last_patrol_at {
        None => true,
        Some(last) => {
            messages_since_last >= cfg.patrol_every_messages
                || now.since(last) >= cfg.patrol_interval_ms
        }
    }
}

impl WorkBook {
    /// session 进入 idle 时它名下没着落的项（按 W 号、再按种类）。
    pub fn stop_violations(&self, session: &SessionId, now: Ts) -> Vec<StopViolation> {
        let mut out = Vec::new();
        for it in self.state.items.values() {
            if it.owner.as_ref() != Some(session)
                || !matches!(it.status, Status::Open | Status::Active)
            {
                continue;
            }
            if it.missed_checks > 0 {
                out.push(StopViolation {
                    item: it.id,
                    kind: StopKind::Unanswered,
                });
            }
            if it.next_check_at.is_some_and(|t| t <= now) {
                out.push(StopViolation {
                    item: it.id,
                    kind: StopKind::Overdue,
                });
            }
            let deps_done = it
                .depends_on
                .iter()
                .all(|d| self.state.items[d].status == Status::Done);
            if it.status == Status::Open && it.next_check_at.is_none() && deps_done {
                out.push(StopViolation {
                    item: it.id,
                    kind: StopKind::ReadyUnscheduled,
                });
            }
        }
        out
    }

    /// idle 边沿（spec §5「停下即检查」）。走 idle 边沿重新唤醒，不是 Stop hook 阻塞（spec §2）。
    /// 违反集合跟上次一模一样 = 没改善；连续 `stop_check_limit` 次没改善 → 交行政处，之后保持安静，直到集合变化。
    /// 只动软状态（连续计数），不产出事件。
    pub fn on_idle(&mut self, session: &SessionId, now: Ts) -> StopDecision {
        let now = now.max(self.state.clock);
        let v = self.stop_violations(session, now);
        if v.is_empty() {
            self.state.stop_streaks.remove(session);
            return StopDecision::Clean;
        }
        let limit = self.cfg.stop_check_limit;
        let streak = self.state.stop_streaks.entry(session.clone()).or_default();
        if streak.last != v {
            streak.last = v.clone();
            streak.unimproved = 0;
            streak.escalated = false;
            return StopDecision::Rewake(v);
        }
        if streak.escalated {
            return StopDecision::Suppressed(v);
        }
        streak.unimproved += 1;
        if streak.unimproved >= limit {
            streak.escalated = true;
            StopDecision::Escalate(v)
        } else {
            StopDecision::Rewake(v)
        }
    }

    /// 人的待确认清单：还开着、且此刻卡着至少一个活着的项的问（没人等的问不打扰人）。等得最久的在前。
    pub fn human_waits(&self, now: Ts) -> Vec<HumanWait> {
        let mut waiting: BTreeMap<AskId, (Vec<WorkId>, BTreeSet<SessionId>)> = BTreeMap::new();
        for it in self.state.items.values() {
            if it.status.is_terminal() {
                continue;
            }
            for b in &it.blocked_on {
                if let Blocker::Human(h) = b {
                    let entry = waiting.entry(*h).or_default();
                    entry.0.push(it.id);
                    entry.1.extend(it.owner.clone());
                }
            }
        }
        let mut out: Vec<HumanWait> = waiting
            .into_iter()
            .filter_map(|(h, (items, owners))| {
                let ask = &self.state.asks[&h];
                if ask.state != AskState::Open {
                    return None;
                }
                let waited_ms = now.since(ask.asked_at).max(0);
                Some(HumanWait {
                    ask: h,
                    items,
                    owners: owners.into_iter().collect(),
                    waited_ms,
                    tier: wait_tier(waited_ms, &self.cfg),
                    default_due_at: ask.default.as_ref().map(|d| ask.asked_at.plus(d.after_ms)),
                })
            })
            .collect();
        out.sort_by_key(|w| (std::cmp::Reverse(w.waited_ms), w.ask));
        out
    }

    /// 升到了更高一档、还没通知过的问（24h 置顶到人的清单；72h 问「还要吗」）。
    pub fn escalations_due(&self, now: Ts) -> Vec<(AskId, WaitTier)> {
        self.human_waits(now)
            .into_iter()
            .filter(|w| w.tier > self.state.asks[&w.ask].escalated)
            .map(|w| (w.ask, w.tier))
            .collect()
    }

    /// 记下「这一档已经通知过了」。档位只升不降。
    pub fn note_escalation(
        &mut self,
        by: &Actor,
        now: Ts,
        ask: AskId,
        tier: WaitTier,
    ) -> Result<Applied<()>, Rejected> {
        self.transact(by, now, |d| {
            d.permit(Action::NoteEscalation, Ctx::default())?;
            let a = d.s.asks.get_mut(&ask).ok_or(Rejected::UnknownAsk(ask))?;
            if a.state != AskState::Open {
                return Err(Rejected::AskNotOpen(ask));
            }
            if tier <= a.escalated {
                return Err(Rejected::Invalid("escalation tiers only go up"));
            }
            a.escalated = tier;
            d.emit(EventKind::EscalationNoted { ask, tier });
            Ok(())
        })
    }

    /// 默认方案已到点、可以 `apply_default` 的问（只算还卡着活项的）。
    pub fn defaults_due(&self, now: Ts) -> Vec<AskId> {
        self.human_waits(now)
            .into_iter()
            .filter(|w| w.default_due_at.is_some_and(|t| t <= now))
            .map(|w| w.ask)
            .collect()
    }

    /// 行政巡检的代码检查部分。`unrecoverable` 同 `owner_lost`。
    pub fn patrol(&self, now: Ts, unrecoverable: &BTreeSet<SessionId>) -> Patrol {
        let views = self.entry_views();
        let mut channel_live: BTreeMap<ChannelId, bool> = BTreeMap::new();
        for it in self.state.items.values() {
            if let Some(ch) = &it.channel {
                *channel_live.entry(ch.clone()).or_default() |= it.status.is_live();
            }
        }
        Patrol {
            unowned_items: self.unowned_items(),
            unclaimed_entries: views
                .iter()
                .filter(|v| v.status == EntryStatus::Unclaimed)
                .map(|v| v.no)
                .collect(),
            needs_ruling: views
                .iter()
                .filter(|v| v.status == EntryStatus::NeedsRuling)
                .map(|v| v.no)
                .collect(),
            lost: self.owner_lost(now, unrecoverable),
            stop_escalated: self
                .state
                .stop_streaks
                .iter()
                .filter(|(_, s)| s.escalated)
                .map(|(sid, _)| sid.clone())
                .collect(),
            long_waits: self
                .human_waits(now)
                .into_iter()
                .filter(|w| w.tier >= WaitTier::Pinned)
                .map(|w| w.ask)
                .collect(),
            archivable_channels: channel_live
                .into_iter()
                .filter(|(_, live)| !live)
                .map(|(ch, _)| ch)
                .collect(),
        }
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-work`
Expected: `test result: ok. 100 passed; 0 failed`

- [ ] **Step 5: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/work/src/lib.rs crates/work/src/watch.rs
git diff --cached --stat
git commit -m "crewd-work: stop-check with anti-loop escalation, human-wait tiers, default-option expiry, patrol decisions

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 10: 公开 API、验收场景、性质测试、交付记录

`lib.rs` 加 re-export，让 M4b 写 `use crewd_work::*`。两个集成测试只用公开 API：`scenario` 是 spec §1 验收判据的纯逻辑版（派 → 整理 → 检查点 → 卡在人身上只卡那一件 → 同一小时推进别的 → 人回复直接叫醒 → 带证据完成；失联重派；落库再读回不丢到期检查与待答问题）；`fuzz` 用固定种子的随机命令序列（含墙钟回跳）逐步核：成功 → 整份不变量成立、序号连续、号码只增、终结项不再变、账本时间不倒退；被拒 → 账本与日志原样不动；所有只读视图在任何可达状态上不 panic。它还断言生成器真的让每种命令都成功过、成功总数够多——否则上面的检查可能在空转。

**Files:**
- Create: `crewd/crates/work/tests/scenario.rs`、`crewd/crates/work/tests/fuzz.rs`
- Modify: `crewd/crates/work/src/lib.rs`、`crewd/README.md`、`docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`

**Interfaces:**
- Consumes: Task 1–9 的全部公开项。
- Produces：crate 根的 re-export（见下方 `lib.rs`），即「公开 API 一览」的全部名字都能从 `crewd_work::` 直接取到。

- [ ] **Step 1: 写失败的测试**

`crates/work/tests/scenario.rs`（整个文件）：

```rust
//! spec §1 验收判据的纯逻辑版：只用公开 API，按时间顺序走一遍「派 → 整理 → 检查点 → 卡在人身上
//! 只卡那一件 → 人回复直接叫醒负责人 → 带证据完成」，以及失联重派、重启（落库再读回）不丢到期检查。
use crewd_work::*;
use std::collections::BTreeSet;

const T0: Ts = Ts(1_700_000_000_000);

fn sid(s: &str) -> SessionId {
    SessionId::new(s)
}

fn go(next: Ts, note: &str) -> Report {
    Report {
        status: Some(Progress::Active),
        next_check_at: Some(next),
        note: note.into(),
    }
}

fn woken_owners(events: &[Event]) -> Vec<SessionId> {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::BlockerCleared { owner, .. } => owner.clone(),
            _ => None,
        })
        .collect()
}

#[test]
fn todo_flows_from_dispatch_to_evidence_without_a_human_nudge() {
    let mut b = WorkBook::new(CrewId::new("crew"), Config::default());
    let a = Actor::worker("a");
    b.register_entry(T0, EntryNo(1)).unwrap();
    assert_eq!(
        b.entry_view(EntryNo(1)).unwrap().status,
        EntryStatus::Unclaimed
    );

    // 调度层派给 a：占位项「现在就查」。
    let t = T0.plus(MINUTE);
    let placeholder = b
        .claim_entry(
            &Actor::System,
            t,
            EntryNo(1),
            vec![NewItem::titled("整理 #1").owned_by("a").check_at(t)],
        )
        .unwrap()
        .value[0];
    let due = b.due_checkpoints(t);
    assert_eq!(
        due,
        [CheckpointDue {
            owner: sid("a"),
            items: vec![placeholder]
        }]
    );
    b.record_checkpoint_wake(&Actor::System, t, &[placeholder])
        .unwrap();

    // a 醒来，把入口整理成两项（前端依赖后端），占位项拆成它们的父项。
    let t = t.plus(MINUTE);
    let created = b
        .create_item(
            &a,
            t,
            NewItem::titled("后端")
                .under(ItemRef::Id(placeholder))
                .owned_by("a"),
        )
        .unwrap()
        .value;
    let backend = created;
    let frontend = b
        .create_item(
            &a,
            t,
            NewItem::titled("前端")
                .under(ItemRef::Id(placeholder))
                .after(ItemRef::Id(backend))
                .owned_by("a"),
        )
        .unwrap()
        .value;
    b.report(&a, t, placeholder, go(t.plus(2 * HOUR), "拆成后端、前端"))
        .unwrap();
    b.report(&a, t, backend, go(t.plus(HOUR), "先改 schema"))
        .unwrap();
    assert_eq!(b.item(placeholder).unwrap().missed_checks, 0, "回报清零");

    // 后端卡在人身上：只卡这一件，能先做的拆出来。
    let t = t.plus(10 * MINUTE);
    let blocked = b
        .block(
            &a,
            t,
            backend,
            vec![BlockOn::NewAsk {
                question: "旧数据要不要迁？".into(),
                default: None,
            }],
            Continuation::Split(vec![NewItem::titled("新表结构（不依赖答案）")]),
        )
        .unwrap()
        .value;
    let ask = blocked.asks[0];
    let part = blocked.split[0];
    assert_eq!(
        b.load_of(&sid("a")),
        3,
        "blocked 的后端不算负载：占位项、前端、拆出的部分"
    );

    // a 停下：拆出的部分还没排期 → 立刻再叫一次。
    assert_eq!(
        b.on_idle(&sid("a"), t),
        StopDecision::Rewake(vec![StopViolation {
            item: part,
            kind: StopKind::ReadyUnscheduled
        }])
    );
    // 同一小时内 a 推进了别的工作。
    let t = t.plus(5 * MINUTE);
    b.report(&a, t, part, go(t.plus(HOUR), "建新表")).unwrap();
    assert_eq!(b.on_idle(&sid("a"), t), StopDecision::Clean);

    // 人回复 → 直接叫醒负责人，后端回到 active、立刻到期。
    let t = t.plus(20 * MINUTE);
    let answered = b.answer_ask(&Actor::Human, t, ask, "要迁").unwrap();
    assert_eq!(woken_owners(&answered.events), [sid("a")]);
    assert_eq!(b.item(backend).unwrap().status, Status::Active);
    assert!(
        b.due_checkpoints(t)
            .iter()
            .any(|d| d.items.contains(&backend))
    );

    // 带证据收尾：子项先完，父项才完；入口跟着完成。
    let t = t.plus(30 * MINUTE);
    b.mark_done(&a, t, part, Evidence::Commit("1111111".into()))
        .unwrap();
    b.mark_done(&a, t, backend, Evidence::Commit("2222222".into()))
        .unwrap();
    b.report(&a, t, frontend, go(t.plus(HOUR), "后端好了，开前端"))
        .unwrap();
    b.mark_done(
        &a,
        t,
        frontend,
        Evidence::Url("https://example.invalid/pr/1".into()),
    )
    .unwrap();
    assert_eq!(
        b.entry_view(EntryNo(1)).unwrap().status,
        EntryStatus::InProgress
    );
    b.mark_done(&a, t, placeholder, Evidence::Text("前后端均已合入".into()))
        .unwrap();
    let view = b.entry_view(EntryNo(1)).unwrap();
    assert_eq!(
        (view.status, view.done, view.items.len()),
        (EntryStatus::Done, 1, 1)
    );

    // 事件序号连续、无重复。
    let seqs: Vec<u64> = b.log().iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (1..=b.state().last_seq).collect::<Vec<u64>>());
}

#[test]
fn silent_owner_is_replaced_and_the_new_owner_is_due_at_once() {
    let mut b = WorkBook::new(CrewId::new("crew"), Config::default());
    b.register_entry(T0, EntryNo(1)).unwrap();
    let w = b
        .claim_entry(
            &Actor::worker("a"),
            T0,
            EntryNo(1),
            vec![NewItem::titled("x")],
        )
        .unwrap()
        .value[0];
    b.report(&Actor::worker("a"), T0, w, go(T0.plus(HOUR), "开工"))
        .unwrap();
    let none = BTreeSet::new();
    let mut t = T0.plus(HOUR);
    for _ in 0..2 {
        let due = b.due_checkpoints(t);
        assert_eq!(due.len(), 1);
        b.record_checkpoint_wake(&Actor::System, t, &due[0].items)
            .unwrap();
        t = t.plus(30 * MINUTE);
    }
    let lost = b.owner_lost(t, &none);
    assert_eq!(
        lost,
        [LostOwner {
            item: w,
            owner: sid("a"),
            cause: LostCause::Silent { missed: 2 }
        }]
    );
    b.reassign_lost(&Actor::System, t, w, sid("b"), "a 两次检查没回报")
        .unwrap();
    assert_eq!(
        b.due_checkpoints(t),
        [CheckpointDue {
            owner: sid("b"),
            items: vec![w]
        }]
    );
    assert_eq!(b.owner_lost(t, &none), []);
}

#[test]
fn restart_loses_no_due_check_or_open_question() {
    let mut b = WorkBook::new(CrewId::new("crew"), Config::default());
    b.register_entry(T0, EntryNo(1)).unwrap();
    let ids = b
        .claim_entry(
            &Actor::worker("a"),
            T0,
            EntryNo(1),
            vec![NewItem::titled("x"), NewItem::titled("y")],
        )
        .unwrap()
        .value;
    b.report(&Actor::worker("a"), T0, ids[0], go(T0.plus(HOUR), "开工"))
        .unwrap();
    b.block(
        &Actor::worker("a"),
        T0,
        ids[1],
        vec![BlockOn::NewAsk {
            question: "q".into(),
            default: Some(DefaultOption {
                text: "按 A".into(),
                after_ms: 2 * HOUR,
            }),
        }],
        Continuation::Nothing,
    )
    .unwrap();

    // 「落库」= 序列化整份状态；「重启」= 读回。
    let saved = serde_json::to_string(b.state()).unwrap();
    let later = T0.plus(3 * HOUR);
    let back = WorkBook::restore(
        serde_json::from_str(&saved).unwrap(),
        Config::default(),
        later,
    )
    .unwrap();
    assert_eq!(back.due_checkpoints(later), b.due_checkpoints(later));
    assert_eq!(back.due_checkpoints(later)[0].items, [ids[0]]);
    assert_eq!(back.defaults_due(later), [AskId(1)]);
    assert_eq!(back.human_waits(later), b.human_waits(later));
}
```

`crates/work/tests/fuzz.rs`（整个文件）：

```rust
//! 性质测试：随机命令序列（确定性生成器，不引第三方 crate）打在一本账上，每一步之后检查
//! ① 成功 → 整份不变量成立、事件序号连续、号码只增、项不消失、终结项不复活、账本时间不倒退；
//! ② 被拒 → 账本与事件日志原样不动；
//! ③ 所有只读视图在任何可达状态上都不 panic。
//! 另确认生成器真的让每种命令都成功过、且成功得够多（否则上面的检查是空转）。
use crewd_work::invariants::check;
use crewd_work::*;
use std::collections::{BTreeMap, BTreeSet};

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, one_in: u64) -> bool {
        self.below(one_in) == 0
    }
}

const SESSIONS: [&str; 3] = ["a", "b", "c"];

/// 命令种类与抽中的权重。
const KINDS: [(&str, u64); 19] = [
    ("register_entry", 1),
    ("claim_entry", 6),
    ("create_item", 5),
    ("report", 9),
    ("block", 5),
    ("answer_ask", 3),
    ("apply_default", 2),
    ("mark_done", 4),
    ("drop_item", 3),
    ("relink", 2),
    ("request_transfer", 3),
    ("accept_transfer", 3),
    ("withdraw_transfer", 2),
    ("assign", 3),
    ("reassign_lost", 2),
    ("record_checkpoint_wake", 3),
    ("close_entry", 1),
    ("note_escalation", 2),
    ("link_channel", 2),
];

fn pick_kind(r: &mut Rng) -> &'static str {
    let total: u64 = KINDS.iter().map(|(_, w)| w).sum();
    let mut x = r.below(total);
    for (name, w) in KINDS {
        if x < w {
            return name;
        }
        x -= w;
    }
    unreachable!()
}

fn session(r: &mut Rng) -> SessionId {
    SessionId::new(SESSIONS[r.below(3) as usize])
}

fn any_actor(r: &mut Rng) -> Actor {
    match r.below(7) {
        0..=2 => Actor::worker(SESSIONS[r.below(3) as usize]),
        3 => Actor::captain("c"),
        4 => Actor::admin("d"),
        5 => Actor::Human,
        _ => Actor::System,
    }
}

/// 一半概率让操作者就是该项的负责人（否则几乎全被权限拒掉，测不到深处）。
fn actor_for(r: &mut Rng, b: &WorkBook, id: WorkId) -> Actor {
    match b.item(id).and_then(|it| it.owner.clone()) {
        Some(owner) if r.chance(2) => Actor::worker(owner.as_str()),
        _ => any_actor(r),
    }
}

/// 多数时候挑一个满足 `pred` 的项；偶尔挑任意已发出的号或一个还不存在的号。
fn item_where(r: &mut Rng, b: &WorkBook, pred: impl Fn(&WorkItem) -> bool) -> WorkId {
    let hits: Vec<WorkId> = b
        .state()
        .items
        .values()
        .filter(|it| pred(it))
        .map(|it| it.id)
        .collect();
    if hits.is_empty() || r.chance(4) {
        WorkId(1 + r.below(u64::from(b.state().last_item) + 1) as u32)
    } else {
        hits[r.below(hits.len() as u64) as usize]
    }
}

fn item(r: &mut Rng, b: &WorkBook) -> WorkId {
    item_where(r, b, |it| it.status.is_live())
}

/// 多数时候挑一条还开着的问。
fn ask(r: &mut Rng, b: &WorkBook) -> AskId {
    let open: Vec<AskId> = b
        .state()
        .asks
        .values()
        .filter(|a| a.state == AskState::Open)
        .map(|a| a.id)
        .collect();
    if open.is_empty() || r.chance(4) {
        AskId(1 + r.below(u64::from(b.state().last_ask) + 1) as u32)
    } else {
        open[r.below(open.len() as u64) as usize]
    }
}

fn entry(r: &mut Rng, b: &WorkBook) -> EntryNo {
    EntryNo(1 + r.below(b.state().entries.len() as u64 + 1) as i64)
}

fn text(r: &mut Rng, s: &str) -> String {
    if r.chance(12) { " ".into() } else { s.into() }
}

fn evidence(r: &mut Rng) -> Evidence {
    match r.below(5) {
        0 => Evidence::Commit("abc".into()),
        1 => Evidence::Url("https://example.invalid".into()),
        2 | 3 => Evidence::Text("ok".into()),
        _ => Evidence::Text("".into()),
    }
}

/// 下次检查时间：多数合法，偶尔过去 / 太远 / 不给。
fn check_time(r: &mut Rng, now: Ts) -> Option<Ts> {
    match r.below(6) {
        0 => None,
        1 => Some(now.plus(-(r.below(HOUR as u64) as i64))),
        2 => Some(now.plus(2 * HOUR + 1 + r.below(HOUR as u64) as i64)),
        _ => Some(now.plus(1 + r.below(2 * HOUR as u64) as i64)),
    }
}

fn new_item(r: &mut Rng, b: &WorkBook, index: usize) -> NewItem {
    let mut n = NewItem::titled(&text(r, "task"));
    if r.chance(4) {
        n.parent = Some(if index > 0 && r.chance(2) {
            ItemRef::Batch(r.below(index as u64 + 1) as usize)
        } else {
            ItemRef::Id(item(r, b))
        });
    }
    if r.chance(3) {
        n.depends_on.push(if index > 0 && r.chance(2) {
            ItemRef::Batch(r.below(index as u64) as usize)
        } else {
            ItemRef::Id(item(r, b))
        });
    }
    if r.chance(3) {
        n.owner = Some(session(r));
    }
    if r.chance(4) {
        n.next_check_at = check_time(r, b.state().clock);
    }
    n
}

fn batch(r: &mut Rng, b: &WorkBook, max: u64) -> Vec<NewItem> {
    (0..r.below(max + 1) as usize)
        .map(|i| new_item(r, b, i))
        .collect()
}

fn ok<T>(res: Result<Applied<T>, Rejected>) -> Result<Vec<Event>, Rejected> {
    res.map(|a| a.events)
}

/// 随机挑一条命令打上去，返回命令名与结果。
fn step(r: &mut Rng, b: &mut WorkBook, now: Ts) -> (&'static str, Result<Vec<Event>, Rejected>) {
    let kind = pick_kind(r);
    let res = match kind {
        "register_entry" => {
            let no = EntryNo(b.state().entries.len() as i64 + 1);
            ok(b.register_entry(now, no))
        }
        "claim_entry" => {
            let (by, e) = (any_actor(r), entry(r, b));
            let items = batch(r, b, 3);
            ok(b.claim_entry(&by, now, e, items))
        }
        "create_item" => {
            let by = any_actor(r);
            let n = new_item(r, b, 0);
            ok(b.create_item(&by, now, n))
        }
        "report" => {
            let id = item(r, b);
            let by = actor_for(r, b, id);
            let status = match r.below(3) {
                0 => None,
                1 => Some(Progress::Open),
                _ => Some(Progress::Active),
            };
            let report = Report {
                status,
                next_check_at: check_time(r, now),
                note: text(r, "progress"),
            };
            ok(b.report(&by, now, id, report))
        }
        "block" => {
            let id = item(r, b);
            let by = actor_for(r, b, id);
            let mut on = Vec::new();
            for _ in 0..r.below(3) {
                on.push(match r.below(3) {
                    0 => BlockOn::NewAsk {
                        question: text(r, "q?"),
                        default: r.chance(2).then(|| DefaultOption {
                            text: "A".into(),
                            after_ms: r.below(4 * HOUR as u64) as i64,
                        }),
                    },
                    1 => BlockOn::Ask(ask(r, b)),
                    _ => BlockOn::Item(item(r, b)),
                });
            }
            let rest = if r.chance(2) {
                Continuation::Nothing
            } else {
                Continuation::Split(batch(r, b, 2))
            };
            ok(b.block(&by, now, id, on, rest))
        }
        "answer_ask" => {
            let by = if r.chance(2) {
                Actor::Human
            } else {
                any_actor(r)
            };
            let h = ask(r, b);
            ok(b.answer_ask(&by, now, h, &text(r, "yes")))
        }
        "apply_default" => {
            let by = if r.chance(2) {
                Actor::System
            } else {
                any_actor(r)
            };
            let h = ask(r, b);
            ok(b.apply_default(&by, now, h))
        }
        "mark_done" => {
            let id = item(r, b);
            let by = actor_for(r, b, id);
            ok(b.mark_done(&by, now, id, evidence(r)))
        }
        "drop_item" => {
            let id = item(r, b);
            let by = actor_for(r, b, id);
            let kind = [
                DropKind::Duplicate,
                DropKind::Superseded,
                DropKind::NotNeeded,
            ][r.below(3) as usize];
            let ev = r.chance(2).then(|| evidence(r));
            let reason = DropReason {
                kind,
                note: text(r, "why"),
                evidence: ev,
            };
            ok(b.drop_item(&by, now, id, reason))
        }
        "relink" => {
            let id = item(r, b);
            let by = actor_for(r, b, id);
            let parent = r.chance(2).then(|| item(r, b));
            let deps = (0..r.below(3)).map(|_| item(r, b)).collect();
            ok(b.relink(&by, now, id, parent, deps))
        }
        "request_transfer" => {
            let id = item(r, b);
            let by = actor_for(r, b, id);
            let to = session(r);
            ok(b.request_transfer(&by, now, id, to, &text(r, "handover")))
        }
        "accept_transfer" => {
            let id = item_where(r, b, |it| it.pending_transfer.is_some());
            let by = match b.item(id).and_then(|it| it.pending_transfer.clone()) {
                Some(t) if r.chance(2) => Actor::worker(t.to.as_str()),
                _ => any_actor(r),
            };
            ok(b.accept_transfer(&by, now, id))
        }
        "withdraw_transfer" => {
            let id = item_where(r, b, |it| it.pending_transfer.is_some());
            let by = actor_for(r, b, id);
            ok(b.withdraw_transfer(&by, now, id))
        }
        "assign" => {
            let (id, by, to) = (item(r, b), any_actor(r), session(r));
            ok(b.assign(&by, now, id, to, &text(r, "assign")))
        }
        "reassign_lost" => {
            let (id, by, to) = (item(r, b), any_actor(r), session(r));
            ok(b.reassign_lost(&by, now, id, to, &text(r, "lost")))
        }
        "record_checkpoint_wake" => {
            let ids: Vec<WorkId> = (0..1 + r.below(2)).map(|_| item(r, b)).collect();
            let by = if r.chance(2) {
                Actor::System
            } else {
                any_actor(r)
            };
            ok(b.record_checkpoint_wake(&by, now, &ids))
        }
        "close_entry" => {
            let (by, e) = (any_actor(r), entry(r, b));
            let ev = r.chance(2).then(|| evidence(r));
            ok(b.close_entry(&by, now, e, &text(r, "ruling"), ev))
        }
        "note_escalation" => {
            let by = if r.chance(2) {
                Actor::System
            } else {
                any_actor(r)
            };
            let tier =
                [WaitTier::Fresh, WaitTier::Pinned, WaitTier::StillWanted][r.below(3) as usize];
            ok(b.note_escalation(&by, now, ask(r, b), tier))
        }
        "link_channel" => {
            let id = item(r, b);
            let by = actor_for(r, b, id);
            let ch = r.chance(2).then(|| ChannelId::new("ch"));
            ok(b.link_channel(&by, now, id, ch))
        }
        other => unreachable!("{other}"),
    };
    (kind, res)
}

/// 所有只读视图都要能在任何可达状态上跑（它们按不变量用 `[]` 取值，坏状态会 panic）。
fn exercise_views(b: &WorkBook, now: Ts) {
    let dead: BTreeSet<SessionId> = [SessionId::new("b")].into();
    let _ = b.entry_views();
    let _ = b.ready_items();
    let _ = b.unowned_items();
    let _ = b.due_checkpoints(now);
    let _ = b.owner_lost(now, &dead);
    let _ = b.human_waits(now);
    let _ = b.escalations_due(now);
    let _ = b.defaults_due(now);
    let _ = b.patrol(now, &dead);
    for s in SESSIONS {
        let s = SessionId::new(s);
        let _ = b.my_items(&s);
        let _ = b.load_of(&s);
        let _ = b.incoming_transfers(&s);
        let _ = b.stop_violations(&s, now);
    }
}

fn run(seed: u64, steps: usize, wins: &mut BTreeMap<&'static str, usize>) {
    let mut r = Rng::new(seed);
    let mut b = WorkBook::new(CrewId::new("crew"), Config::default());
    let mut now = Ts(1_700_000_000_000);
    b.register_entry(now, EntryNo(1)).unwrap();
    for i in 0..steps {
        // 时间大多往前走，偶尔墙钟往回跳。
        now = if r.chance(20) {
            now.plus(-(r.below(20 * MINUTE as u64) as i64))
        } else {
            now.plus(r.below(40 * MINUTE as u64) as i64)
        };
        let before = b.state().clone();
        let log_len = b.log().len();
        let (kind, res) = step(&mut r, &mut b, now);
        let ctx = format!("seed {seed} step {i} {kind}");
        match res {
            Ok(events) => {
                *wins.entry(kind).or_default() += 1;
                let after = b.state();
                assert_eq!(check(after, b.config(), after.clock), Ok(()), "{ctx}");
                let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
                let expect: Vec<u64> = (before.last_seq + 1..=after.last_seq).collect();
                assert_eq!(seqs, expect, "{ctx}: 序号连续");
                assert!(events.iter().all(|e| e.at == after.clock), "{ctx}");
                assert_eq!(b.log().len(), log_len + events.len(), "{ctx}");
                assert!(after.last_item >= before.last_item, "{ctx}");
                assert!(after.last_ask >= before.last_ask, "{ctx}");
                assert!(after.clock >= before.clock, "{ctx}: 账本时间不倒退");
                for (id, old) in &before.items {
                    let new = &after.items[id];
                    assert!(new.revision >= old.revision, "{ctx}");
                    if old.status.is_terminal() {
                        assert_eq!(new, old, "{ctx}: 终结项不再变");
                    }
                }
            }
            Err(_) => {
                assert_eq!(b.state(), &before, "{ctx}: 被拒的命令改了账本");
                assert_eq!(b.log().len(), log_len, "{ctx}");
            }
        }
        if r.chance(5) {
            let s = session(&mut r);
            let _ = b.on_idle(&s, now);
            let st = b.state();
            assert_eq!(check(st, b.config(), st.clock), Ok(()), "{ctx}: on_idle");
        }
        exercise_views(&b, now);
    }
}

#[test]
fn random_command_sequences_preserve_every_invariant() {
    let mut wins = BTreeMap::new();
    for seed in 0..48 {
        run(seed, 400, &mut wins);
    }
    let total: usize = wins.values().sum();
    assert!(
        total > 3_000,
        "成功的命令太少（{total} / 19200），生成器没打到深处：{wins:?}"
    );
}

#[test]
fn every_command_kind_succeeds_somewhere() {
    let mut wins = BTreeMap::new();
    for seed in 100..148 {
        run(seed, 400, &mut wins);
    }
    let never: Vec<&str> = KINDS
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| wins.get(k).copied().unwrap_or(0) < 5)
        .collect();
    assert_eq!(never, Vec::<&str>::new(), "这些命令成功不到 5 次：{wins:?}");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-work --tests`
Expected: 两个集成测试编译失败，`cannot find type WorkBook in this scope` 一类（`use crewd_work::*` 现在只带进模块名，还没有 re-export）。

- [ ] **Step 3: 实现**

`crates/work/src/lib.rs`（整个文件）：

```rust
//! crewd 的工作跟进核心（M4a）：两层 Todo（入口 #N + 工作项 W<n> 的 DAG）、负责人、检查点、
//! 卡住只卡那一件、停下即检查、等人升级、行政巡检的纯判定（spec §4–§6、§8、§9）。
//! sans-IO：不读时钟、不碰文件 / 数据库 / tokio；时间一律由调用方传入（Unix 毫秒）。持久化与 MCP 接线归 M4b。
pub mod block;
pub mod book;
pub mod events;
pub mod graph;
pub mod ids;
pub mod invariants;
pub mod model;
pub mod owner;
pub mod perm;
pub mod progress;
#[cfg(test)]
mod testkit;
pub mod views;
pub mod watch;

pub use block::{BlockOn, BlockResult, Continuation};
pub use book::{Applied, Rejected, WorkBook};
pub use events::{Event, EventKind};
pub use ids::{
    AskId, ChannelId, CrewId, EntryNo, HOUR, MINUTE, ParseIdError, SessionId, Ts, WorkId,
};
pub use invariants::Violation;
pub use model::{
    Actor, AskState, Blocker, Body, BookState, Config, DefaultOption, DoneLevel, DropKind,
    DropReason, EntryClosure, EntryTodo, Evidence, HowChanged, HumanAsk, ItemRef, NewItem, Note,
    OwnerChange, PendingTransfer, Role, Status, StopKind, StopStreak, StopViolation, WaitTier,
    WorkItem,
};
pub use perm::Action;
pub use progress::{Progress, Report};
pub use views::{CheckpointDue, EntryStatus, EntryView, LostCause, LostOwner, MyItem};
pub use watch::{HumanWait, Patrol, StopDecision, patrol_due, wait_tier};
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-work`
Expected: 单测 `100 passed`；`tests/fuzz.rs` `2 passed`（debug 约 1–2s）；`tests/scenario.rs` `3 passed`。

- [ ] **Step 5: 验这把尺子会红（性质测试不是空转）**

把事务外壳里的不变量校验整个短路掉，性质测试必须当场抓到：

```bash
cp crates/work/src/book.rs crates/work/src/book.rs.bak
sed -i '' 's/        invariants::check(&d.s, &self.cfg, now).map_err(Rejected::Invariant)?;/        let _ = invariants::check(\&d.s, \&self.cfg, now);/' crates/work/src/book.rs
grep -c 'let _ = invariants::check' crates/work/src/book.rs
cargo test -p crewd-work --test fuzz 2>&1 | grep -E 'FAILED|test result|panicked at' -A1
cargo test -p crewd-work --lib 2>&1 | grep -E 'test result'
cp crates/work/src/book.rs.bak crates/work/src/book.rs
cmp crates/work/src/book.rs crates/work/src/book.rs.bak && rm crates/work/src/book.rs.bak
```

Expected：`grep -c` 输出 `1`；`fuzz` 两条都 FAILED（写计划时读数：`seed 0 step 17 block`、`seed 100 step 68 relink` 处整份不变量不成立）；单测 `89 passed; 11 failed`；还原后 `cmp` 无输出。

- [ ] **Step 6: README 与 spec 交付记录**

`README.md` 在「终端核心」那一行下面加一行：

```markdown
- 工作跟进核心：`crates/work`（两层 Todo、工作项 DAG 与不变量、负责人、检查点、停下即检查、等人升级、巡检判定；纯计算，不起进程、不用 tokio、不读时钟；持久化与 MCP 接线在 M4b）
```

spec `docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md` 末尾追加下面一节。写计划时 spec 的最后一节是 §17；若执行时末尾已多出别的节（例如 M2b 的交付记录先合入了），把标题里的 18 改成「最后一节编号 + 1」：

```markdown
## 18. M4a 交付记录与遗留（分支 crewd-m4a，计划 `docs/superpowers/plans/2026-10-06-crewd-m4a-work-core.md`）

M4 拆为 **M4a** 工作跟进核心（纯计算，本节）→ **M4b** 持久化（store 的下一个 schema 版本）+ MCP 工具（§9）+ 检查点 / 人回复 / 停下即检查的唤醒接线 + 「我的工作项」注入（§4.3）+ 迁移（§10）。

已交付：`crewd/crates/work`（crewd-work；只依赖 serde，不起进程、不碰 tokio / 文件 / 数据库、不读时钟）——编号 `W<n>` / `H<n>` / `#N` 分开命名空间（字符串形式唯一、按机组编号）；字段表（§4.2 全部字段 + `channel`（§17）、`lastReport`（§4.3 的「下一步」）、`dropReason`）；权限表（负责人 / 机长 / 行政处 / 人 / 调度层）；父项、依赖、卡点三种边的并图找环与整份不变量（每次修改后整份校验，被拒 = 什么都不动、不消耗号码）；命令：登记 / 认领入口、建项（含无主项）、关联群、回报 + 下次检查、卡住（必答「其余能否先推进」、可拆分、新问可附默认方案）、人回复、默认方案到点、带证据完成、事实性 / 业务放弃、改链接、入口裁定关闭、转交请求 / 接受 / 撤回、指派、失联重派、检查点叫醒记账、升级记账；视图：入口状态（由第二层推导）、就绪 / 待认领 / 负载、我的工作项、到期检查点（按负责人合并）、失联候选、停下即检查（含防死循环）、人的待确认清单与 24h / 72h 升级、默认方案到点、巡检汇总与触发；事件日志。持久化边界 = 整份 `BookState`（serde）+ 事件，`WorkBook::restore` 读回时整份校验。验收：§1 判据的纯逻辑场景测试 3 条；固定种子的随机命令序列性质测试（48 × 400 步 × 2 组，含墙钟回跳）。

与本 spec 原文不同 / 补充的决定（理由见计划「裁定」）：
- 图的边方向：父 → 子、项 → 依赖、项 → 卡住它的项（卡点里的工作项也进图）。父项可以依赖子项，子项不能依赖祖先，两项互相卡住被拒。
- 补的不变量：active / blocked 必须有负责人；nextCheckAt 只在有负责人的 open / active 项上；blockedOn 不指向已结束的问或已终结的项；活项不挂在已终结的父项下（done 要求子项全部终结）、不依赖 dropped 项、不来自已裁定关闭的入口。
- 人回复 / 默认方案到点 / 卡它的项完成：卡点清空且依赖已齐 → active、nextCheckAt = now；依赖没齐 → open（§5 原文「回到 active」与 §4.2「依赖未齐不能 active」冲突，按不变量）。卡点没清空也照样叫醒负责人。
- 拆出的子项不继承入口；入口状态只看直接关联项；没有活项时至少一项 done 即完成，全部 dropped 才交裁定。
- 停下即检查的「active 缺 nextCheckAt」在不变量下不可能，换成「检查时间已过」；另加「叫醒过没回报」「依赖已齐的 open 项没排检查」。连续 3 次违反集合不变 → 交行政处，之后静默。
- 失联：检查点唤醒后检查时间顺延 30 分钟（`recheck_after_ms`，本计划定，可配）；missedChecks ≥ 2 且顺延时间也过 → 失联候选；负责人对自己项的任何成功操作都清零，别人替它回报不清零。恢复失败的 session 名下的 blocked 项也算失联（否则人回复了没人接）。
- 账本记录见过的最晚时间，命令用 max(now, clock)：墙钟往回跳不会让「检查时间 ≤ now + 2h」对已有项突然不成立而卡死整本账。
- 人的待确认清单只列此刻仍卡着活项的问。
- 行政巡检只把「停下即检查交过来的 session」「关联项全部终结的群」判为需要起行政 session；失联重派、待认领、入口派活由调度层直接做。

遗留（M4b 起处理）：
- H<n> 现由工作账本按机组发；M3 的人类待确认账本若另有编号，M4b 必须合成一个命名空间（计划「给人的待决问题」Q1）。
- 停下即检查的连续计数是软状态：存在 `BookState` 里但不产出事件，M4b 若只按事件增量落库会丢它（丢了最多多叫 3 次）。
- 事务克隆整份状态（几百项量级够用）；`Rejected` 没有人类可读文案，MCP 层映射。
- 跨机组 DAG 边（§13 Q5）仍不做；一本账 = 一个机组在结构上排除了它。
```

- [ ] **Step 7: 全量检查并提交**

```bash
./scripts/check.sh
git status --short
git add crates/work/src/lib.rs crates/work/tests/scenario.rs crates/work/tests/fuzz.rs README.md ../docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md
git diff --cached --stat
git commit -m "crewd-work: public re-exports, acceptance scenario and randomized property tests; M4a delivery record

Co-Authored-By: Claude <noreply@anthropic.com>"
```

`git diff --cached --stat` 应该正好 6 个文件；`git status --short` 里不应有 `.bak`。

---

## 覆盖对照（spec → 任务）

| spec 条目 | 任务 |
|---|---|
| §4.1 入口原文不改、`#N` 沿用；状态由第二层推导（待认领 / 进行中 / 完成 / 全 dropped 交裁定） | 4（登记）、8（`entry_view`）、6（`close_entry` 裁定） |
| §4.1 认领入口 = 登记工作项；入口下自动一行说明 + 链接 | 4（`claim_entry`、`EntryClaimed` 事件）、8（`EntryView`：项、完成数、负责人）；文字渲染归 M4b |
| §4.2 字段表全部字段（W<n>、parent、dependsOn、sources、owner、ownerHistory、status、blockedOn、nextCheckAt、missedChecks、evidence、pendingTransfer、revision、createdAt、updatedAt） | 2（类型）、4（revision / 时间戳自动维护） |
| §4.2 不变量：无环（parent ∪ dependsOn）；单负责人且只有负责人 / 机长 / 行政处改状态；active ⇒ nextCheckAt ≤ now + 上限；done ⇒ evidence；依赖未齐不能 active；blocked ⇒ blockedOn 非空 | 3（`check`）、2（`perm`）、4（每次修改整份校验）、10（性质测试） |
| §4.2 v1 依赖只限同机组 / §13 Q5 | 裁定 2（一本账 = 一个机组） |
| §4.3 我的工作项（id、标题、状态、下一步、检查时间、被谁卡） | 8（`my_items`、`incoming_transfers`）；注入归 M4b |
| §5 绑定负责人、空负责人 = 待认领 | 4（建项 / 认领）、7（`assign` 领无主项）、8（`unowned_items`） |
| §5 换负责人：原负责人发起 + 新负责人接受，或机长强制；说明必填，写 ownerHistory | 7 |
| §5 检查点：到点唤醒，同一 session 合并一条 | 8（`due_checkpoints`）、7（`record_checkpoint_wake`）；投递归 M4b |
| §5 负责人失联：K 次无回报或不可恢复 → 重派（排除原负责人） | 7（`reassign_lost` 拒绝原负责人）、8（`owner_lost`） |
| §5 停下即检查 + 3 次无改善交行政处 | 9（`stop_violations`、`on_idle`） |
| §5 人回复 → 直接唤醒负责人、blocked → active 且 nextCheckAt = now | 5（`answer_ask`、`release`、`BlockerCleared`） |
| §6 标 blocked 必须关联待确认 / 工作项并回答能否先推进，能则拆子项 | 5（`block`、`Continuation`） |
| §6 blocked 不计负载 | 8（`load_of`） |
| §6 等人 24h 置顶、72h 问「还要吗」 | 9（`human_waits`、`escalations_due`、`note_escalation`） |
| §6 默认方案（X 小时无回复按 A 办）/ §13 Q2 到点照常走审批 | 5（`apply_default`）、9（`defaults_due`）；审批话术归 M4b |
| §8.2 行政无业务决策权，只能基于事实关闭 | 2（`perm`）、6（事实性放弃必带证据、入口事实关闭） |
| §8.4 巡检触发（10 条消息 / 30min）、只有需要动手的事才起行政 session | 9（`patrol_due`、`Patrol::needs_admin_session`） |
| §9 MCP 工具语义 | 4–9 的命令；接线见「M4b 接线对照」 |
| §10 迁移（plan.json → 工作项归机长；未结入口 → 占位项 nextCheckAt = now 或留待认领） | 4（`check_at(now)` 允许、`register_entry`）、10（场景测试里的派活占位）；导入本身归 M4b |
| §12 单元：DAG 不变量、换负责人规则、检查点调度（假时钟）、停下即检查与防死循环、入口状态推导 | 3、7、8、9、10 |
| §13 Q4 退出 ≠ 失联 | 8（`owner_lost` 只认调用方给的「恢复失败」名单） |
| §13 Q9 机长可接活 | 4（机长认领缺省自己当负责人） |
| §17 工作项可关联群；换负责人拉进关联群；结束后归档闲置群 | 4（`link_channel`）、7（`OwnerChanged.channel`）、9（`archivable_channels`） |

## 自查记录（写完计划后对着 spec 跑的一遍）

- **覆盖**：上表逐条有任务。没有任务的 spec 条目都明确归了别的里程碑（见「范围 / 不做」）。
- **占位扫描**：每个代码步骤都是完整代码（由写计划时跑通的源文件原样嵌入）；唯一的「按执行时情况改」是 Task 10 Step 6 的 spec 节号，给了确定的规则。
- **类型一致**：各任务的 Interfaces 与代码同源；命令签名统一为 `(&mut self, by: &Actor, now: Ts, ..) -> Result<Applied<T>, Rejected>`（`register_entry` 没有 `by`，固定为 System；`on_idle` 只动软状态，直接返回 `StopDecision`）。
- **Review Focus**：5 条各有一个钉住它的测试（写在对应任务的测试里），其中 1、5 各有一个变异步骤证明那条测试会红；第 2 条另由性质测试逐条覆盖。

## 给人的待决问题

> **已拍板（2026-10-06，执行时记录；详见 spec §19）**：Q1 H 号只由工作账本按机组发，M3 的人类待确认台账复用这套编号；Q2 人在机长之上、可直接改任何工作项；Q3 `recheck_after_ms` 与 K 在 `Config` 里可配，默认 15 分钟、K=2（约 30 分钟判失联）。

1. **H<n> 归谁发**：本计划让工作账本在「卡住时新开的问」上按机组发 H 号。M3 的人类待确认 / 审批台账如果有自己的编号，M4b 必须合成一个命名空间（否则又是两本账号码会撞）。要不要现在就定「H 号只由 M3 的待确认账本发，工作账本只引用」？
2. **人 = 机长之上**：本计划让人能直接回报 / 卡住 / 完成 / 放弃 / 指派任何工作项（UI 上会出现这些按钮的前提）。如果你只想在 UI 上「回复待确认 + 裁定入口」，权限表收窄是一行改动。
3. **叫醒后多久再叫 / 判失联**：默认 30 分钟（K=2 → 一个不回话的 session 最快约 1 小时被判失联、换人）。这个节奏你觉得对吗？
