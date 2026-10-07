# crewd M3a.2 协作模型对齐（N1）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 2026-10-07 的人类裁定与采纳的消息模型落到 main 上的数据层与协作层（`crewd-store` / `crewd-proto` / `crewd-collab` / `crewd-legacy` / `crewd-server` 的 M3a 部分），在 M3b T12 与 M3c T10a 之前完成，两条分支随后合 main：
1. **删掉审批**（人 2026-10-07：「pendingcrew 不做审批了，claude code 和 codex 会处理好的」）：collab `approvals`、proto 的审批类型 / 方法 / 事件、server 的审批 RPC、渲染里的审批文字、唤醒里的审批来源；v4 迁移删表。
2. **H2 单根**：总机组是机组树唯一的根 = 组织管理层，其余机组都在它下面；每个**一级机组**（总机组的直系子）保留自己的人类群，总机组有自己的（管理用）人类群；今天一切「找根机组的人类群」改成「找一级机组的人类群」（总机组自己 → 它自己的）。`builtin` 特例分支删掉，只留轻量的根规则。旧数据的多个根挂到总机组下。
3. **消息模型**：看得见 = 群成员（+ B15 的可读规则）；@ 只决定叫醒谁——删 @ 收窄与 `broadcast` 这个 mention 种类；dm 他人可读（B15，没有私有群）；在人类群里回复一条问 = 回答这条问；跨机组说话 = dm（`post.to` 在 M3b T12/T13 做，这里只给 collab 侧原语）。
4. 顺带：session 标题（S80）、删机组 = 归档（S22，人已同意）、拒绝文字的稳定码（S71）。

**Architecture:** 不加新 crate、不改分层（spec §22.2 的三层：store / collab / server）。schema v3 → **v4**（一次迁移：删审批表、`crews.builtin` 改名 `org_root` 并保证唯一、旧根挂到总机组下、去掉库里的 `broadcast` mention、作废排队中的审批唤醒、`crews.archived_at`、session 标题回填）。collab 里「人类群归属」只有一个函数 `crews::home_of`，`root_of` / `is_root` 删除（名字留着、语义悄悄变是已知的坑：编译错误就是给 M3b / M3c 的合并清单）。拒绝从自由字符串改成 `Refusal { code, args }`，文字放 `prompts/refusals.{zh,en}.md`。

**Tech Stack:** 同 M3a（Rust 1.94 / edition 2024、rusqlite 0.40 bundled、serde、schemars 1.2、chrono、uuid）。不新增第三方依赖。

**Spec:** `docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`：**§24（2026-10-07 复审：改进计划与裁定，本计划的直接来源）**、§22.2（群规则、根机组、迁移）、§22.5（冻结接口：本计划**有意破坏**其中几条签名，见「接口变更」）、§22.7、§23。研究底稿（私有，不进仓库）：`docs/private/2026-10-07-studies/{synthesis,rethink-proposals,oss-agent-collab}.md`（S64 / S66 / S68 / S71 / S80 / S22、RT#1–#4、AC§3 / §5）。

> **本计划的代码没有编译过。** 写计划时读的是 `main@ef3f1d4`（含 M3a / M3a.1）的源码。代码块是精确的意图：编不过时按编译器修语法 / 借用 / 导入，**不改语义、不改测试断言**；改了断言的一律在交付记录里写明为什么。每个任务的「确认失败」步骤必须真红过才往下走。文中的 `file:line` 都是 `ef3f1d4` 上的行号，前面的任务改过文件后会漂，以 grep 为准。

## Global Constraints

- 所有命令在 worktree 的 `crewd/` 目录下跑（`/Users/hey/Untitled/Pendingname/PendingCrew/.claude/worktrees/crewd-n1-collab/crewd`），不 `cd` 到别的 worktree；找不到 `cargo` 先 `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`。
- 不加依赖；`Cargo.lock` 不应变化（变了先查为什么）。
- 依赖方向不变：collab → store / proto / work；server → collab。collab 不读时钟、不碰 tokio。
- **给 LLM 看的文字不写进代码**：渲染文字在 `crates/collab/prompts/render.{zh,en}.md`，拒绝文字（Task 10 起）在 `crates/collab/prompts/refusals.{zh,en}.md`；节缺失 / 两种语言节名或占位符不一致有测试。
- 线上 JSON camelCase；params 一律 `deny_unknown_fields`；契约改了就重生成 `schema/crewd-api.schema.json`（`cargo run -p crewd -- schema > schema/crewd-api.schema.json`），e2e 的漂移测试会盯。本计划把 `API_VERSION` 从 2 提到 **3**（Task 1，见裁定 9）。
- 测试**只用合成数据**（仓库公开）：`crew-a`、`cap-a`、`w-a1`、「示例消息」这类编的名字。Task 11 的真实数据演练在**副本**上跑，计数只写进私有的任务报告（`.superpowers/`），**不写进 spec**。
- 迁移在一个事务里完成：失败 = 库保持 v3、crewd 拒绝启动并说清原因。
- 每个任务结束跑 `./scripts/check.sh`（fmt + clippy -D warnings + test，全 workspace）全绿再提交。**不跑 `--ignored` 测试**。不按名字 / 模式杀进程。
- **变异核对**：变异前先把文件 `cp` 到 scratchpad 备份，还原用 `cp` 拷回；**不要** `git checkout -- <file>`（会丢掉未提交的实现，且伪装成「这刀没有测试红」）。红了要看**红的是哪条用例名**，期望的那条没红就是方向反了。
- 提交：只 `git add` 该任务列出的文件（显式路径，相对 `crewd/`；spec 写 `../docs/...`），提交前 `git diff --cached --stat` 看行数；**不 `--amend`**；提交信息末尾一行 `Co-Authored-By: Claude <noreply@anthropic.com>`。
- 本机 shell 是 zsh：照抄命令，别把多个参数塞进一个变量再展开（zsh 不做词分割）。
- **文件归属**：本计划只改 main 上已有的 M3a 文件与 store / proto / legacy；**不碰** `crates/wake/`（只在 crewd-m3c 分支上）、`server/src/agent/**`、`server/src/orchestra/**`、`collab/src/{worldview,turn_trace}.rs`（M3b 接手的桩，本计划只在编译需要时改一行）、`exec/src/launch.rs`（M3b 的文件，见「合并注意」）。

## 次序与外部依赖

- **N4（存储安全：迁移前快照）必须先合 main，N1 才能合 main。** v4 是切换后第一次会在真实库上跑的迁移（删表、改父边、改 mention），必须在 N4 的「迁移前 `VACUUM INTO` 快照」之后。执行 N1 时随时可以先做；合 main 前 `git merge main`（含 N4），若 N4 改了 `schema::migrate` 的形状（例如统一迁移执行器 S29），把 `migrate_v4` 按 N4 的形状放进去，**快照必须在 v4 的任何 DDL 之前**。
- **M4b 原定的 v4（工作账本事件表等）顺延为 v5。** 本计划占用 v4。
- M3b / M3c 在 N1 合 main 之后 `git merge main`；冲突与编译错误按文末「合并注意」逐条处理。
- **不碰 crewd-wake**：唤醒侧跟进（`plan.rs` 去收窄、dm 默认叫醒对方、C22 对拍改写、审批唤醒）列在文末给 M3c T10a。

## 范围

做：删审批（collab / proto / server / 渲染 / 唤醒来源 / v4 删表）；schema v4 + 迁移 + 导入器；总机组单根（`org_root` 唯一、旧根挂到它下面、号码 0、一级机组人类群）；`crews::{org_root, is_org_root, home_of, is_home}` 取代 `root_of` / `is_root`；单根下的 create / adopt / release / delete / reorder 规则，删 builtin 特例（不能改名、不提问、不参与收编的那几处）；@ 不再收窄可见性、删 `MentionKind::Broadcast`（proto / store 词表 / 迁移剥掉旧数据 / 导入丢掉）、reply_to 只 @ 原发送者（且只在他仍是成员时）、注入行显示 @ 了谁；B15 可读规则 `channels::{readable_by, readable_channels}`、`channels::dm_peer`、`directory::resolve`（`post.to` 的原语）；人类群里回复一条开着的问 = 回答它；session 标题必填或从 brief 派生（≤18 字）+ 旧数据回填；`crew.delete` = 归档、`crew.unarchive`、永久删除只留 collab 内部函数 `crews::purge`（没有 RPC，留给 N4 带快照再开）；`CollabError` 的拒绝改成稳定码 + md 文字 + RPC `error.data.refusal`；真实数据只读演练；spec 交付记录。

不做：唤醒规划 / 投递（M3c，crewd-wake）；`post.to` / `post` 默认值 / 工具表 / 世界观文字（M3b T12–T15c）；PreToolUse 审批闸与 hook 注册的删除（M3b 分支上自己删，见合并注意）；永久删除 RPC 与它前面的快照（N4）；工作模型（M4b）；Jev（M5b）。

## 写计划时定下的裁定（执行时照做，有异议先回报）

1. **组织根的标记改名 `org_root`**：`crews.builtin` → `crews.org_root`（`ALTER TABLE … RENAME COLUMN`），`Crew.builtin` → `Crew.orgRoot`；部分唯一索引 `crews_one_org_root ON crews(org_root) WHERE org_root=1` 保证至多一个。理由：语义从「Swift 带来的内建机组」变成「组织唯一的根」，同名不同义是这个项目反复踩过的坑（成员同名时编译器不报、只静默降级）；改名让每个旧用法在编译期现形。
2. **总机组的 id 与号码**：库里已有 `builtin=1` 的机组 → 它就是总机组（多个 → 取 `created_at` 最早、再按 id，其余降为普通机组）；没有 → 新建，id 固定 `crewd_store::crews::ORG_ROOT_ID = "pendingcrew-chief"`（与 Swift 的内建机组同 id，导入的旧数据天然对上）。默认标题是数据默认值（人可改名）：`meta.locale = 'en'` → `Headquarters`，否则 `总机组`；常量放在 store，不算给 LLM 的提示词。**号码 0**（没有号、且 0 没人用时），这样 `post(to: "0")` 能找到总机组机长 [推，§24 列为待人确认]。工作目录 NULL。
3. **空库不建总机组**：迁移 / 回填只在 `crews` 非空时确保总机组；空库由第一次 `crews::create` 懒建（导入要求空库，迁移不能凭空造一行）。
4. **不变式 I-root**：除总机组外每个机组至少一条父边；**指向总机组的边只能是唯一的父**。于是「一级机组」= 父边恰好是 `[总机组]`。`adopt(X, p≠根)` 加边时顺手删掉 X 的根边；摘掉最后一条父边 = 回到总机组下。`adopt(X, 根)`：X 已是一级 → 空操作；否则 `Invalid`（指路：用 `release` 摘掉其它父边）。`release(X, from, to=根)` 等同 `to=None`。
5. **人类群归属 `home_of`**：总机组 → 自己；一级机组 → 自己；更深 → 往上走到的一级祖先里 `created_at` 最早的（再按 id；多父歧义沿用 §13 Q1 的口径）。防御：没有父边的非根机组（旧形状 / 脏数据）当作自己的 home——与旧 `root_of` 遇环退回自己同一种「全函数」处理，Task 4 有测试钉住「任何结构操作之后没有这种机组」。
6. **根的轻量规则（只有这些）**：不能有父（不能被收编、转挂、摘出）、不能删、不能归档；新机组默认挂在它下面；它的人类群是管理用人类群（同一条懒建规则）；号码 0。其余（改名、提问、起 session、当机长）与普通机组一样。
7. **顶层只剩总机组**：`crew.reorder` 的 `parentId` 改为必填（`crews::reorder(tx, parent: &str, …)`）；`crews.position` 只对总机组有意义（恒为 1）。
8. **reply_to 只 @ 原发送者，而且他得还在群里**：@ = 叫醒，不在群里的人叫不醒（M3c 只在成员里选）；被回复的是人 → `@human`；是 system → 不 @ 任何人。不再追加 `broadcast`。**人回复一条开着的问时不追加 @提问者**：叫醒走问的回答路径（`ask_answer`，带 resume_note），不重复。
9. **契约版本 2 → 3**（Task 1 提）：删审批方法 / 事件 / 主题、`Crew.builtin` → `orgRoot`、`MentionKind` 少一个值、`CrewDetail.rootId` → `homeId`、`ask.list {rootCrewId}` → `{homeCrewId}`、`crew.reorder.parentId` 必填、`crew.delete` 语义变成归档、`crew.unarchive` 新增、`RpcError.data` 新增——全是破坏性改动，一次提版本。
10. **B15 可读规则**（读 = `read_channel` / `search` 这类主动拉取；**注入与唤醒只认成员资格**）：session S 读得到群 C ⇔ ①S 是 C 的有效成员；或 ②C 挂在 S 的机组下（C 归属 S 的机组，或 C 里有 S 机组的在群 session——即 `channel.list(S 的机组)` 列得出的群：人在侧栏这个机组下看得见什么、这个机组的 session 就读得到什么）；或 ③C 是 S 的 home 的人类群。收听不在这里（调用方并上 `listens::channels_of`）。
11. **人类群里回复问 = 回答**：只认 `Sender::Human`、只认 `category = "ask"` 且有 `ask_meta.message_id` 对得上的那条、只认问还开着；同一事务里先落这条回复、再 `asks::answer(tx, crew, ask, text)`。问已答 / 已撤 / 已按默认办 → 当普通消息。session 回复问 → 普通消息（只有人能答问）。
12. **session 标题**：`register` 时 `title` 去空白后非空且 ≤ 18 个字符（`chars().count()`）→ 用它；超长 → `Invalid`；没给 → 从 `brief` 的第一条非空行派生（压空白、截前 18 字）；两者都没有 → `Invalid`。v4 回填：显示名与标题都空的旧行，标题 = brief 第一行前 18 字，没有 brief = `机组标题 + ' #' + 分机号`（机长没号 = 机组标题）。
13. **删机组 = 归档**：`crews::archive`（`crew.delete` 调它）：拒总机组、拒还有 `desired='running'` 的 session、拒还有**未归档的子机组**（先挪走或归档子机组，不做级联）；写 `crews.archived_at`，把**归属这个机组**的全部群标归档（消息、session 行、账本全部保留）；session 不再能注册进归档机组。`crews::unarchive`（`crew.unarchive`）：清 `archived_at`，大厅解档、是 home 就把已有的人类群解档；topic / dm 留归档（人逐个解档）。原 `crews::delete` 的级联删除改名 `crews::purge`，**没有 RPC**（N4 加「永久删除前先快照」后再开）。
14. **拒绝码**：`CollabError::{NotFound, Invalid, Forbidden, Conflict}` 都改成装 `Refusal { code: Code, args: Vec<(&'static str, String)> }`；`Code` 是封闭枚举（`refusal_codes!` 宏：变体、线上名 `area.snake_name`、允许的占位符）；`Refusal::text(Locale)` 从 md 渲染；`Display` = 中文（日志、RPC message 的缺省）；RPC 错误多带 `data: {refusal: "<code>"}`，客户端按码本地化。文字要写「下一步怎么做」（AC§3）。
15. **审批彻底删除，不留壳**：没有「审批已停用」的空方法、没有保留的表、没有保留的 `approval` 主题；库里旧消息的 `category = 'approval'` 照常可读（历史），但 `approval` 不再是保留分类；排队中的 `source = 'approval:…'` 唤醒在 v4 里作废（`dropped`，`last_error` 写原因）。

## 人的裁定（2026-10-07）在 N1 的落点

| 裁定 | N1 做什么 | 落点 |
|---|---|---|
| 不做审批（runner 自己的权限系统处理） | 删 collab `approvals`、proto 审批类型 / 方法 / 事件 / 主题 / `Decision` 词表、server 审批 RPC 与事件映射、render 的审批节与 `WakeBody::Approval`、`wakes::source::approval`、保留分类 `approval`；v4 `DROP TABLE approvals`、作废排队的审批唤醒 | Task 1、Task 2 |
| H2 总机组 = 唯一根 + 管理层；一级机组各有人类群；总机组有管理用人类群；无 builtin 特例 | `org_root` 唯一；迁移 / 导入把旧根挂到总机组下；`home_of` 取代 `root_of`；根的轻量规则（裁定 6） | Task 2、3、4 |
| H1 被 @ 先回一声留在世界观 | 不是本计划的代码（M3b T15c） | — |
| 消息模型（采纳）：看得见 = 群成员；@ 只叫醒；删收窄与 broadcast；dm 他人可读；dm 里说话默认叫醒对方；人类群里回复问 = 回答；跨机组 = dm | 删 `visible_to` / `directed_targets` / `Broadcast`；reply_to 规则（裁定 8）；`readable_by`（裁定 10）；`dm_peer`；`directory::resolve`；回复即回答（裁定 11）。dm 默认叫醒在 M3c | Task 5、6、7 |
| 每个群有群主（可转让）、重启问人、不自动排序 | 已在 M3a；N1 只保证 `crew.reorder` 在单根下仍是人排（裁定 7） | Task 4 |
| `crew.delete` 默认归档；永久删除先快照（人已同意） | 归档 / 解档；`purge` 无 RPC，快照归 N4 | Task 9 |
| 完成不需要验证凭据；工作模型命名（原始需求 / 计划 / 待人类处理） | M4b，不在 N1；只写进 spec §24 | — |

## Review Focus

1. **v4 之后组织里恰好一个没有父边的机组，就是总机组；人排过的顶层次序原样搬到总机组下；一级机组原有的人类群不归档、不换 id。**（Task 2 `v3_roots_are_reparented_under_the_one_org_root_in_their_sidebar_order`、`a_first_level_crews_human_channel_survives_the_migration`）
2. **每一处「去人类群」都去 home，而不是总机组**：问、重启询问、往人类群发言、人类群的隐式机长成员、`ask.list` 过滤、问被答后的回退叫醒。（Task 3 全部；删掉 `root_of` 让漏网的调用点编不过）
3. **@ 不再让任何人看不见**：注入面只排除「自己发的」；回复一个已离群的人不会凭空 @ 一个叫不醒的目标。（Task 5 `a_mention_wakes_but_never_hides_everyone_in_the_channel_sees_it`、`replying_to_someone_who_left_the_channel_mentions_nobody`）
4. **人回复一条开着的问，问被答、提问者恰好被叫一次、回复本身不再额外 @**；已答的问被回复不改答案。（Task 7）
5. **审批删得干净且没误删同名的东西**：codex 自己的 `approvalsReviewer` / `requestApproval`、终端的 `PendingDecision` / `DecisionView`、Jev 的 `gate`、exec 的 `OrphanDecision`、work 的 `StopDecision`、§13 Q2「到点照常过权限审批」说的 runner 权限框——这些都**不是**我们的审批，一行不动。（Task 1 Step 6 的反向 grep）

## 文件结构

```
crewd/crates/collab/
  src/approvals.rs                删除                                                  (T1)
  src/lib.rs                      删 approvals 模块与 Change::ApprovalChanged；加 refusal 模块 (T1, T10)
  src/wakes.rs                    删 source::approval                                    (T1)
  src/render.rs                   删审批渲染；MsgLine.directed_to → mentions             (T1, T5)
  src/prompts.rs                  REQUIRED 表增删节                                      (T1, T5)
  prompts/render.{zh,en}.md       删 notice.approval / wake.approval_*；改 wake.ask_defaulted；line.directed* → line.at* (T1, T5)
  prompts/refusals.{zh,en}.md     新建                                                    (T10)
  src/refusal.rs                  新建：Refusal / Code                                     (T10)
  src/crews.rs                    org_root / home_of / is_home；单根规则；archive / unarchive / purge (T3, T4, T9)
  src/channels.rs                 人类群按 home；readable_by / readable_channels / dm_peer；归档 (T3, T6, T9)
  src/messages.rs                 人类群按 home；reply_to 规则；回复即回答；保留分类去掉 approval (T1, T3, T5, T7)
  src/cursors.rs                  删 visible_to / directed_targets                         (T5)
  src/asks.rs                     home；不再拒总机组提问                                    (T3, T4)
  src/restart.rs                  按 home 分组公告                                          (T3)
  src/directory.rs                resolve                                                  (T6)
  src/sessions.rs                 标题规则；归档机组拒注册                                  (T8, T9)
  src/testkit.rs                  单根夹具（backfill_v4）                                   (T4)
crewd/crates/store/src/{schema,crews,lib,messages}.rs   v4、org_root、ORG_ROOT_ID、ensure_org_root、MENTION_KINDS (T2, T5, T8, T9)
crewd/crates/proto/src/{lib,model,params}.rs            删审批；orgRoot；homeId；homeCrewId；reorder；MentionKind；archivedAt；RpcError.data；API_VERSION 3 (T1–T5, T9, T10)
crewd/crates/legacy/src/{crews,whiteboard,lib}.rs + tests/import_basic.rs   org_root、backfill_v4、丢 broadcast (T2, T5)
crewd/crates/server/src/{dispatch,hub,lib,ledger}.rs + tests/socket.rs      删审批 RPC；新参数；归档；错误码 (T1–T4, T9, T10)
crewd/crates/crewd/tests/e2e.rs                         方法表                                   (T1, T9)
crewd/schema/crewd-api.schema.json                      重生成                                   (T1–T5, T9, T10)
docs/superpowers/specs/…design.md                       §25 交付记录                             (T11)
```

## 接口变更（M3a 冻结接口里被本计划**有意**改掉的；M3b / M3c 合并时照此改）

| 旧 | 新 | 任务 |
|---|---|---|
| `crewd_collab::approvals::*`、`Change::ApprovalChanged`、`wakes::source::approval`、`render::WakeBody::Approval` | 删除 | T1 |
| proto `Approval`、`Decision`、`ApprovalDecideParams`、`methods::APPROVAL_*`、`events::APPROVAL_CHANGED`、主题 `approval` | 删除 | T1 |
| `Crew.builtin` / `crews.builtin` | `Crew.org_root`（线上 `orgRoot`）/ `crews.org_root`，至多一个 | T2 |
| `crews::root_of`、`crews::is_root` | `crews::home_of`、`crews::is_home`、`crews::org_root`、`crews::is_org_root` | T3 |
| `CrewDetail.root_id`、`AskListParams.root_crew_id`、`asks::waits(.., root)` | `home_id`、`home_crew_id`、`asks::waits(.., home)` | T3 |
| `crews::reorder(tx, Option<&str>, ..)`、`CrewReorderParams.parent_id: Option` | `crews::reorder(tx, &str, ..)`、必填 | T4 |
| `MentionKind::Broadcast`、`cursors::{visible_to, directed_targets}`、`render::MsgLine.directed_to` | 删除；`MsgLine.mentions: Vec<String>` | T5 |
| — | `channels::{readable_by, readable_channels, dm_peer}`、`directory::resolve` | T6 |
| `messages::Posted { message, unwatched_human_mention }` | 多 `answered_ask: Option<(String, String)>` | T7 |
| `sessions::register` 接受无标题 | 标题必填或由 brief 派生（≤18 字） | T8 |
| `crews::delete`（级联）、`crew.delete` = 删 | `crews::purge`（无 RPC）；`crews::{archive, unarchive}`；`crew.delete` = 归档；`crew.unarchive`；`Crew.archived_at` | T9 |
| `CollabError::{NotFound,Invalid,Forbidden,Conflict}(String)` | `(Refusal)`；`RpcError.data` | T10 |

---

### Task 1: 删掉审批（collab / proto / server / 渲染 / 唤醒来源）

人 2026-10-07：PendingCrew 不做审批，claude code 与 codex 自己的权限系统处理。本任务删代码；**表在 Task 2 的 v4 里删**（本任务结束时表还在、没人读写，无害）。

**Files:**
- Delete: `crates/collab/src/approvals.rs`
- Modify: `crates/collab/src/lib.rs`、`crates/collab/src/wakes.rs`、`crates/collab/src/render.rs`、`crates/collab/src/prompts.rs`、`crates/collab/src/messages.rs`、`crates/collab/src/asks.rs`（一行注释）、`crates/collab/prompts/render.zh.md`、`crates/collab/prompts/render.en.md`、`crates/proto/src/lib.rs`、`crates/proto/src/model.rs`、`crates/proto/src/params.rs`、`crates/server/src/dispatch.rs`、`crates/server/src/hub.rs`、`crates/server/tests/socket.rs`、`crates/crewd/tests/e2e.rs`、`schema/crewd-api.schema.json`（重生成）

**Interfaces:**
- Consumes: 无新接口。
- Produces: 「接口变更」表第 1、2 行（全是删除）；`API_VERSION = 3`。

**删除清单（`ef3f1d4` 行号；先 grep 核对再删，名字像的不一定是它）**

| 文件:行 | 删什么 |
|---|---|
| `collab/src/approvals.rs`（整文件，341 行） | 模块 |
| `collab/src/lib.rs:1` | 模块文档里的「审批、」 |
| `collab/src/lib.rs:5` | `pub mod approvals;` |
| `collab/src/lib.rs:69-71` | `Change::ApprovalChanged { id }` |
| `collab/src/wakes.rs:91-94` | `source::approval` 及其文档注释 |
| `collab/src/render.rs:47-52` | `WakeBody::Approval { tool, summary, allow }` |
| `collab/src/render.rs:102` | `const SRC_APPROVAL` |
| `collab/src/render.rs:385-398` | `wake_text` 里 `WakeBody::Approval` 分支 |
| `collab/src/render.rs:465-466` | 注释里「审批还没决，」 |
| `collab/src/render.rs:516-533` | `build_batch` 里 `WakeKind::AskAnswer if r.source.starts_with(SRC_APPROVAL)` 整个分支（之后 `approval:` 开头的 source 落到普通 ask 解析，解析不出 → `unusable`，与任何坏 source 同路） |
| `collab/src/render.rs:834-866` | 测试 `approval_decision_wakes_with_retry_once_or_no` |
| `collab/src/render.rs:1057-1085`（约） | 测试 `a_denied_approval_wakes_with_no` |
| `collab/src/render.rs:1215-1219` | 某测试里构造的 `WakeBody::Approval` 项：删掉这一项，断言按剩下的项改 |
| `collab/src/render.rs:1300-1330`（约） | 测试里 `source::approval("ap-nope")` 与 `ALTER TABLE approvals RENAME TO approvals_gone` 那段（「审批表坏了 → unusable」）：整段删；同一测试还测别的 source 的部分保留 |
| `collab/src/render.rs:1828-1836` | `WakeBody::Approval` 项，同上处理 |
| `collab/src/prompts.rs:72` | `("notice.approval", ..)` |
| `collab/src/prompts.rs:94-95` | `("wake.approval_allowed", ..)`、`("wake.approval_denied", ..)` |
| `collab/prompts/render.{zh,en}.md:1` | 文件头注释里的 approvals |
| `collab/prompts/render.{zh,en}.md:4-7`（约） | `## notice.approval` 整节 |
| `collab/prompts/render.{zh,en}.md:73` | `wake.ask_defaulted` 最后一句（「到点后的动作照常过审批闸…」）**改写**：zh「到点后的动作照常经过你自己的权限确认（claude / codex 的权限框）；被拦下就先干别的。」en「Actions after the deadline still go through your own permission prompts (claude / codex); if one is blocked, move on to other work.」 |
| `collab/prompts/render.{zh,en}.md:75-80` | `## wake.approval_allowed`、`## wake.approval_denied` 两节 |
| `collab/src/messages.rs:117`、`:122` | 注释里的「审批」 |
| `collab/src/messages.rs:151` | `RESERVED_CATEGORIES: [&str; 3] = ["ask", "approval", "restart"]` → `[&str; 2] = ["ask", "restart"]` |
| `collab/src/messages.rs:1115` | 测试循环里的 `"approval"`（改成 `["ask", "restart"]`） |
| `collab/src/asks.rs:15` | 注释「到点后照常走审批」→「照常经过 runner 自己的权限确认」（只改注释） |
| `proto/src/lib.rs:42-43、78-79` | `APPROVAL_LIST` / `APPROVAL_DECIDE` 常量与方法表里两项 |
| `proto/src/lib.rs:93` | 主题表里的 `"approval"` |
| `proto/src/lib.rs:103、229` | `APPROVAL_CHANGED` 与事件表一项 |
| `proto/src/lib.rs:133、135` | 注释里的「审批」 |
| `proto/src/lib.rs:437-444` | 契约表里两个审批方法 |
| `proto/src/lib.rs:975`（附近） | 测试里列方法名的那一项 |
| `proto/src/model.rs:235` | `closed_vocab!(Decision { .. })`（只有审批用；**不是** `DecisionView`） |
| `proto/src/model.rs:380-401`（约） | `pub struct Approval` |
| `proto/src/params.rs:198`（起） | `ApprovalDecideParams` |
| `server/src/dispatch.rs:5` | `use` 里的 `approvals` |
| `server/src/dispatch.rs:324-333`（约） | `APPROVAL_LIST` / `APPROVAL_DECIDE` 两个分支 |
| `server/src/dispatch.rs:372` | 注释里「与「审批已经决过」同一个码」→ 只说问 |
| `server/src/hub.rs:71` | `use` 里的 `approvals` |
| `server/src/hub.rs:110-111` | `Change::ApprovalChanged` 分支 |
| `server/tests/socket.rs:86` | 夹具注释里的「申请了一次审批；」 |
| `server/tests/socket.rs:134-142` | 夹具里的 `approvals::gate(..)?;` |
| `server/tests/socket.rs:260`（附近） | `approval.decide` 的错误码用例 |
| `server/tests/socket.rs:324`（附近） | 方法清单里的 `"approval.list"` |
| `server/tests/socket.rs:377-444` | 测试 `asks_approvals_and_restart_round_trip`：改名 `asks_and_restart_round_trip`，删其中审批段落（`approval.list` / `approval.decide` 与 C10 断言），问与重启的断言保留 |
| `server/tests/socket.rs:459` | 「问、审批、重启询问各一条公告」→ 人类群未读 3 → **2**，文字改成「问、重启询问各一条公告」 |
| `server/tests/socket.rs:726`、`:786` | 注释里的审批 |
| `server/tests/socket.rs:788-864` | 测试 `ask_approval_restart_channel_and_crew_changes_reach_their_topics_once`：改名去掉 approval，订阅主题表去掉 `"approval"`，删 `approval.list` / `approval.decide` 与 `["approval.changed"]` 那几行 |
| `crewd/tests/e2e.rs:275` | 注释里的「审批、」 |
| `crewd/tests/e2e.rs:382-386`（约） | 方法表里 `approval.list` / `approval.decide` 两项 |

**不要动（名字像，但不是我们的审批）**：`exec/src/codex/{proto,session}.rs`、`exec/src/bin/fake_codex.rs`、`exec/tests/{fake_codex,live_codex}.rs` 里的 `approvalsReviewer` / `requestApproval` / `codex_approval_kind`（codex 自己的权限协议）与 `live_codex.rs` 的 `fn gate`；`exec/src/launch.rs:175` 的 `--gate computer-use`（M3b 的文件，M3b 分支上删，见合并注意）；`exec/src/delivery.rs:307` 与 `term/src/input_box.rs:19` 说的「命令审批」对话框（runner 的权限框，屏幕识别要留着）；`term` 的 `PendingDecision` / `menu.rs` 的「approve edits」；`proto::DecisionView` / `SessionLive.pending_decision`（屏幕上的待决策框）；`jev::decisions::gate`；`exec::registry::OrphanDecision`；`work::StopDecision`；`work/src/{block,model}.rs` 里「到点后照常走权限审批（§13 Q2）」——那是 runner 的权限框，裁定不变，不改。

- [ ] **Step 1: 写失败的测试**

`crates/collab/src/messages.rs` 测试模块里加：

```rust
    /// 不做审批了（人 2026-10-07）：`approval` 不再是保留分类，发言可以带它（只是一个普通标签）。
    #[test]
    fn approval_is_no_longer_a_reserved_category() {
        let mut s = store();
        let p = ok(&mut s, |tx| {
            post(
                tx,
                session("w-a1"),
                NewPost {
                    category: Some("approval".into()),
                    ..post_text("crew-a/lobby", "普通发言")
                },
            )
        });
        assert_eq!(p.message.category.as_deref(), Some("approval"));
    }
```

`crates/proto/src/lib.rs` 测试模块里加（`api_schema` 的名字以文件里实际的为准）：

```rust
    #[test]
    fn there_is_no_approval_surface_left() {
        let schema = serde_json::to_string(&api_schema()).unwrap();
        assert!(!schema.contains("approval"), "契约里不能再有任何审批方法 / 事件 / 类型");
        assert_eq!(API_VERSION, 3);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-collab approval_is_no_longer_a_reserved_category` → FAIL（`分类 "approval" 留给系统公告`）。
Run: `cargo test -p crewd-proto there_is_no_approval_surface_left` → FAIL。

- [ ] **Step 3: 按删除清单删**

照表删。`API_VERSION` 2 → 3（`proto/src/lib.rs:14`），注释写「v3：删审批、单根、@ 不再收窄（N1，spec §24）」。`render.rs` 删完后多余的导入一并删（clippy 会报）。

- [ ] **Step 4: 重生成契约、跑测试确认通过**

```bash
cargo run -p crewd -- schema > schema/crewd-api.schema.json
cargo test -p crewd-collab -p crewd-proto -p crewd-server -p crewd
```
Expected: 全绿。socket / e2e 里数字对不上先看是不是审批那一条公告少了（人类群未读 3 → 2），别的数字变了先查。

- [ ] **Step 5: 变异核对**

备份 `crates/collab/src/messages.rs`，把 `RESERVED_CATEGORIES` 改回含 `"approval"`：Expected `approval_is_no_longer_a_reserved_category` 红。`cp` 还原，再跑绿。

- [ ] **Step 6: 反向 grep（证明没漏，也没误删）**

```bash
grep -rn -i 'approval' crates/collab crates/proto crates/server schema/crewd-api.schema.json
```
Expected：只剩 Step 1 的两条测试里的字符串、`wake.ask_defaulted` 改写后**不含** approval；`store/src/schema.rs` 的 `CREATE TABLE approvals` 还在（Task 2 删）。再：
```bash
git diff --stat ef3f1d4 -- crates/exec crates/term crates/jev crates/work
```
Expected：空（这些 crate 一个文件都没动）。

- [ ] **Step 7: 全量检查并提交**

```bash
./scripts/check.sh
git rm crates/collab/src/approvals.rs
git add crates/collab/src/lib.rs crates/collab/src/wakes.rs crates/collab/src/render.rs crates/collab/src/prompts.rs crates/collab/src/messages.rs crates/collab/src/asks.rs crates/collab/prompts/render.zh.md crates/collab/prompts/render.en.md crates/proto/src/lib.rs crates/proto/src/model.rs crates/proto/src/params.rs crates/server/src/dispatch.rs crates/server/src/hub.rs crates/server/tests/socket.rs crates/crewd/tests/e2e.rs schema/crewd-api.schema.json
git diff --cached --stat
git commit -m "crewd: remove approvals (collab ledger, RPC, events, wake source, render text); contract v3

PendingCrew no longer files or decides tool approvals: claude code and codex
handle permissions themselves (human ruling 2026-10-07). The approvals table
is dropped by the v4 migration in the next commit.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: schema v4 + 总机组单根的数据形状（store / proto / legacy）

**Files:**
- Modify: `crates/store/src/schema.rs`（`SCHEMA_VERSION = 4`、`V4_DDL`、`migrate_v4`、`pub fn backfill_v4`、测试）、`crates/store/src/crews.rs`（`builtin` → `org_root`、`ORG_ROOT_ID`、`org_root`、`ensure_org_root`、读 `archived_at`）、`crates/store/src/lib.rs`（`:148` 夹具字段）、`crates/proto/src/model.rs`（`Crew.builtin` → `org_root`，`Crew.archived_at`）、`crates/legacy/src/crews.rs`、`crates/legacy/src/lib.rs`、`crates/legacy/tests/import_basic.rs`、`crates/collab/src/{testkit,crews,asks}.rs`（字段改名，**行为不变**）、`crates/server/src/ledger.rs:100`、`schema/crewd-api.schema.json`

**Interfaces:**
- Consumes: M3a 的 `backfill_v3`、`channels::ensure_lobby`、`meta_get`、`now_ts`。
- Produces:
  - `pub const crewd_store::crews::ORG_ROOT_ID: &str = "pendingcrew-chief";`
  - `pub fn crewd_store::crews::org_root(conn: &Connection) -> Result<Option<String>>`
  - `pub fn crewd_store::crews::ensure_org_root(conn: &Connection, now_ts: &str) -> Result<String>`（已有 → 返回它；否则建：标题按 `meta.locale`、号码 0、`position` 1、建大厅；**不加任何父边**）
  - `pub fn crewd_store::schema::backfill_v4(conn: &Connection) -> Result<()>`（幂等；迁移与导入共用）
  - `Crew.org_root: bool`（线上 `orgRoot`）；`Crew.archived_at: Option<String>`；列 `crews.org_root`、`crews.archived_at`（Task 9 才写）；`approvals` 表删除。

- [ ] **Step 1: 写失败的测试**

`crates/store/src/schema.rs` 测试模块里加（沿用 `v2_db()` 与 `one()`）：

```rust
    /// 停在 v3 的库：内建 chief、两个旧根（crew-x 人排在 crew-a 前面）、子机组 crew-b（也挂着 chief）；
    /// crew-a 有人类群；一条排队中的审批唤醒。
    fn v3_db() -> Connection {
        let c = v2_db();
        migrate_v3(&c).unwrap();
        c.execute_batch(
            r#"
INSERT INTO crews(id,title,number,builtin,created_at,updated_at) VALUES
  ('crew-x','另一个根',3,0,'2026-09-03T00:00:00.000Z','2026-09-03T00:00:00.000Z');
INSERT INTO channels(id,crew_id,kind,name,created_by,created_at) VALUES
  ('crew-x/lobby','crew-x','lobby','','system','2026-09-03T00:00:00.000Z'),
  ('crew-a/human','crew-a','human','','system','2026-09-01T00:00:00.000Z');
UPDATE crews SET position = CASE id WHEN 'crew-x' THEN 1 WHEN 'crew-a' THEN 2 ELSE 9 END;
-- 旧数据里「既挂在 chief 下、又有别的父」的机组：v4 后根边必须去掉（不变式 I-root）。
INSERT INTO crew_parents(crew_id,parent_id,position) VALUES('crew-b','chief',0);
INSERT INTO wakes(session_id,kind,source,priority,not_before,state,attempts,created_at) VALUES
  ('worker-a1','ask_answer','approval:ap-1','human',0,'pending',0,'2026-09-04T00:00:00.000Z');
"#,
        )
        .unwrap();
        c
    }

    fn rows(c: &Connection, sql: &str) -> Vec<String> {
        let mut st = c.prepare(sql).unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    #[test]
    fn v3_roots_are_reparented_under_the_one_org_root_in_their_sidebar_order() {
        let c = v3_db();
        migrate(&c).unwrap();
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 4);
        assert_eq!(rows(&c, "SELECT id FROM crews WHERE org_root=1"), ["chief"]);
        assert_eq!(
            rows(&c, "SELECT id FROM crews c WHERE NOT EXISTS (SELECT 1 FROM crew_parents p WHERE p.crew_id=c.id)"),
            ["chief"],
            "只剩一个没有父边的：总机组"
        );
        assert_eq!(
            rows(&c, "SELECT crew_id FROM crew_parents WHERE parent_id='chief' ORDER BY position"),
            ["crew-x", "crew-a"],
            "人排过的顶层次序原样搬到总机组下"
        );
        assert_eq!(rows(&c, "SELECT parent_id FROM crew_parents WHERE crew_id='crew-b'"), ["crew-a"], "子机组不动");
        assert_eq!(one::<i64>(&c, "SELECT number FROM crews WHERE id='chief'"), 0);
    }

    #[test]
    fn a_first_level_crews_human_channel_survives_the_migration() {
        let c = v3_db();
        migrate(&c).unwrap();
        assert_eq!(
            one::<Option<String>>(&c, "SELECT archived_at FROM channels WHERE id='crew-a/human'"),
            None,
            "一级机组保留自己的人类群（H2）"
        );
    }

    #[test]
    fn a_v3_database_without_a_builtin_crew_gets_one_org_root() {
        let c = v3_db();
        c.execute_batch(
            "PRAGMA foreign_keys=OFF; DELETE FROM channels WHERE crew_id='chief'; DELETE FROM crews WHERE id='chief'; PRAGMA foreign_keys=ON;",
        )
        .unwrap();
        migrate(&c).unwrap();
        assert_eq!(rows(&c, "SELECT id FROM crews WHERE org_root=1"), [crate::crews::ORG_ROOT_ID]);
        assert_eq!(one::<i64>(&c, "SELECT COUNT(*) FROM channels WHERE id='pendingcrew-chief/lobby'"), 1, "新建的总机组也有大厅");
        assert_eq!(one::<i64>(&c, "SELECT COUNT(*) FROM crew_parents WHERE parent_id='pendingcrew-chief'"), 2);
    }

    #[test]
    fn extra_builtin_crews_are_demoted_and_the_earliest_stays_the_root() {
        let c = v3_db();
        c.execute("UPDATE crews SET builtin=1 WHERE id='crew-x'", []).unwrap();
        migrate(&c).unwrap();
        assert_eq!(rows(&c, "SELECT id FROM crews WHERE org_root=1"), ["chief"]);
        assert_eq!(rows(&c, "SELECT parent_id FROM crew_parents WHERE crew_id='crew-x'"), ["chief"]);
    }

    #[test]
    fn at_most_one_org_root_is_enforced_by_the_schema() {
        let c = v3_db();
        migrate(&c).unwrap();
        assert!(c.execute("UPDATE crews SET org_root=1 WHERE id='crew-x'", []).is_err());
    }

    #[test]
    fn v4_drops_the_approvals_table_and_voids_queued_approval_wakes() {
        let c = v3_db();
        migrate(&c).unwrap();
        assert_eq!(one::<i64>(&c, "SELECT COUNT(*) FROM sqlite_master WHERE name='approvals'"), 0);
        assert_eq!(one::<String>(&c, "SELECT state FROM wakes WHERE source='approval:ap-1'"), "dropped");
    }

    #[test]
    fn fresh_database_is_v4_and_has_no_org_root_until_a_crew_exists() {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 4);
        assert_eq!(one::<i64>(&c, "SELECT COUNT(*) FROM crews"), 0, "导入要求空库：迁移不能凭空建总机组");
    }

    #[test]
    fn backfill_v4_is_idempotent() {
        let c = v3_db();
        migrate(&c).unwrap();
        let snap = |c: &Connection| -> (String, String, i64) {
            (
                one(c, "SELECT group_concat(crew_id||'>'||parent_id||'#'||position, ',') FROM (SELECT * FROM crew_parents ORDER BY crew_id, parent_id)"),
                one(c, "SELECT group_concat(id, ',') FROM crews WHERE org_root=1"),
                one(c, "SELECT COUNT(*) FROM channels"),
            )
        };
        let a = snap(&c);
        backfill_v4(&c).unwrap();
        migrate(&c).unwrap();
        assert_eq!(snap(&c), a);
    }
```

`wakes` 的列以 v3 DDL 为准（`not_before` INTEGER 毫秒、`attempts` 非空）；对不上按 DDL 改插入语句，不改断言。

`crates/legacy/tests/import_basic.rs` 里加：

```rust
#[test]
fn imported_top_level_crews_hang_under_the_builtin_chief() {
    let dest = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dest.path().join("crewd.db")).unwrap();
    import(&fixture(), &mut store, dest.path()).unwrap();
    let list = crewd_store::crews::list(store.conn()).unwrap();
    let a = list.iter().find(|c| c.id == "local-aaaa").unwrap();
    assert_eq!(a.parent_ids, ["pendingcrew-chief"]);
    let chief = list.iter().find(|c| c.id == "pendingcrew-chief").unwrap();
    assert!(chief.org_root && chief.parent_ids.is_empty());
    assert_eq!(chief.number, Some(0));
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-store v3_roots_are_reparented` → 编不过（`backfill_v4` / `org_root` 不存在）= 失败。Run: `cargo test -p crewd-legacy imported_top_level_crews_hang_under_the_builtin_chief` → 编不过。

- [ ] **Step 3: store 实现**

`crates/store/src/crews.rs`：SQL 里的 `builtin` 一律改 `org_root`（`upsert` 的列与 `ON CONFLICT` 子句、`list` / `get` 的选择列），并读出 `archived_at`；加：

```rust
/// 组织根（总机组）的固定 id：与 Swift 的内建机组同 id，导入的旧数据天然对上（N1 裁定 2）。
pub const ORG_ROOT_ID: &str = "pendingcrew-chief";

/// 新建总机组时的默认标题（数据默认值，人可以改名；不是给 LLM 的提示词）。
fn default_root_title(conn: &Connection) -> Result<&'static str> {
    Ok(match crate::meta_get(conn, "locale")?.as_deref() {
        Some("en") => "Headquarters",
        _ => "总机组",
    })
}

pub fn org_root(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT id FROM crews WHERE org_root=1", [], |r| r.get(0))
        .optional()?)
}

/// 总机组：有就返回；没有就建（号码 0、侧栏次序 1、大厅），不加任何父边。
/// 只有 store 的回填与 collab 的 `crews::create` 调它。
pub fn ensure_org_root(conn: &Connection, now_ts: &str) -> Result<String> {
    if let Some(id) = org_root(conn)? {
        return Ok(id);
    }
    let zero_taken: i64 = conn.query_row("SELECT COUNT(*) FROM crews WHERE number=0", [], |r| r.get(0))?;
    conn.execute(
        "INSERT INTO crews(id,title,number,org_root,created_at,updated_at,position) VALUES(?1,?2,?3,1,?4,?4,1)",
        params![ORG_ROOT_ID, default_root_title(conn)?, (zero_taken == 0).then_some(0i64), now_ts],
    )?;
    crate::channels::ensure_lobby(conn, ORG_ROOT_ID, now_ts)?;
    Ok(ORG_ROOT_ID.to_string())
}
```

（补 `use rusqlite::OptionalExtension;`。）

`crates/store/src/schema.rs`：

```rust
pub const SCHEMA_VERSION: i64 = 4;

/// v4（N1，spec §24）：删审批；组织根标记改名并保证唯一；归档列。数据部分在 `backfill_v4`。
const V4_DDL: &str = r#"
DROP INDEX IF EXISTS approvals_open;
DROP TABLE IF EXISTS approvals;
ALTER TABLE crews RENAME COLUMN builtin TO org_root;
ALTER TABLE crews ADD COLUMN archived_at TEXT;
"#;

fn migrate_v4(conn: &Connection) -> Result<()> {
    // N4 的迁移前快照必须在这之前（计划「次序与外部依赖」）。
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(V4_DDL)?;
    backfill_v4(&tx)?;
    tx.execute_batch("CREATE UNIQUE INDEX crews_one_org_root ON crews(org_root) WHERE org_root=1;")?;
    if let Some(report) = foreign_key_violations(&tx)? {
        return Err(StoreError::Invalid(format!(
            "v4 迁移后外键不成立，已回滚（库保持 v3）：{report}"
        )));
    }
    tx.execute_batch("PRAGMA user_version = 4")?;
    tx.commit()?;
    Ok(())
}

/// v4 的数据回填（幂等；迁移与旧 JSON 导入共用，导入在 `backfill_v3` 之后调）。
/// 空机组表时只做与机组无关的几步（总机组由第一次建机组时懒建，N1 裁定 3）。
pub fn backfill_v4(conn: &Connection) -> Result<()> {
    // 1) 多个组织根：留 created_at 最早的（再按 id），其余降为普通机组。
    conn.execute_batch(
        "UPDATE crews SET org_root=0 WHERE org_root=1 AND id <>
           (SELECT id FROM crews WHERE org_root=1 ORDER BY created_at, id LIMIT 1);",
    )?;
    // 2) 作废排队中的审批唤醒（审批已删除，N1 裁定 15）。
    conn.execute(
        "UPDATE wakes SET state='dropped', last_error='approvals removed (v4)', settled_at=?1
          WHERE source LIKE 'approval:%' AND state IN ('pending','inflight')",
        [crate::now_ts()],
    )?;
    // （Task 5 在这里加：剥掉 broadcast mention。）
    let crews: i64 = conn.query_row("SELECT COUNT(*) FROM crews", [], |r| r.get(0))?;
    if crews == 0 {
        return Ok(());
    }
    let root = crate::crews::ensure_org_root(conn, &crate::now_ts())?;
    // 3) 根不挂在任何人下面；号码 0（没有号、且 0 没人用时）；侧栏次序恒为 1。
    conn.execute("DELETE FROM crew_parents WHERE crew_id=?1", [&root])?;
    conn.execute(
        "UPDATE crews SET number=0 WHERE id=?1 AND number IS NULL
            AND NOT EXISTS (SELECT 1 FROM crews WHERE number=0)",
        [&root],
    )?;
    conn.execute("UPDATE crews SET position=1 WHERE id=?1", [&root])?;
    // 4) 没有父边的其它机组挂到根下：次序 = 旧顶层次序（position, created_at, id），接在根已有的子之后。
    conn.execute(
        "INSERT INTO crew_parents(crew_id,parent_id,position)
           SELECT c.id, ?1,
                  (SELECT COALESCE(MAX(position),0) FROM crew_parents WHERE parent_id=?1)
                  + ROW_NUMBER() OVER (ORDER BY COALESCE(c.position,0), c.created_at, c.id)
             FROM crews c
            WHERE c.id <> ?1
              AND NOT EXISTS (SELECT 1 FROM crew_parents p WHERE p.crew_id=c.id)",
        [&root],
    )?;
    // 5) 不变式 I-root：指向根的边只能是唯一的父（N1 裁定 4）。
    conn.execute(
        "DELETE FROM crew_parents WHERE parent_id=?1
            AND crew_id IN (SELECT crew_id FROM crew_parents WHERE parent_id <> ?1)",
        [&root],
    )?;
    // （Task 8 在这里加：session 标题回填。）
    Ok(())
}
```

`migrate` 末尾加 `if v < 4 { migrate_v4(conn)?; }`。`migrate_v3` 与 `V3_DDL` 不动（v3 的输入里列还叫 `builtin`）。

已有测试里 `migrate` 之后断言 `PRAGMA user_version = 3` 的，改成 `SCHEMA_VERSION`；断言「迁移中止、库保持 v2」的不动。`fresh_database_is_v3_and_has_no_lobbies_to_fill` 删掉（被上面的 v4 版本取代）。v2 夹具里有 chief：v2→v4 后 crew-a 多一条指向 chief 的父边——断言父边 / 子序的老测试按「总机组在上」改期望，并在交付记录里列出改了哪几条。

`crates/store/src/lib.rs:148`：`builtin: false` → `org_root: false`。

- [ ] **Step 4: proto / legacy / 其余改名**

`proto/src/model.rs`：`Crew.builtin` → `pub org_root: bool`（注释「组织唯一的根（总机组，spec §24 H2）；至多一个」）；`Crew.position` 的注释改成「只有总机组用（恒为 1）；子机组的次序看父的 `childIds`」；加 `#[serde(default, skip_serializing_if = "Option::is_none")] pub archived_at: Option<String>`（Task 9 才写）。

`legacy/src/crews.rs:51`：`builtin: c.is_builtin` → `org_root: c.is_builtin`；同一份旧数据里多于一个 `isBuiltin` → 只保留 `createdAt` 最早的为 `true`，其余 `false` 并 `cx.report.warn(..)`（否则 v4 的唯一索引让导入失败）。`legacy/src/lib.rs` 在 `backfill_v3(tx)?;` 之后加 `crewd_store::schema::backfill_v4(tx)?;`（注释：总机组单根）。

collab 里只做**字段改名、行为不变**（行为在 Task 3 / 4）：`testkit.rs` 夹具 `builtin` → `org_root`；`crews.rs` 的 `refuse_builtin` SQL `SELECT builtin` → `SELECT org_root`、`crew.builtin` → `crew.org_root`（`update` / `delete` / `siblings`）；`asks.rs:155` 同。`server/src/ledger.rs:100` 同。

- [ ] **Step 5: 重生成契约、跑测试**

```bash
cargo run -p crewd -- schema > schema/crewd-api.schema.json
cargo test -p crewd-store -p crewd-legacy -p crewd-collab -p crewd-server -p crewd
```
Expected: 全绿。

- [ ] **Step 6: 变异核对**

1. 第 4) 段的 `ORDER BY` 改成 `ORDER BY c.created_at, c.id`：Expected `v3_roots_are_reparented_under_the_one_org_root_in_their_sidebar_order` 红（crew-a 排到 crew-x 前面）。
2. 删掉第 5) 段（根边只做唯一父）：Expected `v3_roots_are_reparented_under_the_one_org_root_in_their_sidebar_order` 的「子机组不动」那句红（夹具里 crew-b 同时挂着 crew-a 与 chief）。
3. `ensure_org_root` 不建大厅：Expected `a_v3_database_without_a_builtin_crew_gets_one_org_root` 红。

都用 `cp` 备份 / 还原。

- [ ] **Step 7: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/store/src/schema.rs crates/store/src/crews.rs crates/store/src/lib.rs crates/proto/src/model.rs crates/legacy/src/crews.rs crates/legacy/src/lib.rs crates/legacy/tests/import_basic.rs crates/collab/src/testkit.rs crates/collab/src/crews.rs crates/collab/src/asks.rs crates/server/src/ledger.rs schema/crewd-api.schema.json
git diff --cached --stat
git commit -m "crewd-store: schema v4 (one org root, old roots re-parented under it, approvals table dropped)

crews.builtin becomes crews.org_root with a partial unique index; backfill_v4
(shared by migration and legacy import) keeps the earliest builtin as the org
root (or creates pendingcrew-chief), gives it number 0, hangs every other
parentless crew under it in the human's top-level order, and voids queued
approval wakes.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: `home_of` 取代 `root_of`（一切人类群都去一级机组）

**Files:**
- Modify: `crates/collab/src/crews.rs`、`crates/collab/src/channels.rs`、`crates/collab/src/messages.rs`、`crates/collab/src/asks.rs`、`crates/collab/src/restart.rs`、`crates/collab/src/directory.rs`（注释）、`crates/proto/src/model.rs`（`CrewDetail.home_id`）、`crates/proto/src/params.rs`（`AskListParams.home_crew_id`）、`crates/server/src/dispatch.rs`、`schema/crewd-api.schema.json`

**Interfaces:**
- Consumes: Task 2 的 `crewd_store::crews::org_root`。
- Produces:
  - `pub fn crews::org_root(conn: &Connection) -> Result<Option<String>>`
  - `pub fn crews::is_org_root(conn: &Connection, id: &str) -> Result<bool>`
  - `pub fn crews::is_home(conn: &Connection, id: &str) -> Result<bool>`：总机组；或父边恰好是 `[总机组]`；或（旧形状 / 脏数据）没有父边
  - `pub fn crews::home_of(conn: &Connection, id: &str) -> Result<String>`（裁定 5）
  - 删除 `crews::root_of`、`crews::is_root`
  - `CrewDetail.home_id`；`AskListParams.home_crew_id`；`asks::waits(conn, now_ms, home: Option<&str>)`

- [ ] **Step 1: 写失败的测试**

`crates/collab/src/crews.rs` 测试模块（本任务 testkit 还是多根形状；测试自己把 crew-a / crew-x 挂到 chief 下）：

```rust
    fn single_root(s: &crewd_store::Store) {
        for c in ["crew-a", "crew-x"] {
            crewd_store::crews::set_parents(s.conn(), c, &["chief".to_string()]).unwrap();
        }
    }

    #[test]
    fn home_is_the_first_level_ancestor_and_the_org_root_is_its_own_home() {
        let s = store();
        single_root(&s);
        assert_eq!(home_of(s.conn(), "chief").unwrap(), "chief");
        assert_eq!(home_of(s.conn(), "crew-a").unwrap(), "crew-a");
        assert_eq!(home_of(s.conn(), "crew-b").unwrap(), "crew-a");
        assert!(is_home(s.conn(), "chief").unwrap());
        assert!(is_home(s.conn(), "crew-a").unwrap());
        assert!(!is_home(s.conn(), "crew-b").unwrap());
        assert!(is_org_root(s.conn(), "chief").unwrap() && !is_org_root(s.conn(), "crew-a").unwrap());
        // 多父：二级机组同时挂在两个一级机组下 → created_at 最早的一级祖先（§13 Q1 同口径）。
        crewd_store::crews::set_parents(s.conn(), "crew-b", &["crew-x".to_string(), "crew-a".to_string()]).unwrap();
        assert_eq!(home_of(s.conn(), "crew-b").unwrap(), "crew-a");
    }

    #[test]
    fn crew_detail_names_the_home_not_the_org_root() {
        let mut s = store();
        single_root(&s);
        let d = ok(&mut s, |tx| detail(tx, "crew-b"));
        assert_eq!(d.home_id, "crew-a");
    }
```

`crates/collab/src/asks.rs` 测试模块：

```rust
    #[test]
    fn a_second_level_ask_goes_to_its_first_level_human_channel_not_the_org_root() {
        let mut s = store();
        for c in ["crew-a", "crew-x"] {
            crewd_store::crews::set_parents(s.conn(), c, &["chief".to_string()]).unwrap();
        }
        let a = ok(&mut s, |tx| open(tx, "w-b1", OpenAsk { question: "用哪个分支？".into(), ..Default::default() }));
        let m = crate::messages::get(s.conn(), a.message_id.as_deref().unwrap()).unwrap();
        assert_eq!(m.channel_id, "crew-a/human");
        assert!(!crate::channels::human_channel_exists(s.conn(), "chief").unwrap());
        assert_eq!(waits(s.conn(), NOW, Some("crew-a")).unwrap().len(), 1);
        assert!(waits(s.conn(), NOW, Some("chief")).unwrap().is_empty(), "按 home 过滤");
    }
```

`crates/collab/src/restart.rs` 测试模块：

```rust
    #[test]
    fn a_restart_prompt_is_announced_in_each_first_level_human_channel() {
        let mut s = store();
        for c in ["crew-a", "crew-x"] {
            crewd_store::crews::set_parents(s.conn(), c, &["chief".to_string()]).unwrap();
        }
        ok(&mut s, |tx| open(tx, vec!["w-b1".into(), "cap-x".into()]));
        let n = |ch: &str| crate::messages::list(s.conn(), ch, None, None, 10).map(|v| v.len()).unwrap_or(0);
        assert_eq!((n("crew-a/human"), n("crew-x/human")), (1, 1));
        assert!(!crate::channels::human_channel_exists(s.conn(), "chief").unwrap());
    }
```

`crates/collab/src/messages.rs` 测试模块：

```rust
    #[test]
    fn sessions_join_only_their_home_human_channel_and_the_org_root_has_its_own() {
        let mut s = store();
        for c in ["crew-a", "crew-x"] {
            crewd_store::crews::set_parents(s.conn(), c, &["chief".to_string()]).unwrap();
        }
        ok(&mut s, |tx| post(tx, session("w-b1"), post_text("crew-a/human", "子机组要人拍板")));
        assert!(matches!(
            err(&mut s, |tx| post(tx, session("cap-a"), post_text("chief/human", "越级"))),
            CollabError::Forbidden(_)
        ));
        ok(&mut s, |tx| post(tx, Sender::Human, post_text("chief/human", "管理群里人说话")));
        let e = err(&mut s, |tx| post(tx, Sender::Human, post_text("crew-b/human", "发错地方")));
        assert!(e.to_string().contains("crew-a/human"), "{e}");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-collab home_is_the_first_level_ancestor` → 编不过 = 失败。

- [ ] **Step 3: 实现**

`crews.rs`：删 `root_of` / `is_root`，加：

```rust
pub fn org_root(conn: &Connection) -> Result<Option<String>> {
    Ok(crewd_store::crews::org_root(conn)?)
}

pub fn is_org_root(conn: &Connection, id: &str) -> Result<bool> {
    Ok(get(conn, id)?.org_root)
}

/// 有自己人类群的机组（H2）：总机组；一级机组（父边恰好是 [总机组]）。没有父边的非根机组只会出现在
/// 旧形状 / 脏数据里，当作自己的 home（全函数，同旧 `root_of` 遇环退回自己）。
pub fn is_home(conn: &Connection, id: &str) -> Result<bool> {
    if is_org_root(conn, id)? {
        return Ok(true);
    }
    let ps = parents(conn, id)?;
    Ok(ps.is_empty() || (ps.len() == 1 && is_org_root(conn, &ps[0])?))
}

/// 人类群归属（N1 裁定 5）：自己是 home → 自己；否则往上走，遇到的 home 里 created_at 最早的（再按 id）。
pub fn home_of(conn: &Connection, id: &str) -> Result<String> {
    get(conn, id)?;
    if is_home(conn, id)? {
        return Ok(id.to_string());
    }
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from(parents(conn, id)?);
    let mut homes: Vec<(String, String)> = Vec::new();
    while let Some(c) = queue.pop_front() {
        if !seen.insert(c.clone()) || is_org_root(conn, &c)? {
            continue;
        }
        if is_home(conn, &c)? {
            homes.push((created_at(conn, &c)?, c));
        } else {
            queue.extend(parents(conn, &c)?);
        }
    }
    Ok(homes.into_iter().min().map(|(_, h)| h).unwrap_or_else(|| id.to_string()))
}
```

把每个旧调用点换掉（编译器会列全）：
- `crews.rs`：`adopt` 的 `was_root` → `was_home = is_home(crew)`，加边后 `was_home && !is_home(crew)` 才 `archive_human_of`；`release` / `delete` / `point_captain_at` 里的 `is_root` → `is_home`；`detail` 的 `root_id: root_of(..)` → `home_id: home_of(..)`。
- `channels.rs`：`ensure_human_channel(tx, home)` 判 `crews::is_home`（错误文字「{x} 没有人类群，人类群在 {home_of}/human」）；`channels_of_session`、`list_for_crew` 的 `is_root` → `is_home`；文档注释里「根机组」→「home（总机组 / 一级机组）」；`members()` 的 Human 分支不变（`ch.crew_id` 本来就是 home）。
- `messages.rs::post_with`：`/human` 前缀段 `is_root` → `is_home`、指路用 `home_of`；跨树判断改成 `crews::home_of(conn, &sessions::crew_of(conn, id)?)? != ch.crew_id` → `Forbidden`（「{id} 的人类群是 {home}/human」）；删注释里 C7 的说法（C7 已作废，跨机组说话走 dm）。`announce_to_human(tx, root, ..)` 参数改名 `home`。
- `asks.rs`：`open` 用 `home_of`；`wake_asker` 第三候选「根机组机长」→「home 机长」；`waits` 参数改名 `home`、比较 `home_of`。
- `restart.rs`：`by_root` → `by_home`，`home_of`。
- `directory.rs:7` 注释里的 `root_of` 改 `home_of`。
- proto：`CrewDetail.root_id` → `home_id`（注释「这个机组的人类群在 `<homeId>/human`；总机组 / 一级机组 = 自己」）；`AskListParams.root_crew_id` → `home_crew_id`；`dispatch.rs` 跟着改。

- [ ] **Step 4: 重生成契约、跑测试**

```bash
cargo run -p crewd -- schema > schema/crewd-api.schema.json
cargo test -p crewd-collab -p crewd-server -p crewd
```
Expected: 全绿（testkit 仍多根，旧测试期望不变：多根形状下 `home_of` 与旧 `root_of` 给同一个答案）。

- [ ] **Step 5: 变异核对**

`home_of` 的 `min()` 改 `max()`：Expected `home_is_the_first_level_ancestor_and_the_org_root_is_its_own_home` 红（多父那句）。`asks::open` 的 `home_of(..)` 换成 `crews::org_root(..)?.unwrap()`：Expected `a_second_level_ask_goes_to_its_first_level_human_channel_not_the_org_root` 红。`cp` 还原。

- [ ] **Step 6: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/collab/src/crews.rs crates/collab/src/channels.rs crates/collab/src/messages.rs crates/collab/src/asks.rs crates/collab/src/restart.rs crates/collab/src/directory.rs crates/proto/src/model.rs crates/proto/src/params.rs crates/server/src/dispatch.rs schema/crewd-api.schema.json
git diff --cached --stat
git commit -m "crewd-collab: home_of replaces root_of; every human channel is the first-level crew's

asks, restart notices, posting into a human channel, the implicit captain
member, ask.list filtering and the ask-answer fallback all resolve to the
first-level crew (the org root resolves to itself). root_of / is_root are
deleted so callers on other branches fail to compile instead of silently
pointing at the org root. CrewDetail.homeId, ask.list homeCrewId.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: 单根下的机组树规则；删 builtin 特例

**Files:**
- Modify: `crates/collab/src/crews.rs`、`crates/collab/src/testkit.rs`、`crates/collab/src/asks.rs`、`crates/collab/src/channels.rs`（测试期望）、`crates/collab/src/messages.rs`（测试期望）、`crates/proto/src/params.rs`（`CrewReorderParams.parent_id: String`）、`crates/server/src/dispatch.rs`、`crates/server/tests/socket.rs`（reorder / create 的期望）、`schema/crewd-api.schema.json`

**Interfaces:**
- Consumes: Task 2 `ensure_org_root`、Task 3 `is_home` / `home_of` / `is_org_root`。
- Produces: `crews::create` 无父 → 总机组下（必要时懒建总机组）；`adopt` / `release` / `delete` 按裁定 4、6；`pub fn reorder(tx, parent: &str, ordered: &[String]) -> Result<Vec<Crew>>`；`update` 不再拒改总机组的名；`asks::open` 不再拒总机组的 session。删 `refuse_builtin`、`siblings(None)`。

- [ ] **Step 1: testkit 改成单根**

`testkit.rs::store()` 在 `backfill_v3(c)` 之后加 `crewd_store::schema::backfill_v4(c).unwrap();`，夹具注释改成：

```text
/// chief（总机组，号 0，无 session）
///   ├ crew-a（1 号，一级）  机长 cap-a；w-a1（分机 2）、w-a2（分机 3）
///   │   └ crew-b（2 号）    机长 cap-b；w-b1（分机 2）
///   └ crew-x（3 号，一级）  机长 cap-x
```

- [ ] **Step 2: 写失败的测试**

`crates/collab/src/crews.rs` 测试模块：

```rust
    fn new_crew(title: &str) -> NewCrew {
        NewCrew { title: title.into(), working_directory: "/tmp/example".into(), ..Default::default() }
    }

    #[test]
    fn a_new_crew_without_a_parent_lands_last_under_the_org_root() {
        let mut s = store();
        let c = ok(&mut s, |tx| create(tx, new_crew("新机组")));
        assert_eq!(parents(s.conn(), &c.id).unwrap(), ["chief"]);
        assert_eq!(children(s.conn(), "chief").unwrap().last(), Some(&c.id));
        assert!(is_home(s.conn(), &c.id).unwrap());
    }

    #[test]
    fn the_first_crew_in_an_empty_database_creates_the_org_root() {
        let mut s = crewd_store::Store::open_in_memory().unwrap();
        let c = ok(&mut s, |tx| create(tx, new_crew("第一个")));
        let root = org_root(s.conn()).unwrap().unwrap();
        assert_eq!(root, crewd_store::crews::ORG_ROOT_ID);
        assert_eq!(parents(s.conn(), &c.id).unwrap(), [root.clone()]);
        assert!(crate::channels::get(s.conn(), &crewd_store::channels::lobby_id(&root)).is_ok());
    }

    #[test]
    fn the_org_root_cannot_be_adopted_released_or_deleted() {
        let mut s = store();
        assert!(matches!(err(&mut s, |tx| adopt(tx, "chief", "crew-a")), CollabError::Forbidden(_)));
        assert!(matches!(err(&mut s, |tx| release(tx, "chief", "crew-a", None)), CollabError::Forbidden(_)));
        assert!(matches!(err(&mut s, |tx| delete(tx, "chief")), CollabError::Forbidden(_)));
    }

    #[test]
    fn adopting_a_first_level_crew_drops_its_root_edge_and_releasing_it_brings_it_back() {
        let mut s = store();
        ok(&mut s, |tx| crate::channels::ensure_human_channel(tx, "crew-x"));
        ok(&mut s, |tx| adopt(tx, "crew-x", "crew-a"));
        assert_eq!(parents(s.conn(), "crew-x").unwrap(), ["crew-a"], "指向根的边只能是唯一的父");
        assert_eq!(home_of(s.conn(), "crew-x").unwrap(), "crew-a");
        assert!(crate::channels::get(s.conn(), "crew-x/human").unwrap().archived_at.is_some());
        ok(&mut s, |tx| release(tx, "crew-x", "crew-a", None));
        assert_eq!(parents(s.conn(), "crew-x").unwrap(), ["chief"], "摘出 = 回到总机组下");
        assert!(crate::channels::get(s.conn(), "crew-x/human").unwrap().archived_at.is_none());
    }

    #[test]
    fn adopting_under_the_org_root_is_a_noop_for_first_level_crews_and_refused_otherwise() {
        let mut s = store();
        ok(&mut s, |tx| adopt(tx, "crew-a", "chief"));
        assert_eq!(parents(s.conn(), "crew-a").unwrap(), ["chief"]);
        assert!(matches!(err(&mut s, |tx| adopt(tx, "crew-b", "chief")), CollabError::Invalid(_)));
        ok(&mut s, |tx| release(tx, "crew-b", "crew-a", Some("chief")));
        assert_eq!(parents(s.conn(), "crew-b").unwrap(), ["chief"], "release to=根 ≡ to=None");
    }

    #[test]
    fn deleting_a_parent_moves_its_only_child_under_the_org_root() {
        let mut s = store();
        ok(&mut s, |tx| delete(tx, "crew-a"));
        assert_eq!(parents(s.conn(), "crew-b").unwrap(), ["chief"]);
        assert!(is_home(s.conn(), "crew-b").unwrap());
    }

    #[test]
    fn reorder_takes_a_parent_and_the_org_root_is_renamable() {
        let mut s = store();
        let got = ok(&mut s, |tx| reorder(tx, "chief", &["crew-x".to_string(), "crew-a".to_string()]));
        assert_eq!(got.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["crew-x", "crew-a"]);
        assert_eq!(children(s.conn(), "chief").unwrap(), ["crew-x", "crew-a"]);
        ok(&mut s, |tx| update(tx, "chief", CrewPatch { title: Some("管理层".into()), ..Default::default() }));
        assert_eq!(get(s.conn(), "chief").unwrap().title, "管理层");
    }

    /// 不变式：任何结构操作之后，没有父边的只有总机组（`is_home` 的「无父 = home」防御分支走不到）。
    #[test]
    fn after_any_structural_op_the_only_parentless_crew_is_the_org_root() {
        let mut s = store();
        let parentless = |s: &crewd_store::Store| {
            count(s, "SELECT COUNT(*) FROM crews c WHERE NOT EXISTS (SELECT 1 FROM crew_parents p WHERE p.crew_id=c.id)")
        };
        assert_eq!(parentless(&s), 1);
        ok(&mut s, |tx| adopt(tx, "crew-x", "crew-b"));
        ok(&mut s, |tx| release(tx, "crew-x", "crew-b", None));
        ok(&mut s, |tx| delete(tx, "crew-a"));
        ok(&mut s, |tx| create(tx, new_crew("再建一个")));
        assert_eq!(parentless(&s), 1);
    }
```

`crates/collab/src/asks.rs`：把 `the_builtin_crew_does_not_ask` 换成：

```rust
    #[test]
    fn the_org_roots_own_asks_go_to_its_management_human_channel() {
        let mut s = store();
        let cap = ok(&mut s, |tx| {
            crate::sessions::register(tx, crate::sessions::NewSession {
                crew_id: "chief".into(),
                role: crewd_proto::Role::Captain,
                executor: "claude".into(),
                title: Some("总机长".into()),
                ..Default::default()
            })
        });
        let a = ok(&mut s, |tx| open(tx, &cap.id, OpenAsk { question: "全局要不要冻结发版？".into(), ..Default::default() }));
        let m = crate::messages::get(s.conn(), a.message_id.as_deref().unwrap()).unwrap();
        assert_eq!(m.channel_id, "chief/human");
    }
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-collab` → 新测试红（`reorder` 签名编不过）。旧测试里因单根而变的期望（例如「删父后成孤儿根」、`siblings(None)` 相关、`crew.list` 顶层序、`the_builtin_chief_crew_is_a_root_and_keeps_its_human_channel`）按新规则改，列进交付记录。

- [ ] **Step 4: 实现**

- `create`：`parent_id` 为 `None` 或等于总机组 → `let root = crewd_store::crews::ensure_org_root(tx.conn(), &now)?;`（新建了总机组就再记 `CrewChanged{root}`），插边 `(crew, root)`、`put_last(tx, &crew.id, &root)`；否则照旧 `adopt(tx, &crew.id, p)`。
- 删 `refuse_builtin`；加 `fn refuse_org_root(conn, id) -> Result<()>`（是总机组 → `Forbidden`「总机组是组织的根，不能被收编、摘出或删除」），`adopt` / `release` / `delete` 对被操作的 `crew` 调它（`adopt` 的 `parent` 是根不在这里拒）。
- `adopt(crew, parent)`：`parent` 是根 → `parents(crew) == [root]` 则直接返回，否则 `Invalid`（「挪到一级：用 release 摘掉 {crew} 的其它上级」）；否则照旧检查、插边，**再删 `crew` 指向根的边**，`was_home && !is_home(crew)` → `archive_human_of`。
- `release(crew, from, to)`：`to == Some(根)` 视为 `None`；摘边后 `parents(crew)` 为空 → 插根边、`put_last(.., root)`、`human_channel_exists` 则 `ensure_human_channel`（解档）。
- `delete`：孤儿（父只剩被删的这个）→ 插根边、`put_last`、有人类群则解档。`update`：删「内建机组不能改名」。
- `put_last(tx, crew, parent: &str)` 只剩按边的那一支；`siblings` 只剩 `children(p)`；`reorder(tx, parent: &str, ..)`。
- `asks::open`：删总机组拒绝。proto `CrewReorderParams.parent_id: String`（去掉 `#[serde(default)]`），dispatch 传 `&p.parent_id`。

- [ ] **Step 5: 重生成契约、跑测试**

```bash
cargo run -p crewd -- schema > schema/crewd-api.schema.json
cargo test -p crewd-collab -p crewd-server -p crewd
```

- [ ] **Step 6: 变异核对**

1. `adopt` 删掉「删根边」那句：Expected `adopting_a_first_level_crew_drops_its_root_edge_and_releasing_it_brings_it_back` 红。
2. `release` 删掉「没父了就挂回根」：Expected `after_any_structural_op_the_only_parentless_crew_is_the_org_root` 红。
3. `refuse_org_root` 改成永远 `Ok(())`：Expected `the_org_root_cannot_be_adopted_released_or_deleted` 红。

`cp` 备份 / 还原。

- [ ] **Step 7: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/collab/src/crews.rs crates/collab/src/testkit.rs crates/collab/src/asks.rs crates/collab/src/channels.rs crates/collab/src/messages.rs crates/proto/src/params.rs crates/server/src/dispatch.rs crates/server/tests/socket.rs schema/crewd-api.schema.json
git diff --cached --stat
git commit -m "crewd-collab: crew tree under the single org root; no builtin special cases

New crews land under the org root (created lazily in an empty database);
an edge to the root is only ever a crew's sole parent; releasing the last
parent or deleting it moves a crew back under the root. The root cannot be
adopted, released or deleted, but can be renamed and can ask like any crew.
crew.reorder needs a parentId.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: 消息模型——@ 只决定叫醒谁（删收窄与 broadcast）

**Files:**
- Modify: `crates/proto/src/model.rs`（`MentionKind` 去 `Broadcast`，`default Human`）、`crates/proto/src/lib.rs`（测试 `:832`）、`crates/store/src/messages.rs`（`MENTION_KINDS`）、`crates/store/src/schema.rs`（`backfill_v4` 加剥 broadcast；老测试期望）、`crates/legacy/src/whiteboard.rs`、`crates/legacy/tests/import_basic.rs`、`crates/collab/src/cursors.rs`、`crates/collab/src/messages.rs`、`crates/collab/src/render.rs`、`crates/collab/src/prompts.rs`、`crates/collab/prompts/render.{zh,en}.md`、`schema/crewd-api.schema.json`

**Interfaces:**
- Produces: `MentionKind ∈ {session, captain, human}`；删 `cursors::{visible_to, directed_targets}`；`render::MsgLine.mentions: Vec<String>`（被 @ 的 session 的标签，读者本人渲染成 `line.at.you`）；md 节 `line.at {targets}`、`line.at.you` 取代 `line.directed`、`line.directed.unknown`；`unread_for` 只排除自己发的。

- [ ] **Step 1: 写失败的测试**

`crates/collab/src/cursors.rs`：删 `directed_messages_are_hidden_from_others_but_human_and_broadcast_are_not`、`visibility_follows_the_stored_target_not_the_current_captain`、`a_window_of_invisible_messages_does_not_stall_the_unread_surface`、`paging_collects_the_oldest_deliverable_messages_across_hidden_ones`、`directed_targets_distinguishes_no_narrowing_from_unresolvable_and_dedupes`；`leading_run_of_undeliverable_messages_advances_the_cursor` 改成「开头一段**自己发的**」；`legacy_captain_mention_is_resolved_once_in_the_data_and_stays_put` 只保留数据层落定的断言（`target_id` 写成当时的机长），删可见性断言。加：

```rust
    #[test]
    fn a_mention_wakes_but_never_hides_everyone_in_the_channel_sees_it() {
        let mut s = store();
        let to_a2 = say(&mut s, "w-a1", "crew-a/lobby", vec![at(MentionKind::Session, Some("w-a2"))]);
        let to_cap = say(&mut s, "w-a1", "crew-a/lobby", vec![at(MentionKind::Captain, None)]);
        assert_eq!(unread_ids(&mut s, "w-a2"), [to_a2.id.clone(), to_cap.id.clone()]);
        assert_eq!(unread_ids(&mut s, "cap-a"), [to_a2.id.clone(), to_cap.id.clone()], "@ 别人的也看得见");
        assert!(unread_ids(&mut s, "w-a1").is_empty(), "自己发的不是自己的未读");
    }
```

`crates/collab/src/messages.rs`：删 `reply_to_appends_broadcast_and_the_replied_sender_without_duplicates`、`replying_to_your_own_message_does_not_mention_yourself`、`replying_to_a_system_message_only_broadcasts`，加：

```rust
    #[test]
    fn reply_to_mentions_only_the_replied_sender_and_never_broadcast() {
        let mut s = store();
        let reply = |s: &mut crewd_store::Store, who: Sender, to: &str, text: &str| {
            ok(s, |tx| post(tx, who, NewPost { in_reply_to: Some(to.into()), ..post_text("crew-a/lobby", text) }))
        };
        let q = ok(&mut s, |tx| post(tx, session("w-a2"), post_text("crew-a/lobby", "这个怎么做？")));
        let r = reply(&mut s, session("w-a1"), &q.message.id, "这样做");
        assert_eq!(r.message.mentions, [m(MentionKind::Session, Some("w-a2"))]);
        let again = ok(&mut s, |tx| {
            post(tx, session("w-a1"), NewPost {
                in_reply_to: Some(q.message.id.clone()),
                mentions: vec![m(MentionKind::Session, Some("w-a2"))],
                ..post_text("crew-a/lobby", "补一句")
            })
        });
        assert_eq!(again.message.mentions, [m(MentionKind::Session, Some("w-a2"))], "去重");
        assert!(reply(&mut s, session("w-a1"), &r.message.id, "自言自语").message.mentions.is_empty(), "回复自己不 @ 自己");
        let h = ok(&mut s, |tx| post(tx, Sender::Human, post_text("crew-a/lobby", "人的问题")));
        assert_eq!(reply(&mut s, session("w-a1"), &h.message.id, "答人").message.mentions, [m(MentionKind::Human, None)]);
        let sys = ok(&mut s, |tx| post(tx, Sender::System, post_text("crew-a/lobby", "系统公告")));
        assert!(reply(&mut s, session("w-a1"), &sys.message.id, "收到").message.mentions.is_empty(), "回复系统消息不叫醒任何人");
    }

    #[test]
    fn replying_to_someone_who_left_the_channel_mentions_nobody() {
        let mut s = store();
        let t = ok(&mut s, |tx| {
            crate::channels::create(tx, &session("w-a1"), crate::channels::NewChannel {
                crew_id: "crew-a".into(),
                kind: ChannelKind::Topic,
                name: "发版".into(),
                members: vec![MemberRef::session("w-a2")],
                ..Default::default()
            })
        });
        let q = ok(&mut s, |tx| post(tx, session("w-a2"), post_text(&t.id, "我先撤了")));
        ok(&mut s, |tx| crate::channels::remove_member(tx, &Sender::Human, &t.id, &MemberRef::session("w-a2")));
        let r = ok(&mut s, |tx| {
            post(tx, session("w-a1"), NewPost { in_reply_to: Some(q.message.id.clone()), ..post_text(&t.id, "好") })
        });
        assert!(r.message.mentions.is_empty(), "@ = 叫醒；不在群里的人叫不醒，不凭空 @");
    }
```

`crates/collab/src/render.rs`：删 `a_directed_message_with_no_resolvable_target_is_still_marked` 与其它断言 `directed_to` 的段落（`:725`、`:1161-1166`、`:1811-1820` 附近改成断言 `mentions`），加：

```rust
    #[test]
    fn a_line_shows_who_was_mentioned_and_marks_the_reader() {
        let mut s = store();
        let m = ok(&mut s, |tx| {
            crate::messages::post(tx, crate::Sender::Session("w-a1".into()), crate::messages::NewPost {
                channel_id: "crew-a/lobby".into(),
                text: "你俩看一下".into(),
                mentions: vec![
                    crewd_proto::Mention { kind: crewd_proto::MentionKind::Session, target_id: Some("w-a2".into()) },
                    crewd_proto::Mention { kind: crewd_proto::MentionKind::Captain, target_id: None },
                ],
                ..Default::default()
            })
        })
        .message;
        let unread = vec![("crew-a/lobby".to_string(), vec![m])];
        let blocks = blocks_for(s.conn(), "w-a2", Locale::Zh, &unread).unwrap();
        let ln = &blocks[0].lines[0];
        assert_eq!(ln.mentions.len(), 2);
        assert_eq!(ln.mentions[0], render_file(Locale::Zh).get("line.at.you"));
        assert!(ln.mentions[1].contains("名字-cap-a"), "{:?}", ln.mentions);
        let text = injection(Locale::Zh, &blocks);
        assert!(text.contains(render_file(Locale::Zh).get("line.at.you")), "{text}");
    }
```

（`injection` 的签名以 `render.rs` 现有的为准。）

`crates/store/src/schema.rs`：

```rust
    #[test]
    fn v4_strips_broadcast_mentions_and_keeps_the_rest_in_order() {
        let c = v3_db();
        migrate(&c).unwrap();
        assert_eq!(
            one::<i64>(&c, "SELECT COUNT(*) FROM messages m, json_each(m.mentions) j WHERE json_extract(j.value,'$.kind')='broadcast'"),
            0
        );
        assert_eq!(one::<String>(&c, "SELECT mentions FROM messages WHERE id='m4'"), r#"[{"kind":"human"}]"#);
    }
```

`crates/legacy/tests/import_basic.rs`：

```rust
#[test]
fn broadcast_mentions_are_dropped_on_import_and_the_message_is_kept() {
    let src = fixture_copy();
    let board = src.path().join("whiteboards/local-aaaa.json");
    let text = std::fs::read_to_string(&board).unwrap().replace(
        r#""mentions":[{"kind":"session","target_id":"worker-2222bbbb"}]"#,
        r#""mentions":[{"kind":"broadcast"},{"kind":"session","target_id":"worker-2222bbbb"}]"#,
    );
    std::fs::write(&board, text).unwrap();
    let dest = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dest.path().join("crewd.db")).unwrap();
    let r = import(src.path(), &mut store, dest.path()).unwrap();
    assert_eq!(r.counts["messages"], 3, "带 broadcast 的消息照常导入");
    assert_eq!(r.counts["broadcastMentionsDropped"], 1);
    let m = crewd_store::messages::get(store.conn(), "11111111-0000-4000-8000-000000000001").unwrap().unwrap();
    assert_eq!(m.mentions.len(), 1);
    assert_eq!(m.mentions[0].kind, crewd_proto::MentionKind::Session);
}
```

proto 测试 `lib.rs:832` 附近的 `MentionKind::Broadcast` 序列化断言改成 `assert_eq!(MentionKind::parse("broadcast"), None);`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-collab a_mention_wakes_but_never_hides` → FAIL（cap-a 看不见 @w-a2 的那条）。其余新测试编不过或红。

- [ ] **Step 3: 实现**

- proto：`closed_vocab!(MentionKind { Session = "session", Captain = "captain", Human = "human" } default Human);`
- store：`MENTION_KINDS = &["session", "captain", "human"]`；`backfill_v4` 在「作废审批唤醒」之后、`crews == 0` 提前返回**之前**加（空机组的库也可能有消息）：
  ```sql
  UPDATE messages SET mentions = (
    SELECT json_group_array(json(value)) FROM (
      SELECT value FROM json_each(messages.mentions)
       WHERE json_extract(value,'$.kind') <> 'broadcast' ORDER BY key))
   WHERE json_valid(mentions) AND json_type(mentions)='array'
     AND EXISTS (SELECT 1 FROM json_each(messages.mentions) j WHERE json_extract(j.value,'$.kind')='broadcast');
  ```
  `check_v3_vocabulary` 的 SQL 照旧接受 `broadcast`（那是 v2 的输入词表），**不**改成引用 `MENTION_KINDS`。老测试 `migration_resolves_untargeted_captain_mentions_once_and_is_idempotent`（`:1090`、`:1099`）的期望数组去掉 `{"kind":"broadcast"}`。
- legacy `whiteboard.rs::insert_one`：词表检查之前去掉 `kind == "broadcast"` 的项，每去一个 `cx.report.bump("broadcastMentionsDropped")`（`m` 改 `mut`；计数 API 以 `report` 为准）。
- collab `cursors.rs`：删 `visible_to`、`directed_targets` 与其文档；`unread_for` 的条件只剩 `m.sender_session_id.as_deref() != Some(session)`，注释改成「开头一段自己发的消息被游标越过」。
- collab `messages.rs::post_with`：`MentionKind::Human | MentionKind::Broadcast` 分支只剩 `Human`；reply 段改成：
  ```rust
    if let Some(rid) = &p.in_reply_to {
        let replied = get(conn, rid)?;
        if replied.channel_id != ch.id {
            return Err(/* 同原来 */);
        }
        // 裁定 8：只 @ 原发送者，且他得还在群里（@ = 叫醒，叫不醒的不 @）；system 发的不 @ 任何人。
        match (replied.sender_kind, replied.sender_session_id) {
            (SenderKind::Human, _) => push_mention(&mut mentions, Mention { kind: MentionKind::Human, target_id: None }),
            (SenderKind::Session | SenderKind::Captain, Some(sid))
                if sender_session_id.as_deref() != Some(sid.as_str())
                    && channels::is_member(conn, &ch.id, &MemberRef::session(sid.clone()))? =>
            {
                push_mention(&mut mentions, Mention { kind: MentionKind::Session, target_id: Some(sid) })
            }
            _ => {}
        }
    }
  ```
- collab `render.rs`：`MsgLine.directed_to` → `pub mentions: Vec<String>`（文档：「这条 @ 了谁（session / 机长的标签，同 `sender` 的渲染）；读者本人 = `line.at.you`。空 = 没 @ 任何 session」）；`blocks_for` 对 `m.mentions` 里带 `target_id` 的 `Session` / `Captain` 逐个：等于 `session` → `f.get("line.at.you")`，否则 `target_label(conn, l, id)`，去重保序；`block_text` 里 `line.directed*` 段换成「`mentions` 非空 → `fill(f.get("line.at"), &[("targets", &names.join(", "))])` + 空格」。不再需要的 `use … cursors` 删掉。
- `prompts.rs::REQUIRED`：删 `line.directed`、`line.directed.unknown`，加 `("line.at", &["targets"])`、`("line.at.you", &[])`。md：zh `## line.at` → `（@{targets}）`、`## line.at.you` → `你`；en `## line.at` → `(@{targets})`、`## line.at.you` → `you`。删两节 `line.directed*`。

- [ ] **Step 4: 重生成契约、跑测试**

```bash
cargo run -p crewd -- schema > schema/crewd-api.schema.json
cargo test -p crewd-proto -p crewd-store -p crewd-legacy -p crewd-collab -p crewd-server -p crewd
```

- [ ] **Step 5: 变异核对**

1. `unread_for` 加回「有 Session mention 且目标不是我就跳过」：Expected `a_mention_wakes_but_never_hides_everyone_in_the_channel_sees_it` 红。
2. reply 段去掉 `is_member` 条件：Expected `replying_to_someone_who_left_the_channel_mentions_nobody` 红。
3. `backfill_v4` 删掉剥 broadcast 那段：Expected `v4_strips_broadcast_mentions_and_keeps_the_rest_in_order` 红（读消息反序列化失败的红也算证明这段必需）。

`cp` 备份 / 还原。

- [ ] **Step 6: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/proto/src/model.rs crates/proto/src/lib.rs crates/store/src/messages.rs crates/store/src/schema.rs crates/legacy/src/whiteboard.rs crates/legacy/tests/import_basic.rs crates/collab/src/cursors.rs crates/collab/src/messages.rs crates/collab/src/render.rs crates/collab/src/prompts.rs crates/collab/prompts/render.zh.md crates/collab/prompts/render.en.md schema/crewd-api.schema.json
git diff --cached --stat
git commit -m "crewd: a mention only decides who wakes; broadcast and @-narrowing are gone

Everyone in a channel sees every message; injected lines show who was
mentioned. reply_to mentions only the replied sender while they are still a
member (human -> @human, system -> nobody). The broadcast mention kind is
removed from the contract and store vocabulary, stripped from stored
messages by v4 and dropped (counted) on legacy import.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 6: B15 可读规则 + dm 原语（`readable_by`、`dm_peer`、`directory::resolve`）

**Files:**
- Modify: `crates/collab/src/channels.rs`、`crates/collab/src/directory.rs`

**Interfaces:**
- Produces:
  - `pub fn channels::readable_by(conn: &Connection, channel: &str, session: &str) -> Result<bool>`（裁定 10）
  - `pub fn channels::readable_channels(conn: &Connection, session: &str) -> Result<Vec<String>>`（同一条规则；含已归档；按 `created_at, id`；不含收听）
  - `pub fn channels::dm_peer(conn: &Connection, channel: &str, of: &MemberRef) -> Result<Option<MemberRef>>`（不是 dm、或 `of` 不在 dm 里 → `None`）
  - `pub fn directory::resolve(conn: &Connection, address: &str) -> Result<String>`：`"7-3"` → 那个 session；`"7"` / `"7-1"` → 7 号机组此刻的机长；号不存在 → `NotFound`；机组没有机长 → `Invalid`；格式不对 → `Invalid`

- [ ] **Step 1: 写失败的测试**

`crates/collab/src/channels.rs` 测试模块：

```rust
    #[test]
    fn a_dm_between_two_sessions_is_readable_by_both_crews_but_injected_into_neither() {
        let mut s = store();
        let (dm, _) = ok(&mut s, |tx| ensure_dm(tx, "crew-a", &MemberRef::session("w-a1"), &MemberRef::session("cap-x")));
        let x2 = ok(&mut s, |tx| {
            crate::sessions::register(tx, crate::sessions::NewSession {
                crew_id: "crew-x".into(),
                role: crewd_proto::Role::Worker,
                executor: "claude".into(),
                title: Some("丙二".into()),
                ..Default::default()
            })
        })
        .id;
        assert!(readable_by(s.conn(), &dm.id, "w-a2").unwrap(), "发起方机组的同事读得到（B15）");
        assert!(readable_by(s.conn(), &dm.id, &x2).unwrap(), "对方机组的同事也读得到（dm 显示在两边机组下）");
        assert!(!readable_by(s.conn(), &dm.id, "w-b1").unwrap(), "子机组不因上级的 dm 读得到");
        assert!(readable_channels(s.conn(), "w-a2").unwrap().contains(&dm.id));
        assert!(!is_member(s.conn(), &dm.id, &MemberRef::session("w-a2")).unwrap(), "读得到 ≠ 成员：不注入、不叫醒");
    }

    #[test]
    fn the_home_human_channel_is_readable_by_its_subtree_and_topics_by_the_owning_crew() {
        let mut s = store();
        ok(&mut s, |tx| ensure_human_channel(tx, "crew-a"));
        assert!(readable_by(s.conn(), "crew-a/human", "w-b1").unwrap());
        assert!(!readable_by(s.conn(), "crew-a/human", "cap-x").unwrap());
        let t = ok(&mut s, |tx| create(tx, &Sender::Session("w-a1".into()), topic("crew-a", vec![])));
        assert!(readable_by(s.conn(), &t.id, "w-a2").unwrap(), "没有私有群：本机组的 topic 本机组都读得到");
        assert!(!readable_by(s.conn(), &t.id, "cap-x").unwrap());
    }

    #[test]
    fn dm_peer_is_the_other_party_and_nothing_else() {
        let mut s = store();
        let (dm, _) = ok(&mut s, |tx| ensure_dm(tx, "crew-a", &MemberRef::session("w-a1"), &MemberRef::session("cap-x")));
        assert_eq!(dm_peer(s.conn(), &dm.id, &MemberRef::session("w-a1")).unwrap(), Some(MemberRef::session("cap-x")));
        assert_eq!(dm_peer(s.conn(), &dm.id, &MemberRef::session("w-a2")).unwrap(), None, "旁观者没有对方");
        assert_eq!(dm_peer(s.conn(), "crew-a/lobby", &MemberRef::session("w-a1")).unwrap(), None, "不是 dm");
    }
```

（`topic(crew, members)` 是该模块已有的帮手，`:790`。）

`crates/collab/src/directory.rs` 测试模块：

```rust
    #[test]
    fn an_address_resolves_to_a_session_or_a_crews_current_captain() {
        let s = store();
        assert_eq!(resolve(s.conn(), "1-2").unwrap(), "w-a1");
        assert_eq!(resolve(s.conn(), "1").unwrap(), "cap-a");
        assert_eq!(resolve(s.conn(), "1-1").unwrap(), "cap-a");
        assert!(matches!(resolve(s.conn(), "9").unwrap_err(), CollabError::NotFound(_)));
        assert!(matches!(resolve(s.conn(), "1-9").unwrap_err(), CollabError::NotFound(_)));
        assert!(matches!(resolve(s.conn(), "0").unwrap_err(), CollabError::Invalid(_)), "总机组现在没有机长");
        assert!(matches!(resolve(s.conn(), "甲").unwrap_err(), CollabError::Invalid(_)));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-collab readable_by` → 编不过 = 失败。

- [ ] **Step 3: 实现**

```rust
/// B15（人 2026-10-07：没有私有群，dm 别人也读得到）+ N1 裁定 10。读 = 主动拉取（read_channel / search）；
/// 注入与唤醒只认 `members`。收听不在这里，调用方并上 `listens::channels_of`。
pub fn readable_by(conn: &Connection, channel: &str, session: &str) -> Result<bool> {
    let ch = get(conn, channel)?;
    if members(conn, channel)?.contains(&MemberRef::session(session.to_string())) {
        return Ok(true);
    }
    let my_crew = sessions::crew_of(conn, session)?;
    if ch.crew_id == my_crew {
        return Ok(true);
    }
    let crewmate_inside: i64 = conn.query_row(
        "SELECT COUNT(*) FROM channel_members cm JOIN sessions s ON cm.member_kind='session' AND cm.member_id=s.id
          WHERE cm.channel_id=?1 AND cm.left_at IS NULL AND s.crew_id=?2",
        params![channel, my_crew],
        |r| r.get(0),
    )?;
    if crewmate_inside > 0 {
        return Ok(true);
    }
    Ok(ch.kind == ChannelKind::Human && ch.crew_id == crews::home_of(conn, &my_crew)?)
}
```

`readable_channels`：候选 = `SELECT id FROM channels ORDER BY created_at, id`（全表；M3 量级可接受，注释写明「量大再换成按机组 / 成员的候选」），逐个过 `readable_by`。`dm_peer`：不是 `Dm` → `None`；`explicit` 成员里有 `of` → 返回另一个，否则 `None`。`directory::resolve`：`split_once('-')`；号码 `parse::<i64>()` 失败 → `Invalid`；按 `number` 找机组，找不到 → `NotFound`；分机缺省或 `1` → `crews::captain_of`，`None` → `Invalid`；否则按 `(crew_id, ext)` 找 session，找不到 → `NotFound`。

- [ ] **Step 4: 跑测试、变异核对**

Run: `cargo test -p crewd-collab`。变异：`readable_by` 去掉 `crewmate_inside` 那段：Expected `a_dm_between_two_sessions_is_readable_by_both_crews_but_injected_into_neither` 红（对方机组那句）。`cp` 还原。

- [ ] **Step 5: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/collab/src/channels.rs crates/collab/src/directory.rs
git diff --cached --stat
git commit -m "crewd-collab: readable_by / readable_channels (B15: no private channels), dm_peer, directory::resolve

A session can read a channel it is in, any channel shown under its own
crew (owned by it or with a crewmate inside, dms included) and its home's
human channel; injection and wakes still follow membership only. dm_peer and
resolve are the collab-side primitives for post.to (M3b T12) and the dm wake
default (M3c T10a).

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 7: 人类群里回复一条问 = 回答它（S66）

**Files:**
- Modify: `crates/collab/src/messages.rs`、`crates/collab/src/asks.rs`（按消息 id 找问）、`crates/server/src/dispatch.rs`（只在 `message.post` 的返回带 `Posted` 字段时才改）

**Interfaces:**
- Produces: `messages::Posted.answered_ask: Option<(String, String)>`（(机组, H 号)）；`pub(crate) fn asks::by_message(conn: &Connection, message_id: &str) -> Result<Option<(String, String)>>`。

- [ ] **Step 1: 写失败的测试**

`crates/collab/src/messages.rs` 测试模块（`use crewd_proto::AskStateView;`）：

```rust
    fn open_ask(s: &mut crewd_store::Store) -> crewd_proto::Ask {
        ok(s, |tx| {
            crate::asks::open(tx, "w-b1", crate::asks::OpenAsk {
                question: "用哪个分支？".into(),
                resume_note: Some("接着发版".into()),
                ..Default::default()
            })
        })
    }

    #[test]
    fn a_human_reply_to_an_open_ask_answers_it_and_wakes_the_asker_once() {
        let mut s = store();
        let a = open_ask(&mut s);
        let (p, ch) = ok_at(&mut s, NOW + MIN, |tx| {
            post(tx, Sender::Human, NewPost { in_reply_to: a.message_id.clone(), ..post_text("crew-a/human", "用 main") })
        });
        assert_eq!(p.answered_ask, Some(("crew-b".to_string(), a.ask.clone())));
        assert!(p.message.mentions.is_empty(), "叫醒走问的回答路径，回复本身不再 @ 提问者");
        let after = crate::asks::get(s.conn(), "crew-b", &a.ask).unwrap();
        assert_eq!((after.state, after.answer.as_deref()), (AskStateView::Answered, Some("用 main")));
        assert!(ch.iter().any(|c| matches!(c, Change::AskAnswered { ask, .. } if *ask == a.ask)));
        let w = crate::wakes::list(s.conn(), Some("w-b1"), 10).unwrap();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].source, crate::wakes::source::ask("crew-b", &a.ask));
    }

    #[test]
    fn replying_to_an_ask_that_is_no_longer_open_is_just_a_message() {
        let mut s = store();
        let a = open_ask(&mut s);
        ok(&mut s, |tx| crate::asks::answer(tx, "crew-b", &a.ask, "先答的"));
        let p = ok(&mut s, |tx| {
            post(tx, Sender::Human, NewPost { in_reply_to: a.message_id.clone(), ..post_text("crew-a/human", "改主意了") })
        });
        assert_eq!(p.answered_ask, None);
        assert_eq!(crate::asks::get(s.conn(), "crew-b", &a.ask).unwrap().answer.as_deref(), Some("先答的"));
    }

    #[test]
    fn a_session_reply_to_an_ask_does_not_answer_it() {
        let mut s = store();
        let a = open_ask(&mut s);
        let p = ok(&mut s, |tx| {
            post(tx, session("cap-a"), NewPost { in_reply_to: a.message_id.clone(), ..post_text("crew-a/human", "我觉得用 main") })
        });
        assert_eq!(p.answered_ask, None);
        assert_eq!(crate::asks::get(s.conn(), "crew-b", &a.ask).unwrap().state, AskStateView::Open);
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-collab a_human_reply_to_an_open_ask` → 编不过（`answered_ask`）= 失败。

- [ ] **Step 3: 实现**

`asks.rs`：

```rust
/// 这条消息是哪条问的公告（`ask_meta.message_id`）。
pub(crate) fn by_message(conn: &Connection, message_id: &str) -> Result<Option<(String, String)>> {
    Ok(conn
        .query_row(
            "SELECT crew_id, ask FROM ask_meta WHERE message_id=?1",
            [message_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}
```

`messages.rs::post_with`：reply 段之前算 `answering: Option<(String, String)>`——`Sender::Human`、`in_reply_to` 指向的消息 `category == Some("ask")`、`asks::by_message` 有、`asks::get(..).state == AskStateView::Open` 全满足才是 `Some`；是的话**跳过** reply 段的自动 @（裁定 8 的例外）。消息落库、记完变更之后：`if let Some((crew, h)) = &answering { asks::answer(tx, crew, h, &message.text)?; }`（同一事务；`answer` 自己记 `AskChanged` / `AskAnswered` 并入队唤醒）。`Posted` 加字段、各处构造补上。文档注释写明裁定 11。

- [ ] **Step 4: 跑测试、变异核对**

Run: `cargo test -p crewd-collab -p crewd-server`。变异：去掉 `Sender::Human` 条件：Expected `a_session_reply_to_an_ask_does_not_answer_it` 红。去掉「问还开着」条件：Expected `replying_to_an_ask_that_is_no_longer_open_is_just_a_message` 红（`answer` 报 `AskNotOpen`，整条回滚）。`cp` 还原。

- [ ] **Step 5: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/collab/src/messages.rs crates/collab/src/asks.rs
git diff --cached --stat
git commit -m "crewd-collab: a human reply to an open ask in a human channel answers it

Same transaction: the reply is stored, then asks::answer runs and wakes the
asker through the existing ask_answer path (with its resume note); the
reply adds no extra mention. Replies to closed asks, and session replies,
stay plain messages. Posted.answered_ask reports it.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

（`dispatch.rs` 若跟着改，一并 `git add`。）

---

### Task 8: session 标题（S80：≤18 字，从 brief 派生；旧数据回填）

**Files:**
- Modify: `crates/collab/src/sessions.rs`、`crates/store/src/schema.rs`（`backfill_v4` 加标题回填 + 测试）、调用 `register` 却不给标题的测试（collab 各模块、`server/tests/socket.rs`）

**Interfaces:**
- Produces: `pub const sessions::TITLE_MAX_CHARS: usize = 18;`；`register` 的标题规则（裁定 12）；`pub(crate) fn sessions::title_from_brief(brief: &str) -> Option<String>`。

- [ ] **Step 1: 写失败的测试**

`crates/collab/src/sessions.rs` 测试模块：

```rust
    #[test]
    fn a_session_without_a_title_gets_one_from_its_brief() {
        let mut s = store();
        let got = ok(&mut s, |tx| {
            register(tx, NewSession {
                title: None,
                brief: Some("\n  一二三四五六七八九十甲乙丙丁戊己庚辛壬癸\n第二行".into()),
                ..worker("crew-a")
            })
        });
        assert_eq!(got.title.as_deref(), Some("一二三四五六七八九十甲乙丙丁戊己庚辛"));
    }

    #[test]
    fn a_session_needs_a_short_title_or_a_brief() {
        let mut s = store();
        assert!(matches!(
            err(&mut s, |tx| register(tx, NewSession { title: None, brief: None, ..worker("crew-a") })),
            CollabError::Invalid(_)
        ));
        assert!(
            matches!(
                err(&mut s, |tx| register(tx, NewSession { title: Some("一二三四五六七八九十甲乙丙丁戊己庚辛壬".into()), ..worker("crew-a") })),
                CollabError::Invalid(_)
            ),
            "19 字超长"
        );
        let ok18 = ok(&mut s, |tx| register(tx, NewSession { title: Some("  一二三四五六七八九十甲乙丙丁戊己庚辛  ".into()), ..worker("crew-a") }));
        assert_eq!(ok18.title.as_deref(), Some("一二三四五六七八九十甲乙丙丁戊己庚辛"));
    }
```

`crates/store/src/schema.rs`：

```rust
    #[test]
    fn v4_gives_every_unnamed_session_a_title() {
        let c = v3_db();
        c.execute_batch(
            "UPDATE sessions SET display_name=NULL, title=NULL;
             UPDATE sessions SET brief='修首页滚动' WHERE id='worker-a2';",
        )
        .unwrap();
        migrate(&c).unwrap();
        assert_eq!(
            one::<i64>(&c, "SELECT COUNT(*) FROM sessions WHERE COALESCE(display_name,'')='' AND COALESCE(title,'')=''"),
            0,
            "标签绝不回落到原始 id"
        );
        assert_eq!(one::<String>(&c, "SELECT title FROM sessions WHERE id='worker-a2'"), "修首页滚动");
        assert_eq!(one::<String>(&c, "SELECT title FROM sessions WHERE id='worker-a1'"), "示例机组 #5");
    }
```

（`worker-a1` 在 v2 夹具里分机号 5、机组标题「示例机组」。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-collab a_session_without_a_title` → FAIL（标题为 None）。

- [ ] **Step 3: 实现**

`register`：

```rust
    let title = match n.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) if t.chars().count() <= TITLE_MAX_CHARS => t.to_string(),
        Some(_) => return Err(/* Invalid：标题最长 18 字 */),
        None => n.brief.as_deref().and_then(title_from_brief).ok_or_else(|| /* Invalid：要有 title（≤18 字）或 brief */)?,
    };
```

`refuse_sender_label("标题", &title)` 用这个值，写行时 `title: Some(title)`。`title_from_brief`：第一条 `trim()` 后非空的行，内部空白压成一个空格，取前 18 个 `char`。`backfill_v4` 在第 5) 段之后加（SQLite 的 `substr` 按字符算；写不对就改成在 Rust 里逐行读出、算好、`UPDATE`——**行为以测试为准**）：

```sql
UPDATE sessions SET title = COALESCE(
    NULLIF(substr(trim(substr(ltrim(brief, char(10)||char(13)||' '), 1,
           instr(ltrim(brief, char(10)||char(13)||' ')||char(10), char(10)) - 1)), 1, 18), ''),
    (SELECT c.title FROM crews c WHERE c.id=sessions.crew_id) || CASE WHEN ext IS NULL THEN '' ELSE ' #'||ext END)
 WHERE COALESCE(display_name,'')='' AND COALESCE(title,'')='';
```

其它测试里不带标题的 `register` 调用补 `title: Some(..)`。

- [ ] **Step 4: 跑测试、变异核对**

变异：`TITLE_MAX_CHARS` 改 19：Expected `a_session_needs_a_short_title_or_a_brief` 红。`cp` 还原。

- [ ] **Step 5: 全量检查并提交**

```bash
./scripts/check.sh
git status --short
git add crates/collab/src/sessions.rs crates/store/src/schema.rs
git diff --cached --stat
git commit -m "crewd: session titles are at most 18 characters, derived from the brief when missing

register refuses an over-long or missing title (with no brief to derive one
from); v4 backfills a title for every legacy session that has neither a
display name nor a title, so labels never fall back to the raw id.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

（被补标题的其它测试文件按 `git status --short` 列出的显式路径一并 `git add`。）

---

### Task 9: 删机组 = 归档（S22；人已同意）

**Files:**
- Modify: `crates/collab/src/crews.rs`（`archive`、`unarchive`、`delete` → `purge`）、`crates/collab/src/sessions.rs`（归档机组拒注册）、`crates/proto/src/lib.rs`（`methods::CREW_UNARCHIVE`、方法表、契约表）、`crates/proto/src/params.rs`（`CrewUnarchiveParams { crew_id }`）、`crates/server/src/dispatch.rs`（`crew.delete` → `archive`，`crew.unarchive`）、`crates/server/tests/socket.rs`、`crates/crewd/tests/e2e.rs`、`schema/crewd-api.schema.json`

**Interfaces:**
- Produces: `pub fn crews::archive(tx, id) -> Result<Crew>`、`pub fn crews::unarchive(tx, id) -> Result<Crew>`（裁定 13；变更：`CrewChanged{id}` + 每个被动到的群 `ChannelChanged`）；`pub fn crews::purge(tx, id) -> Result<()>`（原 `delete`，无 RPC）；RPC `crew.delete {crewId}` → `Crew`（归档后的），`crew.unarchive {crewId}` → `Crew`。

- [ ] **Step 1: 写失败的测试**

`crates/collab/src/crews.rs` 测试模块（原 `delete` 的测试改名调 `purge`）：

```rust
    #[test]
    fn archiving_a_crew_keeps_everything_and_unarchiving_brings_it_back() {
        let mut s = store();
        ok(&mut s, |tx| crate::channels::ensure_human_channel(tx, "crew-x"));
        ok(&mut s, |tx| {
            crate::messages::post(tx, crate::Sender::Human, crate::messages::NewPost {
                channel_id: "crew-x/lobby".into(),
                text: "归档前的一句".into(),
                ..Default::default()
            })
        });
        let c = ok(&mut s, |tx| archive(tx, "crew-x"));
        assert!(c.archived_at.is_some());
        for ch in ["crew-x/lobby", "crew-x/human"] {
            assert!(crate::channels::get(s.conn(), ch).unwrap().archived_at.is_some(), "{ch}");
        }
        assert_eq!(count(&s, "SELECT COUNT(*) FROM messages WHERE channel_id='crew-x/lobby'"), 1, "消息保留");
        assert!(crate::sessions::get(s.conn(), "cap-x").is_ok(), "session 行保留");
        assert!(
            matches!(
                err(&mut s, |tx| crate::sessions::register(tx, crate::sessions::NewSession {
                    crew_id: "crew-x".into(),
                    role: crewd_proto::Role::Worker,
                    executor: "claude".into(),
                    title: Some("新人".into()),
                    ..Default::default()
                })),
                CollabError::Conflict(_)
            ),
            "归档的机组不收新 session"
        );
        let c = ok(&mut s, |tx| unarchive(tx, "crew-x"));
        assert!(c.archived_at.is_none());
        for ch in ["crew-x/lobby", "crew-x/human"] {
            assert!(crate::channels::get(s.conn(), ch).unwrap().archived_at.is_none(), "{ch}");
        }
    }

    #[test]
    fn archiving_refuses_the_org_root_running_sessions_and_live_children() {
        let mut s = store();
        assert!(matches!(err(&mut s, |tx| archive(tx, "chief")), CollabError::Forbidden(_)));
        assert!(matches!(err(&mut s, |tx| archive(tx, "crew-a")), CollabError::Conflict(_)), "crew-b 还在它下面");
        s.conn().execute("UPDATE sessions SET desired='running' WHERE id='cap-x'", []).unwrap();
        assert!(matches!(err(&mut s, |tx| archive(tx, "crew-x")), CollabError::Conflict(_)));
    }
```

`crates/server/tests/socket.rs`：`crew.delete` 的往返用例改成断言返回的机组 `archivedAt` 非空、`crew.list` 里仍有它、`crew.unarchive` 后 `archivedAt` 消失。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-collab archiving_a_crew` → 编不过 = 失败。

- [ ] **Step 3: 实现**

`archive`：`refuse_org_root`；`desired='running'` 计数 > 0 → `Conflict`；`children(id)` 里有 `archived_at IS NULL` 的 → `Conflict`（「先把子机组挪走或归档」）；`UPDATE crews SET archived_at=?`；先 `SELECT id FROM channels WHERE crew_id=?1 AND archived_at IS NULL`，再统一标归档，逐个记 `ChannelChanged`；`CrewChanged`。`unarchive`：清 `archived_at`；大厅解档；`is_home && human_channel_exists` → `channels::ensure_human_channel`（它会解档并钉机长游标）。`sessions::register`：机组 `archived_at.is_some()` → `Conflict`。原 `delete` 改名 `purge`，文档注释「永久删除；没有 RPC——N4 加『永久删除前先快照』后再开」。dispatch：`crew.delete` → `crews::archive` 返回 `Crew`（契约表里 `crew.delete` 的结果从 `{}` 改成 `Crew`），`crew.unarchive` 新方法。

- [ ] **Step 4: 重生成契约、跑测试、变异核对**

```bash
cargo run -p crewd -- schema > schema/crewd-api.schema.json
cargo test -p crewd-collab -p crewd-server -p crewd
```
变异：`archive` 去掉「有未归档子机组」检查：Expected `archiving_refuses_the_org_root_running_sessions_and_live_children` 红。`cp` 还原。

- [ ] **Step 5: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/collab/src/crews.rs crates/collab/src/sessions.rs crates/proto/src/lib.rs crates/proto/src/params.rs crates/server/src/dispatch.rs crates/server/tests/socket.rs crates/crewd/tests/e2e.rs schema/crewd-api.schema.json
git diff --cached --stat
git commit -m "crewd: crew.delete archives the crew; crew.unarchive restores it

Archiving keeps sessions, channels, messages and the work book; it refuses
the org root, crews with running sessions and crews with live children.
The old cascade is crews::purge with no RPC until N4 adds a snapshot before
permanent deletion.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 10: 拒绝文字的稳定码（S71）

**Files:**
- Create: `crates/collab/src/refusal.rs`、`crates/collab/prompts/refusals.zh.md`、`crates/collab/prompts/refusals.en.md`
- Modify: `crates/collab/src/lib.rs` 与 collab 里每一个构造 `CollabError::{NotFound,Invalid,Forbidden,Conflict}` 的文件及其测试里解构字符串的地方；`crates/proto/src/lib.rs`（`RpcError.data`）；`crates/server/src/{dispatch,lib,hub}.rs`；`schema/crewd-api.schema.json`

**Interfaces:**
- Produces:
  ```rust
  pub struct Refusal { pub code: Code, pub args: Vec<(&'static str, String)> }
  impl Refusal {
      pub fn new(code: Code) -> Self;
      pub fn arg(self, key: &'static str, value: impl Into<String>) -> Self; // debug_assert!(code.args().contains(&key))
      pub fn text(&self, l: Locale) -> String;                                // 从 refusals.{zh,en}.md 渲染（prompts::fill）
  }
  impl Display for Refusal { /* text(Locale::Zh) */ }
  pub enum Code { /* refusal_codes! 生成 */ }  // Code::ALL、as_str() -> "channel.dm_cannot_grow"、args() -> &'static [&'static str]
  pub enum CollabError { NotFound(Refusal), Invalid(Refusal), Forbidden(Refusal), Conflict(Refusal), Work(Rejected), Store(StoreError) }
  impl CollabError { pub fn refusal(&self) -> Option<&Refusal> }
  ```
  proto：`RpcError { code, message, #[serde(default, skip_serializing_if = "Option::is_none")] data: Option<RpcErrorData> }`，`RpcErrorData { refusal: String }`；`RpcError::new` 不变（`data: None`），加 `with_refusal(self, code: &str) -> Self`。

**码的命名与文字**：`<模块>.<snake 名>`，同一句话在多处 = 同一个码（例：`channel.archived`「群 {channel} 已归档，不收消息；要继续就解档，或新开一个群」）。**每条文字写「下一步怎么做」**（AC§3；例：`channel.dm_cannot_grow`「私聊只能两个人；要拉更多人就新建一个 topic 群」、`channel.dm_cannot_leave`「私聊不能退出；要结束就归档」、`channel.owner_must_transfer`「群主要先把群主转让给别的成员再退群」、`message.not_a_member`「{who} 不是群 {channel} 的成员；成员有：{members}」、`crew.org_root_fixed`「总机组是组织的根，不能被收编、摘出、删除或归档」）。en 同义，不逐字直译。码清单用下面的命令生成后逐条定名：

```bash
grep -n 'CollabError::\(NotFound\|Invalid\|Forbidden\|Conflict\)(' crates/collab/src/*.rs
```

- [ ] **Step 1: 写失败的测试**

`crates/collab/src/refusal.rs` 测试模块：

```rust
    #[test]
    fn every_code_has_a_section_in_both_locales_with_exactly_its_placeholders() {
        for l in [Locale::Zh, Locale::En] {
            let f = refusal_file(l);
            for c in Code::ALL {
                let mut used = crate::prompts::placeholders(f.get(c.as_str()));
                used.sort();
                let mut want: Vec<&str> = c.args().to_vec();
                want.sort();
                assert_eq!(used, want, "{l:?} {}", c.as_str());
            }
            assert_eq!(f.section_names().len(), Code::ALL.len(), "{l:?}：md 里不能有没有码的节");
        }
    }

    #[test]
    fn a_dm_refuses_new_members_with_a_stable_code_and_text_in_both_languages() {
        let mut s = crate::testkit::store();
        let (dm, _) = crate::testkit::ok(&mut s, |tx| {
            crate::channels::ensure_dm(tx, "crew-a", &MemberRef::session("w-a1"), &MemberRef::session("w-a2"))
        });
        let e = crate::testkit::err(&mut s, |tx| {
            crate::channels::add_members(tx, &crate::Sender::Session("w-a1".into()), &dm.id, &[MemberRef::session("cap-a")])
        });
        let r = e.refusal().expect("规则拒绝带码");
        assert_eq!(r.code, Code::DmCannotGrow);
        assert!(!r.text(Locale::En).chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)), "英文文字里没有中文");
        assert_eq!(e.to_string(), r.text(Locale::Zh));
    }
```

（`prompts::placeholders` 在 main 上若不存在就在 `prompts.rs` 里加——扫 `{名字}`，与 `fill` 同一种语法；M3b 分支上已有同名函数，合并时以一份为准，见合并注意。）

`crates/server/src/dispatch.rs` 测试：

```rust
    #[test]
    fn a_refusal_carries_its_code_to_the_client() {
        use crewd_collab::refusal::{Code, Refusal};
        let e = collab_err(CollabError::Invalid(Refusal::new(Code::DmCannotGrow)));
        assert_eq!(e.code, RpcError::INVALID_PARAMS);
        assert_eq!(e.data.as_ref().map(|d| d.refusal.as_str()), Some("channel.dm_cannot_grow"));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-collab every_code_has_a_section` → 编不过 = 失败。

- [ ] **Step 3: 实现**

`refusal.rs` 用一个声明宏一次写出变体、线上名、占位符：

```rust
macro_rules! refusal_codes {
    ($($v:ident = $s:literal [$($a:literal),*]),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Code { $($v),+ }
        impl Code {
            pub const ALL: &'static [Code] = &[$(Code::$v),+];
            pub fn as_str(self) -> &'static str { match self { $(Code::$v => $s),+ } }
            pub fn args(self) -> &'static [&'static str] { match self { $(Code::$v => &[$($a),*]),+ } }
        }
    };
}
refusal_codes! {
    DmCannotGrow = "channel.dm_cannot_grow" [],
    // …… 其余逐条，照上面的 grep 清单
}
```

md 格式同 `render.*.md`（`prompts::parse`，第一个 `##` 前是注释）；`refusal_file(l)` 照 `render_file` 写两个 `LazyLock`。`CollabError` 四个变体改装 `Refusal`，`#[error("{0}")]` 不变。逐个调用点改成 `CollabError::Invalid(Refusal::new(Code::X).arg("channel", &ch.id))`。测试里 `let CollabError::Invalid(why) = e` 再 `why.contains(..)` 的，优先改成断言 `code`，其次 `e.to_string().contains(..)`。`server/src/dispatch.rs::collab_err`：四个分支 `RpcError::new(.., r.to_string()).with_refusal(r.code.as_str())`；`server/src/lib.rs:258` 测试里的 `CollabError::Invalid("中途失败".into())` 改用任一个码。把 `render_code_has_no_prose` 的做法扩一份到 `refusal.rs`：`collab/src` 下非测试、非注释的代码里，`CollabError::` 所在行不得出现中文（编译器已挡住字符串，这把尺子挡「把中文塞进 `.arg` 的值」）。

- [ ] **Step 4: 重生成契约、跑测试、变异核对**

```bash
cargo run -p crewd -- schema > schema/crewd-api.schema.json
cargo test --workspace
```
变异：删 `refusals.en.md` 里随便一节：Expected `every_code_has_a_section_in_both_locales_with_exactly_its_placeholders` 红；给某条 zh 文字多加一个 `{x}`：同一条红。`cp` 还原。

- [ ] **Step 5: 全量检查并提交**

```bash
./scripts/check.sh
git status --short
git add crates/collab/src/refusal.rs crates/collab/prompts/refusals.zh.md crates/collab/prompts/refusals.en.md crates/collab/src/lib.rs crates/collab/src/channels.rs crates/collab/src/crews.rs crates/collab/src/messages.rs crates/collab/src/asks.rs crates/collab/src/listens.rs crates/collab/src/prompts.rs crates/collab/src/render.rs crates/collab/src/restart.rs crates/collab/src/sessions.rs crates/collab/src/settings.rs crates/collab/src/todos.rs crates/collab/src/wakes.rs crates/collab/src/directory.rs crates/collab/src/cursors.rs crates/proto/src/lib.rs crates/server/src/dispatch.rs crates/server/src/lib.rs crates/server/src/hub.rs schema/crewd-api.schema.json
git diff --cached --stat
git commit -m "crewd-collab: refusals carry a stable code; their text lives in refusals.{zh,en}.md

CollabError's rule variants hold a Refusal { code, args }; the text is
rendered per locale from md (with what to do next) and RPC errors carry
data.refusal so clients can localise. English sessions no longer get
Chinese refusal text.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

（`git add` 只加 `git status --short` 里实际改了的那些。）

---

### Task 11: 合入 N4、真实数据只读演练、交付记录

**Files:**
- Modify: `../docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`（新增 §25「N1 交付记录与遗留」）

- [ ] **Step 1: 合入 N4**

`git merge main`（N4 必须已在 main）；把 `migrate_v4` 放进 N4 的迁移框架，确认 v3 → v4 前有快照（N4 的测试会盯）。`./scripts/check.sh` 全绿后提交合并。

- [ ] **Step 2: 只读演练（副本，照 §23 的做法）**

源数据目录只读复制到 scratchpad；拿一份已是 v3 的副本库（或用 M3a 合并时的 crewd 导入副本）；用一个只调 `crewd_store::Store::open` 的临时程序触发 v4 迁移（**不起 serve**）。核：`user_version` 4；恰好一个 `org_root`、恰好一个没有父边的机组；原先每个根都成了一级机组且相对次序不变；原先每个根的人类群仍未归档；`broadcast` mention 0；`approvals` 表不存在、`approval:` 唤醒没有 pending / inflight；没有标题也没有显示名的 session 0；外键 0、`integrity_check` ok；再开一次各读数不变。**计数只写进私有的任务报告**（`.superpowers/sdd/…/task-11-report.md`），副本与产出用完即删。

- [ ] **Step 3: 交付记录**

spec 新增 §25：已交付（逐任务一句）、与本计划不同的地方（改了哪些老测试的期望、为什么）、遗留、给 M3b / M3c 的合并注意（照本计划文末两节，按实际落地更新）。**不写真实数据的计数。**

- [ ] **Step 4: 提交**

```bash
git add ../docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md
git diff --cached --stat
git commit -m "docs: spec section 25 records the N1 collab alignment delivery

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## 合并注意：M3b（crewd-m3b @ c6c97bd 读过）

N1 合 main 后 M3b `git merge main`。编译错误就是这张清单（有意删名字，不留别名）：

| M3b 的位置 | 变化 | 怎么改 |
|---|---|---|
| `server/src/agent/approval.rs`；`exec/src/launch.rs` 的 PreToolUse 注册（`HookEvent::PreToolUse`、`--gate` / `approval_gates`、`CommsSpec.approval_gates`、gate matcher）；`agent/port.rs:1195-1210` `approvals::NewApproval` / `gate`；`agent/catalog.rs` 与 `tools.*.md` 里的审批文字（`r.approval_*`）；`world-model.*.md` 讲审批的段落；`proto/src/agent.rs` 的 pre-tool-use 事件；`crewd hook pre-tool-use` 子命令 | collab 的审批全删（人 2026-10-07） | **M3b 分支上自己删整个审批闸**：PreToolUse 不再注册（只剩 post-tool-use / stop）。main 上 `exec/src/launch.rs:175` 的 `--gate computer-use` 随 M3b 对 launch.rs 的整体改写消失 |
| `collab/src/worldview.rs:81`（`root_of`）、`:108/133/175/180` 的 `builtin_crew` 与 `role.captain.builtin` / `ask.builtin` / `builtin` 节 | `root_of` 删；`Crew.builtin` → `org_root`；H2 无 builtin 特例 | `crews::home_of`；三节删，换成「根 / 管理机组」的轻量一句（T15c / S84） |
| `collab/src/turn_trace.rs:222-223` | `root_of` 删 | `crews::home_of` + `ensure_human_channel` |
| `server/src/agent/port.rs:10、860-976`（`cursors::visible_to` 过滤 read / search） | 删 | 不再过滤；可读范围换成 collab 的 `channels::readable_by` / `readable_channels` + 收听 |
| `port.rs:769-835` `Reader`（roots、`inside`、`can_read`）、`:480-490` `set_listen` 的「别的树」门 | 单根之后「树」没有意义；B15 规则在 collab | 删 `Reader`，用 `channels::readable_by`；收听的门改成「`readable_by` 为真才能收听」（或放开到全组织——M3b 自定，记进裁定） |
| `port.rs:49-60` `From<CollabError>`、`:485` `CollabError::Forbidden("…".into())` | 变体装 `Refusal` | `Self::Refused(r.text(locale))`（agent 的语言）；`:485` 随 `Reader` 一起删 |
| `port.rs:194/218/321` `builtin` / `builtin_crew` | `Crew.org_root` | T15b 删 `not_on_builtin` 前先映射到 `org_root`；T15b 后删 |
| `port.rs:521-526` 人类群 = `ensure_human_channel(root_of(crew))` | 同上 | `home_of` |
| `server/src/agent/inject.rs:513/534/718`（测试里 `MentionKind::Broadcast`）与 `render::MsgLine.directed_to` | 删 | 按 `MsgLine.mentions` 改；注入测试「@ 了别人的不进 / broadcast 放宽」改成「群里全进、显示 @ 了谁」 |
| `collab::prompts::placeholders`（M3b 用过） | N1 Task 10 若在 main 上新加了同名函数 | 合并时只留一份（语法与 `fill` 一致），两边调用点都指向它 |
| T12 `post` | `Posted.answered_ask`；`directory::resolve`；`channels::ensure_dm` | 回执写明「已回答 H<n>」；`post.to` 用 `resolve` + `ensure_dm(tx, 我的机组, me, 对方)`；不给 channel / to / reply_to 报错（S64） |
| T13 `contact`、T15 `report_to_parent` / `message_child_crew` | C7 作废 | 不写（跨机组 = dm） |
| T14 `ask` | 总机组也能问；问去 home 人类群 | 删 `not_on_builtin` 对 `ask` 的限制 |
| T15 `start_session` / `create_child_crew` | `sessions::register` 要标题或 brief（≤18 字） | 工具参数把 title / brief 传下去；超长时把 collab 的拒绝文字回给 agent |
| 任何 `crews::delete` / `crew.delete` 的用法 | 改成归档 / `purge` | 机长工具不提供删除 |

## 合并注意：M3c（crewd-m3c @ 5590ca1 读过）——给 T10a 的唤醒侧跟进

| 位置 | 变化 | 怎么改 |
|---|---|---|
| `crates/wake/src/plan.rs:53、113-128、208` `Mention::Broadcast`、`visible_to`，以及表驱动测试里的 broadcast 用例 | 收窄与 broadcast 删除 | 删 `visible_to`：候选 = 群成员 ∪ 收听者，只排除发送者；删 `Mention::Broadcast` |
| C22 对拍（`plan::visible_to` == `cursors::visible_to`） | collab 一侧已删 | 改成对拍「注入面（`cursors::unread_for`）与规划的候选在同一条消息上一致：都是成员、都排除自己」 |
| `plan.rs:145-153` 人发、没 @ 的兜底链里的 `dm_peer` | dm 默认叫醒对方（采纳的消息模型 + S47） | **session 在 dm 里发言 = 隐式 @ 对方**（`ctx.dm_peer` 由 DbPort 用 `channels::dm_peer` 填）；表驱动加一例；环路熔断兜底 |
| `server/src/orchestra/testkit.rs:12、497-529` MemPort 的未读镜像用 `visible_to` | 删 | 只排除自己发的 |
| `orchestra` 里任何「审批有结果」的唤醒处理（source `approval:` / `WakeBody::Approval`），以及待决策通知里「交给审批」的分支 | 审批删除 | 删；v4 已把排队中的审批唤醒作废 |
| 待决策 / 熔断 / 投递放弃的「找人类群」 | 人类群归 home | DbPort 用 `crews::home_of` + `crewd_store::channels::human_id` |
| `Core::start_new` 注册 session | 标题必填或 brief | `StartRequest` 的 title / brief 原样传下去 |
| 归档机组 | 不收新 session | 起 / 恢复归档机组里的 session 前查 `Crew.archived_at`，拒并说明（T10a / T11） |
| `dispatch.rs` 里 M3c 加的方法若构造 `CollabError` 字符串 | 变体装 `Refusal` | 用码 |

## 待定 / 要人确认（写在 spec §24，执行前不阻塞）

- 总机组的号码定为 0（`post(to: "0")` = 找总机组机长）——[推]，人可改。
- 二级机组的问去一级祖先的人类群（S68 的建议）；多父时取最早建的一级祖先。
- B15 的「挂在我机组下的群我读得到」不包括子机组的群（只看本机组；子树只读得到 home 人类群）。
