# crewd M3a 数据与 API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 落地 spec §22.2：schema v3（群、成员、注视、游标、收听、唤醒队列、熔断、审批、工作账本、问的附注，messages 改按群归属）与 v2→v3 迁移；store 搬进专用线程的 `Db` actor；新 crate `crewd-collab`（同步、sans-clock）承载全部协作写路径（群规则、发消息与 @ 解析、游标 / 红标 / 可见性、机组 DAG 与编号、人类待确认经 `crewd-work` 发 H 号、审批台账、唤醒入队与队列状态、给 agent 看的渲染）；客户端 RPC 与事件全部换成新表；契约整理到能给 Swift 生成代码；把 §22.5 列出的、M3b / M3c 共享的接口全部定死。

**Architecture:** 三层。**存储层** `crewd-store`：schema、迁移、`Db`（一条 `crewd-db` 线程拥有 `Store`，`db.call(|s| ..).await` 串行执行）、导入要用的最少行函数。**协作层** `crewd-collab`：每个操作是 `fn(&mut Tx, ..) -> collab::Result<T>`，`Tx` = 一个 SQLite 事务 + 调用方给的 `now_ms` + 这次产生的 `Change` 清单；`collab::run(store, now_ms, f)` 提交后把 `Change` 交回。**服务层** `crewd-server`：`AppState::collab(f)` 在库线程上跑 `collab::run`，提交后**仍在库线程上**把 `Change` 变成客户端事件与内部 `CollabEvent` 发到 `Hub`（库线程串行 = 事件顺序与 seq 一致）；RPC 分发只做参数解析与错误映射。

**Tech Stack:** Rust 1.94（edition 2024）、rusqlite 0.40（bundled，带 JSON1）、tokio 1.53（store 只用 `sync`）、serde / serde_json、schemars 1.2、chrono 0.4、uuid 1（v4）、crewd-work（M4a）。全部已在 `Cargo.lock` 里，不新增第三方 crate；只多出 `crewd-collab` 一条。

**Spec:** `docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`：§22（全部；§22.2 范围、§22.5 冻结接口、§22.6 给 M4b 的口、§22.7 待拍板）、§17（多群模型与 @人类规则）、§19（M4a：`open_ask` 发 H 号、Q1）、§14 / §18 遗留（Db 线程、契约整理）、§13 Q1（多父归最早的根）。Swift 对照（路径相对仓库根）：`Sources/Stores/LocalCrewStore.swift:356-526`（DAG）、`Sources/Stores/CrewDirectory.swift:8-13, 125-170`（号码）、`Sources/Mcp/CrewWhiteboardVisibility.swift`（可见性）。

> **本计划的代码没有编译过。** 写计划时只读了 `crewd-m2b@b05a57e` 的源码与 rusqlite / schemars 的源码来核 API，没有在副本里重放。代码块是精确的意图：编不过时按编译器修语法 / 借用 / 导入，**不改语义、不改测试断言**；改了断言的一律在交付记录里写明为什么。每个任务的「确认失败」步骤必须真红过才往下走。

## Global Constraints

- 所有命令在 `crewd/` 目录下跑（worktree 里就是 `<worktree>/crewd`，不 `cd` 到别的 worktree）；找不到 `cargo` 先 `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`。
- 依赖一律 `cargo add --offline`（只用 `Cargo.lock` 里已有的版本），**不要手写版本号**。`Cargo.lock` 要提交。
- 依赖方向：`crewd-collab` → `crewd-store`、`crewd-proto`、`crewd-work`；`crewd-server` → `crewd-collab`；`crewd-collab` **不依赖** tokio / server / exec，不读时钟（时间一律是 `Tx` 里调用方给的 `now_ms`），不起进程。`crewd-store` 只多依赖 tokio 的 `sync` 特性（oneshot）。
- **给 LLM 看的文字不写进代码**（spec §22.2；记忆：prompt 不内联）：渲染文字只在 `crates/collab/prompts/render.{zh,en}.md`，格式同 `crates/jev/src/prompts.rs`（`include_str!` + `## 节名`），节缺失有测试。错误信息、日志不算提示词。
- 线上 JSON 一律 camelCase；新枚举在线上是 lowercase / snake_case 字符串；全部 params 结构 `#[serde(deny_unknown_fields)]`。
- 测试**只用合成数据**（仓库公开）：机组 / session / 消息全用 `crew-a`、`cap-a`、`w-a1`、「示例消息」这类编的名字；不读真实数据目录、不读 `~/.claude`。Task 15 的真实数据演练在**副本**上跑、只报计数、产出不进仓库。
- 迁移在一个事务里完成：失败 = 库保持 v2、crewd 拒绝启动并说清原因（不做半截迁移、不静默丢行）。
- 每个任务结束跑 `./scripts/check.sh`（fmt + clippy -D warnings + test，全 workspace）全绿再提交。**永远不跑 `--ignored` 测试**（`live_claude` 等真机测试只在人点头后另跑）。**不按名字 / 模式杀进程**（不用 `pkill` / `killall` / `kill $(pgrep ..)`）；测试里起的 serve 用 oneshot / 子进程句柄关。
- 提交：只 `git add` 该任务列出的文件（显式路径，相对 `crewd/`；spec 路径写 `../docs/...`），提交前看 `git diff --cached --stat` 行数对不对；**不 `--amend`**；提交信息末尾一行 `Co-Authored-By: Claude <noreply@anthropic.com>`（就这一行，不带模型名）。
- 文件归属（spec §22.5）：本计划创建的 `collab/src/{worldview,turn_trace}.rs` 只放签名与最小实现，M3b 接手；`server/src/orchestra/`、`crates/wake/` 本计划不建。
- 本机 shell 是 zsh：照抄给出的命令，别把多个参数塞进一个变量再展开。

## 范围

做：Db actor；schema v3 + 迁移（含 seq 高水位、sender / mention 词表归一、大厅回填、游标钉末尾、机长指针、分机号、机组号计数器）；导入器改写 v3；proto 的类型化词表与全部新线上类型；`crewd-collab`（群、成员、注视、发消息、列表与搜索、游标、未读、红标、可见性、收听、机组 DAG、编号、通讯录、session 行与其生命周期字段的写路径、人类待确认、审批、唤醒入队与队列状态、熔断台账、渲染、世界观 / 回合收尾的签名桩）；`Hub` 的内部事件通道与 `AppState::collab`；`SessionControl` trait + `FakeSessionControl` + 默认的 `NoSessions`；`TokenTable`；§22.2 表里全部客户端 RPC 与事件，删掉 `whiteboard.*`；契约整理（单一 `$defs`、类型化的方法 / 事件表、`ServerFrame`、枚举）；`crewd rpc` 驱动全部方法的端到端测试；真实数据只读演练；spec 交付记录。

不做（归 M3b / M3c / M4b）：起停进程、会话管理、唤醒投递与准入判定（M3c；本计划只给队列的读写函数）；`crewd mcp` / `crewd hook`、工具表、世界观正文、`turn_trace` 判定（M3b）；把旧人类 Todo 账迁成 H 号（M4b，§10）；私有群（Q3）；附件上传（`message.post` 的 `attachments` 非空一律拒，见裁定 14）；`session.changed` 事件的发送（M3c，本计划只定类型与主题）。

## 写计划时定下的裁定（执行时照做，有异议先回报）

1. **crate 叫 `crates/collab`**（包 `crewd-collab`，库 `crewd_collab`）。SQL 与规则都在 collab；store 只留 schema / 迁移 / `Db` / 导入要用的行函数（`messages`、`crews`、`sessions`、`todos`、`archive` 已有的那些，加上 `channels::{lobby_id, ensure_lobby}` 与 `schema::backfill_v3`）。理由：规则和它要的 SQL 分在两个 crate，改一条规则要改两处。
2. **`Tx` + `Change` 清单**：collab 不碰 tokio，所以不能自己发事件；每个写操作往 `Tx` 里记 `Change`，`collab::run` 提交成功才把清单交回（回滚 = 清单作废，不会有「事件发了、数据没落」）。`AppState::collab` 在**库线程上**提交并发布，事件顺序与提交顺序一致（取代 M1「持锁期间发布」的做法）。
3. **`CollabEvent` 的类型放在 collab**：spec §22.5 写「`CollabEvent` 定义在 `crewd-server::hub`」，但产生它的是同步的 collab（不能依赖 server）。所以类型是 `crewd_collab::Change`，`crewd_server::hub` 里 `pub use crewd_collab::Change as CollabEvent;`。对 M3c 而言名字与位置都照 spec（`crewd_server::hub::CollabEvent`）。
4. **时间**：collab 只认 `now_ms: i64`（Unix 毫秒，调用方给）；写进 TEXT 列的时间用 `crewd_store::ts_from_ms(ms)`（RFC 3339、毫秒、`Z`），与 M1 的 `now_ts()` 同格式。需要比较的时间列（`wakes.not_before`、`listens.until`）存 INTEGER 毫秒。crewd-work 的 `Ts` = `Ts(now_ms)`。
5. **群 id**：大厅 `'<crew_id>/lobby'`、人类群 `'<root_crew_id>/human'`（确定性，spec §22.2）；topic / dm 是 `'ch-' + uuid v4 simple`。大厅与人类群的 `name` 存空串，客户端按 `kind` 本地化显示（不在库里写界面文案）。
6. **sender 与 mention 词表收紧**：`messages.sender_kind ∈ {human, session, captain, system}`（v3 表上有 CHECK）；迁移把旧的 `user` → `human`、`pendingcrew` → `system`。`Mention.kind ∈ {session, captain, human, broadcast}`。迁移前先查：还有别的值 → 迁移中止，报出全部认不出的值（不猜、不丢）。导入器同样归一，认不出的行进报告的 skipped。
7. **seq 高水位不回退**：messages 表重建后把 `sqlite_sequence` 恢复到 max(旧高水位, 现有最大 seq)。删过尾部消息的库，迁移后新消息不会复用旧 seq（客户端按 `afterSeq` 增量拉）。
8. **v3 在 §22.2 之外补的列**（spec 说缺字段改 v3，我补，并在交付记录里列出）：`crews.captain_session_id`（当前机长指针：旧数据里一个机组可能有多条 `captain-*` 行，没有指针就答不出「此刻的机长」；迁移取 role=captain 中 `updated_at` 最新的）、`crews.next_ext`（分机号计数器，不回收）、`sessions.ext`（worker 的分机号；机长恒为 1，不存）、`meta.nextCrewNumber`（机组号计数器，不回收；迁移取 max(旧 `local-crews.json` 归档里的 `nextCrewNumber`, max(number)+1)）。`crews.title_source` 与 `sessions.ext` 从 `extra` JSON 里取（`$.titleSource`、`$.member.extensionNumber`）；`extra` 的其余部分不解析。
9. **游标缺行 = 第一次读时钉到末尾**：`unread_for` 碰到没有游标行的（session, 群）先写一行钉在当前末尾、本次不返回任何消息；所有入群点（注册 session、加成员、自动入群、建群、换机长）也显式钉。读永远不会把历史整段倒给 agent（#595）。人类游标缺行 = 0（新群一开始是空的；迁移已钉旧群）。
10. **自己发的消息不算自己的未读**；人类发消息时把人类游标推进到这条（人发言 = 看过这个群到这里）。
11. **人类可以往任何群发言、不改成员关系**（spec §17「人类可以进任何群发言」）；session 只能往自己是成员的群发，例外是根机组人类群：发言即显式入群。system 可以往任何群发。dm = 恰好两个不同成员、至多一个是人类、不能加人（spec 写「恰好两个 session」，但 `channel.create` 是人调的，人发起 dm 时一方必然是人——按「两人」解释，见 spec 问题 6）。
12. **@ 在发送时解析**（spec §22.2）：`captain` 落成群归属机组此刻的 `captain_session_id`（没有机长 → 拒）；`session` 目标必须是群成员（不是 → 拒，错误里列出成员）；`reply_to` 必须是同群消息，自动追加 `broadcast` + 被回复者（session 发的 → `session`；人发的 → `human`；system 发的只追加 broadcast），去重。回执带 `unwatchedHumanMention`：mentions 含 human、群没被注视、且不是根机组人类群——文案由 M3b 渲染。
13. **问的归属**：session 的问记在**它自己机组**的工作账本（H 号按机组发，§19 Q1），公告发到该机组的根的人类群（懒建），`ask_meta` 记公告消息 id 与 resume_note。公告的发送者是提问的 session 本人（`category = "ask"`，mentions = `[human]`），不检查它是不是人类群成员（内部投递）。回答后唤醒的 `source` 是 `'ask:<crew_id>/H<n>'`（H 号按机组，`'H<n>'` 单独不唯一；spec 写的是 `'H<n>'`，见 spec 问题 4）。
14. **附件**：上传协议 spec 没定；`message.post` 带非空 `attachments` 一律 `-32602 附件上传尚未实现`，不接受客户端给的路径（防路径注入）。M1 导入的旧附件照常列出。
15. **`SessionControl` 比 §22.5 多两个方法**：`resume`（M3c 的 `session.resume` RPC）与 `handoff_captain`（M3b 的交接工具「经 SessionControl」）。trait 在 M3a 的文件里，M3b / M3c 不能改它，所以现在就补齐（见 spec 问题 2）。参数一律 owned（`String`），方法返回 `BoxFuture<'_, ..>`，可做 `dyn`。
16. **队列与 session 生命周期的写路径都在 collab**：spec §22.5 只列了 `enqueue_wake`，但 M3c 只能改 `crates/wake/` 与 `server/src/orchestra/`，而「M3b / M3c 只经 collab 写协作数据」。所以 M3a 提供 M3c / M3b 要的全部写函数：`wakes::{enqueue, due_for, mark_inflight, mark_delivered, mark_failed, mark_dropped, reset_inflight, next_due_at, sessions_with_due, recent_for, list}`、`wakes::latches::{set, clear, get, list}`、`sessions::{register, set_desired, set_working, set_resume_failed, set_awaiting_question, set_profile, note_output}`、`crews::set_captain`、`restart::*`、`channels` 的群主转让（见 spec 问题 1）。
17. **`ask.answer` 的 BlockerCleared 回调点**：spec §22.6 写「M3c 在 `ask.answer` 里留一个回调点」，但 `ask.answer` 是 M3a 的 RPC、M3c 不改 dispatch。改为：`asks::answer` 产出 `Change::AskAnswered { crew_id, ask, asked_by, cleared }`（`cleared` = M4a 事件里每条 `BlockerCleared` 的 (项, 负责人)），M3c / M4b 订阅 `CollabEvent` 处理（见 spec 问题 3）。
18. **契约版本**：`API_VERSION` 1 → 2（`whiteboard.*` 删除、`Message.crewId` → `channelId` 都是破坏性改动）。

## 人的拍板（spec §22.7，2026-10-06，commit fdc884e）在 M3a 的落点

§22.7 的 Q1–Q5 已由人拍板，取代原推荐。本计划按拍板写；每条在 M3a 里的落点收在一处：

| 拍板 | M3a 做什么（数据 + API，行为归 M3c / M3b） | 落点 |
|---|---|---|
| Q1 重启后**问人**是否恢复上次在跑的 session | 表 `restart_prompts`；`collab::restart::{open, answer, pending, get}`：`open` 记名单并在每个涉及的根机组人类群各发一条 @human 公告；`answer(Resume \| Later)` 只记账、产出 `Change::RestartAnswered`，**真正的 `--resume` 由 M3c 订阅后做**；`Later` 之后还能再答 `Resume`。RPC `restart.pending` / `restart.answer`，事件 `restart.changed`。期间入队的唤醒照常落 `wakes` | Task 10（`restart.rs`）、Task 13 |
| Q2 **每个群有群主**；人在群里不 @ 人时 Jev 选、Jev 缺席 / 低置信交群主；session 可**收听**群 | `channels.owner_kind / owner_id`（topic / dm 默认建群者；大厅、人类群存 NULL = 归属机组此刻的机长，现算）；转让 = 原群主发起 + 新群主接受，或机长 / 人强制（同工作项负责人规则）：`pending_owner_*` 三列 + `channels::{owner_of, request_owner_transfer, accept_owner_transfer, withdraw_owner_transfer, force_owner}`；RPC `channel.setOwner`（人 = 强制）。收听：`listens` 表 `until` 可空（NULL = 一直收听），`listens::{set, clear, active_for_channel, channels_of}`；收听的群进注入面（`cursors::unread_for`）。叫谁的判定本身在 M3c（候选 = 群成员 ∪ 收听者，同一条消息每个 session 只叫一次） | Task 7、Task 9、Task 13 |
| Q3 私有群：不做 | 不建任何可读性字段；`channel.list` / `message.list` 不按成员过滤 | — |
| Q4 删 raise / clear_attention、crew_ordering_signals、arrange_crews 及其存储；侧栏用基础序（最近有动静在上） | 不建 attention / 排序表（旧 `crew-arrangement.json` 只在 M1 的 `legacy_archive` 里，不读）；`crew.list` 按「最近动静」降序返回，并带 `lastActivityAt`（该机组自己的群里最新一条消息的时间，没有消息用 `createdAt`） | Task 6（`crews::list_by_activity`）、Task 13 |
| Q5 **默认不设并发上限**（设置里可选）；防**异常唤醒循环**：「来源 → 目标」与「目标 session」两级熔断，判据 = 短时间内重复唤醒且没有新产出 | `collab::settings::{max_live_sessions() -> Option<u32>（缺省 None）, set_max_live_sessions(Option<u32>)}`；熔断台账 `wake_latches(scope ∈ {edge, session})`，`edge` 的 id = `'<来源 session>→<目标 session>'`；`wakes.origin`（触发这次唤醒的 session，可空）+ `WakeRequest.origin`；「新产出」的数据：`sessions.last_output_at`，由 `messages::post`（session 发言）自动写、`sessions::note_output(tx, session)` 供 M3b 的 post-tool-use hook 与 M4b 的工作项命令调用；`wakes::recent_for(conn, target, since_ms)` 给熔断判定读近期唤醒。**阈值与判据常量归 M3c 的 `crewd-wake`**（拍板要求「写成一处常量，带测试」） | Task 5（`wakes.rs`）、Task 6（`sessions.rs`）、Task 8 |

v3 的 `wake_latches.scope` 因此是 `edge|session`（§22.2 原写的 `session|crew|machine` 作废：拍板禁止按全机总量限流）。

另有一个本计划自己的默认、请人顺带确认：**语言**。渲染有 zh / en 两份；`collab::settings::locale(conn)` 读 `meta.locale`（缺省 `zh`）。M3 范围里没有设置页 RPC，要改先手工写 meta（见 spec 问题 9）。

## Review Focus

1. **删过尾部消息的旧库迁移后，新消息的 seq 不回退**：客户端拿着 `afterSeq = 旧最大值` 增量拉，回退会让新消息永远拉不到。（Task 3 `migration_keeps_the_seq_high_water_mark`）
2. **迁移 / 导入 / 新 session / 新加入群之后，没有任何人凭空多出一屏未读、红标或唤醒**：游标缺行不能当 0。（Task 3 `migration_creates_no_unread_and_no_badges`、Task 9 `missing_cursor_pins_to_end_instead_of_dumping_history`、Task 7 `joining_a_topic_starts_at_the_end`）
3. **旧数据里出现认不出的 sender / mention 词时，迁移整体中止、库保持 v2、报出那些值**，而不是 CHECK 约束报一句天书或悄悄丢行。（Task 3 `unknown_sender_kind_aborts_migration_and_keeps_v2`）
4. **换了机长之后，旧消息里的 @机长 仍指向当时那位**，新消息的 @机长 指向新机长；可见性按落库的 targetId 判，不按「此刻是不是机长」。（Task 8 `captain_mention_is_resolved_at_send_time`、Task 9 `visibility_follows_the_stored_target_not_the_current_captain`）
5. **一条事务里途中失败（比如开问时人类群公告写失败、release-to 新边成环）不留半截**：H 号不消耗、旧父边还在、没有事件发出。（Task 6 `release_to_failure_leaves_no_side_effect`、Task 10 `failed_ask_consumes_no_h_number_and_emits_nothing`）

## 文件结构

```
crewd/crates/store/
  Cargo.toml                 + tokio(sync)；dev + tokio(rt, macros, time)                      (T1)
  src/lib.rs                 pub mod db / channels；StoreError::{DbClosed, DbJobPanicked}；now_ms、ts_from_ms (T1, T3)
  src/db.rs                  Db actor                                                          (T1)
  src/schema.rs              V3、migrate、backfill_v3、迁移测试                                  (T3)
  src/channels.rs            lobby_id、human_id、ensure_lobby                                   (T3)
  src/messages.rs            按 channel_id；normalize_sender_kind；list 加 before；get           (T3)
  src/crews.rs / sessions.rs 读写 v3 新列                                                       (T4)
  tests/content.rs           Message 字段改名跟着改                                             (T3, T4)
crewd/crates/work/
  src/{model,perm,events,book,block}.rs   withdraw_ask                                         (T2)
crewd/crates/proto/
  src/model.rs               类型化词表、v3 字段、全部新线上类型与 params                         (T3, T4)
  src/lib.rs                 methods / events 常量、parse 助手、api_schema 重构                   (T4, T13, T14)
crewd/crates/legacy/
  src/{crews,whiteboard,lib}.rs  导入写 v3（大厅、归一、backfill）                              (T3)
crewd/crates/collab/                                                                          (T5–T11)
  Cargo.toml
  src/lib.rs                 Tx、run、Change、CollabError、Result、Locale
  src/testkit.rs             单测夹具（cfg(test)）
  src/settings.rs            locale、max_live_sessions（Option，缺省不设）
  src/restart.rs             重启后问人是否恢复（Q1）
  src/wakes.rs               WakeKind、WakePriority、WakeRequest（含 origin）、source 约定、队列读写、近期唤醒、熔断台账（edge / session）
  src/crews.rs               root_of、captain_of、建 / 改 / 收编 / 摘出 / 删、编号、set_captain、detail
  src/sessions.rs            register（分机号）、get / list、生命周期字段 setter
  src/directory.rs           通讯录行
  src/channels.rs            建群、成员、群主与转让、归档、注视、人类群懒建、成员判定、列表与摘要
  src/messages.rs            post（校验 + @ 解析 + 落库）、list、search、get
  src/cursors.rs             游标、unread_for、advance、mark_read、badge、visible_to、directed_targets
  src/listens.rs             收听表
  src/asks.rs                工作账本读写、open / answer / withdraw / apply_due_defaults、视图
  src/approvals.rs           审批台账
  src/prompts.rs             md 节解析（照 jev）
  src/render.rs              ChannelBlock、WakeBatch、injection、wake_text、build_batch、blocks_for
  src/worldview.rs           WorldCtx、context_for、render（M3b 接手）
  src/turn_trace.rs          TurnEnd、on_turn_end（M3b 接手）
  prompts/render.zh.md / render.en.md
crewd/crates/server/
  Cargo.toml                 + crewd-collab
  src/lib.rs                 AppState { db, hub, sessions, tokens }、AppState::collab、serve 用 Db  (T1, T12)
  src/hub.rs                 Hub 双通道、CollabEvent、emit                                      (T12)
  src/control.rs             SessionControl、StartRequest、Inspection、QuotaReport、ControlError、NoSessions、FakeSessionControl (T12)
  src/token.rs               TokenTable                                                        (T12)
  src/dispatch.rs            全部客户端 RPC                                                     (T1, T3, T13)
  src/conn.rs                主题表                                                            (T13)
  tests/socket.rs            改写成新方法                                                       (T13)
crewd/crates/crewd/
  tests/e2e.rs               import→serve→message.*；rpc 驱动全部方法                          (T3, T14)
  tests/claude_cli.rs        Session / Crew 字面量改 ..Default::default()                       (T4)
crewd/schema/crewd-api.schema.json   重新生成                                                  (T13, T14)
crewd/README.md              一段                                                             (T15)
docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md   追加 M3a 交付记录   (T15)
```

## 冻结给 M3b / M3c 的接口（§22.5 + §22.7 拍板；M3a 合进 main 后只能加、不能改签名）

```rust
// ── crewd-store ──
pub struct Db;                                         // Clone
impl Db {
    pub fn spawn(store: Store) -> std::io::Result<Db>;
    pub async fn call<T: Send + 'static, F: FnOnce(&mut Store) -> T + Send + 'static>(&self, f: F) -> crewd_store::Result<T>;
}
pub fn now_ms() -> i64;
pub fn ts_from_ms(ms: i64) -> String;

// ── crewd-collab（全部同步；写函数第一个参数 &mut Tx，读函数 &rusqlite::Connection）──
pub struct Tx<'a>;
impl Tx<'_> { pub fn conn(&self) -> &Connection; pub fn now_ms(&self) -> i64; pub fn now_ts(&self) -> String; pub fn note(&mut self, c: Change); }
pub fn run<T>(store: &mut Store, now_ms: i64, f: impl FnOnce(&mut Tx<'_>) -> Result<T>) -> Result<(T, Vec<Change>)>;
pub enum Change {
    MessageAppended { channel_id: String, seq: i64, message_id: String, sender_session_id: Option<String> },
    ChannelChanged { channel_id: String }, CrewChanged { crew_id: String }, CrewRemoved { crew_id: String },
    BadgeChanged { channel_id: String }, SessionChanged { session_id: String },
    AskChanged { crew_id: String, ask: String },
    AskAnswered { crew_id: String, ask: String, asked_by: Option<String>, cleared: Vec<(String, Option<String>)> },
    ApprovalChanged { id: String }, WakeEnqueued { wake_id: i64, session_id: String },
    RestartChanged { id: i64 }, RestartAnswered { id: i64, answer: RestartAnswer, session_ids: Vec<String> },
}
pub enum CollabError { NotFound(String), Invalid(String), Forbidden(String), Conflict(String), Work(crewd_work::Rejected), Store(StoreError) }
pub type Result<T> = std::result::Result<T, CollabError>;
pub enum Locale { Zh, En }

messages::{post(tx, Sender, NewPost) -> Posted, list(conn, channel, after, before, limit), search(conn, &Search), get(conn, id)}
    // Sender::{Human, Session(String), System}；Posted { message: Message, unwatched_human_mention: bool }
channels::{create(tx, &Sender, NewChannel) -> Channel, ensure_dm(tx, crew, a: MemberRef, b: MemberRef) -> (Channel, bool),
           update(tx, by: &Sender, id, ChannelPatch) -> Channel, add_members(tx, by, id, &[MemberRef]) -> Channel,
           remove_member(tx, by, id, &MemberRef) -> Channel, watch(tx, id, on) -> ChannelSummary,
           ensure_human_channel(tx, root_crew) -> Channel, get(conn, id), is_member(conn, id, &MemberRef) -> bool,
           members(conn, id) -> Vec<MemberRef>, channels_of_session(conn, session) -> Vec<Channel>,
           list_for_crew(tx, crew) -> Vec<ChannelSummary>, summary(conn, id) -> ChannelSummary,
           owner_of(conn, id) -> Option<MemberRef>, request_owner_transfer(tx, by, id, to), accept_owner_transfer(tx, by, id),
           withdraw_owner_transfer(tx, by, id), force_owner(tx, by, id, to)}
cursors::{unread_for(tx, session) -> Vec<(String, Vec<Message>)>, advance(tx, reader, channel, seq), mark_read(tx, channel, seq) -> Badge,
          badge(conn, channel) -> Badge, visible_to(&Message, session) -> bool, directed_targets(&Message, session) -> Option<Vec<String>>}
listens::{set(tx, session, channel, until_ms: Option<i64>, senders: Vec<String>), clear(tx, session, channel: Option<&str>),
          active_for_channel(conn, channel, now_ms) -> Vec<Listen>, channels_of(conn, session, now_ms) -> Vec<String>}
crews::{root_of, captain_of, get, list, list_by_activity, detail, children, create, update, adopt, release, delete, set_captain}
sessions::{register(tx, NewSession) -> Session, get, list, crew_of, role_of, running_desired, set_desired, set_working,
           set_resume_failed, set_awaiting_question, set_profile, note_output}
directory::list(conn, query: Option<&str>) -> Vec<DirectoryRow>
asks::{open(tx, asker: &str, OpenAsk) -> Ask, answer(tx, crew, ask, text) -> Ask, withdraw(tx, by: &Sender, crew, ask, reason) -> Ask,
       apply_due_defaults(tx, crew) -> Vec<String>, get(conn, crew, ask) -> Ask, waits(conn, now_ms, root: Option<&str>) -> Vec<HumanWait>,
       with_book(tx, crew, |book: &mut WorkBook, now: Ts| -> Result<R>) -> Result<R>}      // M4b 用 with_book 跑工作项命令
approvals::{request(tx, NewApproval) -> Approval, decide(tx, id, allow, by: &str) -> Approval, get(conn, id), list_pending(conn)}
restart::{open(tx, session_ids: Vec<String>) -> Option<RestartPrompt>, answer(tx, id, RestartAnswer) -> RestartPrompt,
          pending(conn) -> Option<RestartPrompt>, get(conn, id)}
wakes::{WakeKind, WakePriority, WakeState, WakeRequest { session, kind, source, priority, not_before_ms, note, origin }, Enqueued, WakeRow,
        source::{message, ask, timer, quota, restart, system}, enqueue, due_for, mark_inflight, mark_delivered, mark_failed, mark_dropped,
        reset_inflight, next_due_at, sessions_with_due, recent_for, list}
wakes::latches::{Scope::{Edge { from, to }, Session(String)}, Latch, set, clear, get, list}
render::{ChannelBlock, MsgLine, WakeBatch, WakeItem, WakeBody, blocks_for(conn, session, &[(String, Vec<Message>)]) -> Vec<ChannelBlock>,
         injection(Locale, &[ChannelBlock]) -> String, wake_text(Locale, &WakeBatch) -> String,
         build_batch(conn, session, &[WakeRow]) -> Result<WakeBatch>}
settings::{locale(conn) -> Locale, max_live_sessions(conn) -> Option<u32>, set_max_live_sessions(tx, Option<u32>)}
worldview::{WorldCtx, context_for(conn, session, Locale) -> Result<WorldCtx>, render(&WorldCtx) -> String}       // M3b 实现
turn_trace::{TurnEnd, on_turn_end(tx, session, prompt_id, last_text) -> Result<TurnEnd>}                        // M3b 实现

// ── crewd-server ──
pub use crewd_collab::Change as CollabEvent;                                          // hub.rs
impl Hub { pub fn subscribe_collab(&self) -> broadcast::Receiver<CollabEvent>; pub fn emit(&self, conn: &Connection, changes: &[Change]); }
impl AppState {
    pub fn new(db: Db) -> Self;                               // sessions = NoSessions，tokens 空表
    pub fn with_sessions(self, s: Arc<dyn SessionControl>) -> Self;   // M3c 在 serve 启动段换上真实现
    pub async fn collab<T: Send + 'static>(&self, f: impl FnOnce(&mut Tx<'_>) -> collab::Result<T> + Send + 'static) -> collab::Result<T>;
}
pub trait SessionControl: Send + Sync {                  // control.rs；参数 owned，返回 BoxFuture<'_, Result<_, ControlError>>
    fn start(&self, req: StartRequest) -> BoxFuture<'_, Result<Session, ControlError>>;
    fn resume(&self, session_id: String) -> BoxFuture<'_, Result<(), ControlError>>;
    fn stop(&self, session_id: String, reason: String) -> BoxFuture<'_, Result<(), ControlError>>;
    fn nudge(&self, session_id: String, text: String) -> BoxFuture<'_, Result<(), ControlError>>;
    fn inspect(&self, session_id: String, max_lines: usize) -> BoxFuture<'_, Result<Inspection, ControlError>>;
    fn set_profile(&self, session_id: String, model: Option<String>, effort: Option<String>) -> BoxFuture<'_, Result<(), ControlError>>;
    fn handoff_captain(&self, crew_id: String, to_session_id: String) -> BoxFuture<'_, Result<(), ControlError>>;
    fn quota(&self) -> BoxFuture<'_, Result<QuotaReport, ControlError>>;
    fn live(&self, session_id: String) -> BoxFuture<'_, Result<SessionLive, ControlError>>;
}
pub struct NoSessions;  pub struct FakeSessionControl;  // control.rs
pub struct TokenTable;  impl TokenTable { pub fn mint(&self, session: &str) -> String; pub fn verify(&self, session: &str, token: &str) -> bool; pub fn revoke(&self, session: &str); }

// ── crewd-proto（线上类型，Task 4 / Task 10 / Task 13 给出全文）──
SenderKind, MentionKind, Mention, Role, Desired, ChannelKind, MemberRef, Channel, ChannelSummary, Badge, Crew(+v3 字段), CrewDetail,
Session(+v3 字段), Message(channelId), Ask, AskStateView, HumanWait, Approval, DirectoryRow, SessionLive, HealthView, DecisionView,
RestartPrompt, RestartAnswer；methods::* 与 events::* 常量
```

`CommsSpec` 的改动归 M3b（spec §22.5），本计划不碰 `crewd-exec`。

---

### Task 1: `Db` actor（store 搬到专用线程）

M1 / M2b 遗留：RPC 在 tokio 工作线程上同步调 SQLite。`Db` 把 `Store` 交给一条名为 `crewd-db` 的线程，`call` 把闭包送过去、等 oneshot。闭包 panic 不杀线程（`catch_unwind`，事务随 `Transaction` drop 回滚），调用方拿到 `DbJobPanicked`。`AppState` 的 `tokio::sync::Mutex<Store>` 换成 `Db`。

**Files:**
- Create: `crewd/crates/store/src/db.rs`
- Modify: `crewd/crates/store/Cargo.toml`、`crewd/crates/store/src/lib.rs`、`crewd/crates/server/src/lib.rs`、`crewd/crates/server/src/dispatch.rs`、`crewd/crates/server/src/hub.rs`、`crewd/Cargo.lock`

**Interfaces:**
- Consumes: `crewd_store::Store`。
- Produces: `crewd_store::db::Db`（re-export `crewd_store::Db`）：`spawn(Store) -> io::Result<Db>`、`async call(f) -> Result<T>`；`StoreError::DbClosed`、`StoreError::DbJobPanicked`；`crewd_server::AppState { pub db: Db, pub hub: Hub }`、`AppState::new(db: Db)`；`Hub: Clone`。

- [ ] **Step 1: 加依赖**

```bash
cargo add --offline -p crewd-store tokio --features sync
cargo add --offline -p crewd-store --dev tokio --features rt,macros,time
```

- [ ] **Step 2: 写失败的测试**

`crates/store/src/db.rs`（整个文件，先只有测试会用到的声明；实现在 Step 4）：

```rust
//! 专用线程上的数据库 actor（spec §22.2；§14 / §18 遗留：RPC 不在 tokio 线程上同步调 SQLite）。
//! 一条名为 `crewd-db` 的线程独占 `Store`；`call` 按提交顺序串行执行闭包。
use crate::{Result, Store, StoreError};
use std::sync::mpsc;

type Job = Box<dyn FnOnce(&mut Store) + Send>;

/// 可克隆的句柄。所有句柄都 drop 之后线程退出、库关闭。
#[derive(Clone)]
pub struct Db {
    tx: mpsc::Sender<Job>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn db() -> Db {
        Db::spawn(Store::open_in_memory().unwrap()).unwrap()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_slow_job_does_not_block_the_runtime() {
        let db = db();
        let slow = {
            let db = db.clone();
            tokio::spawn(async move {
                db.call(|_| std::thread::sleep(Duration::from_millis(400)))
                    .await
            })
        };
        tokio::task::yield_now().await;
        let t = Instant::now();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            t.elapsed() < Duration::from_millis(200),
            "单线程 runtime 被库调用卡住了：{:?}",
            t.elapsed()
        );
        slow.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn jobs_run_in_submission_order() {
        let db = db();
        db.call(|s| {
            s.conn()
                .execute_batch("CREATE TABLE t(v INTEGER)")
                .unwrap()
        })
        .await
        .unwrap();
        let mut waits = Vec::new();
        for i in 0..50 {
            let db = db.clone();
            waits.push(db.call(move |s| {
                s.conn().execute("INSERT INTO t(v) VALUES(?1)", [i]).unwrap();
            }));
        }
        for w in waits {
            w.await.unwrap();
        }
        let got: Vec<i64> = db
            .call(|s| {
                let mut st = s.conn().prepare("SELECT v FROM t ORDER BY rowid").unwrap();
                st.query_map([], |r| r.get(0))
                    .unwrap()
                    .collect::<rusqlite::Result<_>>()
                    .unwrap()
            })
            .await
            .unwrap();
        assert_eq!(got, (0..50).collect::<Vec<i64>>());
    }

    #[tokio::test]
    async fn a_panicking_job_is_reported_and_the_thread_survives() {
        let db = db();
        let r: Result<()> = db.call(|_| panic!("boom")).await;
        assert!(matches!(r, Err(StoreError::DbJobPanicked)), "{r:?}");
        let n: i64 = db
            .call(|s| s.conn().query_row("SELECT 41 + 1", [], |r| r.get(0)).unwrap())
            .await
            .unwrap();
        assert_eq!(n, 42);
    }

    #[tokio::test]
    async fn a_panic_inside_a_transaction_rolls_it_back() {
        let db = db();
        db.call(|s| s.conn().execute_batch("CREATE TABLE t(v INTEGER)").unwrap())
            .await
            .unwrap();
        let _ = db
            .call(|s| {
                let _ = s.with_tx(|tx| {
                    tx.execute("INSERT INTO t(v) VALUES(1)", [])?;
                    panic!("half way");
                    #[allow(unreachable_code)]
                    Ok(())
                });
            })
            .await;
        let n: i64 = db
            .call(|s| s.conn().query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0)).unwrap())
            .await
            .unwrap();
        assert_eq!(n, 0);
    }
}
```

`crates/store/src/lib.rs`：在模块声明里加 `pub mod db;` 与 `pub use db::Db;`，`StoreError` 加两项：

```rust
    /// 库线程已经不在了（进程正在退出）。
    #[error("数据库线程已退出")]
    DbClosed,
    /// 交给库线程的闭包 panic 了；该闭包里的事务已回滚，线程照常服务。
    #[error("数据库任务 panic，已回滚")]
    DbJobPanicked,
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-store db::`
Expected: 编译失败，`no function or associated item named 'spawn'` / `no method named 'call'`。

- [ ] **Step 4: 实现**

在 `db.rs` 的 `struct Db` 之后、测试模块之前加：

```rust
impl Db {
    /// 把 `store` 交给库线程；之后只能经 `call` 用它。
    pub fn spawn(store: Store) -> std::io::Result<Db> {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("crewd-db".into())
            .spawn(move || {
                let mut store = store;
                while let Ok(job) = rx.recv() {
                    job(&mut store);
                }
            })?;
        Ok(Db { tx })
    }

    /// 在库线程上跑 `f`（与其它调用串行，按提交顺序）。
    pub async fn call<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Store) -> T + Send + 'static,
    {
        let (done, wait) = tokio::sync::oneshot::channel::<Result<T>>();
        let job: Job = Box::new(move |s| {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(s)));
            let _ = done.send(r.map_err(|_| StoreError::DbJobPanicked));
        });
        self.tx.send(job).map_err(|_| StoreError::DbClosed)?;
        wait.await.map_err(|_| StoreError::DbClosed)?
    }
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-store db::`
Expected: 4 passed。（panic 测试会在 stderr 打出 panic 信息，正常。）

- [ ] **Step 6: server 换成 `Db`——先写失败的测试**

`crates/server/src/hub.rs`：`pub struct Hub` 加 `#[derive(Clone)]`（`broadcast::Sender` 可克隆）。

`crates/server/src/lib.rs` 的测试模块里加：

```rust
    #[tokio::test(flavor = "current_thread")]
    async fn hello_answers_while_a_slow_db_job_runs() {
        use crewd_proto::{Request, methods};
        let db = crewd_store::Db::spawn(Store::open_in_memory().unwrap()).unwrap();
        let state = Arc::new(AppState::new(db.clone()));
        let slow = tokio::spawn(async move {
            db.call(|_| std::thread::sleep(std::time::Duration::from_millis(500)))
                .await
        });
        tokio::task::yield_now().await;
        let req = Request {
            id: 1,
            method: methods::HELLO.into(),
            params: serde_json::Value::Null,
        };
        let r = tokio::time::timeout(
            std::time::Duration::from_millis(150),
            dispatch::handle(&state, req),
        )
        .await
        .expect("慢查询进行中 system.hello 也要马上回");
        assert!(r.error.is_none());
        let req = Request {
            id: 2,
            method: methods::CREW_LIST.into(),
            params: serde_json::Value::Null,
        };
        let r = dispatch::handle(&state, req).await;
        assert!(r.error.is_none(), "{r:?}");
        slow.await.unwrap().unwrap();
    }
```

Run: `cargo test -p crewd-server hello_answers_while`
Expected: 编译失败（`AppState::new` 要 `Store`）。

- [ ] **Step 7: 实现 server 侧**

`crates/server/src/lib.rs`：

```rust
use crewd_store::{Db, Store};

pub struct AppState {
    pub db: Db,
    pub hub: Hub,
}

impl AppState {
    pub fn new(db: Db) -> Self {
        Self { db, hub: Hub::new() }
    }
}
```

`serve` 里：

```rust
    let store = Store::open(&data_dir.join("crewd.db"))?;
    let db = Db::spawn(store)?;
    let state = Arc::new(AppState::new(db));
```

`crates/server/src/dispatch.rs`：每处 `let s = state.store.lock().await; ... s.conn()` 改成 `state.db.call(move |s| ...).await`，两层错误都映射成 `internal`。`whiteboard.post` 整段放进一个闭包，**在闭包里**插入并发布（库线程串行 = 事件顺序与 seq 一致）：

```rust
async fn db<T: Send + 'static>(
    state: &AppState,
    f: impl FnOnce(&mut crewd_store::Store) -> crewd_store::Result<T> + Send + 'static,
) -> Result<T, RpcError> {
    state.db.call(f).await.map_err(internal)?.map_err(internal)
}
```

`CREW_LIST`：`to_value(db(state, |s| crewd_store::crews::list(s.conn())).await?)`；`WHITEBOARD_LIST`、`TODO_LIST` 同理（参数先 move 进闭包）。`WHITEBOARD_POST`：

```rust
            let hub = state.hub.clone();
            let posted = state
                .db
                .call(move |s| -> Result<Value, RpcError> {
                    if !crewd_store::crews::exists(s.conn(), &msg.crew_id).map_err(internal)? {
                        return Err(invalid(format!("机组不存在: {}", msg.crew_id)));
                    }
                    msg.seq = crewd_store::messages::insert(s.conn(), &msg, None)
                        .map_err(internal)?
                        .ok_or_else(|| internal("新消息 id 冲突"))?;
                    let v = to_value(&msg)?;
                    hub.publish(Event {
                        event: events::WHITEBOARD_APPENDED.into(),
                        data: v.clone(),
                    });
                    Ok(v)
                })
                .await
                .map_err(internal)??;
            Ok(posted)
```

（`whiteboard.*` 在 Task 3 改按大厅、Task 13 删除；这里只换线程。）

- [ ] **Step 8: 跑全量检查**

Run: `./scripts/check.sh`
Expected: 全绿（`crewd-server` 的 socket 集成测试照旧通过：行为没变，只换了执行线程）。

- [ ] **Step 9: 提交**

```bash
git add crates/store/Cargo.toml crates/store/src/db.rs crates/store/src/lib.rs crates/server/src/lib.rs crates/server/src/dispatch.rs crates/server/src/hub.rs Cargo.lock
git diff --cached --stat
git commit -m "crewd-store: Db actor on a dedicated thread; server RPC goes through it

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: crewd-work 补 `withdraw_ask`

spec §22.2：M3 只用 `open_ask / answer_ask / apply_default`，外加 `withdraw_ask(by, now, ask, reason)`——提问者 / 机长 / 人可撤；`AskState::Withdrawn { reason, at }`；问还卡着活项时拒绝。

**Files:**
- Modify: `crewd/crates/work/src/model.rs`（`AskState`）、`crewd/crates/work/src/perm.rs`（`Action::WithdrawAsk`、`Ctx.asker`、`ALL`、表）、`crewd/crates/work/src/events.rs`（`EventKind::AskWithdrawn`）、`crewd/crates/work/src/book.rs`（`Rejected::AskInUse` + `Display`）、`crewd/crates/work/src/block.rs`（命令 + 测试）

**Interfaces:**
- Consumes: M4a 的 `WorkBook::transact`、`Draft::{permit, emit}`、`testkit::{book, claim, kinds, T1}`。
- Produces: `pub fn withdraw_ask(&mut self, by: &Actor, now: Ts, ask: AskId, reason: &str) -> Result<Applied<()>, Rejected>`；`AskState::Withdrawn { reason: String, at: Ts }`；`Action::WithdrawAsk`；`Ctx::asker: Option<&SessionId>`；`Rejected::AskInUse(AskId)`；`EventKind::AskWithdrawn { ask: AskId, reason: String }`（serde 名 `ask_withdrawn`）。

- [ ] **Step 1: 写失败的测试**

`crates/work/src/block.rs` 的 `#[cfg(test)] mod tests` 里加（沿用该模块已有的 `use`；缺的补 `use crate::progress::{Progress, Report};`、`use crate::ids::HOUR;`）：

```rust
    #[test]
    fn withdraw_ask_closes_a_standalone_question() {
        let mut b = book();
        let h = b
            .open_ask(&Actor::worker("w"), T1, "发版窗口定在周几？", None, None)
            .unwrap()
            .value;
        let r = b
            .withdraw_ask(&Actor::worker("w"), T1, h, "已经自己查到了")
            .unwrap();
        assert_eq!(kinds(&r.events), ["ask_withdrawn"]);
        assert!(matches!(
            b.ask(h).unwrap().state,
            AskState::Withdrawn { ref reason, .. } if reason == "已经自己查到了"
        ));
        assert!(b.human_waits(T1).is_empty(), "撤回的问不再进人的清单");
        assert_eq!(
            b.answer_ask(&Actor::Human, T1, h, "周三").unwrap_err(),
            Rejected::AskNotOpen(h)
        );
        assert_eq!(
            b.withdraw_ask(&Actor::worker("w"), T1, h, "再撤一次").unwrap_err(),
            Rejected::AskNotOpen(h)
        );
    }

    #[test]
    fn only_the_asker_the_captain_or_the_human_may_withdraw() {
        let mut b = book();
        let h = b
            .open_ask(&Actor::worker("w"), T1, "要不要升级依赖？", None, None)
            .unwrap()
            .value;
        assert_eq!(
            b.withdraw_ask(&Actor::worker("x"), T1, h, "不是我的问").unwrap_err(),
            Rejected::Forbidden(Action::WithdrawAsk)
        );
        assert_eq!(
            b.withdraw_ask(&Actor::System, T1, h, "调度层不撤").unwrap_err(),
            Rejected::Forbidden(Action::WithdrawAsk)
        );
        b.withdraw_ask(&Actor::captain("c"), T1, h, "机长撤").unwrap();
        let h2 = b
            .open_ask(&Actor::worker("w"), T1, "第二个问题？", None, None)
            .unwrap()
            .value;
        b.withdraw_ask(&Actor::Human, T1, h2, "人撤").unwrap();
    }

    #[test]
    fn an_ask_that_still_blocks_a_live_item_cannot_be_withdrawn() {
        let mut b = book();
        let w = claim(&mut b, "w", &["接入支付"])[0];
        let r = b
            .block(
                &Actor::worker("w"),
                T1,
                w,
                vec![BlockOn::NewAsk {
                    question: "用哪家支付？".into(),
                    default: None,
                }],
                Continuation::Nothing,
            )
            .unwrap();
        let h = r.value.asks[0];
        let before = b.state().clone();
        assert_eq!(
            b.withdraw_ask(&Actor::worker("w"), T1, h, "不问了").unwrap_err(),
            Rejected::AskInUse(h)
        );
        assert_eq!(b.state(), &before, "被拒 = 原样不动");
        // 负责人不等了（report 回 active 会清空卡点），之后可以撤。
        b.report(
            &Actor::worker("w"),
            T1,
            w,
            Report {
                status: Some(Progress::Active),
                next_check_at: Some(T1.plus(HOUR)),
                note: "先用默认的".into(),
            },
        )
        .unwrap();
        b.withdraw_ask(&Actor::worker("w"), T1, h, "不问了").unwrap();
    }

    #[test]
    fn withdrawing_needs_a_reason_and_an_existing_ask() {
        let mut b = book();
        assert_eq!(
            b.withdraw_ask(&Actor::Human, T1, AskId(9), "x").unwrap_err(),
            Rejected::UnknownAsk(AskId(9))
        );
        let h = b
            .open_ask(&Actor::worker("w"), T1, "问题？", None, None)
            .unwrap()
            .value;
        assert_eq!(
            b.withdraw_ask(&Actor::worker("w"), T1, h, "  ").unwrap_err(),
            Rejected::Blank("reason")
        );
    }
```

`crates/work/src/perm.rs` 的测试模块里加：

```rust
    #[test]
    fn withdraw_ask_is_for_the_asker_captain_and_human() {
        let me = sid("w");
        let mine = Ctx {
            asker: Some(&me),
            ..Ctx::default()
        };
        assert!(allowed(&Actor::worker("w"), WithdrawAsk, mine));
        assert!(!allowed(&Actor::worker("x"), WithdrawAsk, mine));
        assert!(!allowed(&Actor::admin("a"), WithdrawAsk, Ctx::default()));
        assert!(allowed(&Actor::captain("c"), WithdrawAsk, Ctx::default()));
        assert!(allowed(&Actor::Human, WithdrawAsk, Ctx::default()));
        assert!(!allowed(&Actor::System, WithdrawAsk, Ctx::default()));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-work withdraw`
Expected: 编译失败（`withdraw_ask` / `WithdrawAsk` / `AskState::Withdrawn` / `Rejected::AskInUse` 不存在）。

- [ ] **Step 3: 实现**

`model.rs`，`AskState` 加一项：

```rust
    /// 提问者 / 机长 / 人撤回了（问没被回答就不要了）。
    Withdrawn { reason: String, at: Ts },
```

`perm.rs`：`Action` 在 `ApplyDefault` 之后加 `/// 撤回一条还开着的问（ctx.asker = 提问的 session）。 WithdrawAsk,`；`ALL` 改成 `[Action; 23]`，同一位置插入 `Action::WithdrawAsk`；`Ctx` 加字段：

```rust
    /// 提这条问的 session（撤回 H 号用）。
    pub asker: Option<&'a SessionId>,
```

`allowed`：`Actor::System` 的白名单不加；`Actor::Human` 的黑名单不加（人可撤）；`Actor::Session` 分支在 `OpenAsk => true,` 之后加：

```rust
                WithdrawAsk => *role == Role::Captain || ctx.asker == Some(id),
```

（已有的 grants / denied 清单测试不受影响：这些测试里 `asker` 都是 `None`，worker / admin 拿不到它；机长、人本来就在「允许」一侧。执行时若某条清单测试变了，停下查，不改期望。）

`events.rs`：`EventKind` 在 `AskDefaulted` 之后加：

```rust
    AskWithdrawn {
        ask: AskId,
        reason: String,
    },
```

`book.rs`：`Rejected` 在 `DefaultNotDue` 之后加 `/// 这条问还卡着活项。 AskInUse(AskId),`；`Display` 加 `Rejected::AskInUse(h) => write!(f, "{h} still blocks a live item"),`。

`block.rs`，`apply_default` 之后加：

```rust
    /// 撤回一条还开着的问（spec §22.2；提问者 / 机长 / 人）。还卡着活项的问不能撤——
    /// 先让负责人改道（report 回 open / active 会清空卡点），否则那一项会卡在一条没人会答的问上。
    pub fn withdraw_ask(
        &mut self,
        by: &Actor,
        now: Ts,
        ask: AskId,
        reason: &str,
    ) -> Result<Applied<()>, Rejected> {
        let asker = self
            .state
            .asks
            .get(&ask)
            .and_then(|a| a.asked_by.session().cloned());
        self.transact(by, now, |d| {
            let now = d.now;
            let a = d.s.asks.get(&ask).ok_or(Rejected::UnknownAsk(ask))?;
            let open = a.state == AskState::Open;
            d.permit(
                Action::WithdrawAsk,
                Ctx {
                    asker: asker.as_ref(),
                    ..Ctx::default()
                },
            )?;
            let reason = reason.trim().to_string();
            if reason.is_empty() {
                return Err(Rejected::Blank("reason"));
            }
            if !open {
                return Err(Rejected::AskNotOpen(ask));
            }
            let in_use = d
                .s
                .items
                .values()
                .any(|it| it.status.is_live() && it.blocked_on.contains(&Blocker::Human(ask)));
            if in_use {
                return Err(Rejected::AskInUse(ask));
            }
            d.s.asks.get_mut(&ask).expect("checked above").state = AskState::Withdrawn {
                reason: reason.clone(),
                at: now,
            };
            d.emit(EventKind::AskWithdrawn { ask, reason });
            Ok(())
        })
    }
```

编译器会指出所有对 `AskState` / `EventKind` / `Action` 的穷举 `match`（若有）：一律按「撤回 = 不再开着」处理（与 Answered / Defaulted 同侧），并在交付记录里列出改了哪几处。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-work`
Expected: 新增 5 条通过，M4a 原有的单测、`scenario`、`fuzz` 全部照旧通过（数字对不上先查，别改期望）。

- [ ] **Step 5: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/work/src/model.rs crates/work/src/perm.rs crates/work/src/events.rs crates/work/src/book.rs crates/work/src/block.rs
git diff --cached --stat
git commit -m "crewd-work: withdraw_ask (asker, captain or human; refused while it blocks a live item)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 3: schema v3 + 迁移；messages 按群归属；导入器写 v3

v3 是 M3 唯一一次迁移（spec §22.1）：一次建齐 M3b / M3c / M4b 要的表。messages 重建（`crew_id` → `channel_id`）；每个机组建大厅、旧消息归大厅；游标钉在末尾（迁移不产生未读 / 红标 / 唤醒）；sender 词表归一；补机长指针、分机号、机组号计数器（裁定 6–8）。数据回填写成一个幂等函数 `backfill_v3`，迁移与导入器共用。`whiteboard.*` 在本任务里临时改成读写大厅（Task 13 删除）。

**Files:**
- Create: `crewd/crates/store/src/channels.rs`
- Modify: `crewd/crates/store/src/schema.rs`、`crewd/crates/store/src/lib.rs`、`crewd/crates/store/src/messages.rs`、`crewd/crates/store/tests/content.rs`、`crewd/crates/proto/src/model.rs`（`Message.crew_id` → `channel_id`）、`crewd/crates/proto/src/lib.rs`（测试）、`crewd/crates/legacy/src/crews.rs`、`crewd/crates/legacy/src/whiteboard.rs`、`crewd/crates/legacy/src/lib.rs`、`crewd/crates/legacy/tests/import_basic.rs`、`crewd/crates/server/src/dispatch.rs`、`crewd/crates/server/tests/socket.rs`、`crewd/crates/crewd/tests/e2e.rs`、`crewd/schema/crewd-api.schema.json`（重新生成）

**Interfaces:**
- Consumes: Task 1 的 `Store`。
- Produces: `crewd_store::schema::{SCHEMA_VERSION = 3, backfill_v3(conn: &Connection) -> Result<()>}`；`crewd_store::channels::{lobby_id(crew: &str) -> String, human_id(root: &str) -> String, ensure_lobby(conn, crew_id: &str, created_at: &str) -> Result<String>}`；`crewd_store::messages::{normalize_sender_kind(&str) -> Option<&'static str>, insert(conn, &Message, extra) -> Result<Option<i64>>, list(conn, channel_id: &str, after_seq: i64, before_seq: Option<i64>, limit: u32) -> Result<Vec<Message>>, get(conn, id: &str) -> Result<Option<Message>>, max_seq(conn, channel_id: &str) -> Result<i64>}`；`crewd_store::{now_ms() -> i64, ts_from_ms(i64) -> String}`；`proto::Message.channel_id`。

- [ ] **Step 1: 写失败的测试（迁移）**

`crates/store/src/schema.rs` 的测试模块里：把已有的 `v1_database_is_migrated_to_executor_vocabulary` 里 `assert_eq!(v, 2);` 改成 `assert_eq!(v, SCHEMA_VERSION);`，然后加：

```rust
    use crate::{archive, channels, messages};

    /// 一个停在 v2 的库（真实用户升级前的样子）。
    fn v2_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(V1).unwrap();
        conn.execute_batch(V2).unwrap();
        conn.execute_batch(
            r#"
INSERT INTO crews(id,title,number,builtin,created_at,updated_at,extra) VALUES
  ('crew-a','示例机组',1,0,'2026-09-01T00:00:00.000Z','2026-09-01T00:00:00.000Z','{"titleSource":"human"}'),
  ('crew-b','子机组',2,0,'2026-09-02T00:00:00.000Z','2026-09-02T00:00:00.000Z',NULL),
  ('chief','总机组',NULL,1,'2026-08-01T00:00:00.000Z','2026-08-01T00:00:00.000Z',NULL);
INSERT INTO crew_parents(crew_id,parent_id) VALUES('crew-b','crew-a');
INSERT INTO sessions(id,crew_id,role,executor,updated_at,extra) VALUES
  ('captain-old','crew-a','captain','claude','2026-09-01T00:00:00.000Z',NULL),
  ('captain-new','crew-a','captain','claude','2026-09-05T00:00:00.000Z',NULL),
  ('worker-a1','crew-a','worker','claude','2026-09-01T00:00:00.000Z','{"member":{"extensionNumber":5}}'),
  ('worker-a2','crew-a','worker','codex','2026-09-01T00:00:00.000Z',NULL),
  ('worker-b1','crew-b','worker','claude','2026-09-02T00:00:00.000Z',NULL);
INSERT INTO messages(id,crew_id,sender_kind,sender_session_id,text,mentions,created_at) VALUES
  ('m1','crew-a','user',NULL,'人发的第一条','[]','2026-09-01T01:00:00.000Z'),
  ('m2','crew-a','session','worker-a1','收到','[{"kind":"session","targetId":"captain-new"}]','2026-09-01T01:01:00.000Z'),
  ('m3','crew-b','pendingcrew',NULL,'系统通知','[{"kind":"human"}]','2026-09-02T01:00:00.000Z'),
  ('m4','crew-a','captain','captain-new','@人类 请看','[{"kind":"human"},{"kind":"broadcast"}]','2026-09-01T02:00:00.000Z');
"#,
        )
        .unwrap();
        conn
    }

    fn one<T: rusqlite::types::FromSql>(c: &Connection, sql: &str) -> T {
        c.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn v2_database_migrates_to_v3_with_one_lobby_per_crew() {
        let c = v2_db();
        let seqs_before: Vec<i64> = {
            let mut st = c.prepare("SELECT seq FROM messages ORDER BY seq").unwrap();
            st.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
        };
        migrate(&c).unwrap();
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 3);
        let lobbies: Vec<(String, String)> = {
            let mut st = c
                .prepare("SELECT id, crew_id FROM channels WHERE kind='lobby' ORDER BY id")
                .unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert_eq!(
            lobbies,
            [
                ("chief/lobby".to_string(), "chief".to_string()),
                ("crew-a/lobby".to_string(), "crew-a".to_string()),
                ("crew-b/lobby".to_string(), "crew-b".to_string()),
            ]
        );
        let seqs_after: Vec<i64> = {
            let mut st = c.prepare("SELECT seq FROM messages ORDER BY seq").unwrap();
            st.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
        };
        assert_eq!(seqs_before, seqs_after, "seq 原样搬");
        let rows: Vec<(String, String, String)> = {
            let mut st = c
                .prepare("SELECT id, channel_id, sender_kind FROM messages ORDER BY seq")
                .unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert_eq!(
            rows,
            [
                ("m1".into(), "crew-a/lobby".into(), "human".into()),
                ("m2".into(), "crew-a/lobby".into(), "session".into()),
                ("m3".into(), "crew-b/lobby".into(), "system".into()),
                ("m4".into(), "crew-a/lobby".into(), "captain".into()),
            ]
        );
        assert_eq!(
            one::<i64>(&c, "SELECT COUNT(*) FROM pragma_foreign_key_check"),
            0
        );
        assert_eq!(one::<i64>(&c, "PRAGMA foreign_keys"), 1, "迁移后外键检查重新打开");
    }

    #[test]
    fn migration_keeps_the_seq_high_water_mark() {
        let c = v2_db();
        c.execute("DELETE FROM messages WHERE id='m4'", []).unwrap();
        migrate(&c).unwrap();
        let m = crewd_proto::Message {
            id: "m5".into(),
            channel_id: "crew-a/lobby".into(),
            sender_kind: "human".into(),
            text: "迁移后的新消息".into(),
            created_at: "2026-10-01T00:00:00.000Z".into(),
            ..Default::default()
        };
        let seq = messages::insert(&c, &m, None).unwrap().unwrap();
        assert_eq!(seq, 5, "删掉的 seq 4 不能被新消息复用");
    }

    #[test]
    fn migration_creates_no_unread_and_no_badges() {
        let c = v2_db();
        migrate(&c).unwrap();
        // 每个 session 在自己机组大厅的游标 = 该大厅最大 seq；人类在每个群的游标 = 该群最大 seq。
        let behind: i64 = one(
            &c,
            "SELECT COUNT(*) FROM sessions s
               LEFT JOIN read_cursors r ON r.reader=s.id AND r.channel_id=s.crew_id||'/lobby'
              WHERE r.seq IS NULL
                 OR r.seq < COALESCE((SELECT MAX(seq) FROM messages m WHERE m.channel_id=s.crew_id||'/lobby'),0)",
        );
        assert_eq!(behind, 0);
        let human_behind: i64 = one(
            &c,
            "SELECT COUNT(*) FROM channels ch
               LEFT JOIN read_cursors r ON r.reader='human' AND r.channel_id=ch.id
              WHERE r.seq IS NULL
                 OR r.seq < COALESCE((SELECT MAX(seq) FROM messages m WHERE m.channel_id=ch.id),0)",
        );
        assert_eq!(human_behind, 0);
        assert_eq!(one::<i64>(&c, "SELECT COUNT(*) FROM wakes"), 0);
    }

    #[test]
    fn migration_fills_captain_pointer_extensions_and_counters() {
        let c = v2_db();
        archive::put(
            &c,
            "local-crews.json",
            br#"{"version":1,"nextCrewNumber":9,"crews":[]}"#,
        )
        .unwrap();
        migrate(&c).unwrap();
        assert_eq!(
            one::<String>(&c, "SELECT captain_session_id FROM crews WHERE id='crew-a'"),
            "captain-new",
            "多条 captain 行时取最近更新的"
        );
        assert_eq!(
            one::<Option<String>>(&c, "SELECT captain_session_id FROM crews WHERE id='crew-b'"),
            None
        );
        assert_eq!(one::<i64>(&c, "SELECT ext FROM sessions WHERE id='worker-a1'"), 5);
        assert_eq!(
            one::<Option<i64>>(&c, "SELECT ext FROM sessions WHERE id='worker-a2'"),
            None,
            "旧数据没号的不编（同 Swift：通讯录里不放叫不出的号）"
        );
        assert_eq!(one::<i64>(&c, "SELECT next_ext FROM crews WHERE id='crew-a'"), 6);
        assert_eq!(one::<i64>(&c, "SELECT next_ext FROM crews WHERE id='crew-b'"), 2);
        assert_eq!(
            one::<String>(&c, "SELECT title_source FROM crews WHERE id='crew-a'"),
            "human"
        );
        assert_eq!(
            one::<String>(&c, "SELECT value FROM meta WHERE key='nextCrewNumber'"),
            "9",
            "归档里的计数器比 max(number)+1 大，取大的（号码不回收）"
        );
        assert_eq!(
            one::<String>(&c, "SELECT desired FROM sessions WHERE id='worker-a1'"),
            "stopped"
        );
    }

    #[test]
    fn unknown_sender_kind_aborts_migration_and_keeps_v2() {
        let c = v2_db();
        c.execute(
            "INSERT INTO messages(id,crew_id,sender_kind,text,created_at) VALUES('mx','crew-a','robot','?','2026-09-03T00:00:00.000Z')",
            [],
        )
        .unwrap();
        let err = migrate(&c).unwrap_err().to_string();
        assert!(err.contains("robot"), "{err}");
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 2);
        assert_eq!(
            one::<i64>(&c, "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name='crew_id'"),
            1,
            "messages 没被动过"
        );
        assert_eq!(
            one::<i64>(&c, "SELECT COUNT(*) FROM sqlite_master WHERE name='channels'"),
            0
        );
    }

    #[test]
    fn unknown_mention_kind_aborts_migration() {
        let c = v2_db();
        c.execute(
            "INSERT INTO messages(id,crew_id,sender_kind,text,mentions,created_at) VALUES('my','crew-a','session','?','[{\"kind\":\"everyone\"}]','2026-09-03T00:00:00.000Z')",
            [],
        )
        .unwrap();
        let err = migrate(&c).unwrap_err().to_string();
        assert!(err.contains("everyone"), "{err}");
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 2);
    }

    #[test]
    fn migrate_and_backfill_are_idempotent() {
        let c = v2_db();
        migrate(&c).unwrap();
        let snapshot = |c: &Connection| -> (i64, i64, i64, String) {
            (
                one(c, "SELECT COUNT(*) FROM channels"),
                one(c, "SELECT COUNT(*) FROM read_cursors"),
                one(c, "SELECT SUM(next_ext) FROM crews"),
                one(c, "SELECT value FROM meta WHERE key='nextCrewNumber'"),
            )
        };
        let a = snapshot(&c);
        migrate(&c).unwrap();
        backfill_v3(&c).unwrap();
        assert_eq!(snapshot(&c), a);
    }

    #[test]
    fn fresh_database_is_v3_and_has_no_lobbies_to_fill() {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        assert_eq!(one::<i64>(&c, "PRAGMA user_version"), 3);
        assert_eq!(one::<i64>(&c, "SELECT COUNT(*) FROM channels"), 0);
        assert_eq!(channels::lobby_id("x"), "x/lobby");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-store schema::`
Expected: 编译失败（`Message` 没有 `channel_id` / `Default`，`backfill_v3`、`channels` 不存在）。

- [ ] **Step 3: proto 改字段**

`crates/proto/src/model.rs`：`Message` 加 `Default` derive（`#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]`），`pub crew_id: String,` 改成：

```rust
    /// 归属的群（spec §17）：大厅是 `<crewId>/lobby`。
    pub channel_id: String,
```

`sender_kind` 的文档注释改成 `/// "human" | "session" | "captain" | "system"（v3 归一后的词表；Task 4 换成枚举）`。`crates/proto/src/lib.rs` 的测试 `message_uses_camel_case_and_skips_none` 里 `crew_id: "local-a".into()` → `channel_id: "local-a/lobby".into()`，断言 `v["crewId"]` → `v["channelId"] == "local-a/lobby"`。

- [ ] **Step 4: 实现 store**

`crates/store/src/lib.rs`：模块加 `pub mod channels;`；加时间助手（`now_ts` 改为基于它）：

```rust
/// Unix 毫秒（crewd 里唯一读墙钟的地方之一；collab 不读时钟，由调用方传入）。
pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// 毫秒 → RFC 3339（毫秒、`Z`），与 `now_ts()` 同格式，可按字符串比较先后。
pub fn ts_from_ms(ms: i64) -> String {
    chrono::DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn now_ts() -> String {
    ts_from_ms(now_ms())
}
```

加测试 `ts_from_ms_roundtrips`：`assert_eq!(ts_from_ms(0), "1970-01-01T00:00:00.000Z"); assert_eq!(ts_from_ms(1_790_000_000_123), "2026-09-21T13:33:20.123Z");`（第二个值执行时用 `date -u -r 1790000000` 核一次，对不上以 chrono 为准改**期望字符串**并在提交说明里写一句——这是本计划唯一允许按实测改的期望）。

已有测试 `is_unique_violation_only_matches_unique_and_primary_key` 里那条 `INSERT INTO messages(id,crew_id,...)` 改成先 `channels::ensure_lobby(s.conn(), "a", "x").unwrap();` 再 `INSERT INTO messages(id,channel_id,sender_kind,text,created_at) VALUES('m1','a/lobby','human','t','x')`。

`crates/store/src/channels.rs`（整个文件）：

```rust
//! 群 id 规则与导入 / 迁移要用的最少行函数。群的规则在 crewd-collab。
use crate::Result;
use rusqlite::{Connection, params};

/// 机组大厅的 id（确定性，spec §22.2）。
pub fn lobby_id(crew_id: &str) -> String {
    format!("{crew_id}/lobby")
}

/// 根机组人类群的 id（确定性；懒建）。
pub fn human_id(root_crew_id: &str) -> String {
    format!("{root_crew_id}/human")
}

/// 确保机组有大厅（已有就不动）。返回大厅 id。
pub fn ensure_lobby(conn: &Connection, crew_id: &str, created_at: &str) -> Result<String> {
    let id = lobby_id(crew_id);
    conn.execute(
        "INSERT OR IGNORE INTO channels(id,crew_id,kind,name,created_by,created_at)
         VALUES(?1,?2,'lobby','','system',?3)",
        params![id, crew_id, created_at],
    )?;
    Ok(id)
}
```

`crates/store/src/messages.rs`（整个文件替换）：

```rust
use crate::{Result, json_col};
use crewd_proto::Message;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

/// v3 的 sender 词表（裁定 6）。旧词 `user` / `pendingcrew` 归一；认不出 → None。
pub fn normalize_sender_kind(k: &str) -> Option<&'static str> {
    match k {
        "human" | "user" => Some("human"),
        "session" => Some("session"),
        "captain" => Some("captain"),
        "system" | "pendingcrew" => Some("system"),
        _ => None,
    }
}

const COLS: &str = "seq,id,channel_id,sender_kind,sender_session_id,sender_name,text,category,in_reply_to,mentions,attachments,created_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    Ok(Message {
        seq: r.get(0)?,
        id: r.get(1)?,
        channel_id: r.get(2)?,
        sender_kind: r.get(3)?,
        sender_session_id: r.get(4)?,
        sender_name: r.get(5)?,
        text: r.get(6)?,
        category: r.get(7)?,
        in_reply_to: r.get(8)?,
        mentions: json_col(r, 9)?,
        attachments: json_col(r, 10)?,
        created_at: r.get(11)?,
    })
}

/// 写入一条消息。id 已存在 → `Ok(None)`（幂等）；否则返回分配到的 seq。
pub fn insert(conn: &Connection, m: &Message, extra: Option<&Value>) -> Result<Option<i64>> {
    let n = conn.execute(
        "INSERT INTO messages(id,channel_id,sender_kind,sender_session_id,sender_name,text,category,in_reply_to,mentions,attachments,created_at,extra)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
         ON CONFLICT(id) DO NOTHING",
        params![
            m.id,
            m.channel_id,
            m.sender_kind,
            m.sender_session_id,
            m.sender_name,
            m.text,
            m.category,
            m.in_reply_to,
            serde_json::to_string(&m.mentions)?,
            serde_json::to_string(&m.attachments)?,
            m.created_at,
            extra.map(|v| v.to_string()),
        ],
    )?;
    Ok((n > 0).then(|| conn.last_insert_rowid()))
}

/// `after_seq < seq`（且 `seq < before_seq`，若给了）的消息，按 seq 升序。
/// 给了 `before_seq` 时取紧挨着它的**最后** `limit` 条（往上翻页），仍按升序返回。
pub fn list(
    conn: &Connection,
    channel_id: &str,
    after_seq: i64,
    before_seq: Option<i64>,
    limit: u32,
) -> Result<Vec<Message>> {
    let mut out = match before_seq {
        None => {
            let mut st = conn.prepare(&format!(
                "SELECT {COLS} FROM messages WHERE channel_id=?1 AND seq>?2 ORDER BY seq LIMIT ?3"
            ))?;
            st.query_map(params![channel_id, after_seq, limit], row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        }
        Some(before) => {
            let mut st = conn.prepare(&format!(
                "SELECT {COLS} FROM messages WHERE channel_id=?1 AND seq>?2 AND seq<?3 ORDER BY seq DESC LIMIT ?4"
            ))?;
            st.query_map(params![channel_id, after_seq, before, limit], row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        }
    };
    if before_seq.is_some() {
        out.reverse();
    }
    Ok(out)
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Message>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLS} FROM messages WHERE id=?1"),
            [id],
            row,
        )
        .optional()?)
}

/// 群里最大的 seq；空群 0。
pub fn max_seq(conn: &Connection, channel_id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(seq),0) FROM messages WHERE channel_id=?1",
        [channel_id],
        |r| r.get(0),
    )?)
}
```

`crates/store/src/schema.rs`：`SCHEMA_VERSION = 3`；加 V3 常量、迁移与回填（放在 `V2` 之后）：

```rust
/// v3（M3a，spec §22.2 + §22.7 拍板）：群、成员、注视、游标、收听、唤醒队列、熔断台账、审批、工作账本、
/// 问的附注、重启询问；sessions / crews 加列。不含 BEGIN / COMMIT：由 `migrate_v3` 包事务。
const V3_DDL: &str = r#"
CREATE TABLE channels(
  id TEXT PRIMARY KEY,
  crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK(kind IN ('lobby','human','topic','dm')),
  name TEXT NOT NULL DEFAULT '',
  purpose TEXT,
  created_by TEXT NOT NULL,
  owner_kind TEXT CHECK(owner_kind IN ('session','human')),
  owner_id TEXT,
  pending_owner_kind TEXT CHECK(pending_owner_kind IN ('session','human')),
  pending_owner_id TEXT,
  pending_owner_by TEXT,
  pending_owner_at TEXT,
  created_at TEXT NOT NULL,
  archived_at TEXT,
  CHECK((kind IN ('lobby','human')) = (owner_kind IS NULL))
);
CREATE UNIQUE INDEX channels_one_lobby ON channels(crew_id) WHERE kind='lobby';
CREATE UNIQUE INDEX channels_one_human ON channels(crew_id) WHERE kind='human';
CREATE TABLE channel_members(
  channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  member_kind TEXT NOT NULL CHECK(member_kind IN ('session','human')),
  member_id TEXT NOT NULL,
  joined_at TEXT NOT NULL,
  left_at TEXT,
  PRIMARY KEY(channel_id, member_kind, member_id)
);
CREATE INDEX channel_members_member ON channel_members(member_kind, member_id);
CREATE TABLE channel_watch(
  channel_id TEXT PRIMARY KEY REFERENCES channels(id) ON DELETE CASCADE,
  since TEXT NOT NULL
);
CREATE TABLE messages_v3(
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL UNIQUE,
  channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  sender_kind TEXT NOT NULL CHECK(sender_kind IN ('human','session','captain','system')),
  sender_session_id TEXT,
  sender_name TEXT,
  text TEXT NOT NULL,
  category TEXT,
  in_reply_to TEXT,
  mentions TEXT NOT NULL DEFAULT '[]',
  attachments TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL,
  extra TEXT
);
CREATE TABLE read_cursors(
  reader TEXT NOT NULL,
  channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(reader, channel_id)
);
CREATE TABLE listens(
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  until INTEGER,
  senders TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL,
  PRIMARY KEY(session_id, channel_id)
);
CREATE TABLE wakes(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK(kind IN ('message','timer','ask_answer','restart','system','checkpoint','stop_check')),
  source TEXT NOT NULL,
  origin TEXT,
  priority TEXT NOT NULL CHECK(priority IN ('human','system','auto')),
  not_before INTEGER NOT NULL,
  note TEXT,
  state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','inflight','delivered','dropped')),
  attempts INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  created_at TEXT NOT NULL,
  settled_at TEXT,
  UNIQUE(session_id, kind, source)
);
CREATE INDEX wakes_due ON wakes(state, not_before);
CREATE TABLE wake_latches(
  scope TEXT NOT NULL CHECK(scope IN ('edge','session')),
  id TEXT NOT NULL,
  reason TEXT NOT NULL,
  at TEXT NOT NULL,
  PRIMARY KEY(scope, id)
);
CREATE TABLE approvals(
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
  tool_name TEXT NOT NULL,
  summary TEXT NOT NULL,
  tool_use_id TEXT,
  requested_at TEXT NOT NULL,
  decision TEXT CHECK(decision IN ('allow','deny')),
  decided_at TEXT,
  decided_by TEXT
);
CREATE TABLE work_books(
  crew_id TEXT PRIMARY KEY REFERENCES crews(id) ON DELETE CASCADE,
  state TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE ask_meta(
  crew_id TEXT NOT NULL REFERENCES crews(id) ON DELETE CASCADE,
  ask TEXT NOT NULL,
  resume_note TEXT,
  message_id TEXT,
  PRIMARY KEY(crew_id, ask)
);
CREATE TABLE restart_prompts(
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  created_at TEXT NOT NULL,
  session_ids TEXT NOT NULL,
  answer TEXT CHECK(answer IN ('resume','later')),
  answered_at TEXT
);
ALTER TABLE sessions ADD COLUMN title TEXT;
ALTER TABLE sessions ADD COLUMN brief TEXT;
ALTER TABLE sessions ADD COLUMN desired TEXT NOT NULL DEFAULT 'stopped' CHECK(desired IN ('running','stopped'));
ALTER TABLE sessions ADD COLUMN was_working INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN last_active_at TEXT;
ALTER TABLE sessions ADD COLUMN last_output_at TEXT;
ALTER TABLE sessions ADD COLUMN awaiting_question TEXT;
ALTER TABLE sessions ADD COLUMN resume_failed_at TEXT;
ALTER TABLE sessions ADD COLUMN worktree TEXT;
ALTER TABLE sessions ADD COLUMN ext INTEGER;
ALTER TABLE crews ADD COLUMN title_source TEXT;
ALTER TABLE crews ADD COLUMN status_line TEXT;
ALTER TABLE crews ADD COLUMN policy TEXT;
ALTER TABLE crews ADD COLUMN captain_session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL;
ALTER TABLE crews ADD COLUMN next_ext INTEGER NOT NULL DEFAULT 2;
"#;

/// 旧消息搬进各机组大厅（大厅先建，外键才成立）。sender 词表在这里归一（裁定 6）。
const V3_MOVE_MESSAGES: &str = r#"
INSERT OR IGNORE INTO channels(id,crew_id,kind,name,created_by,created_at)
  SELECT id||'/lobby', id, 'lobby', '', 'system', created_at FROM crews;
INSERT INTO messages_v3(seq,id,channel_id,sender_kind,sender_session_id,sender_name,text,category,in_reply_to,mentions,attachments,created_at,extra)
  SELECT seq, id, crew_id||'/lobby',
         CASE sender_kind WHEN 'user' THEN 'human' WHEN 'pendingcrew' THEN 'system' ELSE sender_kind END,
         sender_session_id, sender_name, text, category, in_reply_to, mentions, attachments, created_at, extra
  FROM messages ORDER BY seq;
DROP TABLE messages;
ALTER TABLE messages_v3 RENAME TO messages;
CREATE INDEX messages_channel_seq ON messages(channel_id, seq);
"#;

/// 迁移前的词表检查：有认不出的 sender / mention 词就整体中止（裁定 6），报出全部值。
fn check_v3_vocabulary(conn: &Connection) -> Result<()> {
    let senders: Vec<String> = conn
        .prepare(
            "SELECT DISTINCT sender_kind FROM messages
              WHERE sender_kind NOT IN ('user','human','session','captain','pendingcrew','system')
              ORDER BY 1",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mentions: Vec<String> = conn
        .prepare(
            "SELECT DISTINCT COALESCE(json_extract(j.value,'$.kind'),'<缺 kind>')
               FROM messages m, json_each(m.mentions) j
              WHERE COALESCE(json_extract(j.value,'$.kind'),'') NOT IN ('session','captain','human','broadcast')
              ORDER BY 1",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if senders.is_empty() && mentions.is_empty() {
        return Ok(());
    }
    Err(StoreError::Invalid(format!(
        "v3 迁移中止（库保持 v2）：消息里有认不出的 senderKind {senders:?} / mention kind {mentions:?}。\
         先确认这些值的含义、在 schema.rs 的归一表里补上再升级。"
    )))
}

fn migrate_v3(conn: &Connection) -> Result<()> {
    check_v3_vocabulary(conn)?;
    // 重建表期间关外键（SQLite 官方的改表步骤）；事务里不能改这个 pragma，所以在事务外开关。
    conn.execute_batch("PRAGMA foreign_keys=OFF")?;
    let result = (|| -> Result<()> {
        let tx = conn.unchecked_transaction()?;
        let high_water: i64 = tx.query_row(
            "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name='messages'),0)",
            [],
            |r| r.get(0),
        )?;
        tx.execute_batch(V3_DDL)?;
        tx.execute_batch(V3_MOVE_MESSAGES)?;
        // 重建后 sqlite_sequence 只记得现存最大 seq；恢复旧高水位（裁定 7）。
        let n = tx.execute(
            "UPDATE sqlite_sequence SET seq=MAX(seq, ?1) WHERE name='messages'",
            [high_water],
        )?;
        if n == 0 && high_water > 0 {
            tx.execute(
                "INSERT INTO sqlite_sequence(name, seq) VALUES('messages', ?1)",
                [high_water],
            )?;
        }
        backfill_v3(&tx)?;
        let broken: i64 =
            tx.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r.get(0))?;
        if broken > 0 {
            return Err(StoreError::Invalid(format!(
                "v3 迁移后有 {broken} 条外键不成立，已回滚（库保持 v2）"
            )));
        }
        tx.execute_batch("PRAGMA user_version = 3")?;
        tx.commit()?;
        Ok(())
    })();
    conn.execute_batch("PRAGMA foreign_keys=ON")?;
    result
}

/// v3 的数据回填（幂等；迁移与旧 JSON 导入共用）：每个机组有大厅；`title_source` / worker 分机号从 extra 取；
/// `next_ext`、机长指针、`meta.nextCrewNumber`；所有 session 在自己大厅、人类在每个群的游标钉到末尾
/// （已有游标不动）。不产生任何未读、红标、唤醒（spec §22.2）。
pub fn backfill_v3(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
INSERT OR IGNORE INTO channels(id,crew_id,kind,name,created_by,created_at)
  SELECT id||'/lobby', id, 'lobby', '', 'system', created_at FROM crews;
UPDATE crews SET title_source = json_extract(extra,'$.titleSource')
 WHERE title_source IS NULL AND extra IS NOT NULL AND json_valid(extra)
   AND json_type(extra,'$.titleSource')='text';
UPDATE sessions SET ext = json_extract(extra,'$.member.extensionNumber')
 WHERE ext IS NULL AND role='worker' AND extra IS NOT NULL AND json_valid(extra)
   AND json_type(extra,'$.member.extensionNumber')='integer';
UPDATE crews SET next_ext = MAX(next_ext,
  COALESCE((SELECT MAX(ext)+1 FROM sessions s WHERE s.crew_id=crews.id), 2));
UPDATE crews SET captain_session_id =
  (SELECT id FROM sessions s WHERE s.crew_id=crews.id AND s.role='captain'
    ORDER BY s.updated_at DESC, s.id DESC LIMIT 1)
 WHERE captain_session_id IS NULL;
INSERT OR IGNORE INTO read_cursors(reader,channel_id,seq,updated_at)
  SELECT s.id, s.crew_id||'/lobby',
         COALESCE((SELECT MAX(seq) FROM messages m WHERE m.channel_id=s.crew_id||'/lobby'),0),
         strftime('%Y-%m-%dT%H:%M:%fZ','now')
    FROM sessions s;
INSERT OR IGNORE INTO read_cursors(reader,channel_id,seq,updated_at)
  SELECT 'human', ch.id,
         COALESCE((SELECT MAX(seq) FROM messages m WHERE m.channel_id=ch.id),0),
         strftime('%Y-%m-%dT%H:%M:%fZ','now')
    FROM channels ch;
"#,
    )?;
    let max_no: i64 =
        conn.query_row("SELECT COALESCE(MAX(number),0) FROM crews", [], |r| r.get(0))?;
    let archived = crate::archive::get(conn, "local-crews.json")?
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("nextCrewNumber").and_then(|n| n.as_i64()))
        .unwrap_or(0);
    let current = crate::meta_get(conn, "nextCrewNumber")?
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let next = current.max(archived).max(max_no + 1);
    crate::meta_set(conn, "nextCrewNumber", &next.to_string())?;
    Ok(())
}
```

`migrate` 末尾加 `if v < 3 { migrate_v3(conn)?; }`。注意 `meta` 表在 V1 建，`archive::get` 用的是同一连接；`json_valid` / `json_type` / `json_each` 都是 bundled SQLite 内建的 JSON1。

`crates/store/tests/content.rs`：`Message` 字面量里 `crew_id` → `channel_id: "<crew>/lobby"`（插消息前先 `crewd_store::channels::ensure_lobby`），`messages::list(conn, crew, after, limit)` → `messages::list(conn, &lobby_id(crew), after, None, limit)`；sender 用归一后的词。只做这些机械替换，不改断言的含义。

- [ ] **Step 5: 跑 store 测试**

Run: `cargo test -p crewd-store`
Expected: 新增 8 条 + 已有全部通过。若 `migration_keeps_the_seq_high_water_mark` 红了：先看 `SELECT * FROM sqlite_sequence` 迁移前后，别改期望。

- [ ] **Step 6: 导入器写 v3**

`crates/legacy/src/crews.rs`：`crewd_store::crews::upsert(tx, &crew, ..)?;` 之后加 `crewd_store::channels::ensure_lobby(tx, &crew.id, &crew.created_at)?;`。

`crates/legacy/src/whiteboard.rs` 的 `insert_one`：在构造 `Message` 前：

```rust
    let Some(sender_kind) = crewd_store::messages::normalize_sender_kind(&m.sender_kind) else {
        cx.report.bump("messagesWithUnknownSender");
        cx.report.warn(format!(
            "机组 {crew_id} 的消息 {} senderKind={:?} 认不出，未导入（原件留在归档里）",
            m.id, m.sender_kind
        ));
        return Ok(false);
    };
    if let Some(bad) = m
        .mentions
        .iter()
        .find(|x| !matches!(x.kind.as_str(), "session" | "captain" | "human" | "broadcast"))
    {
        cx.report.bump("messagesWithUnknownMention");
        cx.report.warn(format!(
            "机组 {crew_id} 的消息 {} 的 mention kind={:?} 认不出，未导入（原件留在归档里）",
            m.id, bad.kind
        ));
        return Ok(false);
    }
```

（`report.warn` 若签名不是 `impl Into<String>`，照它现有签名传。）`Message { .. }` 里 `crew_id: crew_id.to_string()` → `channel_id: crewd_store::channels::lobby_id(crew_id)`，`sender_kind: m.sender_kind` → `sender_kind: sender_kind.to_string()`。

`crates/legacy/src/lib.rs` 的 `import`：`archive::import_rest(tx, &mut cx)?;` 之后、`meta_set(importedAt)` 之前加 `crewd_store::schema::backfill_v3(tx)?;`（大厅、游标钉末尾、机长指针、分机号、`nextCrewNumber`——此时 `local-crews.json` 已进归档）。`lib.rs` 测试与 `tests/import_basic.rs` 里 `messages::list(store.conn(), "local-aaaa", 0, 100)` → `messages::list(store.conn(), &crewd_store::channels::lobby_id("local-aaaa"), 0, None, 100)`；断言 `sender_kind == "user"` / `"pendingcrew"` 的改成 `"human"` / `"system"`。在 `tests/import_basic.rs` 加一条：

```rust
#[test]
fn imported_database_has_lobbies_pinned_cursors_and_counters() {
    let (store, _report, _dir) = import_fixture();   // 用该文件已有的导入助手；名字以文件里的为准
    let c = store.conn();
    let lobbies: i64 = c
        .query_row("SELECT COUNT(*) FROM channels WHERE kind='lobby'", [], |r| r.get(0))
        .unwrap();
    let crews: i64 = c.query_row("SELECT COUNT(*) FROM crews", [], |r| r.get(0)).unwrap();
    assert_eq!(lobbies, crews);
    let behind: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM sessions s JOIN read_cursors r
               ON r.reader=s.id AND r.channel_id=s.crew_id||'/lobby'
              WHERE r.seq < (SELECT COALESCE(MAX(seq),0) FROM messages m WHERE m.channel_id=s.crew_id||'/lobby')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(behind, 0, "导入后没有任何 session 有未读");
    assert_eq!(
        crewd_store::meta_get(c, "nextCrewNumber").unwrap().as_deref(),
        Some("3"),
        "夹具 local-crews.json 的 nextCrewNumber"
    );
    let ext: i64 = c
        .query_row("SELECT ext FROM sessions WHERE id='worker-2222bbbb'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ext, 2);
    let cap: String = c
        .query_row("SELECT captain_session_id FROM crews WHERE id='local-aaaa'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(cap, "captain-1111aaaa");
}
```

（`import_fixture` 是占位名：执行时用该测试文件里已有的「导入 basic 夹具」那段代码，原样复制成这个测试的开头。）

- [ ] **Step 7: `whiteboard.*` 临时走大厅**

`crates/server/src/dispatch.rs`：`WHITEBOARD_LIST` 调 `crewd_store::messages::list(s.conn(), &crewd_store::channels::lobby_id(&p.crew_id), after, None, limit)`；`WHITEBOARD_POST` 先 `let kind = crewd_store::messages::normalize_sender_kind(&p.sender_kind).ok_or_else(|| invalid(format!("未知 senderKind: {}", p.sender_kind)))?;`（删掉 `SENDER_KINDS` 常量），`Message { channel_id: lobby_id(&p.crew_id), sender_kind: kind.into(), .. }`。`crates/server/tests/socket.rs` 的 `start()` 在 `crews::upsert` 之后加 `crewd_store::channels::ensure_lobby(s.conn(), "a", "2026-10-01T00:00:00.000Z").unwrap();`（否则消息外键不成立）。`crates/server/tests/socket.rs` 与 `crates/crewd/tests/e2e.rs`：断言消息 `crewId` 的改成 `channelId == "<crew>/lobby"`，断言 `senderKind == "user"` 的改成 `"human"`；其余不动。

- [ ] **Step 8: 重新生成契约、全量检查**

```bash
cargo run -q -p crewd -- schema > schema/crewd-api.schema.json
./scripts/check.sh
```

Expected: 全绿。

- [ ] **Step 9: 提交**

```bash
git add crates/store/src/schema.rs crates/store/src/lib.rs crates/store/src/channels.rs crates/store/src/messages.rs crates/store/tests/content.rs crates/proto/src/model.rs crates/proto/src/lib.rs crates/legacy/src/crews.rs crates/legacy/src/whiteboard.rs crates/legacy/src/lib.rs crates/legacy/tests/import_basic.rs crates/server/src/dispatch.rs crates/server/tests/socket.rs crates/crewd/tests/e2e.rs schema/crewd-api.schema.json
git diff --cached --stat
git commit -m "crewd-store: schema v3 (channels, cursors, wakes, approvals, work books, restart prompts) and v2->v3 migration

Messages move to per-crew lobbies with seq and high-water mark preserved; sender
vocabulary normalized (unknown values abort and keep v2); cursors pinned so the
migration creates no unread, badges or wakes. Importer writes v3 via backfill_v3.

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: proto 类型化词表与 v3 线上类型

§14 遗留：枚举不再是自由字符串。本任务把 `sender_kind` / `Mention.kind` / `role` 换成枚举，`Session` / `Crew` 加 v3 字段，并定义 collab 与 RPC 要用的全部领域类型（params 结构在 Task 13）。store 的行函数读写新列。

**Files:**
- Modify: `crewd/crates/proto/src/model.rs`、`crewd/crates/proto/src/lib.rs`（测试）、`crewd/crates/store/src/crews.rs`、`crewd/crates/store/src/sessions.rs`、`crewd/crates/store/src/messages.rs`、`crewd/crates/store/src/lib.rs`（测试）、`crewd/crates/store/src/schema.rs`（测试）、`crewd/crates/store/tests/content.rs`、`crewd/crates/legacy/src/{crews,sessions,whiteboard,lib}.rs`、`crewd/crates/legacy/tests/import_basic.rs`、`crewd/crates/server/src/{dispatch,ledger}.rs`、`crewd/crates/server/tests/socket.rs`、`crewd/crates/crewd/tests/claude_cli.rs`、`crewd/schema/crewd-api.schema.json`

**Interfaces:**
- Consumes: Task 3。
- Produces（`crewd_proto`，全部 `Serialize + Deserialize + JsonSchema + Clone + Debug + PartialEq`，线上 camelCase）：见下面整段代码；store：`crews::{upsert（写 v3 列）, list, get(conn, id) -> Result<Option<Crew>>}`、`sessions::{upsert（写 v3 列）, list_for_crew, list_all(conn), get(conn, id) -> Result<Option<Session>>}`。

- [ ] **Step 1: 写失败的测试**

`crates/proto/src/lib.rs` 测试模块加：

```rust
    #[test]
    fn vocabularies_are_closed_enums_on_the_wire() {
        assert_eq!(serde_json::to_value(SenderKind::System).unwrap(), json!("system"));
        assert!(serde_json::from_value::<SenderKind>(json!("user")).is_err(), "旧词只在迁移 / 导入里归一");
        assert_eq!(serde_json::to_value(MentionKind::Broadcast).unwrap(), json!("broadcast"));
        assert_eq!(serde_json::to_value(ChannelKind::Dm).unwrap(), json!("dm"));
        assert_eq!(
            serde_json::to_value(MemberRef::Session { id: "w-1".into() }).unwrap(),
            json!({"kind":"session","id":"w-1"})
        );
        assert_eq!(serde_json::to_value(MemberRef::Human).unwrap(), json!({"kind":"human"}));
        assert_eq!(serde_json::to_value(WaitTierView::StillWanted).unwrap(), json!("still_wanted"));
        assert_eq!(serde_json::to_value(RestartAnswer::Later).unwrap(), json!("later"));
        assert_eq!(SenderKind::parse("captain"), Some(SenderKind::Captain));
        assert_eq!(Role::parse("captain"), Some(Role::Captain));
        assert_eq!(Role::parse("chief"), None);
    }

    #[test]
    fn session_and_crew_carry_v3_fields_with_defaults() {
        let s: Session = serde_json::from_value(json!({
            "id":"w-1","crewId":"c","role":"worker","executor":"claude","updatedAt":"t"
        }))
        .unwrap();
        assert_eq!(s.desired, Desired::Stopped);
        assert!(!s.was_working);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["desired"], "stopped");
        assert!(v.get("title").is_none(), "None 不上线");
        let c = Crew {
            id: "c".into(),
            captain_session_id: Some("cap".into()),
            ..Default::default()
        };
        assert_eq!(serde_json::to_value(&c).unwrap()["captainSessionId"], "cap");
    }
```

Run: `cargo test -p crewd-proto`
Expected: 编译失败（类型不存在）。

- [ ] **Step 2: 实现 proto 类型**

`crates/proto/src/model.rs`：`Crew`、`Session` 加 `Default` derive；`Session.role: String` → `pub role: Role`；`Message.sender_kind: String` → `pub sender_kind: SenderKind`；`Mention.kind: String` → `pub kind: MentionKind`（`Mention` 也加 `Default`，`MentionKind` 默认 `Broadcast`）。`Crew` 末尾加：

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// "human" | "captain" | "auto"（谁起的名；旧数据从 extra 回填）。
    pub title_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 机长写的一句状态（`set_crew_status`）。
    pub status_line: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 委派策略（M3b 的 read / update_delegation_policy），自由 JSON。
    pub policy: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 此刻的机长（裁定 8）。
    pub captain_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// 本机组自己的群里最新一条消息的时间（只有 `crew.list` 填；侧栏「最近有动静在上」，§22.7 Q4）。
    pub last_activity_at: Option<String>,
```

`Session` 末尾加：

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<String>,
    #[serde(default)]
    pub desired: Desired,
    /// 分机号（机长恒为 1，不存；没号的旧成员为 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ext: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    #[serde(default)]
    pub was_working: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_active_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_output_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting_question: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_failed_at: Option<String>,
```

文件末尾加（整段）：

```rust
/// 封闭词表：线上是固定字符串，`parse` 认不出就是 None（不猜）。
macro_rules! closed_vocab {
    ($name:ident { $($variant:ident = $s:literal),+ $(,)? } default $def:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
        pub enum $name {
            $(#[serde(rename = $s)] $variant,)+
        }
        impl Default for $name {
            fn default() -> $name {
                $name::$def
            }
        }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $s,)+ }
            }
            pub fn parse(s: &str) -> Option<$name> {
                match s { $($s => Some($name::$variant),)+ _ => None }
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

closed_vocab!(SenderKind { Human = "human", Session = "session", Captain = "captain", System = "system" } default Human);
closed_vocab!(MentionKind { Session = "session", Captain = "captain", Human = "human", Broadcast = "broadcast" } default Broadcast);
closed_vocab!(Role { Captain = "captain", Worker = "worker" } default Worker);
closed_vocab!(Desired { Running = "running", Stopped = "stopped" } default Stopped);
closed_vocab!(ChannelKind { Lobby = "lobby", Human = "human", Topic = "topic", Dm = "dm" } default Topic);
closed_vocab!(AskStateView { Open = "open", Answered = "answered", Defaulted = "defaulted", Withdrawn = "withdrawn" } default Open);
closed_vocab!(WaitTierView { Fresh = "fresh", Pinned = "pinned", StillWanted = "still_wanted" } default Fresh);
closed_vocab!(Decision { Allow = "allow", Deny = "deny" } default Deny);
closed_vocab!(DirectoryKind { Crew = "crew", Session = "session" } default Crew);
closed_vocab!(HealthKindView {
    AuthRequired = "auth_required", UsageLimit = "usage_limit", RateLimited = "rate_limited",
    LaunchFailed = "launch_failed", BriefUndelivered = "brief_undelivered", PromptUndelivered = "prompt_undelivered",
} default AuthRequired);
closed_vocab!(RestartAnswer { Resume = "resume", Later = "later" } default Later);
```

同一张表（给读者对照）：

| 枚举 | 变体 = 线上值 | 默认 |
|---|---|---|
| `SenderKind` | Human="human", Session="session", Captain="captain", System="system" | Human |
| `MentionKind` | Session="session", Captain="captain", Human="human", Broadcast="broadcast" | Broadcast |
| `Role` | Captain="captain", Worker="worker" | Worker |
| `Desired` | Running="running", Stopped="stopped" | Stopped |
| `ChannelKind` | Lobby="lobby", Human="human", Topic="topic", Dm="dm" | Topic |
| `AskStateView` | Open="open", Answered="answered", Defaulted="defaulted", Withdrawn="withdrawn" | Open |
| `WaitTierView` | Fresh="fresh", Pinned="pinned", StillWanted="still_wanted" | Fresh |
| `Decision` | Allow="allow", Deny="deny" | Deny |
| `DirectoryKind` | Crew="crew", Session="session" | Crew |
| `HealthKindView` | AuthRequired="auth_required", UsageLimit="usage_limit", RateLimited="rate_limited", LaunchFailed="launch_failed", BriefUndelivered="brief_undelivered", PromptUndelivered="prompt_undelivered" | AuthRequired |
| `RestartAnswer` | Resume="resume", Later="later" | Later |

其余结构（同文件末尾）：

```rust
/// 群成员 / 群主的引用。人类没有 id（全机只有一个「人」）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MemberRef {
    Session { id: String },
    Human,
}

impl MemberRef {
    pub fn session(id: impl Into<String>) -> MemberRef {
        MemberRef::Session { id: id.into() }
    }
    /// 库里的 (member_kind, member_id)；人类的 id 固定是 "human"。
    pub fn to_row(&self) -> (&'static str, &str) {
        match self {
            MemberRef::Session { id } => ("session", id.as_str()),
            MemberRef::Human => ("human", "human"),
        }
    }
    pub fn from_row(kind: &str, id: &str) -> Option<MemberRef> {
        match kind {
            "session" => Some(MemberRef::session(id)),
            "human" => Some(MemberRef::Human),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    pub crew_id: String,
    pub kind: ChannelKind,
    /// 大厅 / 人类群是空串（客户端按 kind 显示）。
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// "human" | "system" | session id。
    pub created_by: String,
    /// 群主（§22.7 Q2）。大厅 / 人类群 = 归属机组此刻的机长（现算；没有机长时 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<MemberRef>,
    /// 待接受的群主转让对象。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_owner: Option<MemberRef>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelSummary {
    pub channel: Channel,
    pub members: Vec<MemberRef>,
    /// 人类标了「我在注视」。
    pub watched: bool,
    pub unread_for_human: i64,
    /// 未读里 @人类 的条数（红色「叫你」标记，§17）。
    pub human_mentions: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Badge {
    pub channel_id: String,
    pub unread_for_human: i64,
    pub human_mentions: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CrewDetail {
    pub crew: Crew,
    /// 归属的根机组（§13 Q1：多父归最早的根）。
    pub root_id: String,
    pub children: Vec<String>,
    pub channels: Vec<ChannelSummary>,
    pub sessions: Vec<Session>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AskDefault {
    pub text: String,
    pub after_minutes: i64,
}

/// 一条人类待确认（H 号按机组发，§19 Q1）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    pub crew_id: String,
    /// "H3"
    pub ask: String,
    pub question: String,
    /// 提问的 session；None = 系统 / 人。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_by: Option<String>,
    pub asked_at: String,
    pub state: AskStateView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// 回答 / 按默认办 / 撤回的时间。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdraw_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<AskDefault>,
    /// 关于哪个工作项（"W2"）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_note: Option<String>,
    /// 人类群里那条公告。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HumanWait {
    pub ask: Ask,
    pub items: Vec<String>,
    pub owners: Vec<String>,
    pub waited_ms: i64,
    pub tier: WaitTierView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_due_at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub id: String,
    pub session_id: String,
    pub crew_id: String,
    pub tool_name: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    pub requested_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<String>,
    /// "human" | "timeout" | "unreachable"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryRow {
    /// "7" 或 "7-3"
    pub number: String,
    pub kind: DirectoryKind,
    pub name: String,
    pub crew_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// 根在最左，" / " 分隔；根机组本身是空串。
    pub org_path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HealthView {
    pub kind: HealthKindView,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DecisionView {
    pub summary: String,
    pub options: Vec<String>,
    /// 选项带编号（按编号答）；false = 光标式菜单（绝不编造编号，§15）。
    pub numbered: bool,
}

/// 会话的实时状态（`session.live` / `session.changed`，由 M3c 填）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionLive {
    pub session_id: String,
    pub running: bool,
    pub working: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<HealthView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_decision: Option<DecisionView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting_question: Option<String>,
}

/// 重启后「是否立即恢复这 N 个 session」（§22.7 Q1）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RestartPrompt {
    pub id: i64,
    pub created_at: String,
    pub session_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<RestartAnswer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<String>,
}

/// 空结果（`crew.delete` 等）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Empty {}
```

- [ ] **Step 3: store 读写新列**

`crates/store/src/messages.rs` 的 `row()`：`sender_kind` 读成字符串后 `SenderKind::parse`，认不出 → `rusqlite::Error::FromSqlConversionFailure(3, Type::Text, format!("未知 senderKind {s}").into())`；`insert` 里 `m.sender_kind` → `m.sender_kind.as_str()`。

`crates/store/src/crews.rs`：`upsert` 的列表加 `title_source, status_line, policy, captain_session_id`（`policy` 存 `serde_json::to_string`，`None` → NULL）；`list` 的 SELECT 加这 4 列（`policy` 用 `Option<String>` 读出再 `serde_json::from_str`，坏 JSON → 转换错误）；加：

```rust
pub fn get(conn: &Connection, id: &str) -> Result<Option<Crew>> {
    Ok(list(conn)?.into_iter().find(|c| c.id == id))
}
```

（几十个机组量级，复用 `list` 保证读法只有一份；需要时再优化。）

`crates/store/src/sessions.rs`：`upsert` 写全部列（`role.as_str()`、`desired.as_str()`、`was_working as i64`、其余 Option 原样）；抽一个 `const COLS` + `fn row(r) -> rusqlite::Result<Session>`（`Role::parse` / `Desired::parse`，认不出 → 转换错误），`list_for_crew` 用它；加：

```rust
pub fn get(conn: &Connection, id: &str) -> Result<Option<Session>> {
    Ok(conn
        .query_row(&format!("SELECT {COLS} FROM sessions WHERE id=?1"), [id], row)
        .optional()?)
}

pub fn list_all(conn: &Connection) -> Result<Vec<Session>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM sessions ORDER BY crew_id, id"))?;
    Ok(st.query_map([], row)?.collect::<rusqlite::Result<Vec<_>>>()?)
}
```

- [ ] **Step 4: 修所有字面量与调用点**

编译器会列出：`crates/legacy/src/crews.rs:47`（`Crew { .. }` 末尾补 `..Default::default()`）、`crates/legacy/src/sessions.rs:88,113`（`role: role_of(..)` 返回 `Role`：`if id.starts_with("captain-") { Role::Captain } else { Role::Worker }`；补 `..Default::default()`）、`crates/legacy/src/whiteboard.rs`（`sender_kind: SenderKind::parse(sender_kind).expect("normalize_sender_kind 只返回词表内的值")`；`Mention { kind: MentionKind::parse(&x.kind).expect("上面已检查"), .. }`）、`crates/store/src/lib.rs` 测试里的 `Crew` / `Session` 字面量、`crates/store/tests/content.rs`、`crates/server/src/ledger.rs` 测试、`crates/server/tests/socket.rs`、`crates/crewd/tests/claude_cli.rs`、`crates/server/src/dispatch.rs`（`whiteboard.post` 里 `sender_kind: SenderKind::parse(kind).unwrap()`、`mentions` 参数类型随 `Mention` 变）、`crates/proto/src/lib.rs` 测试（`role: "worker".into()` → `Role::Worker` 等）、`crates/store/src/schema.rs` 测试（`sender_kind: "human".into()` → `SenderKind::Human`）、`crates/legacy/src/lib.rs` 与 `crates/legacy/tests/import_basic.rs`（断言 `sender_kind == "human"` → `SenderKind::Human`）。一律只改类型与补 `..Default::default()`，不改测试含义。

- [ ] **Step 5: 全量检查、重新生成契约、提交**

```bash
cargo run -q -p crewd -- schema > schema/crewd-api.schema.json
./scripts/check.sh
git add crates/proto/src/model.rs crates/proto/src/lib.rs crates/store/src/crews.rs crates/store/src/sessions.rs crates/store/src/messages.rs crates/store/src/lib.rs crates/store/src/schema.rs crates/store/tests/content.rs crates/legacy/src/crews.rs crates/legacy/src/sessions.rs crates/legacy/src/whiteboard.rs crates/legacy/src/lib.rs crates/legacy/tests/import_basic.rs crates/server/src/dispatch.rs crates/server/src/ledger.rs crates/server/tests/socket.rs crates/crewd/tests/claude_cli.rs schema/crewd-api.schema.json
git diff --cached --stat
git commit -m "crewd-proto: closed vocabularies and v3 wire types; store reads and writes v3 columns

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 5: `crewd-collab` 骨架：`Tx` / `run` / `Change` / 错误、设置、唤醒队列与熔断台账

本任务定下 collab 的形状（裁定 2–4）与 M3c 最依赖的队列接口（裁定 16、§22.7 Q5）。唤醒**判定**（叫谁、准入、熔断阈值）归 M3c；这里只有持久化的读写。

**Files:**
- Create: `crewd/crates/collab/Cargo.toml`、`crewd/crates/collab/src/lib.rs`、`crewd/crates/collab/src/testkit.rs`、`crewd/crates/collab/src/settings.rs`、`crewd/crates/collab/src/wakes.rs`
- Modify: `crewd/Cargo.lock`

**Interfaces:**
- Consumes: Task 1–4（`Store`、`backfill_v3`、`ts_from_ms`、proto 类型）。
- Produces（`crewd_collab`）：`Tx`、`run`、`Change`、`CollabError`、`Result`、`Locale`（签名见「冻结接口」）；`settings::{locale, set_locale, max_live_sessions, set_max_live_sessions}`；`wakes::{WakeKind, WakePriority, WakeState, WakeRequest, Enqueued, WakeRow, source, enqueue, due_for, sessions_with_due, next_due_at, mark_inflight, mark_delivered, mark_failed, mark_dropped, reset_inflight, recent_for, list}`、`wakes::latches::{Scope, Latch, set, clear, get, list}`；testkit（仅测试）：`NOW`、`MIN`、`store()`、`ok`、`ok_at`、`err`、`count`。

- [ ] **Step 1: 建 crate**

```bash
cargo new --lib crates/collab --name crewd-collab --vcs none
cargo add --offline -p crewd-collab --path crates/store
cargo add --offline -p crewd-collab --path crates/proto
cargo add --offline -p crewd-collab --path crates/work
cargo add --offline -p crewd-collab rusqlite --features bundled
cargo add --offline -p crewd-collab serde --features derive
cargo add --offline -p crewd-collab serde_json chrono thiserror
cargo add --offline -p crewd-collab uuid --features v4
```

- [ ] **Step 2: 写失败的测试**

`crates/collab/src/lib.rs`（整个文件；实现部分在 Step 4 补进同一文件，这里先给出类型与测试）：

```rust
//! crewd 的协作层（spec §22.2）：群、成员、发消息、游标、机组 DAG、人类待确认、审批、唤醒队列。
//! 同步、不碰 tokio、不读时钟：每个写操作是 `fn(&mut Tx, ..) -> Result<T>`，时间是 `Tx` 里调用方给的
//! `now_ms`；操作把「改了什么」记成 `Change`，`run` 提交成功才交回（服务层据此发事件）。
//! M3b / M3c 只经这里写协作数据（spec §22.5）。
pub mod settings;
pub mod wakes;
#[cfg(test)]
mod testkit;

use crewd_proto::RestartAnswer;
use crewd_store::{Store, StoreError};
use rusqlite::Connection;

/// 一次成功提交改了什么。服务层把它变成客户端事件，并原样发到内部通道（`crewd_server::hub::CollabEvent`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    MessageAppended {
        channel_id: String,
        seq: i64,
        message_id: String,
        /// 发消息的 session（人 / 系统发的是 None）。M3c 的熔断判「有没有新产出」会用。
        sender_session_id: Option<String>,
    },
    ChannelChanged { channel_id: String },
    CrewChanged { crew_id: String },
    CrewRemoved { crew_id: String },
    BadgeChanged { channel_id: String },
    SessionChanged { session_id: String },
    AskChanged { crew_id: String, ask: String },
    /// 人回答了一条问（裁定 17）：`cleared` = 卡点被清的 (工作项, 负责人)，M4b 据此叫醒负责人。
    AskAnswered {
        crew_id: String,
        ask: String,
        asked_by: Option<String>,
        cleared: Vec<(String, Option<String>)>,
    },
    ApprovalChanged { id: String },
    WakeEnqueued { wake_id: i64, session_id: String },
    RestartChanged { id: i64 },
    /// 人答了重启询问（§22.7 Q1）：M3c 订阅它，`Resume` 时错开逐个 `--resume`。
    RestartAnswered {
        id: i64,
        answer: RestartAnswer,
        session_ids: Vec<String>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum CollabError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Work(crewd_work::Rejected),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for CollabError {
    fn from(e: rusqlite::Error) -> Self {
        CollabError::Store(StoreError::Sql(e))
    }
}

impl From<serde_json::Error> for CollabError {
    fn from(e: serde_json::Error) -> Self {
        CollabError::Store(StoreError::Json(e))
    }
}

pub type Result<T> = std::result::Result<T, CollabError>;

/// 渲染语言（`settings::locale`）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Locale {
    #[default]
    Zh,
    En,
}

impl Locale {
    pub fn parse(s: &str) -> Option<Locale> {
        match s {
            "zh" => Some(Locale::Zh),
            "en" => Some(Locale::En),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Locale::Zh => "zh",
            Locale::En => "en",
        }
    }
}

/// 一个 SQLite 事务 + 调用方给的时间 + 这次产生的变更。
pub struct Tx<'a> {
    conn: &'a Connection,
    now_ms: i64,
    changes: Vec<Change>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, count, store};

    #[test]
    fn run_commits_and_hands_back_changes_once() {
        let mut s = store();
        let (v, changes) = run(&mut s, NOW, |tx| {
            tx.conn().execute("INSERT INTO meta(key,value) VALUES('k','v')", [])?;
            tx.note(Change::CrewChanged { crew_id: "crew-a".into() });
            tx.note(Change::CrewChanged { crew_id: "crew-a".into() });
            Ok(tx.now_ms())
        })
        .unwrap();
        assert_eq!(v, NOW);
        assert_eq!(changes, [Change::CrewChanged { crew_id: "crew-a".into() }], "同一变更只记一次");
        assert_eq!(count(&s, "SELECT COUNT(*) FROM meta WHERE key='k'"), 1);
    }

    #[test]
    fn an_error_rolls_back_and_drops_changes() {
        let mut s = store();
        let r: Result<()> = run(&mut s, NOW, |tx| {
            tx.conn().execute("INSERT INTO meta(key,value) VALUES('k','v')", [])?;
            tx.note(Change::CrewChanged { crew_id: "crew-a".into() });
            Err(CollabError::Invalid("中途失败".into()))
        })
        .map(|(v, _)| v);
        assert!(matches!(r, Err(CollabError::Invalid(_))));
        assert_eq!(count(&s, "SELECT COUNT(*) FROM meta WHERE key='k'"), 0);
    }

    #[test]
    fn now_ts_is_the_callers_time() {
        let mut s = store();
        let (ts, _) = run(&mut s, 0, |tx| Ok(tx.now_ts())).unwrap();
        assert_eq!(ts, "1970-01-01T00:00:00.000Z");
    }
}
```

`crates/collab/src/testkit.rs`（整个文件）：

```rust
//! 单测夹具（cfg(test)）。全是编的名字，不碰真实数据（仓库公开）。
use crate::{Change, CollabError, Result, Tx};
use crewd_proto::{Crew, Role, Session};
use crewd_store::Store;

/// 2026-09-21T13:33:20Z 附近。
pub(crate) const NOW: i64 = 1_790_000_000_000;
pub(crate) const MIN: i64 = 60_000;

/// 合成的机组树：
/// ```text
/// crew-a（1 号，根）  机长 cap-a；w-a1（分机 2）、w-a2（分机 3）
///   └ crew-b（2 号）  机长 cap-b；w-b1（分机 2）
/// crew-x（3 号，另一棵树的根）机长 cap-x
/// chief（内建总机组，无号、无 session）
/// ```
/// 大厅、机长指针、next_ext、游标都由 `backfill_v3` 建好（与迁移同一条路）。
pub(crate) fn store() -> Store {
    let s = Store::open_in_memory().unwrap();
    let c = s.conn();
    for (id, title, number, builtin, at) in [
        ("crew-a", "机组甲", Some(1), false, "2026-09-01T00:00:00.000Z"),
        ("crew-b", "机组乙", Some(2), false, "2026-09-02T00:00:00.000Z"),
        ("crew-x", "机组丙", Some(3), false, "2026-09-03T00:00:00.000Z"),
        ("chief", "总机组", None, true, "2026-08-01T00:00:00.000Z"),
    ] {
        crewd_store::crews::upsert(
            c,
            &Crew {
                id: id.into(),
                title: title.into(),
                number,
                builtin,
                working_directory: Some("/tmp/example".into()),
                created_at: at.into(),
                updated_at: at.into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    }
    crewd_store::crews::set_parents(c, "crew-b", &["crew-a".to_string()]).unwrap();
    for (id, crew, role, ext) in [
        ("cap-a", "crew-a", Role::Captain, None),
        ("w-a1", "crew-a", Role::Worker, Some(2)),
        ("w-a2", "crew-a", Role::Worker, Some(3)),
        ("cap-b", "crew-b", Role::Captain, None),
        ("w-b1", "crew-b", Role::Worker, Some(2)),
        ("cap-x", "crew-x", Role::Captain, None),
    ] {
        crewd_store::sessions::upsert(
            c,
            &Session {
                id: id.into(),
                crew_id: crew.into(),
                role,
                executor: "claude".into(),
                display_name: Some(format!("名字-{id}")),
                ext,
                updated_at: "2026-09-01T00:00:00.000Z".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    }
    crewd_store::schema::backfill_v3(c).unwrap();
    s
}

/// 在 NOW 跑一个操作，失败就 panic（测试的正路径）。
pub(crate) fn ok<T>(s: &mut Store, f: impl FnOnce(&mut Tx<'_>) -> Result<T>) -> T {
    crate::run(s, NOW, f).unwrap().0
}

/// 在指定时间跑，连同变更一起返回。
pub(crate) fn ok_at<T>(
    s: &mut Store,
    now: i64,
    f: impl FnOnce(&mut Tx<'_>) -> Result<T>,
) -> (T, Vec<Change>) {
    crate::run(s, now, f).unwrap()
}

/// 在 NOW 跑一个应当失败的操作，返回错误。
pub(crate) fn err<T: std::fmt::Debug>(
    s: &mut Store,
    f: impl FnOnce(&mut Tx<'_>) -> Result<T>,
) -> CollabError {
    crate::run(s, NOW, f).unwrap_err()
}

pub(crate) fn count(s: &Store, sql: &str) -> i64 {
    s.conn().query_row(sql, [], |r| r.get(0)).unwrap()
}
```

`crates/collab/src/settings.rs`（整个文件）：

```rust
//! crewd 的设置（存 `meta`）。§22.7 Q5：并发上限默认不设；语言默认 zh（见计划「人的拍板」末段）。
use crate::{CollabError, Locale, Result, Tx};
use rusqlite::Connection;

const LOCALE_KEY: &str = "locale";
const MAX_LIVE_KEY: &str = "maxLiveSessions";

pub fn locale(conn: &Connection) -> Result<Locale> {
    match crewd_store::meta_get(conn, LOCALE_KEY)? {
        None => Ok(Locale::default()),
        Some(v) => Locale::parse(&v)
            .ok_or_else(|| CollabError::Invalid(format!("meta.locale = {v:?}，只认 zh / en"))),
    }
}

pub fn set_locale(tx: &mut Tx<'_>, l: Locale) -> Result<()> {
    crewd_store::meta_set(tx.conn(), LOCALE_KEY, l.as_str())?;
    Ok(())
}

/// 全机同时在跑的 session 上限；None = 不设（§22.7 Q5 默认）。它只挡「拉起会话」（M3c）。
pub fn max_live_sessions(conn: &Connection) -> Result<Option<u32>> {
    match crewd_store::meta_get(conn, MAX_LIVE_KEY)? {
        None => Ok(None),
        Some(v) => v
            .parse::<u32>()
            .ok()
            .filter(|n| *n > 0)
            .map(Some)
            .ok_or_else(|| CollabError::Invalid(format!("meta.maxLiveSessions = {v:?} 不是正整数"))),
    }
}

pub fn set_max_live_sessions(tx: &mut Tx<'_>, n: Option<u32>) -> Result<()> {
    match n {
        None => {
            tx.conn().execute("DELETE FROM meta WHERE key=?1", [MAX_LIVE_KEY])?;
        }
        Some(0) => return Err(CollabError::Invalid("上限必须 ≥ 1；不设上限请传 null".into())),
        Some(n) => crewd_store::meta_set(tx.conn(), MAX_LIVE_KEY, &n.to_string())?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{err, ok, store};

    #[test]
    fn defaults_are_zh_and_no_session_cap() {
        let s = store();
        assert_eq!(locale(s.conn()).unwrap(), Locale::Zh);
        assert_eq!(max_live_sessions(s.conn()).unwrap(), None);
    }

    #[test]
    fn session_cap_is_optional_and_positive() {
        let mut s = store();
        ok(&mut s, |tx| set_max_live_sessions(tx, Some(10)));
        assert_eq!(max_live_sessions(s.conn()).unwrap(), Some(10));
        ok(&mut s, |tx| set_max_live_sessions(tx, None));
        assert_eq!(max_live_sessions(s.conn()).unwrap(), None);
        assert!(matches!(
            err(&mut s, |tx| set_max_live_sessions(tx, Some(0))),
            CollabError::Invalid(_)
        ));
    }

    #[test]
    fn locale_roundtrips_and_garbage_is_an_error() {
        let mut s = store();
        ok(&mut s, |tx| set_locale(tx, Locale::En));
        assert_eq!(locale(s.conn()).unwrap(), Locale::En);
        crewd_store::meta_set(s.conn(), "locale", "fr").unwrap();
        assert!(locale(s.conn()).is_err(), "认不出不猜");
    }
}
```

`crates/collab/src/wakes.rs`：先只放类型与测试（实现在 Step 4）：

```rust
//! 持久化唤醒队列（spec §22.4：取代只在内存里的 `CrewDeferredWakeQueue`）与熔断台账（§22.7 Q5）。
//! 这里只有读写；叫谁、何时投、准入与熔断判据都在 M3c（`crewd-wake` + `server::orchestra`）。
use crate::{Change, CollabError, Result, Tx};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeKind {
    Message,
    Timer,
    AskAnswer,
    Restart,
    System,
    /// M4b 的检查点（M3 里没有写者）。
    Checkpoint,
    /// M4b 的停下即检查（M3 里没有写者）。
    StopCheck,
}

/// 优先级：`Human > System > Auto`（`Ord` 按这个顺序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WakePriority {
    Auto,
    System,
    Human,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WakeState {
    Pending,
    Inflight,
    Delivered,
    Dropped,
}

/// 入队请求。存来源身份，不存渲染好的正文（出队时现渲染，`render::build_batch`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeRequest {
    pub session: String,
    pub kind: WakeKind,
    /// 去重键的一部分：`UNIQUE(session, kind, source)`。用 `source::*` 拼。
    pub source: String,
    pub priority: WakePriority,
    /// 不早于这个时间投（Unix 毫秒）；None = 现在。
    pub not_before_ms: Option<i64>,
    pub note: Option<String>,
    /// 触发这次唤醒的 session（「来源 → 目标」熔断用，§22.7 Q5）；人 / 系统触发的是 None。
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enqueued {
    New(i64),
    /// 同一 (session, kind, source) 已经在表里（不论状态）：不重复入队。
    Duplicate(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeRow {
    pub id: i64,
    pub session: String,
    pub kind: WakeKind,
    pub source: String,
    pub origin: Option<String>,
    pub priority: WakePriority,
    pub not_before_ms: i64,
    pub note: Option<String>,
    pub state: WakeState,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub created_at: String,
    pub settled_at: Option<String>,
}

/// source 的拼法，一处定死（裁定 13）。
pub mod source {
    pub fn message(message_id: &str) -> String {
        format!("msg:{message_id}")
    }
    /// H 号按机组发，所以带上机组。
    pub fn ask(crew_id: &str, ask: &str) -> String {
        format!("ask:{crew_id}/{ask}")
    }
    pub fn timer(token: &str) -> String {
        format!("timer:{token}")
    }
    pub fn quota(reset_ms: i64) -> String {
        format!("quota:{reset_ms}")
    }
    pub fn restart(prompt_id: i64) -> String {
        format!("restart:{prompt_id}")
    }
    pub fn system(tag: &str) -> String {
        format!("system:{tag}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{MIN, NOW, count, err, ok, ok_at, store};

    fn req(session: &str, source: &str, p: WakePriority) -> WakeRequest {
        WakeRequest {
            session: session.into(),
            kind: WakeKind::Message,
            source: source.into(),
            priority: p,
            not_before_ms: None,
            note: None,
            origin: Some("w-a2".into()),
        }
    }

    #[test]
    fn enqueue_dedupes_on_session_kind_source() {
        let mut s = store();
        let (a, ch) = ok_at(&mut s, NOW, |tx| enqueue(tx, req("w-a1", "msg:m1", WakePriority::Auto)));
        let Enqueued::New(id) = a else { panic!("{a:?}") };
        assert_eq!(ch, [Change::WakeEnqueued { wake_id: id, session_id: "w-a1".into() }]);
        let (b, ch) = ok_at(&mut s, NOW, |tx| enqueue(tx, req("w-a1", "msg:m1", WakePriority::Human)));
        assert_eq!(b, Enqueued::Duplicate(id));
        assert!(ch.is_empty(), "重复入队不发变更");
        assert_eq!(count(&s, "SELECT COUNT(*) FROM wakes"), 1);
        let row = &list(s.conn(), Some("w-a1"), 10).unwrap()[0];
        assert_eq!(row.origin.as_deref(), Some("w-a2"));
        assert_eq!(row.not_before_ms, NOW, "None = 现在");
    }

    #[test]
    fn enqueue_for_unknown_session_is_not_found() {
        let mut s = store();
        assert!(matches!(
            err(&mut s, |tx| enqueue(tx, req("ghost", "msg:m1", WakePriority::Auto))),
            CollabError::NotFound(_)
        ));
    }

    #[test]
    fn due_for_orders_by_priority_then_age_and_respects_not_before() {
        let mut s = store();
        ok(&mut s, |tx| {
            enqueue(tx, req("w-a1", "msg:1", WakePriority::Auto))?;
            enqueue(tx, req("w-a1", "msg:2", WakePriority::Human))?;
            enqueue(tx, req("w-a1", "msg:3", WakePriority::System))?;
            enqueue(
                tx,
                WakeRequest {
                    not_before_ms: Some(NOW + 10 * MIN),
                    ..req("w-a1", "timer:t", WakePriority::Human)
                },
            )
        });
        let due: Vec<String> = due_for(s.conn(), "w-a1", NOW).unwrap().into_iter().map(|r| r.source).collect();
        assert_eq!(due, ["msg:2", "msg:3", "msg:1"]);
        assert_eq!(next_due_at(s.conn()).unwrap(), Some(NOW));
        assert_eq!(sessions_with_due(s.conn(), NOW).unwrap(), ["w-a1"]);
        let later: Vec<String> = due_for(s.conn(), "w-a1", NOW + 10 * MIN).unwrap().into_iter().map(|r| r.source).collect();
        assert_eq!(later.len(), 4);
    }

    #[test]
    fn lifecycle_pending_inflight_delivered_and_failures() {
        let mut s = store();
        let Enqueued::New(id) = ok(&mut s, |tx| enqueue(tx, req("w-a1", "msg:1", WakePriority::Auto))) else {
            panic!()
        };
        ok(&mut s, |tx| mark_inflight(tx, &[id]));
        assert!(due_for(s.conn(), "w-a1", NOW).unwrap().is_empty(), "inflight 不再算到期");
        assert!(matches!(err(&mut s, |tx| mark_inflight(tx, &[id])), CollabError::Conflict(_)));
        assert_eq!(ok(&mut s, |tx| mark_failed(tx, id, "输入框没就绪")), 1);
        assert_eq!(due_for(s.conn(), "w-a1", NOW).unwrap()[0].attempts, 1, "失败回到 pending、记次数");
        ok(&mut s, |tx| mark_inflight(tx, &[id]));
        ok(&mut s, |tx| mark_delivered(tx, &[id]));
        let row = &list(s.conn(), None, 10).unwrap()[0];
        assert_eq!(row.state, WakeState::Delivered);
        assert!(row.settled_at.is_some());
        let Enqueued::New(id2) = ok(&mut s, |tx| enqueue(tx, req("w-a1", "msg:2", WakePriority::Auto))) else {
            panic!()
        };
        ok(&mut s, |tx| mark_dropped(tx, id2, "游标已越过：hook 注入过了"));
        assert_eq!(list(s.conn(), Some("w-a1"), 10).unwrap().iter().filter(|r| r.state == WakeState::Dropped).count(), 1);
    }

    #[test]
    fn reset_inflight_returns_everything_in_flight_to_pending() {
        let mut s = store();
        let ids: Vec<i64> = ok(&mut s, |tx| {
            let mut v = Vec::new();
            for src in ["msg:1", "msg:2"] {
                if let Enqueued::New(id) = enqueue(tx, req("w-a1", src, WakePriority::Auto))? {
                    v.push(id);
                }
            }
            Ok(v)
        });
        ok(&mut s, |tx| mark_inflight(tx, &ids));
        assert_eq!(ok(&mut s, reset_inflight), 2);
        assert_eq!(due_for(s.conn(), "w-a1", NOW).unwrap().len(), 2);
    }

    #[test]
    fn recent_for_lists_wakes_created_since() {
        let mut s = store();
        ok_at(&mut s, NOW - 5 * MIN, |tx| enqueue(tx, req("w-a1", "msg:old", WakePriority::Auto)));
        ok_at(&mut s, NOW, |tx| enqueue(tx, req("w-a1", "msg:new", WakePriority::Auto)));
        let got: Vec<String> = recent_for(s.conn(), "w-a1", NOW - MIN).unwrap().into_iter().map(|r| r.source).collect();
        assert_eq!(got, ["msg:new"]);
    }

    #[test]
    fn latches_by_edge_and_by_session() {
        let mut s = store();
        let edge = latches::Scope::Edge { from: "w-a1".into(), to: "w-a2".into() };
        ok(&mut s, |tx| latches::set(tx, &edge, "两人互相叫醒、没有新产出"));
        ok(&mut s, |tx| latches::set(tx, &latches::Scope::Session("w-b1".into()), "反复被叫醒"));
        let got = latches::get(s.conn(), &edge).unwrap().unwrap();
        assert_eq!(got.scope, edge);
        assert_eq!(latches::list(s.conn()).unwrap().len(), 2);
        assert!(ok(&mut s, |tx| latches::clear(tx, &edge)));
        assert!(!ok(&mut s, |tx| latches::clear(tx, &edge)), "再清一次 = 本来就没有");
        assert!(latches::get(s.conn(), &edge).unwrap().is_none());
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-collab`
Expected: 编译失败（`run`、`Tx` 的方法、`wakes` 的函数不存在）。

- [ ] **Step 4: 实现**

`lib.rs`，`struct Tx` 之后加：

```rust
impl<'a> Tx<'a> {
    pub fn conn(&self) -> &'a Connection {
        self.conn
    }
    pub fn now_ms(&self) -> i64 {
        self.now_ms
    }
    pub fn now_ts(&self) -> String {
        crewd_store::ts_from_ms(self.now_ms)
    }
    /// 记一条变更（同一条只记一次）。
    pub fn note(&mut self, c: Change) {
        if !self.changes.contains(&c) {
            self.changes.push(c);
        }
    }
}

/// 在一个 SQLite 事务里跑 `f`；成功才提交并交回变更，失败整体回滚、变更作废。
pub fn run<T>(
    store: &mut Store,
    now_ms: i64,
    f: impl FnOnce(&mut Tx<'_>) -> Result<T>,
) -> Result<(T, Vec<Change>)> {
    let t = store.conn().unchecked_transaction()?;
    let (v, changes) = {
        let mut tx = Tx {
            conn: &t,
            now_ms,
            changes: Vec::new(),
        };
        let v = f(&mut tx)?;
        (v, tx.changes)
    };
    t.commit()?;
    Ok((v, changes))
}
```

`wakes.rs`，测试模块之前加：

```rust
impl WakeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WakeKind::Message => "message",
            WakeKind::Timer => "timer",
            WakeKind::AskAnswer => "ask_answer",
            WakeKind::Restart => "restart",
            WakeKind::System => "system",
            WakeKind::Checkpoint => "checkpoint",
            WakeKind::StopCheck => "stop_check",
        }
    }
    pub fn parse(s: &str) -> Option<WakeKind> {
        [
            WakeKind::Message,
            WakeKind::Timer,
            WakeKind::AskAnswer,
            WakeKind::Restart,
            WakeKind::System,
            WakeKind::Checkpoint,
            WakeKind::StopCheck,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

impl WakePriority {
    pub fn as_str(self) -> &'static str {
        match self {
            WakePriority::Auto => "auto",
            WakePriority::System => "system",
            WakePriority::Human => "human",
        }
    }
    pub fn parse(s: &str) -> Option<WakePriority> {
        match s {
            "auto" => Some(WakePriority::Auto),
            "system" => Some(WakePriority::System),
            "human" => Some(WakePriority::Human),
            _ => None,
        }
    }
}

impl WakeState {
    pub fn as_str(self) -> &'static str {
        match self {
            WakeState::Pending => "pending",
            WakeState::Inflight => "inflight",
            WakeState::Delivered => "delivered",
            WakeState::Dropped => "dropped",
        }
    }
    pub fn parse(s: &str) -> Option<WakeState> {
        match s {
            "pending" => Some(WakeState::Pending),
            "inflight" => Some(WakeState::Inflight),
            "delivered" => Some(WakeState::Delivered),
            "dropped" => Some(WakeState::Dropped),
            _ => None,
        }
    }
}

const COLS: &str = "id,session_id,kind,source,origin,priority,not_before,note,state,attempts,last_error,created_at,settled_at";
/// 优先级高的先出：human 2、system 1、auto 0。
const RANK: &str = "CASE priority WHEN 'human' THEN 2 WHEN 'system' THEN 1 ELSE 0 END";

fn bad(idx: usize, what: &str, v: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        idx,
        rusqlite::types::Type::Text,
        format!("wakes.{what} = {v:?}").into(),
    )
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<WakeRow> {
    let kind: String = r.get(2)?;
    let priority: String = r.get(5)?;
    let state: String = r.get(8)?;
    Ok(WakeRow {
        id: r.get(0)?,
        session: r.get(1)?,
        kind: WakeKind::parse(&kind).ok_or_else(|| bad(2, "kind", kind.clone()))?,
        source: r.get(3)?,
        origin: r.get(4)?,
        priority: WakePriority::parse(&priority).ok_or_else(|| bad(5, "priority", priority.clone()))?,
        not_before_ms: r.get(6)?,
        note: r.get(7)?,
        state: WakeState::parse(&state).ok_or_else(|| bad(8, "state", state.clone()))?,
        attempts: r.get(9)?,
        last_error: r.get(10)?,
        created_at: r.get(11)?,
        settled_at: r.get(12)?,
    })
}

fn rows(conn: &Connection, sql: &str, p: impl rusqlite::Params) -> Result<Vec<WakeRow>> {
    let mut st = conn.prepare(sql)?;
    Ok(st.query_map(p, row)?.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn enqueue(tx: &mut Tx<'_>, r: WakeRequest) -> Result<Enqueued> {
    if r.source.trim().is_empty() {
        return Err(CollabError::Invalid("唤醒的 source 不能为空".into()));
    }
    if crewd_store::sessions::get(tx.conn(), &r.session)?.is_none() {
        return Err(CollabError::NotFound(format!("没有 session {}", r.session)));
    }
    let n = tx.conn().execute(
        "INSERT INTO wakes(session_id,kind,source,origin,priority,not_before,note,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
         ON CONFLICT(session_id,kind,source) DO NOTHING",
        params![
            r.session,
            r.kind.as_str(),
            r.source,
            r.origin,
            r.priority.as_str(),
            r.not_before_ms.unwrap_or(tx.now_ms()),
            r.note,
            tx.now_ts(),
        ],
    )?;
    if n == 0 {
        let id: i64 = tx.conn().query_row(
            "SELECT id FROM wakes WHERE session_id=?1 AND kind=?2 AND source=?3",
            params![r.session, r.kind.as_str(), r.source],
            |x| x.get(0),
        )?;
        return Ok(Enqueued::Duplicate(id));
    }
    let id = tx.conn().last_insert_rowid();
    tx.note(Change::WakeEnqueued {
        wake_id: id,
        session_id: r.session,
    });
    Ok(Enqueued::New(id))
}

/// 该 session 已到期的 pending，优先级高的在前、同档按入队先后。
pub fn due_for(conn: &Connection, session: &str, now_ms: i64) -> Result<Vec<WakeRow>> {
    rows(
        conn,
        &format!(
            "SELECT {COLS} FROM wakes WHERE session_id=?1 AND state='pending' AND not_before<=?2
             ORDER BY {RANK} DESC, id"
        ),
        params![session, now_ms],
    )
}

pub fn sessions_with_due(conn: &Connection, now_ms: i64) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT DISTINCT session_id FROM wakes WHERE state='pending' AND not_before<=?1 ORDER BY session_id",
    )?;
    Ok(st.query_map([now_ms], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?)
}

/// 最早的 pending 的 not_before（M3c 的单个定时器按它醒）。
pub fn next_due_at(conn: &Connection) -> Result<Option<i64>> {
    Ok(conn.query_row(
        "SELECT MIN(not_before) FROM wakes WHERE state='pending'",
        [],
        |r| r.get(0),
    )?)
}

fn set_state(tx: &mut Tx<'_>, ids: &[i64], from: &[&str], to: WakeState, settle: bool) -> Result<()> {
    for id in ids {
        let state: Option<String> = tx
            .conn()
            .query_row("SELECT state FROM wakes WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        let Some(state) = state else {
            return Err(CollabError::NotFound(format!("没有唤醒 {id}")));
        };
        if !from.contains(&state.as_str()) {
            return Err(CollabError::Conflict(format!(
                "唤醒 {id} 是 {state}，不能改成 {}",
                to.as_str()
            )));
        }
        tx.conn().execute(
            "UPDATE wakes SET state=?2, settled_at=CASE WHEN ?3 THEN ?4 ELSE settled_at END WHERE id=?1",
            params![id, to.as_str(), settle, tx.now_ts()],
        )?;
    }
    Ok(())
}

pub fn mark_inflight(tx: &mut Tx<'_>, ids: &[i64]) -> Result<()> {
    set_state(tx, ids, &["pending"], WakeState::Inflight, false)
}

/// 投递确认（`Delivery::Delivered`）或「游标已越过」时记。
pub fn mark_delivered(tx: &mut Tx<'_>, ids: &[i64]) -> Result<()> {
    set_state(tx, ids, &["pending", "inflight"], WakeState::Delivered, true)
}

/// 投递失败：回到 pending、次数 +1、记下原因。返回累计失败次数（M3c 连续 3 次就在大厅 @机长）。
pub fn mark_failed(tx: &mut Tx<'_>, id: i64, error: &str) -> Result<u32> {
    set_state(tx, &[id], &["inflight"], WakeState::Pending, false)?;
    tx.conn().execute(
        "UPDATE wakes SET attempts=attempts+1, last_error=?2 WHERE id=?1",
        params![id, error],
    )?;
    Ok(tx
        .conn()
        .query_row("SELECT attempts FROM wakes WHERE id=?1", [id], |r| r.get(0))?)
}

pub fn mark_dropped(tx: &mut Tx<'_>, id: i64, why: &str) -> Result<()> {
    set_state(tx, &[id], &["pending", "inflight"], WakeState::Dropped, true)?;
    tx.conn()
        .execute("UPDATE wakes SET last_error=?2 WHERE id=?1", params![id, why])?;
    Ok(())
}

/// 启动时：上次没确认的 inflight 全部退回 pending（spec §22.4「重启时 inflight 退回 pending」）。
pub fn reset_inflight(tx: &mut Tx<'_>) -> Result<usize> {
    Ok(tx
        .conn()
        .execute("UPDATE wakes SET state='pending' WHERE state='inflight'", [])?)
}

/// 某个目标 `since_ms` 之后入队的全部唤醒（任何状态），按入队先后。熔断判定读它（§22.7 Q5）。
pub fn recent_for(conn: &Connection, session: &str, since_ms: i64) -> Result<Vec<WakeRow>> {
    rows(
        conn,
        &format!("SELECT {COLS} FROM wakes WHERE session_id=?1 AND created_at>=?2 ORDER BY id"),
        params![session, crewd_store::ts_from_ms(since_ms)],
    )
}

/// 排障用（`wake.list`）：最近的 `limit` 条，新的在前。
pub fn list(conn: &Connection, session: Option<&str>, limit: u32) -> Result<Vec<WakeRow>> {
    match session {
        Some(s) => rows(
            conn,
            &format!("SELECT {COLS} FROM wakes WHERE session_id=?1 ORDER BY id DESC LIMIT ?2"),
            params![s, limit],
        ),
        None => rows(
            conn,
            &format!("SELECT {COLS} FROM wakes ORDER BY id DESC LIMIT ?1"),
            params![limit],
        ),
    }
}

/// 熔断台账（§22.7 Q5：「来源 → 目标」与「目标 session」两级；绝不按全机总量）。
/// 只有人能从客户端解（M3c 的 `admission.recover`）。
pub mod latches {
    use crate::{Result, Tx};
    use rusqlite::{Connection, OptionalExtension, params};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Scope {
        Edge { from: String, to: String },
        Session(String),
    }

    impl Scope {
        fn key(&self) -> (&'static str, String) {
            match self {
                Scope::Edge { from, to } => ("edge", format!("{from}→{to}")),
                Scope::Session(s) => ("session", s.clone()),
            }
        }
        fn from_key(scope: &str, id: &str) -> Option<Scope> {
            match scope {
                "edge" => id.split_once('→').map(|(f, t)| Scope::Edge {
                    from: f.to_string(),
                    to: t.to_string(),
                }),
                "session" => Some(Scope::Session(id.to_string())),
                _ => None,
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Latch {
        pub scope: Scope,
        pub reason: String,
        pub at: String,
    }

    pub fn set(tx: &mut Tx<'_>, scope: &Scope, reason: &str) -> Result<()> {
        let (s, id) = scope.key();
        tx.conn().execute(
            "INSERT INTO wake_latches(scope,id,reason,at) VALUES(?1,?2,?3,?4)
             ON CONFLICT(scope,id) DO UPDATE SET reason=excluded.reason, at=excluded.at",
            params![s, id, reason, tx.now_ts()],
        )?;
        Ok(())
    }

    /// 返回是否真的清掉了一条。
    pub fn clear(tx: &mut Tx<'_>, scope: &Scope) -> Result<bool> {
        let (s, id) = scope.key();
        Ok(tx
            .conn()
            .execute("DELETE FROM wake_latches WHERE scope=?1 AND id=?2", params![s, id])?
            > 0)
    }

    pub fn get(conn: &Connection, scope: &Scope) -> Result<Option<Latch>> {
        let (s, id) = scope.key();
        Ok(conn
            .query_row(
                "SELECT reason, at FROM wake_latches WHERE scope=?1 AND id=?2",
                params![s, id],
                |r| {
                    Ok(Latch {
                        scope: scope.clone(),
                        reason: r.get(0)?,
                        at: r.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn list(conn: &Connection) -> Result<Vec<Latch>> {
        let mut st = conn.prepare("SELECT scope, id, reason, at FROM wake_latches ORDER BY at, scope, id")?;
        let raw = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(raw
            .into_iter()
            .filter_map(|(s, id, reason, at)| {
                Scope::from_key(&s, &id).map(|scope| Latch { scope, reason, at })
            })
            .collect())
    }
}
```

（`recent_for` 按 `created_at` 字符串比较：`ts_from_ms` 与入队写的 `now_ts()` 同格式，字典序 = 时间序。）

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-collab`
Expected: lib 3 + settings 3 + wakes 7 = 13 passed。

- [ ] **Step 6: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/collab/Cargo.toml crates/collab/src/lib.rs crates/collab/src/testkit.rs crates/collab/src/settings.rs crates/collab/src/wakes.rs Cargo.lock
git diff --cached --stat
git commit -m "crewd-collab: Tx/run/Change skeleton, settings, persistent wake queue and loop-breaker latches

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 6: 机组（根、机长、DAG、编号）、session 行与生命周期字段、通讯录

移植 `Sources/Stores/LocalCrewStore.swift:356-526`（禁环、内建不收编也不被收编、只能摘直系子、release-to 先挂新边再摘旧边）与 `CrewDirectory.swift:8-13`（号码终身不变、不回收、分机 1 = 机长）。「失败不留副作用」由事务保证：任何一步出错 `run` 整体回滚。本任务同时建 `cursors.rs` 的最小部分（`advance`、`pin_to_end`），因为注册 session、换机长都要把游标钉到末尾（裁定 9）。

**Files:**
- Create: `crewd/crates/collab/src/crews.rs`、`crewd/crates/collab/src/sessions.rs`、`crewd/crates/collab/src/directory.rs`、`crewd/crates/collab/src/cursors.rs`
- Modify: `crewd/crates/collab/src/lib.rs`（`pub mod crews; pub mod cursors; pub mod directory; pub mod sessions;`）

**Interfaces:**
- Consumes: Task 5 的 `Tx`、testkit；`crewd_store::{crews, sessions, channels, messages, meta_get, meta_set}`。
- Produces：
  - `crews::{get(conn, id) -> Result<Crew>, list(conn), list_by_activity(conn) -> Result<Vec<Crew>>, parents(conn, id) -> Result<Vec<String>>, children(conn, id) -> Result<Vec<String>>, is_root(conn, id) -> Result<bool>, root_of(conn, id) -> Result<String>, captain_of(conn, crew) -> Result<Option<String>>, create(tx, NewCrew) -> Result<Crew>, update(tx, id, CrewPatch) -> Result<Crew>, adopt(tx, crew, parent) -> Result<Crew>, release(tx, crew, from, to: Option<&str>) -> Result<Crew>, delete(tx, id) -> Result<()>, set_captain(tx, crew, session) -> Result<()>}`；`NewCrew { title, working_directory, parent_id: Option<String>, title_source: Option<String> }`；`CrewPatch { title, title_source, working_directory, hidden: Option<bool>, status_line: Option<String>（"" = 清掉）, policy: Option<serde_json::Value> }`（全部字段 `Option`，`Default`）。
  - `sessions::{NewSession, register, get, list(conn, crew: Option<&str>), crew_of, role_of, display_name, running_desired, set_desired, set_working, set_resume_failed, set_awaiting_question, set_profile, note_output, valid_executor}`。
  - `directory::list(conn, query: Option<&str>) -> Result<Vec<DirectoryRow>>`。
  - `cursors::{HUMAN = "human", cursor(conn, reader, channel) -> Result<Option<i64>>, advance(tx, reader, channel, seq) -> Result<()>, pin_to_end(tx, reader, channel) -> Result<i64>}`。

- [ ] **Step 1: 写失败的测试**

`crates/collab/src/cursors.rs`（整个文件，Task 7 / 9 往里加）：

```rust
//! 已读游标（spec §22.2：红标与未读都按游标派生，不建表）。reader = "human" 或 session id。
use crate::{Result, Tx};
use rusqlite::{Connection, OptionalExtension, params};

/// 人类的游标名（Mac / iOS 共用一个）。
pub const HUMAN: &str = "human";

pub fn cursor(conn: &Connection, reader: &str, channel: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT seq FROM read_cursors WHERE reader=?1 AND channel_id=?2",
            params![reader, channel],
            |r| r.get(0),
        )
        .optional()?)
}

/// 推进游标（只进不退）。
pub fn advance(tx: &mut Tx<'_>, reader: &str, channel: &str, seq: i64) -> Result<()> {
    tx.conn().execute(
        "INSERT INTO read_cursors(reader,channel_id,seq,updated_at) VALUES(?1,?2,?3,?4)
         ON CONFLICT(reader,channel_id) DO UPDATE SET seq=MAX(seq, excluded.seq), updated_at=excluded.updated_at",
        params![reader, channel, seq, tx.now_ts()],
    )?;
    Ok(())
}

/// 入群点：把游标钉到群的当前末尾（裁定 9）。返回钉到的 seq。
pub fn pin_to_end(tx: &mut Tx<'_>, reader: &str, channel: &str) -> Result<i64> {
    let end = crewd_store::messages::max_seq(tx.conn(), channel)?;
    advance(tx, reader, channel, end)?;
    Ok(end)
}
```

`crates/collab/src/crews.rs`：文件头 + 类型 + 测试模块（实现在 Step 3）：

```rust
//! 机组：根、机长、DAG、建 / 改 / 删、编号。移植 `LocalCrewStore.swift:356-526`、`CrewDirectory.swift:8-13`。
use crate::{Change, CollabError, Result, Tx};
use crewd_proto::Crew;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{BTreeSet, VecDeque};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewCrew {
    pub title: String,
    pub working_directory: String,
    pub parent_id: Option<String>,
    pub title_source: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CrewPatch {
    pub title: Option<String>,
    pub title_source: Option<String>,
    pub working_directory: Option<String>,
    pub hidden: Option<bool>,
    /// "" = 清掉。
    pub status_line: Option<String>,
    pub policy: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, count, err, ok, store};

    fn edges(s: &crewd_store::Store, crew: &str) -> Vec<String> {
        parents(s.conn(), crew).unwrap()
    }

    #[test]
    fn root_of_is_the_earliest_root_among_ancestors() {
        let mut s = store();
        assert_eq!(root_of(s.conn(), "crew-b").unwrap(), "crew-a");
        assert_eq!(root_of(s.conn(), "crew-a").unwrap(), "crew-a");
        ok(&mut s, |tx| adopt(tx, "crew-b", "crew-x"));
        assert_eq!(root_of(s.conn(), "crew-b").unwrap(), "crew-a", "crew-a 建得更早");
        s.conn()
            .execute("UPDATE crews SET created_at='2026-08-15T00:00:00.000Z' WHERE id='crew-x'", [])
            .unwrap();
        assert_eq!(root_of(s.conn(), "crew-b").unwrap(), "crew-x");
    }

    #[test]
    fn adopt_refuses_cycles_self_and_builtin_and_is_idempotent() {
        let mut s = store();
        assert!(matches!(err(&mut s, |tx| adopt(tx, "crew-a", "crew-b")), CollabError::Invalid(_)), "成环");
        assert!(matches!(err(&mut s, |tx| adopt(tx, "crew-a", "crew-a")), CollabError::Invalid(_)), "自挂");
        assert!(matches!(err(&mut s, |tx| adopt(tx, "chief", "crew-a")), CollabError::Forbidden(_)));
        assert!(matches!(err(&mut s, |tx| adopt(tx, "crew-a", "chief")), CollabError::Forbidden(_)));
        assert!(matches!(err(&mut s, |tx| adopt(tx, "crew-a", "nope")), CollabError::NotFound(_)));
        ok(&mut s, |tx| adopt(tx, "crew-x", "crew-b"));
        ok(&mut s, |tx| adopt(tx, "crew-x", "crew-b"));
        assert_eq!(edges(&s, "crew-x"), ["crew-b"]);
    }

    #[test]
    fn release_only_from_a_direct_parent() {
        let mut s = store();
        assert!(matches!(err(&mut s, |tx| release(tx, "crew-b", "crew-x", None)), CollabError::Invalid(_)));
        assert!(matches!(err(&mut s, |tx| release(tx, "crew-b", "chief", None)), CollabError::Forbidden(_)));
        ok(&mut s, |tx| release(tx, "crew-b", "crew-a", None));
        assert!(edges(&s, "crew-b").is_empty());
        assert_eq!(root_of(s.conn(), "crew-b").unwrap(), "crew-b");
    }

    #[test]
    fn release_to_moves_the_edge() {
        let mut s = store();
        ok(&mut s, |tx| release(tx, "crew-b", "crew-a", Some("crew-x")));
        assert_eq!(edges(&s, "crew-b"), ["crew-x"]);
        ok(&mut s, |tx| release(tx, "crew-b", "crew-x", Some("crew-x")));
        assert_eq!(edges(&s, "crew-b"), ["crew-x"], "转挂给原父 = 不动");
    }

    #[test]
    fn release_to_failure_leaves_no_side_effect() {
        let mut s = store();
        let c = ok(&mut s, |tx| {
            create(tx, NewCrew { title: "孙机组".into(), working_directory: "/tmp/example".into(), parent_id: Some("crew-b".into()), ..Default::default() })
        });
        // crew-b 转挂到自己的子机组下面 = 成环：整体失败，旧边还在。
        assert!(matches!(err(&mut s, |tx| release(tx, "crew-b", "crew-a", Some(&c.id))), CollabError::Invalid(_)));
        assert_eq!(edges(&s, "crew-b"), ["crew-a"]);
        assert_eq!(edges(&s, &c.id), ["crew-b"]);
    }

    #[test]
    fn create_numbers_never_repeat_and_each_crew_gets_a_lobby() {
        let mut s = store();
        let a = ok(&mut s, |tx| create(tx, NewCrew { title: "新机组".into(), working_directory: "/tmp/example".into(), ..Default::default() }));
        assert_eq!(a.number, Some(4));
        assert_eq!(count(&s, &format!("SELECT COUNT(*) FROM channels WHERE id='{}/lobby'", a.id)), 1);
        ok(&mut s, |tx| delete(tx, &a.id));
        let b = ok(&mut s, |tx| create(tx, NewCrew { title: "再建一个".into(), working_directory: "/tmp/example".into(), ..Default::default() }));
        assert_eq!(b.number, Some(5), "删掉的 4 号不回收");
        assert!(matches!(
            err(&mut s, |tx| create(tx, NewCrew { title: "  ".into(), working_directory: "/tmp/example".into(), ..Default::default() })),
            CollabError::Invalid(_)
        ));
        let c = ok(&mut s, |tx| create(tx, NewCrew { title: "子".into(), working_directory: "/tmp/example".into(), parent_id: Some("crew-x".into()), ..Default::default() }));
        assert_eq!(edges(&s, &c.id), ["crew-x"]);
    }

    #[test]
    fn delete_refuses_builtin_and_crews_with_running_sessions() {
        let mut s = store();
        assert!(matches!(err(&mut s, |tx| delete(tx, "chief")), CollabError::Forbidden(_)));
        s.conn().execute("UPDATE sessions SET desired='running' WHERE id='cap-x'", []).unwrap();
        assert!(matches!(err(&mut s, |tx| delete(tx, "crew-x")), CollabError::Conflict(_)));
        s.conn().execute("UPDATE sessions SET desired='stopped' WHERE id='cap-x'", []).unwrap();
        let ((), ch) = crate::testkit::ok_at(&mut s, NOW, |tx| delete(tx, "crew-x"));
        assert!(ch.contains(&Change::CrewRemoved { crew_id: "crew-x".into() }));
        assert_eq!(count(&s, "SELECT COUNT(*) FROM sessions WHERE crew_id='crew-x'"), 0);
        assert_eq!(count(&s, "SELECT COUNT(*) FROM channels WHERE crew_id='crew-x'"), 0);
    }

    #[test]
    fn set_captain_swaps_roles_and_gives_the_old_captain_a_fresh_extension() {
        let mut s = store();
        ok(&mut s, |tx| set_captain(tx, "crew-a", "w-a1"));
        assert_eq!(captain_of(s.conn(), "crew-a").unwrap().as_deref(), Some("w-a1"));
        assert_eq!(count(&s, "SELECT ext FROM sessions WHERE id='cap-a'"), 4, "旧机长拿下一个分机号");
        assert_eq!(count(&s, "SELECT next_ext FROM crews WHERE id='crew-a'"), 5);
        assert_eq!(crate::sessions::role_of(s.conn(), "cap-a").unwrap(), crewd_proto::Role::Worker);
        assert_eq!(crate::sessions::role_of(s.conn(), "w-a1").unwrap(), crewd_proto::Role::Captain);
        assert!(matches!(err(&mut s, |tx| set_captain(tx, "crew-a", "w-b1")), CollabError::Invalid(_)), "别的机组的人");
    }

    #[test]
    fn list_by_activity_puts_the_most_recently_active_crew_first() {
        let mut s = store();
        crewd_store::messages::insert(
            s.conn(),
            &crewd_proto::Message {
                id: "m-x".into(),
                channel_id: "crew-x/lobby".into(),
                text: "有动静".into(),
                created_at: "2026-09-21T00:00:00.000Z".into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        let ids: Vec<String> = list_by_activity(s.conn()).unwrap().into_iter().map(|c| c.id).collect();
        assert_eq!(ids[0], "crew-x");
        let x = list_by_activity(s.conn()).unwrap().into_iter().find(|c| c.id == "crew-x").unwrap();
        assert_eq!(x.last_activity_at.as_deref(), Some("2026-09-21T00:00:00.000Z"));
    }
}
```

`crates/collab/src/sessions.rs`：文件头 + 类型 + 测试模块：

```rust
//! session 行：注册（分机号）与 M3c / M3b 要写的生命周期字段（裁定 16）。
use crate::{Change, CollabError, Result, Tx, crews, cursors};
use crewd_proto::{Desired, Role, Session};
use rusqlite::{Connection, params};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewSession {
    pub crew_id: String,
    pub role: Role,
    /// "claude" | "codex" | "acp:<registry-id>"（spec §16）。
    pub executor: String,
    pub title: Option<String>,
    pub brief: Option<String>,
    pub display_name: Option<String>,
    /// 缺省 = 机组的工作目录。
    pub working_directory: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub worktree: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, count, err, ok, store};

    fn worker(crew: &str) -> NewSession {
        NewSession {
            crew_id: crew.into(),
            role: Role::Worker,
            executor: "claude".into(),
            title: Some("修滚动".into()),
            ..Default::default()
        }
    }

    #[test]
    fn register_assigns_extensions_that_are_never_reused() {
        let mut s = store();
        let a = ok(&mut s, |tx| register(tx, worker("crew-a")));
        assert_eq!(a.ext, Some(4));
        assert_eq!(a.desired, Desired::Stopped);
        assert_eq!(a.working_directory.as_deref(), Some("/tmp/example"));
        s.conn().execute("DELETE FROM sessions WHERE id=?1", [&a.id]).unwrap();
        let b = ok(&mut s, |tx| register(tx, worker("crew-a")));
        assert_eq!(b.ext, Some(5));
        assert!(b.id.starts_with("worker-"));
    }

    #[test]
    fn a_crew_has_at_most_one_captain_via_register() {
        let mut s = store();
        let e = err(&mut s, |tx| register(tx, NewSession { role: Role::Captain, ..worker("crew-a") }));
        assert!(matches!(e, CollabError::Conflict(_)), "{e}");
        let c = ok(&mut s, |tx| {
            let crew = crate::crews::create(tx, crate::crews::NewCrew { title: "空机组".into(), working_directory: "/tmp/example".into(), ..Default::default() })?;
            register(tx, NewSession { role: Role::Captain, ..worker(&crew.id) })
        });
        assert_eq!(c.ext, None, "机长分机恒为 1，不存");
        assert_eq!(crate::crews::captain_of(s.conn(), &c.crew_id).unwrap().as_deref(), Some(c.id.as_str()));
    }

    #[test]
    fn executor_vocabulary_is_checked() {
        let mut s = store();
        assert!(matches!(err(&mut s, |tx| register(tx, NewSession { executor: "terminal".into(), ..worker("crew-a") })), CollabError::Invalid(_)));
        ok(&mut s, |tx| register(tx, NewSession { executor: "acp:opencode".into(), ..worker("crew-a") }));
        assert!(!valid_executor("acp:"));
        assert!(!valid_executor("acp:../x"));
    }

    #[test]
    fn a_new_session_starts_with_no_unread() {
        let mut s = store();
        crewd_store::messages::insert(
            s.conn(),
            &crewd_proto::Message { id: "old".into(), channel_id: "crew-a/lobby".into(), text: "历史".into(), created_at: "t".into(), ..Default::default() },
            None,
        )
        .unwrap();
        let a = ok(&mut s, |tx| register(tx, worker("crew-a")));
        let max = count(&s, "SELECT MAX(seq) FROM messages");
        assert_eq!(crate::cursors::cursor(s.conn(), &a.id, "crew-a/lobby").unwrap(), Some(max));
    }

    #[test]
    fn lifecycle_setters_write_their_columns() {
        let mut s = store();
        ok(&mut s, |tx| set_desired(tx, "w-a1", Desired::Running));
        ok(&mut s, |tx| set_working(tx, "w-a1", true));
        ok(&mut s, |tx| set_resume_failed(tx, "w-a2", true));
        ok(&mut s, |tx| set_awaiting_question(tx, "w-a1", Some("要不要接着做 B？".into())));
        ok(&mut s, |tx| note_output(tx, "w-a1"));
        let a = get(s.conn(), "w-a1").unwrap();
        assert_eq!(a.desired, Desired::Running);
        assert!(a.was_working);
        assert_eq!(a.last_active_at.as_deref(), Some(crewd_store::ts_from_ms(NOW).as_str()));
        assert_eq!(a.last_output_at, a.last_active_at);
        assert_eq!(a.awaiting_question.as_deref(), Some("要不要接着做 B？"));
        assert!(get(s.conn(), "w-a2").unwrap().resume_failed_at.is_some());
        ok(&mut s, |tx| set_resume_failed(tx, "w-a2", false));
        assert!(get(s.conn(), "w-a2").unwrap().resume_failed_at.is_none());
        assert_eq!(running_desired(s.conn()).unwrap().into_iter().map(|x| x.id).collect::<Vec<_>>(), ["w-a1"]);
        assert!(matches!(err(&mut s, |tx| set_desired(tx, "ghost", Desired::Running)), CollabError::NotFound(_)));
    }
}
```

`crates/collab/src/directory.rs`：

```rust
//! 全机通讯录（移植 `CrewDirectory.swift:125-170`）：机组按号升序，每个机组后面跟机长（分机 1）与有号的成员。
use crate::{Result, crews, sessions};
use crewd_proto::{DirectoryKind, DirectoryRow};
use rusqlite::Connection;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::store;

    fn numbers(rows: &[DirectoryRow]) -> Vec<&str> {
        rows.iter().map(|r| r.number.as_str()).collect()
    }

    #[test]
    fn full_table_lists_crews_captains_and_numbered_members() {
        let s = store();
        let rows = list(s.conn(), None).unwrap();
        assert_eq!(numbers(&rows), ["1", "1-1", "1-2", "1-3", "2", "2-1", "2-2", "3", "3-1"]);
        let b = rows.iter().find(|r| r.number == "2").unwrap();
        assert_eq!(b.org_path, "机组甲");
        let b1 = rows.iter().find(|r| r.number == "2-2").unwrap();
        assert_eq!(b1.org_path, "机组甲 / 机组乙");
        assert_eq!(b1.session_id.as_deref(), Some("w-b1"));
        assert_eq!(rows.iter().find(|r| r.number == "1-1").unwrap().name, "名字-cap-a");
    }

    #[test]
    fn query_matches_number_prefix_or_name_and_keeps_the_crew_header() {
        let s = store();
        assert_eq!(numbers(&list(s.conn(), Some("名字-w-b1")).unwrap()), ["2", "2-2"]);
        assert_eq!(numbers(&list(s.conn(), Some("3")).unwrap()), ["3", "3-1"]);
        assert!(list(s.conn(), Some("不存在的名字")).unwrap().is_empty());
        assert_eq!(list(s.conn(), Some("  ")).unwrap().len(), 9, "空白 = 全表");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-collab`
Expected: 编译失败（函数不存在）。

- [ ] **Step 3: 实现**

`crews.rs`，类型之后、测试之前：

```rust
fn not_found(id: &str) -> CollabError {
    CollabError::NotFound(format!("没有机组 {id}"))
}

pub fn get(conn: &Connection, id: &str) -> Result<Crew> {
    crewd_store::crews::get(conn, id)?.ok_or_else(|| not_found(id))
}

pub fn list(conn: &Connection) -> Result<Vec<Crew>> {
    Ok(crewd_store::crews::list(conn)?)
}

pub fn parents(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT p.parent_id FROM crew_parents p JOIN crews c ON c.id=p.parent_id
          WHERE p.crew_id=?1 ORDER BY c.created_at, c.id",
    )?;
    Ok(st.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?)
}

pub fn children(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT p.crew_id FROM crew_parents p JOIN crews c ON c.id=p.crew_id
          WHERE p.parent_id=?1 ORDER BY c.created_at, c.id",
    )?;
    Ok(st.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?)
}

pub fn is_root(conn: &Connection, id: &str) -> Result<bool> {
    Ok(parents(conn, id)?.is_empty())
}

fn descendants(conn: &Connection, id: &str) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    let mut queue = VecDeque::from([id.to_string()]);
    while let Some(c) = queue.pop_front() {
        for k in children(conn, &c)? {
            if out.insert(k.clone()) {
                queue.push_back(k);
            }
        }
    }
    Ok(out)
}

fn created_at(conn: &Connection, id: &str) -> Result<String> {
    Ok(conn.query_row("SELECT created_at FROM crews WHERE id=?1", [id], |r| r.get(0))?)
}

/// 没有父边的祖先里 created_at 最早的（§13 Q1）。自己就是根 → 自己。脏数据成环时有 visited 守卫、退回自己。
pub fn root_of(conn: &Connection, id: &str) -> Result<String> {
    get(conn, id)?;
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([id.to_string()]);
    let mut roots: Vec<(String, String)> = Vec::new();
    while let Some(c) = queue.pop_front() {
        if !seen.insert(c.clone()) {
            continue;
        }
        let ps = parents(conn, &c)?;
        if ps.is_empty() {
            roots.push((created_at(conn, &c)?, c));
        } else {
            queue.extend(ps);
        }
    }
    Ok(roots.into_iter().min().map(|(_, r)| r).unwrap_or_else(|| id.to_string()))
}

pub fn captain_of(conn: &Connection, crew: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT captain_session_id FROM crews WHERE id=?1", [crew], |r| r.get(0))
        .optional()?
        .flatten())
}

fn refuse_builtin(conn: &Connection, ids: &[&str]) -> Result<()> {
    for id in ids {
        let builtin: Option<bool> = conn
            .query_row("SELECT builtin FROM crews WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        if builtin == Some(true) {
            return Err(CollabError::Forbidden(format!("内建机组 {id} 不参与收编 / 摘出")));
        }
    }
    Ok(())
}

fn next_crew_number(tx: &mut Tx<'_>) -> Result<i64> {
    let conn = tx.conn();
    let stored = crewd_store::meta_get(conn, "nextCrewNumber")?.and_then(|s| s.parse::<i64>().ok());
    let max: i64 = conn.query_row("SELECT COALESCE(MAX(number),0) FROM crews", [], |r| r.get(0))?;
    let n = stored.unwrap_or(0).max(max + 1);
    crewd_store::meta_set(conn, "nextCrewNumber", &(n + 1).to_string())?;
    Ok(n)
}

pub fn create(tx: &mut Tx<'_>, n: NewCrew) -> Result<Crew> {
    let title = n.title.trim().to_string();
    if title.is_empty() {
        return Err(CollabError::Invalid("机组名不能为空".into()));
    }
    let wd = n.working_directory.trim().to_string();
    if wd.is_empty() {
        return Err(CollabError::Invalid("工作目录不能为空".into()));
    }
    let number = next_crew_number(tx)?;
    let now = tx.now_ts();
    let crew = Crew {
        id: format!("crew-{}", uuid::Uuid::new_v4().simple()),
        title,
        number: Some(number),
        builtin: false,
        working_directory: Some(wd),
        title_source: n.title_source,
        created_at: now.clone(),
        updated_at: now.clone(),
        ..Default::default()
    };
    crewd_store::crews::upsert(tx.conn(), &crew, None)?;
    crewd_store::channels::ensure_lobby(tx.conn(), &crew.id, &now)?;
    tx.note(Change::CrewChanged { crew_id: crew.id.clone() });
    if let Some(p) = n.parent_id.as_deref() {
        adopt(tx, &crew.id, p)?;
    }
    get(tx.conn(), &crew.id)
}

pub fn update(tx: &mut Tx<'_>, id: &str, p: CrewPatch) -> Result<Crew> {
    let crew = get(tx.conn(), id)?;
    let conn = tx.conn();
    let now = tx.now_ts();
    if let Some(t) = p.title {
        let t = t.trim().to_string();
        if t.is_empty() {
            return Err(CollabError::Invalid("机组名不能为空".into()));
        }
        if crew.builtin {
            return Err(CollabError::Forbidden("内建机组不能改名".into()));
        }
        conn.execute("UPDATE crews SET title=?2, title_source=COALESCE(?3, title_source) WHERE id=?1", params![id, t, p.title_source])?;
    }
    if let Some(wd) = p.working_directory {
        if wd.trim().is_empty() {
            return Err(CollabError::Invalid("工作目录不能为空".into()));
        }
        conn.execute("UPDATE crews SET working_directory=?2 WHERE id=?1", params![id, wd.trim()])?;
    }
    if let Some(h) = p.hidden {
        conn.execute("UPDATE crews SET hidden_at=?2 WHERE id=?1", params![id, h.then(|| now.clone())])?;
    }
    if let Some(line) = p.status_line {
        let line = line.trim().to_string();
        conn.execute("UPDATE crews SET status_line=?2 WHERE id=?1", params![id, (!line.is_empty()).then_some(line)])?;
    }
    if let Some(policy) = p.policy {
        conn.execute("UPDATE crews SET policy=?2 WHERE id=?1", params![id, policy.to_string()])?;
    }
    conn.execute("UPDATE crews SET updated_at=?2 WHERE id=?1", params![id, now])?;
    tx.note(Change::CrewChanged { crew_id: id.into() });
    get(tx.conn(), id)
}

/// 把 `crew`（整棵子树）挂到 `parent` 名下。父边 additive（DAG 多父保留），已是其子 → 不动。
pub fn adopt(tx: &mut Tx<'_>, crew: &str, parent: &str) -> Result<Crew> {
    refuse_builtin(tx.conn(), &[crew, parent])?;
    get(tx.conn(), crew)?;
    get(tx.conn(), parent)?;
    if crew == parent {
        return Err(CollabError::Invalid("机组不能挂在自己下面".into()));
    }
    if descendants(tx.conn(), crew)?.contains(parent) {
        return Err(CollabError::Invalid(format!("{parent} 是 {crew} 的下级，挂上去会成环")));
    }
    let ps = parents(tx.conn(), crew)?;
    if ps.iter().any(|p| p == parent) {
        return get(tx.conn(), crew);
    }
    tx.conn().execute(
        "INSERT INTO crew_parents(crew_id,parent_id) VALUES(?1,?2)",
        params![crew, parent],
    )?;
    tx.note(Change::CrewChanged { crew_id: crew.into() });
    tx.note(Change::CrewChanged { crew_id: parent.into() });
    get(tx.conn(), crew)
}

/// 把 `from` 的直系子 `crew` 摘出；`to` 有值 = 先挂新边（过 adopt 的全部检查）再摘旧边。
pub fn release(tx: &mut Tx<'_>, crew: &str, from: &str, to: Option<&str>) -> Result<Crew> {
    let mut ids = vec![crew, from];
    ids.extend(to);
    refuse_builtin(tx.conn(), &ids)?;
    get(tx.conn(), crew)?;
    if !parents(tx.conn(), crew)?.iter().any(|p| p == from) {
        return Err(CollabError::Invalid(format!("{crew} 不是 {from} 的直系子机组")));
    }
    if to == Some(from) {
        return get(tx.conn(), crew);
    }
    if let Some(t) = to {
        adopt(tx, crew, t)?;
    }
    tx.conn().execute(
        "DELETE FROM crew_parents WHERE crew_id=?1 AND parent_id=?2",
        params![crew, from],
    )?;
    tx.note(Change::CrewChanged { crew_id: crew.into() });
    tx.note(Change::CrewChanged { crew_id: from.into() });
    get(tx.conn(), crew)
}

/// 只能删非内建、且没有 desired=running session 的机组（spec §22.2）。子机组的父边随之消失。
pub fn delete(tx: &mut Tx<'_>, id: &str) -> Result<()> {
    let crew = get(tx.conn(), id)?;
    if crew.builtin {
        return Err(CollabError::Forbidden("内建机组不能删".into()));
    }
    let running: i64 = tx.conn().query_row(
        "SELECT COUNT(*) FROM sessions WHERE crew_id=?1 AND desired='running'",
        [id],
        |r| r.get(0),
    )?;
    if running > 0 {
        return Err(CollabError::Conflict(format!("机组 {id} 还有 {running} 个在跑的 session，先停掉")));
    }
    let around: Vec<String> = parents(tx.conn(), id)?.into_iter().chain(children(tx.conn(), id)?).collect();
    tx.conn().execute("DELETE FROM crews WHERE id=?1", [id])?;
    tx.note(Change::CrewRemoved { crew_id: id.into() });
    for c in around {
        tx.note(Change::CrewChanged { crew_id: c });
    }
    Ok(())
}

/// 换机长（数据部分；进程与世界观由 M3c / M3b 处理）：旧机长降为 worker，没号就领下一个分机号。
pub fn set_captain(tx: &mut Tx<'_>, crew: &str, session: &str) -> Result<()> {
    get(tx.conn(), crew)?;
    let s = crate::sessions::get(tx.conn(), session)?;
    if s.crew_id != crew {
        return Err(CollabError::Invalid(format!("{session} 不在机组 {crew} 里")));
    }
    let old = captain_of(tx.conn(), crew)?;
    if old.as_deref() == Some(session) {
        return Ok(());
    }
    let conn = tx.conn();
    if let Some(o) = &old {
        let has_ext: Option<i64> = conn.query_row("SELECT ext FROM sessions WHERE id=?1", [o], |r| r.get(0))?;
        if has_ext.is_none() {
            conn.execute(
                "UPDATE sessions SET ext=(SELECT next_ext FROM crews WHERE id=?2) WHERE id=?1",
                params![o, crew],
            )?;
            conn.execute("UPDATE crews SET next_ext=next_ext+1 WHERE id=?1", [crew])?;
        }
        conn.execute("UPDATE sessions SET role='worker' WHERE id=?1", [o])?;
    }
    conn.execute("UPDATE sessions SET role='captain' WHERE id=?1", [session])?;
    conn.execute("UPDATE crews SET captain_session_id=?2 WHERE id=?1", params![crew, session])?;
    if is_root(conn, crew)? {
        let human = crewd_store::channels::human_id(crew);
        let exists: bool = conn.query_row("SELECT COUNT(*) FROM channels WHERE id=?1", [&human], |r| r.get::<_, i64>(0))? > 0;
        if exists {
            cursors_pin(tx, session, &human)?;
            tx.note(Change::ChannelChanged { channel_id: human });
        }
    }
    tx.note(Change::CrewChanged { crew_id: crew.into() });
    if let Some(o) = old {
        tx.note(Change::SessionChanged { session_id: o });
    }
    tx.note(Change::SessionChanged { session_id: session.into() });
    Ok(())
}

fn cursors_pin(tx: &mut Tx<'_>, reader: &str, channel: &str) -> Result<()> {
    crate::cursors::pin_to_end(tx, reader, channel).map(|_| ())
}

/// 侧栏基础序（§22.7 Q4）：本机组自己的群里最新一条消息的时间降序；没消息的按建组时间；同时刻按号。
pub fn list_by_activity(conn: &Connection) -> Result<Vec<Crew>> {
    let mut crews = list(conn)?;
    for c in &mut crews {
        let mut st = conn.prepare("SELECT id FROM channels WHERE crew_id=?1")?;
        let chans: Vec<String> = st.query_map([&c.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        let mut best = 0;
        for ch in chans {
            best = best.max(crewd_store::messages::max_seq(conn, &ch)?);
        }
        if best > 0 {
            c.last_activity_at = Some(conn.query_row("SELECT created_at FROM messages WHERE seq=?1", [best], |r| r.get(0))?);
        }
    }
    crews.sort_by(|a, b| {
        let ka = a.last_activity_at.as_deref().unwrap_or(&a.created_at);
        let kb = b.last_activity_at.as_deref().unwrap_or(&b.created_at);
        kb.cmp(ka).then(a.number.cmp(&b.number)).then(a.id.cmp(&b.id))
    });
    Ok(crews)
}
```

`sessions.rs`，类型之后、测试之前：

```rust
pub fn valid_executor(e: &str) -> bool {
    e == "claude"
        || e == "codex"
        || e.strip_prefix("acp:").is_some_and(|id| {
            !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) && !id.contains("..")
        })
}

pub fn get(conn: &Connection, id: &str) -> Result<Session> {
    crewd_store::sessions::get(conn, id)?.ok_or_else(|| CollabError::NotFound(format!("没有 session {id}")))
}

pub fn list(conn: &Connection, crew: Option<&str>) -> Result<Vec<Session>> {
    Ok(match crew {
        Some(c) => crewd_store::sessions::list_for_crew(conn, c)?,
        None => crewd_store::sessions::list_all(conn)?,
    })
}

pub fn crew_of(conn: &Connection, id: &str) -> Result<String> {
    Ok(get(conn, id)?.crew_id)
}

/// 此刻的角色：以机组的机长指针为准（不看 role 列，防两者漂移时说错话）。
pub fn role_of(conn: &Connection, id: &str) -> Result<Role> {
    let crew = crew_of(conn, id)?;
    Ok(if crews::captain_of(conn, &crew)?.as_deref() == Some(id) { Role::Captain } else { Role::Worker })
}

/// 给人 / agent 看的名字：显示名 → 标题 → id。
pub fn display_name(conn: &Connection, id: &str) -> Result<String> {
    let s = get(conn, id)?;
    Ok(s.display_name.or(s.title).unwrap_or(s.id))
}

pub fn running_desired(conn: &Connection) -> Result<Vec<Session>> {
    Ok(list(conn, None)?.into_iter().filter(|s| s.desired == Desired::Running).collect())
}

pub fn register(tx: &mut Tx<'_>, n: NewSession) -> Result<Session> {
    let crew = crews::get(tx.conn(), &n.crew_id)?;
    if !valid_executor(&n.executor) {
        return Err(CollabError::Invalid(format!("执行器 {:?} 不认识（claude / codex / acp:<id>）", n.executor)));
    }
    if n.role == Role::Captain && crews::captain_of(tx.conn(), &crew.id)?.is_some() {
        return Err(CollabError::Conflict(format!("机组 {} 已有机长；换机长用 set_captain", crew.id)));
    }
    let conn = tx.conn();
    let ext = if n.role == Role::Worker {
        let e: i64 = conn.query_row("SELECT next_ext FROM crews WHERE id=?1", [&crew.id], |r| r.get(0))?;
        conn.execute("UPDATE crews SET next_ext=next_ext+1 WHERE id=?1", [&crew.id])?;
        Some(e)
    } else {
        None
    };
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    let s = Session {
        id: format!("{}-{}", n.role.as_str(), &uuid[..12]),
        crew_id: crew.id.clone(),
        role: n.role,
        executor: n.executor,
        display_name: n.display_name,
        working_directory: n.working_directory.or(crew.working_directory.clone()),
        model: n.model,
        effort: n.effort,
        updated_at: tx.now_ts(),
        title: n.title,
        brief: n.brief,
        desired: Desired::Stopped,
        ext,
        worktree: n.worktree,
        ..Default::default()
    };
    crewd_store::sessions::upsert(conn, &s, None)?;
    if n.role == Role::Captain {
        conn.execute("UPDATE crews SET captain_session_id=?2 WHERE id=?1", params![crew.id, s.id])?;
    }
    cursors::pin_to_end(tx, &s.id, &crewd_store::channels::lobby_id(&crew.id))?;
    tx.note(Change::SessionChanged { session_id: s.id.clone() });
    tx.note(Change::CrewChanged { crew_id: crew.id });
    get(tx.conn(), &s.id)
}

fn update_one(tx: &mut Tx<'_>, id: &str, sql: &str, p: impl rusqlite::Params, notify: bool) -> Result<()> {
    if tx.conn().execute(sql, p)? == 0 {
        return Err(CollabError::NotFound(format!("没有 session {id}")));
    }
    if notify {
        tx.note(Change::SessionChanged { session_id: id.into() });
    }
    Ok(())
}

pub fn set_desired(tx: &mut Tx<'_>, id: &str, d: Desired) -> Result<()> {
    let now = tx.now_ts();
    update_one(tx, id, "UPDATE sessions SET desired=?2, updated_at=?3 WHERE id=?1", params![id, d.as_str(), now], true)
}

/// 干活 / 空闲边沿（M3c 收到 `SessionEvent::Working` 时调）：记 was_working 与最后活跃时间。
pub fn set_working(tx: &mut Tx<'_>, id: &str, working: bool) -> Result<()> {
    let now = tx.now_ts();
    update_one(tx, id, "UPDATE sessions SET was_working=?2, last_active_at=?3 WHERE id=?1", params![id, working, now], true)
}

/// 恢复失败（M4b 的 `reassign_lost` 只认这一列，§19）；`false` = 清掉。
pub fn set_resume_failed(tx: &mut Tx<'_>, id: &str, failed: bool) -> Result<()> {
    let at = failed.then(|| tx.now_ts());
    update_one(tx, id, "UPDATE sessions SET resume_failed_at=?2 WHERE id=?1", params![id, at], true)
}

pub fn set_awaiting_question(tx: &mut Tx<'_>, id: &str, q: Option<String>) -> Result<()> {
    update_one(tx, id, "UPDATE sessions SET awaiting_question=?2 WHERE id=?1", params![id, q], true)
}

pub fn set_profile(tx: &mut Tx<'_>, id: &str, model: Option<String>, effort: Option<String>) -> Result<()> {
    let now = tx.now_ts();
    update_one(
        tx,
        id,
        "UPDATE sessions SET model=COALESCE(?2, model), effort=COALESCE(?3, effort), updated_at=?4 WHERE id=?1",
        params![id, model, effort, now],
        true,
    )
}

/// 「有新产出」（§22.7 Q5 熔断判据）：session 发言时 `messages::post` 自动调；M3b 的 post-tool-use hook、
/// M4b 的工作项命令也调它。高频，不发变更。
pub fn note_output(tx: &mut Tx<'_>, id: &str) -> Result<()> {
    let now = tx.now_ts();
    update_one(tx, id, "UPDATE sessions SET last_output_at=?2 WHERE id=?1", params![id, now], false)
}
```

注意 `set_working` 写 `last_active_at` 与 `note_output` 写 `last_output_at` 都是 `tx.now_ts()`，测试里两者相等是因为同一个 NOW。

`directory.rs`，测试之前：

```rust
/// 组织路径：从根到 `crew` 的**上级**，每层取最早建的那个父（多父时与 `root_of` 同一口径），" / " 分隔。
fn org_path(conn: &Connection, crew: &str) -> Result<String> {
    let mut titles = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut cur = crew.to_string();
    while let Some(p) = crews::parents(conn, &cur)?.into_iter().next() {
        if !seen.insert(p.clone()) {
            break;
        }
        titles.push(crews::get(conn, &p)?.title);
        cur = p;
    }
    titles.reverse();
    Ok(titles.join(" / "))
}

pub fn list(conn: &Connection, query: Option<&str>) -> Result<Vec<DirectoryRow>> {
    let mut all = Vec::new();
    let mut numbered: Vec<_> = crews::list(conn)?.into_iter().filter(|c| c.number.is_some()).collect();
    numbered.sort_by_key(|c| c.number);
    for c in numbered {
        let n = c.number.expect("filtered");
        let path = org_path(conn, &c.id)?;
        let self_path = if path.is_empty() { c.title.clone() } else { format!("{path} / {}", c.title) };
        all.push(DirectoryRow { number: n.to_string(), kind: DirectoryKind::Crew, name: c.title.clone(), crew_id: c.id.clone(), session_id: None, org_path: path });
        let captain = crews::captain_of(conn, &c.id)?;
        if let Some(cap) = &captain {
            all.push(DirectoryRow {
                number: format!("{n}-1"),
                kind: DirectoryKind::Session,
                name: sessions::display_name(conn, cap)?,
                crew_id: c.id.clone(),
                session_id: Some(cap.clone()),
                org_path: self_path.clone(),
            });
        }
        let mut members: Vec<_> = sessions::list(conn, Some(&c.id))?
            .into_iter()
            .filter(|s| s.ext.is_some() && Some(&s.id) != captain.as_ref())
            .collect();
        members.sort_by_key(|s| s.ext);
        for s in members {
            all.push(DirectoryRow {
                number: format!("{n}-{}", s.ext.expect("filtered")),
                kind: DirectoryKind::Session,
                name: s.display_name.clone().or(s.title.clone()).unwrap_or(s.id.clone()),
                crew_id: c.id.clone(),
                session_id: Some(s.id),
                org_path: self_path.clone(),
            });
        }
    }
    let q = query.unwrap_or("").trim().to_lowercase();
    if q.is_empty() {
        return Ok(all);
    }
    let hit = |r: &DirectoryRow| {
        r.number.starts_with(&q) || r.name.to_lowercase().contains(&q) || r.org_path.to_lowercase().contains(&q)
    };
    let hit_crews: std::collections::BTreeSet<String> = all.iter().filter(|r| hit(r)).map(|r| r.crew_id.clone()).collect();
    Ok(all
        .into_iter()
        .filter(|r| hit(r) || (r.kind == DirectoryKind::Crew && hit_crews.contains(&r.crew_id)))
        .collect())
}
```

（`query = "3"` 命中 `3`、`3-1`；`1-1` 这类号码前缀与 Swift 同。注意 `"2"` 也会命中 `1-2` 吗？`starts_with("2")` 不会——`1-2` 以 `1` 开头。测试只钉了 `"3"`。）

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-collab`
Expected: Task 5 的 13 条 + crews 9 + sessions 5 + directory 2 = 29 passed。

- [ ] **Step 5: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/collab/src/lib.rs crates/collab/src/crews.rs crates/collab/src/sessions.rs crates/collab/src/directory.rs crates/collab/src/cursors.rs
git diff --cached --stat
git commit -m "crewd-collab: crews (root, captain pointer, DAG, numbering), session rows and lifecycle fields, directory

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 7: 群、成员、群主、人类群、注视、列表与摘要

spec §17 + §22.2「群规则」+ §22.7 Q2（群主）。成员判定一处写死（`is_member` / `members`）：大厅 = 本机组全部 session + 人（隐式）；人类群 = 人 + 根机组机长（隐式）+ 显式行；topic / dm 全部显式。群主：topic / dm 存在列里（默认建群者）；大厅 / 人类群存 NULL、读的时候现算成归属机组此刻的机长。转让 = 原群主发起 + 新群主接受，或机长 / 人强制。根机组被收编 → 它的人类群归档；重新成为根 → `ensure_human_channel` 解档。红标在本任务只给 `cursors::badge`（摘要要用），未读与可见性在 Task 9。

**Files:**
- Create: `crewd/crates/collab/src/channels.rs`
- Modify: `crewd/crates/collab/src/lib.rs`（`pub mod channels;`）、`crewd/crates/collab/src/cursors.rs`（`badge`）、`crewd/crates/collab/src/crews.rs`（`adopt` / `release` 接人类群；加 `detail`）、`crewd/crates/collab/src/testkit.rs`（`msg` 助手）

**Interfaces:**
- Consumes: Task 6。
- Produces：
  - `channels::{NewChannel { crew_id, kind, name, purpose, members: Vec<MemberRef> }, ChannelPatch { name, purpose, archived }, create(tx, by: &Sender, NewChannel) -> Result<Channel>, ensure_dm(tx, crew, a: &MemberRef, b: &MemberRef) -> Result<(Channel, bool)>, get(conn, id) -> Result<Channel>, update(tx, by: &Sender, id, ChannelPatch) -> Result<Channel>, add_members(tx, by: &Sender, id, &[MemberRef]) -> Result<Channel>, remove_member(tx, by: &Sender, id, &MemberRef) -> Result<Channel>, watch(tx, id, on: bool) -> Result<ChannelSummary>, is_watched(conn, id) -> Result<bool>, ensure_human_channel(tx, root) -> Result<Channel>, is_member(conn, id, &MemberRef) -> Result<bool>, members(conn, id) -> Result<Vec<MemberRef>>, channels_of_session(conn, session) -> Result<Vec<Channel>>, list_for_crew(tx, crew) -> Result<Vec<ChannelSummary>>, summary(conn, id) -> Result<ChannelSummary>, owner_of(conn, id) -> Result<Option<MemberRef>>, request_owner_transfer(tx, by, id, to: &MemberRef) -> Result<Channel>, accept_owner_transfer(tx, by, id) -> Result<Channel>, withdraw_owner_transfer(tx, by, id) -> Result<Channel>, force_owner(tx, by, id, to: &MemberRef) -> Result<Channel>}`；`pub(crate) fn archive_human_of(tx, crew)`。
  - `messages::Sender`（本任务先在 `channels.rs` 里用，定义放 `lib.rs`：`pub enum Sender { Human, Session(String), System }` + `fn member(&self) -> Option<MemberRef>`），Task 8 起 `messages` 也用它。
  - `cursors::badge(conn, channel) -> Result<Badge>`；`crews::detail(tx, id) -> Result<CrewDetail>`。
  - testkit：`msg(s: &Store, channel, id, sender_kind, sender_session: Option<&str>, mentions: Vec<Mention>) -> i64`（直接插一条，返回 seq）。

- [ ] **Step 1: 写失败的测试**

`lib.rs` 加（`Locale` 之后）：

```rust
/// 谁在做这次操作（发消息、建群、改群）。机长 / worker 由 collab 按机长指针现查。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sender {
    Human,
    Session(String),
    /// crewd 自己（公告、系统消息）。
    System,
}

impl Sender {
    pub fn member(&self) -> Option<crewd_proto::MemberRef> {
        match self {
            Sender::Human => Some(crewd_proto::MemberRef::Human),
            Sender::Session(id) => Some(crewd_proto::MemberRef::session(id.clone())),
            Sender::System => None,
        }
    }
}
```

`testkit.rs` 末尾加：

```rust
/// 直接往群里插一条消息（绕过 collab 的规则，只为造测试前提），返回 seq。
pub(crate) fn msg(
    s: &Store,
    channel: &str,
    id: &str,
    sender_kind: crewd_proto::SenderKind,
    sender_session: Option<&str>,
    mentions: Vec<crewd_proto::Mention>,
) -> i64 {
    crewd_store::messages::insert(
        s.conn(),
        &crewd_proto::Message {
            id: id.into(),
            channel_id: channel.into(),
            sender_kind,
            sender_session_id: sender_session.map(Into::into),
            text: format!("合成消息 {id}"),
            mentions,
            created_at: crewd_store::ts_from_ms(NOW),
            ..Default::default()
        },
        None,
    )
    .unwrap()
    .unwrap()
}
```

`crates/collab/src/channels.rs`（文件头 + 类型 + 测试；实现在 Step 3）：

```rust
//! 群（spec §17、§22.2「群规则」、§22.7 Q2 群主）。成员判定、群主、人类群懒建都只在这里写一次。
use crate::{Change, CollabError, Result, Sender, Tx, crews, cursors, sessions};
use crewd_proto::{Channel, ChannelKind, ChannelSummary, MemberRef};
use crewd_store::channels::{human_id, lobby_id};
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewChannel {
    pub crew_id: String,
    /// 只能是 Topic 或 Dm（大厅、人类群由 crewd 建）。
    pub kind: ChannelKind,
    pub name: String,
    pub purpose: Option<String>,
    /// 建群者自动入群，不用列。
    pub members: Vec<MemberRef>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelPatch {
    pub name: Option<String>,
    pub purpose: Option<String>,
    pub archived: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, count, err, msg, ok, ok_at, store};
    use crewd_proto::SenderKind;

    fn s(id: &str) -> MemberRef {
        MemberRef::session(id)
    }
    fn topic(crew: &str, members: Vec<MemberRef>) -> NewChannel {
        NewChannel { crew_id: crew.into(), kind: ChannelKind::Topic, name: "发版".into(), purpose: Some("排期".into()), members }
    }

    #[test]
    fn creator_joins_and_owns_a_topic() {
        let mut s_ = store();
        let ch = ok(&mut s_, |tx| create(tx, &Sender::Session("w-a1".into()), topic("crew-a", vec![s("w-a2")])));
        assert!(ch.id.starts_with("ch-"));
        assert_eq!(ch.owner, Some(s("w-a1")));
        assert_eq!(ch.created_by, "w-a1");
        let mut m = members(s_.conn(), &ch.id).unwrap();
        m.sort();
        assert_eq!(m, [s("w-a1"), s("w-a2")]);
        assert!(!is_member(s_.conn(), &ch.id, &MemberRef::Human).unwrap());
        assert!(matches!(
            err(&mut s_, |tx| create(tx, &Sender::Session("w-a1".into()), NewChannel { name: " ".into(), ..topic("crew-a", vec![]) })),
            CollabError::Invalid(_)
        ));
        assert!(matches!(
            err(&mut s_, |tx| create(tx, &Sender::Session("w-a1".into()), NewChannel { kind: ChannelKind::Lobby, ..topic("crew-a", vec![]) })),
            CollabError::Invalid(_)
        ));
        assert!(matches!(
            err(&mut s_, |tx| create(tx, &Sender::Session("w-a1".into()), topic("crew-a", vec![s("ghost")]))),
            CollabError::NotFound(_)
        ));
    }

    #[test]
    fn dm_is_exactly_two_and_cannot_grow() {
        let mut s_ = store();
        let dm = NewChannel { kind: ChannelKind::Dm, name: String::new(), purpose: None, ..topic("crew-a", vec![s("w-b1")]) };
        let ch = ok(&mut s_, |tx| create(tx, &Sender::Session("w-a1".into()), dm.clone()));
        assert!(matches!(err(&mut s_, |tx| add_members(tx, &Sender::Session("w-a1".into()), &ch.id, &[s("w-a2")])), CollabError::Invalid(_)));
        assert!(matches!(
            err(&mut s_, |tx| create(tx, &Sender::Session("w-a1".into()), NewChannel { members: vec![s("w-a2"), s("w-b1")], ..dm.clone() })),
            CollabError::Invalid(_)
        ));
        assert!(matches!(
            err(&mut s_, |tx| create(tx, &Sender::Session("w-a1".into()), NewChannel { members: vec![], ..dm.clone() })),
            CollabError::Invalid(_)
        ));
        let (again, created) = ok(&mut s_, |tx| ensure_dm(tx, "crew-a", &s("w-b1"), &s("w-a1")));
        assert_eq!((again.id.as_str(), created), (ch.id.as_str(), false), "同一对人复用同一个 dm");
        let human_dm = ok(&mut s_, |tx| create(tx, &Sender::Human, NewChannel { members: vec![s("w-a1")], ..dm.clone() }));
        assert_eq!(human_dm.owner, Some(MemberRef::Human));
    }

    #[test]
    fn lobby_and_human_channel_membership_is_implicit() {
        let mut s_ = store();
        let lobby = "crew-a/lobby";
        assert!(is_member(s_.conn(), lobby, &s("w-a1")).unwrap());
        assert!(is_member(s_.conn(), lobby, &MemberRef::Human).unwrap());
        assert!(!is_member(s_.conn(), lobby, &s("w-b1")).unwrap(), "子机组的人不在父机组大厅");
        let h = ok(&mut s_, |tx| ensure_human_channel(tx, "crew-a"));
        assert_eq!(h.id, "crew-a/human");
        assert_eq!(h.kind, ChannelKind::Human);
        assert!(is_member(s_.conn(), &h.id, &s("cap-a")).unwrap(), "根机组机长隐式在人类群");
        assert!(!is_member(s_.conn(), &h.id, &s("w-a1")).unwrap());
        assert!(matches!(err(&mut s_, |tx| ensure_human_channel(tx, "crew-b")), CollabError::Invalid(_)), "只有根机组有人类群");
        assert_eq!(get(s_.conn(), lobby).unwrap().owner, Some(s("cap-a")), "大厅群主 = 机长");
        ok(&mut s_, |tx| crate::crews::set_captain(tx, "crew-a", "w-a1"));
        assert_eq!(get(s_.conn(), &h.id).unwrap().owner, Some(s("w-a1")), "跟着机长走");
        assert!(is_member(s_.conn(), &h.id, &s("w-a1")).unwrap());
    }

    #[test]
    fn channel_list_shows_cross_crew_channels_under_every_members_crew() {
        let mut s_ = store();
        let ch = ok(&mut s_, |tx| create(tx, &Sender::Session("w-a1".into()), topic("crew-a", vec![s("cap-x")])));
        let ids = |crew: &str, s_: &mut crewd_store::Store| -> Vec<String> {
            ok(s_, |tx| list_for_crew(tx, crew)).into_iter().map(|c| c.channel.id).collect()
        };
        assert_eq!(ids("crew-a", &mut s_), ["crew-a/lobby", "crew-a/human", ch.id.as_str()]);
        assert_eq!(ids("crew-x", &mut s_), ["crew-x/lobby", "crew-x/human", ch.id.as_str()], "跨机组成员所在的机组下也显示");
        assert_eq!(ids("crew-b", &mut s_), ["crew-b/lobby"], "非根机组没有人类群");
    }

    #[test]
    fn joining_a_topic_starts_at_the_end() {
        let mut s_ = store();
        let ch = ok(&mut s_, |tx| create(tx, &Sender::Session("w-a1".into()), topic("crew-a", vec![])));
        let seq = msg(&s_, &ch.id, "m1", SenderKind::Session, Some("w-a1"), vec![]);
        ok(&mut s_, |tx| add_members(tx, &Sender::Session("w-a1".into()), &ch.id, &[s("w-a2")]));
        assert_eq!(crate::cursors::cursor(s_.conn(), "w-a2", &ch.id).unwrap(), Some(seq));
        ok(&mut s_, |tx| remove_member(tx, &Sender::Session("w-a2".into()), &ch.id, &s("w-a2")));
        assert!(!is_member(s_.conn(), &ch.id, &s("w-a2")).unwrap());
        let seq2 = msg(&s_, &ch.id, "m2", SenderKind::Session, Some("w-a1"), vec![]);
        ok(&mut s_, |tx| add_members(tx, &Sender::Session("w-a1".into()), &ch.id, &[s("w-a2")]));
        assert_eq!(crate::cursors::cursor(s_.conn(), "w-a2", &ch.id).unwrap(), Some(seq2), "重新入群也从末尾开始");
    }

    #[test]
    fn only_owner_captain_or_human_may_archive_and_lobby_cannot_be_archived() {
        let mut s_ = store();
        let ch = ok(&mut s_, |tx| create(tx, &Sender::Session("w-a1".into()), topic("crew-a", vec![s("w-a2")])));
        let arch = ChannelPatch { archived: Some(true), ..Default::default() };
        assert!(matches!(err(&mut s_, |tx| update(tx, &Sender::Session("w-a2".into()), &ch.id, arch.clone())), CollabError::Forbidden(_)));
        ok(&mut s_, |tx| update(tx, &Sender::Session("cap-a".into()), &ch.id, arch.clone()));
        assert!(get(s_.conn(), &ch.id).unwrap().archived_at.is_some());
        ok(&mut s_, |tx| update(tx, &Sender::Human, &ch.id, ChannelPatch { archived: Some(false), ..Default::default() }));
        assert!(matches!(err(&mut s_, |tx| update(tx, &Sender::Human, "crew-a/lobby", arch.clone())), CollabError::Invalid(_)));
        assert!(matches!(err(&mut s_, |tx| remove_member(tx, &Sender::Session("w-a1".into()), &ch.id, &s("w-a1"))), CollabError::Invalid(_)), "群主先转让再走");
    }

    #[test]
    fn owner_transfer_needs_both_sides_or_force() {
        let mut s_ = store();
        let ch = ok(&mut s_, |tx| create(tx, &Sender::Session("w-a1".into()), topic("crew-a", vec![s("w-a2"), s("cap-x")])));
        let w1 = Sender::Session("w-a1".into());
        let w2 = Sender::Session("w-a2".into());
        assert!(matches!(err(&mut s_, |tx| request_owner_transfer(tx, &w2, &ch.id, &s("w-a2"))), CollabError::Forbidden(_)), "只有群主能发起");
        assert!(matches!(err(&mut s_, |tx| request_owner_transfer(tx, &w1, &ch.id, &s("w-b1"))), CollabError::Invalid(_)), "新群主必须是成员");
        let c = ok(&mut s_, |tx| request_owner_transfer(tx, &w1, &ch.id, &s("w-a2")));
        assert_eq!(c.pending_owner, Some(s("w-a2")));
        assert!(matches!(err(&mut s_, |tx| accept_owner_transfer(tx, &Sender::Session("cap-x".into()), &ch.id)), CollabError::Forbidden(_)));
        let c = ok(&mut s_, |tx| accept_owner_transfer(tx, &w2, &ch.id));
        assert_eq!((c.owner, c.pending_owner), (Some(s("w-a2")), None));
        assert!(matches!(err(&mut s_, |tx| force_owner(tx, &w1, &ch.id, &s("w-a1"))), CollabError::Forbidden(_)), "worker 不能强制");
        let c = ok(&mut s_, |tx| force_owner(tx, &Sender::Session("cap-a".into()), &ch.id, &s("cap-x")));
        assert_eq!(c.owner, Some(s("cap-x")), "归属机组的机长可强制");
        let c = ok(&mut s_, |tx| force_owner(tx, &Sender::Human, &ch.id, &s("w-a1")));
        assert_eq!(c.owner, Some(s("w-a1")));
        assert!(matches!(err(&mut s_, |tx| request_owner_transfer(tx, &Sender::Session("cap-a".into()), "crew-a/lobby", &s("w-a1"))), CollabError::Invalid(_)), "大厅群主跟着机长走");
    }

    #[test]
    fn adoption_archives_the_human_channel_and_release_brings_it_back() {
        let mut s_ = store();
        ok(&mut s_, |tx| ensure_human_channel(tx, "crew-x"));
        ok(&mut s_, |tx| crate::crews::adopt(tx, "crew-x", "crew-a"));
        assert!(get(s_.conn(), "crew-x/human").unwrap().archived_at.is_some());
        ok(&mut s_, |tx| crate::crews::release(tx, "crew-x", "crew-a", None));
        let ids: Vec<String> = ok(&mut s_, |tx| list_for_crew(tx, "crew-x")).into_iter().map(|c| c.channel.id).collect();
        assert!(ids.contains(&"crew-x/human".to_string()));
        assert!(get(s_.conn(), "crew-x/human").unwrap().archived_at.is_none(), "又成了根 → 解档");
    }

    #[test]
    fn summary_carries_watch_and_badges() {
        let mut s_ = store();
        msg(&s_, "crew-a/lobby", "m1", SenderKind::Session, Some("w-a1"), vec![crewd_proto::Mention { kind: crewd_proto::MentionKind::Human, target_id: None }]);
        msg(&s_, "crew-a/lobby", "m2", SenderKind::Human, None, vec![]);
        let (sum, ch) = ok_at(&mut s_, NOW, |tx| watch(tx, "crew-a/lobby", true));
        assert!(sum.watched);
        assert_eq!((sum.unread_for_human, sum.human_mentions), (1, 1), "人自己发的不算人的未读");
        assert_eq!(ch, [Change::ChannelChanged { channel_id: "crew-a/lobby".into() }]);
        assert_eq!(count(&s_, "SELECT COUNT(*) FROM channel_watch"), 1);
        ok(&mut s_, |tx| watch(tx, "crew-a/lobby", false));
        assert!(!is_watched(s_.conn(), "crew-a/lobby").unwrap());
    }

    #[test]
    fn crew_detail_collects_root_children_channels_and_sessions() {
        let mut s_ = store();
        let d = ok(&mut s_, |tx| crate::crews::detail(tx, "crew-b"));
        assert_eq!(d.root_id, "crew-a");
        assert_eq!(d.sessions.len(), 2);
        assert_eq!(d.channels.len(), 1);
        let d = ok(&mut s_, |tx| crate::crews::detail(tx, "crew-a"));
        assert_eq!(d.children, ["crew-b"]);
    }
}
```

`cursors.rs` 加测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{msg, store};
    use crewd_proto::{Mention, MentionKind, SenderKind};

    #[test]
    fn badge_counts_unread_and_human_mentions_after_the_human_cursor() {
        let s = store();
        msg(&s, "crew-a/lobby", "m1", SenderKind::Session, Some("w-a1"), vec![]);
        msg(&s, "crew-a/lobby", "m2", SenderKind::Session, Some("w-a1"), vec![Mention { kind: MentionKind::Human, target_id: None }]);
        let b = badge(s.conn(), "crew-a/lobby").unwrap();
        assert_eq!((b.unread_for_human, b.human_mentions), (2, 1));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-collab channels:: cursors::`
Expected: 编译失败。

- [ ] **Step 3: 实现**

`cursors.rs`：

```rust
/// 人类在某群的红标（spec §22.2：按游标派生）。人自己发的不算；缺游标 = 0（新群一开始是空的）。
pub fn badge(conn: &Connection, channel: &str) -> Result<crewd_proto::Badge> {
    let after = cursor(conn, HUMAN, channel)?.unwrap_or(0);
    let (unread, mentions): (i64, i64) = conn.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(EXISTS(SELECT 1 FROM json_each(m.mentions) j
                                     WHERE json_extract(j.value,'$.kind')='human')),0)
           FROM messages m
          WHERE m.channel_id=?1 AND m.seq>?2 AND m.sender_kind<>'human'",
        params![channel, after],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(crewd_proto::Badge {
        channel_id: channel.into(),
        unread_for_human: unread,
        human_mentions: mentions,
    })
}
```

`channels.rs`，类型之后：

```rust
const COLS: &str = "id,crew_id,kind,name,purpose,created_by,owner_kind,owner_id,pending_owner_kind,pending_owner_id,created_at,archived_at";

struct Row {
    ch: Channel,
    owner: Option<MemberRef>,
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    let kind: String = r.get(2)?;
    let member = |k: Option<String>, id: Option<String>| match (k, id) {
        (Some(k), Some(id)) => MemberRef::from_row(&k, &id),
        _ => None,
    };
    Ok(Row {
        owner: member(r.get(6)?, r.get(7)?),
        ch: Channel {
            id: r.get(0)?,
            crew_id: r.get(1)?,
            kind: ChannelKind::parse(&kind).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, format!("channels.kind = {kind:?}").into())
            })?,
            name: r.get(3)?,
            purpose: r.get(4)?,
            created_by: r.get(5)?,
            owner: None,
            pending_owner: member(r.get(8)?, r.get(9)?),
            created_at: r.get(10)?,
            archived_at: r.get(11)?,
        },
    })
}

pub fn get(conn: &Connection, id: &str) -> Result<Channel> {
    let r = conn
        .query_row(&format!("SELECT {COLS} FROM channels WHERE id=?1"), [id], row)
        .optional()?
        .ok_or_else(|| CollabError::NotFound(format!("没有群 {id}")))?;
    let mut ch = r.ch;
    ch.owner = match ch.kind {
        ChannelKind::Lobby | ChannelKind::Human => crews::captain_of(conn, &ch.crew_id)?.map(MemberRef::session),
        _ => r.owner,
    };
    Ok(ch)
}

pub fn owner_of(conn: &Connection, id: &str) -> Result<Option<MemberRef>> {
    Ok(get(conn, id)?.owner)
}

fn explicit(conn: &Connection, id: &str) -> Result<Vec<MemberRef>> {
    let mut st = conn.prepare(
        "SELECT member_kind, member_id FROM channel_members WHERE channel_id=?1 AND left_at IS NULL ORDER BY joined_at, member_id",
    )?;
    let raw = st
        .query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(raw.into_iter().filter_map(|(k, i)| MemberRef::from_row(&k, &i)).collect())
}

pub fn members(conn: &Connection, id: &str) -> Result<Vec<MemberRef>> {
    let ch = get(conn, id)?;
    let mut out = match ch.kind {
        ChannelKind::Lobby => {
            let mut v = vec![MemberRef::Human];
            v.extend(sessions::list(conn, Some(&ch.crew_id))?.into_iter().map(|s| MemberRef::session(s.id)));
            v
        }
        ChannelKind::Human => {
            let mut v = vec![MemberRef::Human];
            v.extend(crews::captain_of(conn, &ch.crew_id)?.map(MemberRef::session));
            v.extend(explicit(conn, id)?);
            v
        }
        ChannelKind::Topic | ChannelKind::Dm => explicit(conn, id)?,
    };
    let mut seen = std::collections::BTreeSet::new();
    out.retain(|m| seen.insert(m.clone()));
    Ok(out)
}

pub fn is_member(conn: &Connection, id: &str, m: &MemberRef) -> Result<bool> {
    Ok(members(conn, id)?.contains(m))
}

pub fn is_watched(conn: &Connection, id: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT COUNT(*) FROM channel_watch WHERE channel_id=?1", [id], |r| r.get::<_, i64>(0))? > 0)
}

fn sender_ref(by: &Sender) -> Result<MemberRef> {
    by.member().ok_or_else(|| CollabError::Invalid("系统不能以成员身份做这件事".into()))
}

fn check_member_exists(conn: &Connection, m: &MemberRef) -> Result<()> {
    if let MemberRef::Session { id } = m {
        sessions::get(conn, id)?;
    }
    Ok(())
}

fn join(tx: &mut Tx<'_>, id: &str, m: &MemberRef) -> Result<()> {
    let (k, mid) = m.to_row();
    tx.conn().execute(
        "INSERT INTO channel_members(channel_id,member_kind,member_id,joined_at) VALUES(?1,?2,?3,?4)
         ON CONFLICT(channel_id,member_kind,member_id) DO UPDATE SET left_at=NULL, joined_at=excluded.joined_at
           WHERE channel_members.left_at IS NOT NULL",
        params![id, k, mid, tx.now_ts()],
    )?;
    let reader = match m {
        MemberRef::Session { id: s } => s.clone(),
        MemberRef::Human => cursors::HUMAN.to_string(),
    };
    cursors::pin_to_end(tx, &reader, id)?;
    Ok(())
}

pub fn create(tx: &mut Tx<'_>, by: &Sender, n: NewChannel) -> Result<Channel> {
    let creator = sender_ref(by)?;
    crews::get(tx.conn(), &n.crew_id)?;
    if !matches!(n.kind, ChannelKind::Topic | ChannelKind::Dm) {
        return Err(CollabError::Invalid("只能建 topic 或 dm；大厅与人类群由 crewd 建".into()));
    }
    let mut all = vec![creator.clone()];
    for m in n.members {
        check_member_exists(tx.conn(), &m)?;
        if !all.contains(&m) {
            all.push(m);
        }
    }
    let name = n.name.trim().to_string();
    if n.kind == ChannelKind::Topic && name.is_empty() {
        return Err(CollabError::Invalid("群名不能为空".into()));
    }
    if n.kind == ChannelKind::Dm {
        let humans = all.iter().filter(|m| **m == MemberRef::Human).count();
        if all.len() != 2 || humans > 1 {
            return Err(CollabError::Invalid("私聊恰好两个人（至多一方是人类）".into()));
        }
    }
    let id = format!("ch-{}", uuid::Uuid::new_v4().simple());
    let (ok_kind, ok_id) = creator.to_row();
    let created_by = match by {
        Sender::Session(s) => s.clone(),
        _ => "human".to_string(),
    };
    tx.conn().execute(
        "INSERT INTO channels(id,crew_id,kind,name,purpose,created_by,owner_kind,owner_id,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![id, n.crew_id, n.kind.as_str(), name, n.purpose, created_by, ok_kind, ok_id, tx.now_ts()],
    )?;
    for m in &all {
        join(tx, &id, m)?;
    }
    tx.note(Change::ChannelChanged { channel_id: id.clone() });
    get(tx.conn(), &id)
}

/// 两人之间的 dm：已有（未归档、恰好这两人）就复用，否则以 `a` 的身份新建在 `crew` 下。返回 (群, 是否新建)。
pub fn ensure_dm(tx: &mut Tx<'_>, crew: &str, a: &MemberRef, b: &MemberRef) -> Result<(Channel, bool)> {
    let mut st = tx.conn().prepare("SELECT id FROM channels WHERE kind='dm' AND archived_at IS NULL ORDER BY created_at")?;
    let dms: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    for id in dms {
        let mut m = explicit(tx.conn(), &id)?;
        m.sort();
        let mut want = vec![a.clone(), b.clone()];
        want.sort();
        if m == want {
            return Ok((get(tx.conn(), &id)?, false));
        }
    }
    let by = match a {
        MemberRef::Session { id } => Sender::Session(id.clone()),
        MemberRef::Human => Sender::Human,
    };
    let ch = create(tx, &by, NewChannel { crew_id: crew.into(), kind: ChannelKind::Dm, name: String::new(), purpose: None, members: vec![b.clone()] })?;
    Ok((ch, true))
}

/// 能管这个群的：人、归属机组的机长、群主。
fn may_manage(conn: &Connection, by: &Sender, ch: &Channel) -> Result<bool> {
    Ok(match by {
        Sender::Human => true,
        Sender::System => false,
        Sender::Session(s) => {
            crews::captain_of(conn, &ch.crew_id)?.as_deref() == Some(s.as_str())
                || ch.owner == Some(MemberRef::session(s.clone()))
        }
    })
}

pub fn update(tx: &mut Tx<'_>, by: &Sender, id: &str, p: ChannelPatch) -> Result<Channel> {
    let ch = get(tx.conn(), id)?;
    if !may_manage(tx.conn(), by, &ch)? {
        return Err(CollabError::Forbidden("只有群主、机长或人能改这个群".into()));
    }
    let fixed = matches!(ch.kind, ChannelKind::Lobby | ChannelKind::Human);
    if let Some(name) = p.name {
        if fixed {
            return Err(CollabError::Invalid("大厅 / 人类群不能改名".into()));
        }
        let name = name.trim().to_string();
        if name.is_empty() && ch.kind == ChannelKind::Topic {
            return Err(CollabError::Invalid("群名不能为空".into()));
        }
        tx.conn().execute("UPDATE channels SET name=?2 WHERE id=?1", params![id, name])?;
    }
    if let Some(purpose) = p.purpose {
        let purpose = purpose.trim().to_string();
        tx.conn().execute("UPDATE channels SET purpose=?2 WHERE id=?1", params![id, (!purpose.is_empty()).then_some(purpose)])?;
    }
    if let Some(a) = p.archived {
        if fixed {
            return Err(CollabError::Invalid("大厅 / 人类群不能手动归档".into()));
        }
        tx.conn().execute("UPDATE channels SET archived_at=?2 WHERE id=?1", params![id, a.then(|| tx.now_ts())])?;
    }
    tx.note(Change::ChannelChanged { channel_id: id.into() });
    get(tx.conn(), id)
}

pub fn add_members(tx: &mut Tx<'_>, by: &Sender, id: &str, ms: &[MemberRef]) -> Result<Channel> {
    let ch = get(tx.conn(), id)?;
    match ch.kind {
        ChannelKind::Lobby => return Err(CollabError::Invalid("大厅成员 = 本机组全体，不能手动加".into())),
        ChannelKind::Dm => return Err(CollabError::Invalid("私聊不能加人".into())),
        _ => {}
    }
    if ch.archived_at.is_some() {
        return Err(CollabError::Invalid("群已归档".into()));
    }
    let me = sender_ref(by)?;
    if me != MemberRef::Human && !is_member(tx.conn(), id, &me)? {
        return Err(CollabError::Forbidden("只有群成员能拉人".into()));
    }
    for m in ms {
        check_member_exists(tx.conn(), m)?;
        join(tx, id, m)?;
    }
    tx.note(Change::ChannelChanged { channel_id: id.into() });
    get(tx.conn(), id)
}

pub fn remove_member(tx: &mut Tx<'_>, by: &Sender, id: &str, m: &MemberRef) -> Result<Channel> {
    let ch = get(tx.conn(), id)?;
    if ch.kind == ChannelKind::Lobby {
        return Err(CollabError::Invalid("大厅成员 = 本机组全体，不能移出".into()));
    }
    let me = sender_ref(by)?;
    if me != *m && !may_manage(tx.conn(), by, &ch)? {
        return Err(CollabError::Forbidden("只能自己退群，或由群主 / 机长 / 人移出".into()));
    }
    if ch.owner.as_ref() == Some(m) && ch.kind == ChannelKind::Topic {
        return Err(CollabError::Invalid("群主要先把群主转让出去".into()));
    }
    let (k, mid) = m.to_row();
    let n = tx.conn().execute(
        "UPDATE channel_members SET left_at=?4 WHERE channel_id=?1 AND member_kind=?2 AND member_id=?3 AND left_at IS NULL",
        params![id, k, mid, tx.now_ts()],
    )?;
    if n == 0 {
        return Err(CollabError::NotFound("不在这个群里（隐式成员不能移出）".into()));
    }
    tx.note(Change::ChannelChanged { channel_id: id.into() });
    get(tx.conn(), id)
}

pub fn watch(tx: &mut Tx<'_>, id: &str, on: bool) -> Result<ChannelSummary> {
    get(tx.conn(), id)?;
    if on {
        tx.conn().execute("INSERT OR IGNORE INTO channel_watch(channel_id,since) VALUES(?1,?2)", params![id, tx.now_ts()])?;
    } else {
        tx.conn().execute("DELETE FROM channel_watch WHERE channel_id=?1", [id])?;
    }
    tx.note(Change::ChannelChanged { channel_id: id.into() });
    summary(tx.conn(), id)
}

/// 根机组人类群（懒建，spec §22.2）。已归档（根曾被收编、后又独立）→ 解档。
pub fn ensure_human_channel(tx: &mut Tx<'_>, root: &str) -> Result<Channel> {
    crews::get(tx.conn(), root)?;
    if !crews::is_root(tx.conn(), root)? {
        return Err(CollabError::Invalid(format!("{root} 不是根机组，没有人类群")));
    }
    let id = human_id(root);
    let created = tx.conn().execute(
        "INSERT OR IGNORE INTO channels(id,crew_id,kind,name,created_by,created_at) VALUES(?1,?2,'human','','system',?3)",
        params![id, root, tx.now_ts()],
    )? > 0;
    let unarchived = tx.conn().execute("UPDATE channels SET archived_at=NULL WHERE id=?1 AND archived_at IS NOT NULL", [&id])? > 0;
    if created || unarchived {
        if let Some(cap) = crews::captain_of(tx.conn(), root)? {
            cursors::pin_to_end(tx, &cap, &id)?;
        }
        tx.note(Change::ChannelChanged { channel_id: id.clone() });
    }
    get(tx.conn(), &id)
}

/// 根机组被收编：它的人类群归档（消息保留，spec §22.2）。
pub(crate) fn archive_human_of(tx: &mut Tx<'_>, crew: &str) -> Result<()> {
    let id = human_id(crew);
    if tx.conn().execute("UPDATE channels SET archived_at=?2 WHERE id=?1 AND archived_at IS NULL", params![id, tx.now_ts()])? > 0 {
        tx.note(Change::ChannelChanged { channel_id: id });
    }
    Ok(())
}

/// 一个 session 「在」的群：自己机组的大厅 + （是根机组机长时）人类群 + 显式加入且未归档的群。
pub fn channels_of_session(conn: &Connection, session: &str) -> Result<Vec<Channel>> {
    let s = sessions::get(conn, session)?;
    let mut out = vec![get(conn, &lobby_id(&s.crew_id))?];
    if crews::is_root(conn, &s.crew_id)? && crews::captain_of(conn, &s.crew_id)?.as_deref() == Some(session) {
        if let Ok(h) = get(conn, &human_id(&s.crew_id)) {
            if h.archived_at.is_none() {
                out.push(h);
            }
        }
    }
    let mut st = conn.prepare(
        "SELECT cm.channel_id FROM channel_members cm JOIN channels ch ON ch.id=cm.channel_id
          WHERE cm.member_kind='session' AND cm.member_id=?1 AND cm.left_at IS NULL AND ch.archived_at IS NULL
          ORDER BY ch.created_at, ch.id",
    )?;
    let ids: Vec<String> = st.query_map([session], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    for id in ids {
        if !out.iter().any(|c| c.id == id) {
            out.push(get(conn, &id)?);
        }
    }
    Ok(out)
}

pub fn summary(conn: &Connection, id: &str) -> Result<ChannelSummary> {
    let channel = get(conn, id)?;
    let b = cursors::badge(conn, id)?;
    Ok(ChannelSummary {
        members: members(conn, id)?,
        watched: is_watched(conn, id)?,
        unread_for_human: b.unread_for_human,
        human_mentions: b.human_mentions,
        channel,
    })
}

/// `channel.list(crewId)`：归属该机组的群 ∪ 有成员 session 属于该机组的群（§17「同时显示在每个成员所在机组下」）。
/// 根机组会顺手懒建人类群。顺序：大厅、人类群、其余按建群时间。
pub fn list_for_crew(tx: &mut Tx<'_>, crew: &str) -> Result<Vec<ChannelSummary>> {
    crews::get(tx.conn(), crew)?;
    if crews::is_root(tx.conn(), crew)? {
        ensure_human_channel(tx, crew)?;
    }
    let mut st = tx.conn().prepare(
        "SELECT id FROM channels ch WHERE ch.crew_id=?1
            OR ch.id IN (SELECT cm.channel_id FROM channel_members cm JOIN sessions s
                           ON cm.member_kind='session' AND cm.member_id=s.id
                          WHERE s.crew_id=?1 AND cm.left_at IS NULL)
          ORDER BY CASE kind WHEN 'lobby' THEN 0 WHEN 'human' THEN 1 ELSE 2 END, created_at, id",
    )?;
    let ids: Vec<String> = st.query_map([crew], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    ids.iter().map(|id| summary(tx.conn(), id)).collect()
}

fn require_owner_channel(ch: &Channel) -> Result<()> {
    if matches!(ch.kind, ChannelKind::Lobby | ChannelKind::Human) {
        return Err(CollabError::Invalid("大厅 / 人类群的群主跟着机组机长走，换机长即可".into()));
    }
    Ok(())
}

fn set_pending(tx: &mut Tx<'_>, id: &str, to: Option<(&MemberRef, &str)>) -> Result<()> {
    let (k, i, by) = match to {
        Some((m, by)) => {
            let (k, i) = m.to_row();
            (Some(k), Some(i.to_string()), Some(by.to_string()))
        }
        None => (None, None, None),
    };
    let at = to.map(|_| tx.now_ts());
    tx.conn().execute(
        "UPDATE channels SET pending_owner_kind=?2, pending_owner_id=?3, pending_owner_by=?4, pending_owner_at=?5 WHERE id=?1",
        params![id, k, i, by, at],
    )?;
    Ok(())
}

fn set_owner(tx: &mut Tx<'_>, id: &str, to: &MemberRef) -> Result<Channel> {
    let (k, i) = to.to_row();
    tx.conn().execute("UPDATE channels SET owner_kind=?2, owner_id=?3 WHERE id=?1", params![id, k, i])?;
    set_pending(tx, id, None)?;
    tx.note(Change::ChannelChanged { channel_id: id.into() });
    get(tx.conn(), id)
}

/// 群主发起转让（§22.7 Q2：同工作项负责人，双方确认）。
pub fn request_owner_transfer(tx: &mut Tx<'_>, by: &Sender, id: &str, to: &MemberRef) -> Result<Channel> {
    let ch = get(tx.conn(), id)?;
    require_owner_channel(&ch)?;
    let me = sender_ref(by)?;
    if ch.owner.as_ref() != Some(&me) {
        return Err(CollabError::Forbidden("只有群主能发起转让".into()));
    }
    if *to == me {
        return Err(CollabError::Invalid("已经是群主".into()));
    }
    if !is_member(tx.conn(), id, to)? {
        return Err(CollabError::Invalid("新群主必须是群成员".into()));
    }
    let by_s = me.to_row().1.to_string();
    set_pending(tx, id, Some((to, &by_s)))?;
    tx.note(Change::ChannelChanged { channel_id: id.into() });
    get(tx.conn(), id)
}

pub fn accept_owner_transfer(tx: &mut Tx<'_>, by: &Sender, id: &str) -> Result<Channel> {
    let ch = get(tx.conn(), id)?;
    let me = sender_ref(by)?;
    match &ch.pending_owner {
        Some(p) if *p == me => set_owner(tx, id, &me),
        Some(_) => Err(CollabError::Forbidden("转让对象不是你".into())),
        None => Err(CollabError::Invalid("没有待接受的群主转让".into())),
    }
}

/// 撤回 / 拒绝：群主、被转让的人、机长、人。
pub fn withdraw_owner_transfer(tx: &mut Tx<'_>, by: &Sender, id: &str) -> Result<Channel> {
    let ch = get(tx.conn(), id)?;
    if ch.pending_owner.is_none() {
        return Err(CollabError::Invalid("没有待接受的群主转让".into()));
    }
    let me = by.member();
    let allowed = may_manage(tx.conn(), by, &ch)? || me.as_ref() == ch.pending_owner.as_ref();
    if !allowed {
        return Err(CollabError::Forbidden("只有群主、被转让的人、机长或人能撤回".into()));
    }
    set_pending(tx, id, None)?;
    tx.note(Change::ChannelChanged { channel_id: id.into() });
    get(tx.conn(), id)
}

/// 机长（归属机组的）或人强制换群主。
pub fn force_owner(tx: &mut Tx<'_>, by: &Sender, id: &str, to: &MemberRef) -> Result<Channel> {
    let ch = get(tx.conn(), id)?;
    require_owner_channel(&ch)?;
    let allowed = match by {
        Sender::Human => true,
        Sender::Session(s) => crews::captain_of(tx.conn(), &ch.crew_id)?.as_deref() == Some(s.as_str()),
        Sender::System => false,
    };
    if !allowed {
        return Err(CollabError::Forbidden("只有机长或人能强制换群主".into()));
    }
    if !is_member(tx.conn(), id, to)? {
        return Err(CollabError::Invalid("新群主必须是群成员".into()));
    }
    set_owner(tx, id, to)
}
```

`crews.rs`：`adopt` 里在 `INSERT INTO crew_parents` 之前记 `let was_root = ps.is_empty();`，插入之后加 `if was_root { crate::channels::archive_human_of(tx, crew)?; }`；`release` 里删边之后加：

```rust
    if is_root(tx.conn(), crew)? {
        let human = crewd_store::channels::human_id(crew);
        let exists: bool = tx.conn().query_row("SELECT COUNT(*) FROM channels WHERE id=?1", [&human], |r| r.get::<_, i64>(0))? > 0;
        if exists {
            crate::channels::ensure_human_channel(tx, crew)?;
        }
    }
```

（只解档原来就有的；没有的仍然懒建。）再加：

```rust
pub fn detail(tx: &mut Tx<'_>, id: &str) -> Result<crewd_proto::CrewDetail> {
    let crew = get(tx.conn(), id)?;
    Ok(crewd_proto::CrewDetail {
        root_id: root_of(tx.conn(), id)?,
        children: children(tx.conn(), id)?,
        channels: crate::channels::list_for_crew(tx, id)?,
        sessions: crate::sessions::list(tx.conn(), Some(id))?,
        crew,
    })
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-collab`
Expected: 前面 29 条 + channels 10 + cursors 1 = 40 passed。

- [ ] **Step 5: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/collab/src/lib.rs crates/collab/src/channels.rs crates/collab/src/cursors.rs crates/collab/src/crews.rs crates/collab/src/testkit.rs
git diff --cached --stat
git commit -m "crewd-collab: channels with implicit lobby/human membership, owners and transfer, watch, lazy human channel

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 8: 发消息（校验、@ 解析、落库、变更）、列表、搜索

裁定 10–14。`post` 是所有发消息路径的唯一入口（RPC 的人、M3b 的工具、collab 自己的公告）；公告走 `post_internal`（跳过成员检查，其余规则相同）。session 发言自动 `sessions::note_output`（§22.7 Q5 的「新产出」）。

**Files:**
- Create: `crewd/crates/collab/src/messages.rs`
- Modify: `crewd/crates/collab/src/lib.rs`（`pub mod messages;`）

**Interfaces:**
- Consumes: Task 7（`channels::{get, is_member, members, is_watched}`、`Sender`）、Task 6（`crews::{captain_of, root_of}`、`sessions::{role_of, display_name, note_output}`、`cursors::advance`）。
- Produces：`messages::{NewPost { channel_id, text, in_reply_to, mentions: Vec<Mention>, attachments: Vec<Attachment>, category }, Posted { message, unwatched_human_mention }, post(tx, Sender, NewPost) -> Result<Posted>, pub(crate) post_internal(tx, Sender, NewPost) -> Result<Posted>, get(conn, id) -> Result<Message>, list(conn, channel, after: Option<i64>, before: Option<i64>, limit: u32) -> Result<Vec<Message>>, Search { query, crew_id, channel_ids, after, before, limit }, search(conn, &Search) -> Result<Vec<Message>>}`。`Mention` 作输入时：`captain` 不带 targetId（发送时解析），`session` 必带 targetId。

- [ ] **Step 1: 写失败的测试**

`crates/collab/src/messages.rs`（文件头 + 类型 + 测试）：

```rust
//! 发消息（spec §22.2「群规则」、裁定 10–14）、列表、搜索。
use crate::{Change, CollabError, Result, Sender, Tx, channels, crews, cursors, sessions};
use crewd_proto::{Attachment, ChannelKind, MemberRef, Mention, MentionKind, Message, Role, SenderKind};
use rusqlite::Connection;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewPost {
    pub channel_id: String,
    pub text: String,
    pub in_reply_to: Option<String>,
    pub mentions: Vec<Mention>,
    pub attachments: Vec<Attachment>,
    pub category: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Posted {
    pub message: Message,
    /// mentions 含 human、群没被注视、且不是根机组人类群：M3b 回执要明说「人类未注视此群」（§17）。
    pub unwatched_human_mention: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Search {
    pub query: String,
    pub crew_id: Option<String>,
    pub channel_ids: Option<Vec<String>>,
    /// RFC 3339，含。
    pub after: Option<String>,
    pub before: Option<String>,
    pub limit: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{NOW, err, ok, ok_at, store};

    fn post_text(ch: &str, text: &str) -> NewPost {
        NewPost { channel_id: ch.into(), text: text.into(), ..Default::default() }
    }
    fn m(kind: MentionKind, target: Option<&str>) -> Mention {
        Mention { kind, target_id: target.map(Into::into) }
    }
    fn session(id: &str) -> Sender {
        Sender::Session(id.into())
    }

    #[test]
    fn a_session_posts_into_its_lobby_and_the_change_carries_the_sender() {
        let mut s = store();
        let (p, ch) = ok_at(&mut s, NOW, |tx| post(tx, session("w-a1"), post_text("crew-a/lobby", "收到，开工")));
        assert_eq!(p.message.sender_kind, SenderKind::Session);
        assert_eq!(p.message.sender_name.as_deref(), Some("名字-w-a1"));
        assert!(ch.contains(&Change::MessageAppended {
            channel_id: "crew-a/lobby".into(),
            seq: p.message.seq,
            message_id: p.message.id.clone(),
            sender_session_id: Some("w-a1".into()),
        }));
        assert!(ch.contains(&Change::BadgeChanged { channel_id: "crew-a/lobby".into() }));
        assert!(crate::sessions::get(s.conn(), "w-a1").unwrap().last_output_at.is_some(), "发言 = 新产出");
        let cap = ok(&mut s, |tx| post(tx, session("cap-a"), post_text("crew-a/lobby", "好")));
        assert_eq!(cap.message.sender_kind, SenderKind::Captain);
    }

    #[test]
    fn only_members_may_post_except_into_the_root_human_channel() {
        let mut s = store();
        assert!(matches!(err(&mut s, |tx| post(tx, session("w-b1"), post_text("crew-a/lobby", "串门"))), CollabError::Forbidden(_)));
        ok(&mut s, |tx| crate::channels::ensure_human_channel(tx, "crew-a"));
        ok(&mut s, |tx| post(tx, session("w-a1"), post_text("crew-a/human", "要人拍板")));
        assert!(crate::channels::is_member(s.conn(), "crew-a/human", &MemberRef::session("w-a1")).unwrap(), "发言即入群");
        ok(&mut s, |tx| post(tx, Sender::Human, post_text("crew-b/lobby", "人可以在任何群说话")));
        assert!(!crate::channels::is_member(s.conn(), "crew-b/lobby", &MemberRef::session("w-a1")).unwrap());
        ok(&mut s, |tx| post(tx, Sender::System, post_text("crew-x/lobby", "系统通知")));
    }

    #[test]
    fn archived_channels_refuse_messages_and_blank_text_is_refused() {
        let mut s = store();
        let ch = ok(&mut s, |tx| {
            crate::channels::create(tx, &session("w-a1"), crate::channels::NewChannel { crew_id: "crew-a".into(), kind: ChannelKind::Topic, name: "临时".into(), ..Default::default() })
        });
        ok(&mut s, |tx| crate::channels::update(tx, &session("w-a1"), &ch.id, crate::channels::ChannelPatch { archived: Some(true), ..Default::default() }));
        assert!(matches!(err(&mut s, |tx| post(tx, session("w-a1"), post_text(&ch.id, "还在吗"))), CollabError::Invalid(_)));
        assert!(matches!(err(&mut s, |tx| post(tx, session("w-a1"), post_text("crew-a/lobby", "  \n"))), CollabError::Invalid(_)));
        assert!(matches!(
            err(&mut s, |tx| post(tx, Sender::Human, NewPost {
                attachments: vec![Attachment { id: "a".into(), mime: "image/png".into(), size: None, path: "/etc/passwd".into(), filename: None }],
                ..post_text("crew-a/lobby", "带附件")
            })),
            CollabError::Invalid(_)
        ), "裁定 14：附件上传未实现，不收客户端给的路径");
    }

    #[test]
    fn captain_mention_is_resolved_at_send_time() {
        let mut s = store();
        let before = ok(&mut s, |tx| post(tx, session("w-a1"), NewPost { mentions: vec![m(MentionKind::Captain, None)], ..post_text("crew-a/lobby", "@机长 看一下") }));
        assert_eq!(before.message.mentions, [m(MentionKind::Captain, Some("cap-a"))]);
        ok(&mut s, |tx| crate::crews::set_captain(tx, "crew-a", "w-a2"));
        let after = ok(&mut s, |tx| post(tx, session("w-a1"), NewPost { mentions: vec![m(MentionKind::Captain, None)], ..post_text("crew-a/lobby", "@机长 再看一下") }));
        assert_eq!(after.message.mentions, [m(MentionKind::Captain, Some("w-a2"))]);
        let old = get(s.conn(), &before.message.id).unwrap();
        assert_eq!(old.mentions, [m(MentionKind::Captain, Some("cap-a"))], "换机长不改历史");
    }

    #[test]
    fn session_mentions_must_target_members() {
        let mut s = store();
        let e = err(&mut s, |tx| post(tx, session("w-a1"), NewPost { mentions: vec![m(MentionKind::Session, Some("w-b1"))], ..post_text("crew-a/lobby", "@别的机组的人") }));
        let CollabError::Invalid(why) = e else { panic!("{e:?}") };
        assert!(why.contains("w-a2"), "错误里列出成员：{why}");
        assert!(matches!(
            err(&mut s, |tx| post(tx, session("w-a1"), NewPost { mentions: vec![m(MentionKind::Session, None)], ..post_text("crew-a/lobby", "@?") })),
            CollabError::Invalid(_)
        ));
    }

    #[test]
    fn reply_to_appends_broadcast_and_the_replied_sender_without_duplicates() {
        let mut s = store();
        let q = ok(&mut s, |tx| post(tx, session("w-a2"), post_text("crew-a/lobby", "这个怎么做？")));
        let r = ok(&mut s, |tx| post(tx, session("w-a1"), NewPost {
            in_reply_to: Some(q.message.id.clone()),
            mentions: vec![m(MentionKind::Session, Some("w-a2"))],
            ..post_text("crew-a/lobby", "这样做")
        }));
        assert_eq!(r.message.mentions, [m(MentionKind::Session, Some("w-a2")), m(MentionKind::Broadcast, None)]);
        let h = ok(&mut s, |tx| post(tx, Sender::Human, post_text("crew-a/lobby", "人的问题")));
        let r2 = ok(&mut s, |tx| post(tx, session("w-a1"), NewPost { in_reply_to: Some(h.message.id.clone()), ..post_text("crew-a/lobby", "答人") }));
        assert_eq!(r2.message.mentions, [m(MentionKind::Broadcast, None), m(MentionKind::Human, None)]);
        let other = ok(&mut s, |tx| post(tx, session("cap-x"), post_text("crew-x/lobby", "别处")));
        assert!(matches!(
            err(&mut s, |tx| post(tx, session("w-a1"), NewPost { in_reply_to: Some(other.message.id.clone()), ..post_text("crew-a/lobby", "跨群回复") })),
            CollabError::Invalid(_)
        ));
    }

    #[test]
    fn human_mention_in_an_unwatched_channel_is_flagged() {
        let mut s = store();
        let hm = |text: &str| NewPost { mentions: vec![m(MentionKind::Human, None)], ..post_text("crew-a/lobby", text) };
        assert!(ok(&mut s, |tx| post(tx, session("w-a1"), hm("@人类 a"))).unwatched_human_mention);
        ok(&mut s, |tx| crate::channels::watch(tx, "crew-a/lobby", true));
        assert!(!ok(&mut s, |tx| post(tx, session("w-a1"), hm("@人类 b"))).unwatched_human_mention);
        ok(&mut s, |tx| crate::channels::ensure_human_channel(tx, "crew-a"));
        let p = ok(&mut s, |tx| post(tx, session("cap-a"), NewPost { channel_id: "crew-a/human".into(), ..hm("@人类 c") }));
        assert!(!p.unwatched_human_mention, "根机组人类群本来就是找人的地方");
    }

    #[test]
    fn a_human_post_moves_the_human_cursor() {
        let mut s = store();
        let p = ok(&mut s, |tx| post(tx, Sender::Human, post_text("crew-a/lobby", "人说话")));
        assert_eq!(crate::cursors::cursor(s.conn(), "human", "crew-a/lobby").unwrap(), Some(p.message.seq));
    }

    #[test]
    fn list_pages_and_search_filters() {
        let mut s = store();
        let seqs: Vec<i64> = (0..5).map(|i| ok(&mut s, |tx| post(tx, session("w-a1"), post_text("crew-a/lobby", &format!("第{i}条 关键词")))).message.seq).collect();
        ok(&mut s, |tx| post(tx, session("cap-x"), post_text("crew-x/lobby", "别处的关键词")));
        let page = list(s.conn(), "crew-a/lobby", None, Some(seqs[4]), 2).unwrap();
        assert_eq!(page.iter().map(|m| m.seq).collect::<Vec<_>>(), [seqs[2], seqs[3]]);
        let tail = list(s.conn(), "crew-a/lobby", Some(seqs[2]), None, 10).unwrap();
        assert_eq!(tail.len(), 2);
        let all = search(s.conn(), &Search { query: "关键词".into(), limit: 50, ..Default::default() }).unwrap();
        assert_eq!(all.len(), 6);
        assert_eq!(all[0].channel_id, "crew-x/lobby", "新的在前");
        let a = search(s.conn(), &Search { query: "关键词".into(), crew_id: Some("crew-a".into()), limit: 3, ..Default::default() }).unwrap();
        assert_eq!(a.len(), 3);
        assert!(search(s.conn(), &Search { query: " ".into(), limit: 3, ..Default::default() }).is_err());
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-collab messages::`
Expected: 编译失败。

- [ ] **Step 3: 实现**

```rust
pub fn get(conn: &Connection, id: &str) -> Result<Message> {
    crewd_store::messages::get(conn, id)?.ok_or_else(|| CollabError::NotFound(format!("没有消息 {id}")))
}

pub fn list(conn: &Connection, channel: &str, after: Option<i64>, before: Option<i64>, limit: u32) -> Result<Vec<Message>> {
    channels::get(conn, channel)?;
    Ok(crewd_store::messages::list(conn, channel, after.unwrap_or(0), before, limit.clamp(1, 1000))?)
}

pub fn search(conn: &Connection, q: &Search) -> Result<Vec<Message>> {
    let needle = q.query.trim().to_lowercase();
    if needle.is_empty() {
        return Err(CollabError::Invalid("搜索词不能为空".into()));
    }
    let mut sql = String::from(
        "SELECT m.id FROM messages m JOIN channels ch ON ch.id=m.channel_id WHERE instr(lower(m.text), ?1) > 0",
    );
    let mut args: Vec<rusqlite::types::Value> = vec![needle.into()];
    if let Some(c) = &q.crew_id {
        args.push(c.clone().into());
        sql += &format!(" AND ch.crew_id=?{}", args.len());
    }
    if let Some(ids) = &q.channel_ids {
        let mut ph = Vec::new();
        for id in ids {
            args.push(id.clone().into());
            ph.push(format!("?{}", args.len()));
        }
        if ph.is_empty() {
            return Ok(Vec::new());
        }
        sql += &format!(" AND m.channel_id IN ({})", ph.join(","));
    }
    if let Some(a) = &q.after {
        args.push(a.clone().into());
        sql += &format!(" AND m.created_at>=?{}", args.len());
    }
    if let Some(b) = &q.before {
        args.push(b.clone().into());
        sql += &format!(" AND m.created_at<?{}", args.len());
    }
    args.push((q.limit.clamp(1, 500) as i64).into());
    sql += &format!(" ORDER BY m.seq DESC LIMIT ?{}", args.len());
    let mut st = conn.prepare(&sql)?;
    let ids: Vec<String> = st
        .query_map(rusqlite::params_from_iter(args), |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    ids.iter().map(|id| get(conn, id)).collect()
}

pub fn post(tx: &mut Tx<'_>, sender: Sender, p: NewPost) -> Result<Posted> {
    post_with(tx, sender, p, true)
}

/// collab 自己的公告（问、审批、重启询问）：跳过发言者成员检查，其余规则同 `post`。
pub(crate) fn post_internal(tx: &mut Tx<'_>, sender: Sender, p: NewPost) -> Result<Posted> {
    post_with(tx, sender, p, false)
}

fn post_with(tx: &mut Tx<'_>, sender: Sender, p: NewPost, check_member: bool) -> Result<Posted> {
    let conn = tx.conn();
    let ch = channels::get(conn, &p.channel_id)?;
    if ch.archived_at.is_some() {
        return Err(CollabError::Invalid(format!("群 {} 已归档，不收消息", ch.id)));
    }
    if p.text.trim().is_empty() {
        return Err(CollabError::Invalid("消息不能为空".into()));
    }
    if !p.attachments.is_empty() {
        return Err(CollabError::Invalid("附件上传尚未实现".into()));
    }
    let is_root_human = ch.kind == ChannelKind::Human;
    // 发言者
    let (sender_kind, sender_session_id, sender_name) = match &sender {
        Sender::Human => (SenderKind::Human, None, None),
        Sender::System => (SenderKind::System, None, None),
        Sender::Session(id) => {
            let me = MemberRef::session(id.clone());
            if check_member && !channels::is_member(conn, &ch.id, &me)? {
                if is_root_human {
                    // 往根机组人类群发言 = 入群（人回话时它才在群里）。
                    tx.conn().execute(
                        "INSERT INTO channel_members(channel_id,member_kind,member_id,joined_at) VALUES(?1,'session',?2,?3)
                         ON CONFLICT(channel_id,member_kind,member_id) DO UPDATE SET left_at=NULL",
                        rusqlite::params![ch.id, id, tx.now_ts()],
                    )?;
                    cursors::pin_to_end(tx, id, &ch.id)?;
                    tx.note(Change::ChannelChanged { channel_id: ch.id.clone() });
                } else {
                    return Err(CollabError::Forbidden(format!("{id} 不是群 {} 的成员", ch.id)));
                }
            }
            let kind = if sessions::role_of(conn, id)? == Role::Captain { SenderKind::Captain } else { SenderKind::Session };
            (kind, Some(id.clone()), Some(sessions::display_name(conn, id)?))
        }
    };
    // @ 解析（裁定 12）
    let mut mentions: Vec<Mention> = Vec::new();
    let push = |v: &mut Vec<Mention>, m: Mention| {
        if !v.contains(&m) {
            v.push(m);
        }
    };
    for m in p.mentions {
        let resolved = match m.kind {
            MentionKind::Captain => {
                let cap = crews::captain_of(conn, &ch.crew_id)?
                    .ok_or_else(|| CollabError::Invalid(format!("机组 {} 现在没有机长", ch.crew_id)))?;
                Mention { kind: MentionKind::Captain, target_id: Some(cap) }
            }
            MentionKind::Session => {
                let t = m.target_id.clone().ok_or_else(|| CollabError::Invalid("@session 要带 targetId".into()))?;
                if !channels::is_member(conn, &ch.id, &MemberRef::session(t.clone()))? {
                    let names: Vec<String> = channels::members(conn, &ch.id)?
                        .into_iter()
                        .filter_map(|x| match x {
                            MemberRef::Session { id } => Some(id),
                            MemberRef::Human => None,
                        })
                        .collect();
                    return Err(CollabError::Invalid(format!("{t} 不是群 {} 的成员；成员有：{}", ch.id, names.join(", "))));
                }
                Mention { kind: MentionKind::Session, target_id: Some(t) }
            }
            MentionKind::Human | MentionKind::Broadcast => Mention { kind: m.kind, target_id: None },
        };
        push(&mut mentions, resolved);
    }
    if let Some(rid) = &p.in_reply_to {
        let replied = get(conn, rid)?;
        if replied.channel_id != ch.id {
            return Err(CollabError::Invalid("只能回复同一个群里的消息".into()));
        }
        push(&mut mentions, Mention { kind: MentionKind::Broadcast, target_id: None });
        match (replied.sender_kind, replied.sender_session_id) {
            (SenderKind::Human, _) => push(&mut mentions, Mention { kind: MentionKind::Human, target_id: None }),
            (SenderKind::Session | SenderKind::Captain, Some(sid)) => push(&mut mentions, Mention { kind: MentionKind::Session, target_id: Some(sid) }),
            _ => {}
        }
    }
    let mut message = Message {
        seq: 0,
        id: uuid::Uuid::new_v4().to_string(),
        channel_id: ch.id.clone(),
        sender_kind,
        sender_session_id: sender_session_id.clone(),
        sender_name,
        text: p.text,
        category: p.category,
        in_reply_to: p.in_reply_to,
        mentions,
        attachments: vec![],
        created_at: tx.now_ts(),
    };
    message.seq = crewd_store::messages::insert(conn, &message, None)?
        .ok_or_else(|| CollabError::Conflict("新消息 id 冲突".into()))?;
    match &sender {
        Sender::Human => cursors::advance(tx, cursors::HUMAN, &ch.id, message.seq)?,
        Sender::Session(id) => sessions::note_output(tx, id)?,
        Sender::System => {}
    }
    let human_mention = message.mentions.iter().any(|m| m.kind == MentionKind::Human);
    let unwatched_human_mention = human_mention && !is_root_human && !channels::is_watched(conn, &ch.id)?;
    tx.note(Change::MessageAppended {
        channel_id: ch.id.clone(),
        seq: message.seq,
        message_id: message.id.clone(),
        sender_session_id,
    });
    tx.note(Change::BadgeChanged { channel_id: ch.id });
    Ok(Posted { message, unwatched_human_mention })
}
```

（`push` 闭包里不能同时借 `mentions` 两次——照代码那样把 `&mut mentions` 当参数传进去即可。`Message` 若没有 `seq: 0` 以外的默认值问题，可直接用 `..Default::default()`。）

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-collab`
Expected: 40 + 9 = 49 passed。

- [ ] **Step 5: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/collab/src/lib.rs crates/collab/src/messages.rs
git diff --cached --stat
git commit -m "crewd-collab: post with membership rules, send-time @ resolution, reply_to widening; list and search

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 9: 未读、可见性、人类已读、收听

`unread_for` 是注入面（M3b 的 post-tool-use、M3c 的 codex additionalContext）与唤醒「消息类是否已注入」判定共用的那一个函数：群集合 = `channels_of_session` ∪ 正在收听的群（§22.3、§22.7 Q2）；游标缺行先钉末尾、本次不返回（裁定 9）；不含自己发的（裁定 10）；按可见性过滤（移植 `Sources/Mcp/CrewWhiteboardVisibility.swift`：只有 session / captain 收窄，human 不收窄，broadcast 放宽，自己发的永远可见；captain 已在发送时落成具体 targetId，所以按 targetId 判，不看「此刻是不是机长」）。

**Files:**
- Create: `crewd/crates/collab/src/listens.rs`
- Modify: `crewd/crates/collab/src/cursors.rs`、`crewd/crates/collab/src/lib.rs`（`pub mod listens;`）

**Interfaces:**
- Consumes: Task 7–8。
- Produces：`cursors::{unread_for(tx, session) -> Result<Vec<(String, Vec<Message>)>>, mark_read(tx, channel, seq) -> Result<Badge>, visible_to(&Message, session: &str) -> bool, directed_targets(&Message, session: &str) -> Option<Vec<String>>, UNREAD_CAP: u32 = 200}`；`listens::{Listen { session, channel, until_ms: Option<i64>, senders: Vec<String> }, set(tx, session, channel, until_ms, senders) -> Result<()>, clear(tx, session, channel: Option<&str>) -> Result<usize>, active_for_channel(conn, channel, now_ms) -> Result<Vec<Listen>>, channels_of(conn, session, now_ms) -> Result<Vec<String>>}`。

- [ ] **Step 1: 写失败的测试**

`cursors.rs` 的测试模块里加：

```rust
    use crate::Sender;
    use crate::messages::{NewPost, post};
    use crate::testkit::{NOW, MIN, ok, ok_at};

    fn say(s: &mut crewd_store::Store, who: &str, ch: &str, mentions: Vec<Mention>) -> crewd_proto::Message {
        ok(s, |tx| post(tx, Sender::Session(who.into()), NewPost { channel_id: ch.into(), text: format!("{who} 说"), mentions, ..Default::default() })).message
    }
    fn at(kind: MentionKind, t: Option<&str>) -> Mention {
        Mention { kind, target_id: t.map(Into::into) }
    }
    fn unread_ids(s: &mut crewd_store::Store, who: &str) -> Vec<String> {
        ok(s, |tx| unread_for(tx, who)).into_iter().flat_map(|(_, ms)| ms.into_iter().map(|m| m.id)).collect()
    }

    #[test]
    fn missing_cursor_pins_to_end_instead_of_dumping_history() {
        let mut s = store();
        say(&mut s, "w-a1", "crew-a/lobby", vec![]);
        s.conn().execute("DELETE FROM read_cursors WHERE reader='w-a2'", []).unwrap();
        assert!(unread_ids(&mut s, "w-a2").is_empty(), "缺游标不能当 0");
        assert!(cursor(s.conn(), "w-a2", "crew-a/lobby").unwrap().is_some(), "顺手钉上");
        let m = say(&mut s, "w-a1", "crew-a/lobby", vec![]);
        assert_eq!(unread_ids(&mut s, "w-a2"), [m.id]);
    }

    #[test]
    fn own_messages_are_never_unread_and_advance_is_monotonic() {
        let mut s = store();
        say(&mut s, "w-a1", "crew-a/lobby", vec![]);
        assert!(unread_ids(&mut s, "w-a1").is_empty());
        let m = say(&mut s, "w-a2", "crew-a/lobby", vec![]);
        ok(&mut s, |tx| advance(tx, "w-a1", "crew-a/lobby", m.seq));
        ok(&mut s, |tx| advance(tx, "w-a1", "crew-a/lobby", 1));
        assert_eq!(cursor(s.conn(), "w-a1", "crew-a/lobby").unwrap(), Some(m.seq), "只进不退");
    }

    #[test]
    fn directed_messages_are_hidden_from_others_but_human_and_broadcast_are_not() {
        let mut s = store();
        let to_a2 = say(&mut s, "w-a1", "crew-a/lobby", vec![at(MentionKind::Session, Some("w-a2"))]);
        let to_human = say(&mut s, "w-a1", "crew-a/lobby", vec![at(MentionKind::Human, None)]);
        let widened = say(&mut s, "w-a1", "crew-a/lobby", vec![at(MentionKind::Broadcast, None), at(MentionKind::Session, Some("w-a2"))]);
        assert_eq!(unread_ids(&mut s, "w-a2"), [to_a2.id.clone(), to_human.id.clone(), widened.id.clone()]);
        assert_eq!(unread_ids(&mut s, "cap-a"), [to_human.id.clone(), widened.id.clone()]);
        assert!(visible_to(&to_a2, "w-a1"), "自己发的永远可见");
        assert_eq!(directed_targets(&widened, "cap-a"), Some(vec!["w-a2".to_string()]), "看得见但不是给我的 → 要标注");
        assert_eq!(directed_targets(&widened, "w-a2"), None);
        assert_eq!(directed_targets(&to_human, "cap-a"), None, "只 @ 人类 = 广播，不标");
    }

    #[test]
    fn visibility_follows_the_stored_target_not_the_current_captain() {
        let mut s = store();
        let m = say(&mut s, "w-a1", "crew-a/lobby", vec![at(MentionKind::Captain, None)]);
        ok(&mut s, |tx| crate::crews::set_captain(tx, "crew-a", "w-a2"));
        assert!(visible_to(&m, "cap-a"), "发送时的机长是 cap-a");
        assert!(!visible_to(&m, "w-a2"), "后来的机长不是那条消息的对象");
    }

    #[test]
    fn listened_channels_join_the_injection_surface_until_they_expire() {
        let mut s = store();
        let m = say(&mut s, "cap-x", "crew-x/lobby", vec![]);
        assert!(unread_ids(&mut s, "w-a1").is_empty());
        ok(&mut s, |tx| crate::listens::set(tx, "w-a1", "crew-x/lobby", Some(NOW + 30 * MIN), vec![]));
        let m2 = say(&mut s, "cap-x", "crew-x/lobby", vec![]);
        assert_eq!(unread_ids(&mut s, "w-a1"), [m2.id.clone()], "开始收听时从末尾开始，{m:?} 不算");
        let (got, _) = ok_at(&mut s, NOW + 31 * MIN, |tx| unread_for(tx, "w-a1"));
        assert!(got.is_empty(), "收听过期");
    }

    #[test]
    fn mark_read_moves_the_human_cursor_and_clears_the_badge() {
        let mut s = store();
        let m = say(&mut s, "w-a1", "crew-a/lobby", vec![at(MentionKind::Human, None)]);
        let (b, ch) = ok_at(&mut s, NOW, |tx| mark_read(tx, "crew-a/lobby", m.seq));
        assert_eq!((b.unread_for_human, b.human_mentions), (0, 0));
        assert_eq!(ch, [crate::Change::BadgeChanged { channel_id: "crew-a/lobby".into() }]);
    }
```

`crates/collab/src/listens.rs`（文件头 + 类型 + 测试）：

```rust
//! 收听（§22.7 Q2：session 可以收听一个群，持久化；收听者在该群有新消息时也会被叫醒——叫醒判定在 M3c）。
//! `until_ms = None` = 一直收听，直到 `clear`。`senders` 非空 = 只对这些发送者的消息叫醒（判定在 M3c）。
use crate::{CollabError, Result, Tx, channels, cursors, sessions};
use rusqlite::{Connection, params};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listen {
    pub session: String,
    pub channel: String,
    pub until_ms: Option<i64>,
    pub senders: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{MIN, NOW, err, ok, store};

    #[test]
    fn set_replaces_and_clear_removes() {
        let mut s = store();
        ok(&mut s, |tx| set(tx, "w-a1", "crew-x/lobby", None, vec!["cap-x".into()]));
        ok(&mut s, |tx| set(tx, "w-a1", "crew-b/lobby", Some(NOW + MIN), vec![]));
        let l = active_for_channel(s.conn(), "crew-x/lobby", NOW).unwrap();
        assert_eq!(l, [Listen { session: "w-a1".into(), channel: "crew-x/lobby".into(), until_ms: None, senders: vec!["cap-x".into()] }]);
        assert_eq!(channels_of(s.conn(), "w-a1", NOW + 2 * MIN).unwrap(), ["crew-x/lobby"], "过期的不算");
        assert_eq!(ok(&mut s, |tx| clear(tx, "w-a1", Some("crew-x/lobby"))), 1);
        assert_eq!(ok(&mut s, |tx| clear(tx, "w-a1", None)), 1);
        assert!(channels_of(s.conn(), "w-a1", NOW).unwrap().is_empty());
    }

    #[test]
    fn listening_needs_a_live_channel_and_a_future_deadline() {
        let mut s = store();
        assert!(matches!(err(&mut s, |tx| set(tx, "w-a1", "nope", None, vec![])), CollabError::NotFound(_)));
        assert!(matches!(err(&mut s, |tx| set(tx, "w-a1", "crew-x/lobby", Some(NOW), vec![])), CollabError::Invalid(_)));
        assert!(matches!(err(&mut s, |tx| set(tx, "ghost", "crew-x/lobby", None, vec![])), CollabError::NotFound(_)));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-collab cursors:: listens::`
Expected: 编译失败。

- [ ] **Step 3: 实现**

`listens.rs`：

```rust
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Listen> {
    let senders: String = r.get(3)?;
    Ok(Listen {
        session: r.get(0)?,
        channel: r.get(1)?,
        until_ms: r.get(2)?,
        senders: serde_json::from_str(&senders).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
        })?,
    })
}

pub fn set(tx: &mut Tx<'_>, session: &str, channel: &str, until_ms: Option<i64>, senders: Vec<String>) -> Result<()> {
    sessions::get(tx.conn(), session)?;
    let ch = channels::get(tx.conn(), channel)?;
    if ch.archived_at.is_some() {
        return Err(CollabError::Invalid("群已归档".into()));
    }
    if until_ms.is_some_and(|u| u <= tx.now_ms()) {
        return Err(CollabError::Invalid("收听截止时间必须晚于现在".into()));
    }
    tx.conn().execute(
        "INSERT INTO listens(session_id,channel_id,until,senders,created_at) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(session_id,channel_id) DO UPDATE SET until=excluded.until, senders=excluded.senders",
        params![session, channel, until_ms, serde_json::to_string(&senders)?, tx.now_ts()],
    )?;
    if cursors::cursor(tx.conn(), session, channel)?.is_none() {
        cursors::pin_to_end(tx, session, channel)?;
    }
    Ok(())
}

pub fn clear(tx: &mut Tx<'_>, session: &str, channel: Option<&str>) -> Result<usize> {
    Ok(match channel {
        Some(c) => tx.conn().execute("DELETE FROM listens WHERE session_id=?1 AND channel_id=?2", params![session, c])?,
        None => tx.conn().execute("DELETE FROM listens WHERE session_id=?1", [session])?,
    })
}

pub fn active_for_channel(conn: &Connection, channel: &str, now_ms: i64) -> Result<Vec<Listen>> {
    let mut st = conn.prepare(
        "SELECT session_id, channel_id, until, senders FROM listens
          WHERE channel_id=?1 AND (until IS NULL OR until>?2) ORDER BY session_id",
    )?;
    Ok(st.query_map(params![channel, now_ms], row)?.collect::<rusqlite::Result<_>>()?)
}

pub fn channels_of(conn: &Connection, session: &str, now_ms: i64) -> Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT l.channel_id FROM listens l JOIN channels ch ON ch.id=l.channel_id
          WHERE l.session_id=?1 AND (l.until IS NULL OR l.until>?2) AND ch.archived_at IS NULL
          ORDER BY l.channel_id",
    )?;
    Ok(st.query_map(params![session, now_ms], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}
```

`cursors.rs`（`use` 补 `crewd_proto::{Badge, Message, MentionKind}`、`crate::{Change, channels, listens}`）：

```rust
/// 每个群一次最多带多少条未读（再多就是该 `read_channel` 主动拉的量了）。
pub const UNREAD_CAP: u32 = 200;

/// 这条消息对 `session` 可见吗（移植 `CrewWhiteboardVisibility.isVisible`）。
pub fn visible_to(m: &Message, session: &str) -> bool {
    if m.mentions.iter().any(|x| x.kind == MentionKind::Broadcast) {
        return true;
    }
    let narrowing: Vec<_> = m
        .mentions
        .iter()
        .filter(|x| matches!(x.kind, MentionKind::Session | MentionKind::Captain))
        .collect();
    if narrowing.is_empty() || m.sender_session_id.as_deref() == Some(session) {
        return true;
    }
    narrowing.iter().any(|x| x.target_id.as_deref() == Some(session))
}

/// 看得见、但收窄型 mention 里没有它 → 返回那些目标（渲染时标「发给 X 的」）；否则 None。
pub fn directed_targets(m: &Message, session: &str) -> Option<Vec<String>> {
    let targets: Vec<String> = m
        .mentions
        .iter()
        .filter(|x| matches!(x.kind, MentionKind::Session | MentionKind::Captain))
        .filter_map(|x| x.target_id.clone())
        .collect();
    if targets.is_empty() || targets.iter().any(|t| t == session) {
        return None;
    }
    Some(targets)
}

/// 注入面：该 session 所在的群 + 正在收听的群里，游标之后、对它可见、不是它自己发的消息。
/// 只读不推进游标（注入成功后由调用方 `advance`）；缺游标行的群先钉末尾、本次不返回。
pub fn unread_for(tx: &mut Tx<'_>, session: &str) -> Result<Vec<(String, Vec<Message>)>> {
    let mut ids: Vec<String> = channels::channels_of_session(tx.conn(), session)?.into_iter().map(|c| c.id).collect();
    for c in listens::channels_of(tx.conn(), session, tx.now_ms())? {
        if !ids.contains(&c) {
            ids.push(c);
        }
    }
    let mut out = Vec::new();
    for ch in ids {
        let Some(after) = cursor(tx.conn(), session, &ch)? else {
            pin_to_end(tx, session, &ch)?;
            continue;
        };
        let msgs: Vec<Message> = crewd_store::messages::list(tx.conn(), &ch, after, None, UNREAD_CAP)?
            .into_iter()
            .filter(|m| m.sender_session_id.as_deref() != Some(session) && visible_to(m, session))
            .collect();
        if !msgs.is_empty() {
            out.push((ch, msgs));
        }
    }
    Ok(out)
}

/// 人类看过某群到 `seq`（`read.mark`）。Mac / iOS 共用一个人类游标。
pub fn mark_read(tx: &mut Tx<'_>, channel: &str, seq: i64) -> Result<Badge> {
    channels::get(tx.conn(), channel)?;
    advance(tx, HUMAN, channel, seq)?;
    tx.note(Change::BadgeChanged { channel_id: channel.into() });
    badge(tx.conn(), channel)
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-collab`
Expected: 49 + cursors 6 + listens 2 = 57 passed。

- [ ] **Step 5: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/collab/src/lib.rs crates/collab/src/cursors.rs crates/collab/src/listens.rs
git diff --cached --stat
git commit -m "crewd-collab: unread surface with visibility port, human read marks, persistent channel listening

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 10: 人类待确认（H 号）、审批台账、重启询问；提示词文件骨架

- 问：每个机组一本 `crewd_work::WorkBook`，整份 `BookState` 存 `work_books`（M4a 定的持久化边界），每条命令在同一个 SQLite 事务里读、改、写（`with_book`）。开问在同一事务里往根机组人类群发 @human 公告、写 `ask_meta`（裁定 13）。回答：对 `open_ask` 直接开的问给提问者入队 `ask_answer` 唤醒（提问者已无法恢复 → 叫它机组的机长转达，§22.4）；产出 `Change::AskAnswered`（含 BlockerCleared 的负责人，裁定 17）。
- 审批：同步闸，不发 H 号（§22.2）；建行 + 人类群公告；`decide` 只能一次。
- 重启询问（§22.7 Q1）：只记账与公告，`answer` 产出 `Change::RestartAnswered`，恢复动作归 M3c。
- 公告文字在 `prompts/render.{zh,en}.md` 的 `notice.*` 节（本任务建文件与加载器；Task 11 加其余节）。

**Files:**
- Create: `crewd/crates/collab/src/asks.rs`、`crewd/crates/collab/src/approvals.rs`、`crewd/crates/collab/src/restart.rs`、`crewd/crates/collab/src/prompts.rs`、`crewd/crates/collab/prompts/render.zh.md`、`crewd/crates/collab/prompts/render.en.md`
- Modify: `crewd/crates/collab/src/lib.rs`（`pub mod approvals; pub mod asks; pub mod prompts; pub mod restart;`）、`crewd/crates/collab/Cargo.toml`（dev：tempfile）、`crewd/Cargo.lock`

**Interfaces:**
- Consumes: Task 2（`withdraw_ask`）、Task 5（`wakes::enqueue`）、Task 7–8（`channels::ensure_human_channel`、`messages::post_internal`）、`settings::locale`。
- Produces：
  - `asks::{OpenAsk { question, default: Option<AskDefault>, resume_note }, with_book<R>(tx, crew, f: impl FnOnce(&mut WorkBook, Ts) -> Result<R, Rejected>) -> Result<R>, open(tx, asker: &str, OpenAsk) -> Result<Ask>, answer(tx, crew, ask: &str, text) -> Result<Ask>, withdraw(tx, by: &Sender, crew, ask, reason) -> Result<Ask>, apply_due_defaults(tx, crew) -> Result<Vec<String>>, get(conn, crew, ask) -> Result<Ask>, waits(conn, now_ms, root: Option<&str>) -> Result<Vec<HumanWait>>}`。
  - `approvals::{NewApproval { session_id, tool_name, summary, tool_use_id }, request(tx, NewApproval) -> Result<Approval>, decide(tx, id, allow: bool, by: &str) -> Result<Approval>, get(conn, id) -> Result<Approval>, list_pending(conn) -> Result<Vec<Approval>>}`；`by ∈ {"human", "timeout", "unreachable"}`。
  - `restart::{open(tx, session_ids: Vec<String>) -> Result<Option<RestartPrompt>>, answer(tx, id: i64, RestartAnswer) -> Result<RestartPrompt>, pending(conn) -> Result<Option<RestartPrompt>>, get(conn, id) -> Result<RestartPrompt>}`。
  - `prompts::{PromptFile, render_file(Locale) -> &'static PromptFile, fill(template, &[(&str, &str)]) -> String, REQUIRED: &[(&str, &[&str])]}`（`REQUIRED` = 节名 → 允许的占位符）。

- [ ] **Step 1: 提示词文件与加载器（先写测试）**

```bash
cargo add --offline -p crewd-collab --dev tempfile
```

`crates/collab/prompts/render.zh.md`：

```markdown
给人和 agent 看的群内公告与注入文字（crewd-collab 的 render / asks / approvals / restart 读这里）。
每节 `## 名字`；`{占位符}` 由代码填，允许哪些见 src/prompts.rs 的 REQUIRED。

## notice.approval
{session} 请求使用工具 {tool}：{summary}
请在审批列表里允许或拒绝；超时会自动拒绝。

## notice.restart
crewd 重新启动了。上次有 {count} 个 session 在跑，是否现在恢复它们？

## notice.restart.item
- {name}
```

`crates/collab/prompts/render.en.md`：

```markdown
Group notices and injected text shown to people and agents (read by crewd-collab render / asks / approvals / restart).
One `## name` per section; `{placeholders}` are filled by code; see REQUIRED in src/prompts.rs.

## notice.approval
{session} asks to use tool {tool}: {summary}
Allow or deny it in the approvals list; it is denied automatically on timeout.

## notice.restart
crewd restarted. {count} sessions were running before; resume them now?

## notice.restart.item
- {name}
```

`crates/collab/src/prompts.rs`：

```rust
//! 给人 / agent 看的文字全在 `prompts/render.{zh,en}.md`（编译期 include_str!），代码里不写这些字面量。
//! 格式同 `crates/jev/src/prompts.rs`：第一个 `## 名字` 之前是注释；之后每节到下一个 `## ` 为止，去掉首尾空白。
use crate::Locale;
use std::collections::BTreeMap;
use std::sync::LazyLock;

#[derive(Debug)]
pub struct PromptFile {
    pub name: &'static str,
    sections: BTreeMap<&'static str, &'static str>,
}

impl PromptFile {
    pub fn get(&self, section: &str) -> &'static str {
        match self.sections.get(section) {
            Some(t) => t,
            None => panic!("prompts/{}.md 缺少一节：## {section}", self.name),
        }
    }
    pub fn section_names(&self) -> Vec<&'static str> {
        self.sections.keys().copied().collect()
    }
}

pub fn parse(name: &'static str, text: &'static str) -> Result<PromptFile, String> {
    let mut sections = BTreeMap::new();
    let mut current: Option<(&'static str, usize)> = None;
    let mut offset = 0;
    let mut close = |cur: Option<(&'static str, usize)>, end: usize| -> Result<(), String> {
        if let Some((key, start)) = cur {
            let body = text[start..end].trim();
            if body.is_empty() {
                return Err(format!("prompts/{name}.md 的 ## {key} 是空的"));
            }
            if sections.insert(key, body).is_some() {
                return Err(format!("prompts/{name}.md 的 ## {key} 重复"));
            }
        }
        Ok(())
    };
    for line in text.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("## ") {
            close(current.take(), offset)?;
            let key = rest.trim();
            if key.is_empty() {
                return Err(format!("prompts/{name}.md 有一个没有名字的 ##"));
            }
            current = Some((key, offset + line.len()));
        }
        offset += line.len();
    }
    close(current, text.len())?;
    Ok(PromptFile { name, sections })
}

static ZH: LazyLock<PromptFile> =
    LazyLock::new(|| parse("render.zh", include_str!("../prompts/render.zh.md")).unwrap_or_else(|e| panic!("{e}")));
static EN: LazyLock<PromptFile> =
    LazyLock::new(|| parse("render.en", include_str!("../prompts/render.en.md")).unwrap_or_else(|e| panic!("{e}")));

pub fn render_file(l: Locale) -> &'static PromptFile {
    match l {
        Locale::Zh => &ZH,
        Locale::En => &EN,
    }
}

/// 节名 → 允许出现的占位符。两种语言都必须有这些节，且只用这些占位符。
pub const REQUIRED: &[(&str, &[&str])] = &[
    ("notice.approval", &["session", "tool", "summary"]),
    ("notice.restart", &["count"]),
    ("notice.restart.item", &["name"]),
];

/// 把 `{名字}` 换成值。没给的占位符原样留着（测试保证模板只用允许的占位符）。
pub fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// 模板里出现的全部 `{名字}`。
pub fn placeholders(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        let after = &rest[i + 1..];
        match after.find('}') {
            Some(j) => {
                out.push(after[..j].to_string());
                rest = &after[j + 1..];
            }
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_languages_have_every_section_with_only_allowed_placeholders() {
        for l in [Locale::Zh, Locale::En] {
            let f = render_file(l);
            for (section, allowed) in REQUIRED {
                let t = f.get(section);
                for p in placeholders(t) {
                    assert!(allowed.contains(&p.as_str()), "{}.md ## {section} 用了不允许的占位符 {{{p}}}", f.name);
                }
            }
        }
        assert_eq!(render_file(Locale::Zh).section_names(), render_file(Locale::En).section_names(), "zh / en 节名集合一致");
    }

    #[test]
    fn fill_replaces_named_placeholders() {
        assert_eq!(fill("{a} 和 {b}", &[("a", "甲"), ("b", "乙")]), "甲 和 乙");
        assert_eq!(placeholders("x {a} y {b}"), ["a", "b"]);
    }

    #[test]
    #[should_panic(expected = "缺少一节")]
    fn missing_section_names_itself() {
        render_file(Locale::Zh).get("nope");
    }
}
```

- [ ] **Step 2: 写失败的测试（问、审批、重启）**

`crates/collab/src/asks.rs`（文件头 + 类型 + 测试）：

```rust
//! 人类待确认（H 号）：经 crewd-work 的工作账本发号（§19 Q1：工作账本是唯一发号处）。
use crate::messages::{self, NewPost};
use crate::wakes::{self, WakeKind, WakePriority, WakeRequest};
use crate::{Change, CollabError, Result, Sender, Tx, channels, crews, sessions};
use crewd_proto::{Ask, AskDefault, AskStateView, HumanWait, Mention, MentionKind, WaitTierView};
use crewd_work::{Actor, AskId, AskState, BookState, Config, CrewId, DefaultOption, EventKind, HumanAsk, Rejected, Ts, WaitTier, WorkBook};
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct OpenAsk {
    pub question: String,
    /// 「X 分钟没回复就按 A 办」（§6；到点后照常走审批，§13 Q2）。
    pub default: Option<AskDefault>,
    /// 回答后叫醒提问者时带上的「接着做什么」。
    pub resume_note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{MIN, NOW, count, err, ok, ok_at, store};

    fn q(text: &str) -> OpenAsk {
        OpenAsk { question: text.into(), ..Default::default() }
    }

    #[test]
    fn open_issues_h_numbers_from_the_crew_book_and_posts_to_the_root_human_channel() {
        let mut s = store();
        let a = ok(&mut s, |tx| open(tx, "w-b1", OpenAsk { resume_note: Some("拿到答复后继续发版".into()), ..q("发版窗口定在周几？") }));
        assert_eq!((a.crew_id.as_str(), a.ask.as_str()), ("crew-b", "H1"));
        assert_eq!(a.asked_by.as_deref(), Some("w-b1"));
        assert_eq!(a.state, AskStateView::Open);
        let m = crate::messages::get(s.conn(), a.message_id.as_deref().unwrap()).unwrap();
        assert_eq!(m.channel_id, "crew-a/human", "公告发到 crew-b 的根（crew-a）的人类群");
        assert_eq!(m.mentions, [Mention { kind: MentionKind::Human, target_id: None }]);
        let b = ok(&mut s, |tx| open(tx, "cap-b", q("第二个问题？")));
        assert_eq!(b.ask, "H2", "号连续");
        let last_ask = ok(&mut s, |tx| with_book(tx, "crew-b", |book, _| Ok(book.state().last_ask)));
        assert_eq!(last_ask, 2, "与工作账本共用一个计数（M4b 的 block 开新问也走它）");
        let c = ok(&mut s, |tx| open(tx, "w-a1", q("crew-a 的第一个问题？")));
        assert_eq!(c.ask, "H1", "H 号按机组");
    }

    #[test]
    fn failed_ask_consumes_no_h_number_and_emits_nothing() {
        let mut s = store();
        s.conn().execute("INSERT INTO ask_meta(crew_id,ask) VALUES('crew-a','H1')", []).unwrap();
        let e = crate::run(&mut s, NOW, |tx| open(tx, "w-a1", q("会在写附注时失败？"))).unwrap_err();
        assert!(matches!(e, CollabError::Store(_)), "{e:?}");
        assert_eq!(count(&s, "SELECT COUNT(*) FROM work_books"), 0);
        assert_eq!(count(&s, "SELECT COUNT(*) FROM messages"), 0);
        s.conn().execute("DELETE FROM ask_meta", []).unwrap();
        assert_eq!(ok(&mut s, |tx| open(tx, "w-a1", q("这次能成？"))).ask, "H1");
    }

    #[test]
    fn asks_survive_reopening_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crewd.db");
        {
            let mut s = crewd_store::Store::open(&path).unwrap();
            // 复用夹具的数据：把内存库的内容搬不过来，这里直接在文件库上造一个最小机组。
            crate::run(&mut s, NOW, |tx| {
                let c = crate::crews::create(tx, crate::crews::NewCrew { title: "文件库机组".into(), working_directory: "/tmp/example".into(), ..Default::default() })?;
                let w = crate::sessions::register(tx, crate::sessions::NewSession { crew_id: c.id.clone(), executor: "claude".into(), ..Default::default() })?;
                open(tx, &w.id, q("重开库之后还在吗？"))
            })
            .unwrap();
        }
        let s = crewd_store::Store::open(&path).unwrap();
        let waits = waits(s.conn(), NOW + MIN, None).unwrap();
        assert_eq!(waits.len(), 1);
        assert_eq!(waits[0].ask.question, "重开库之后还在吗？");
        assert_eq!(waits[0].waited_ms, MIN);
    }

    #[test]
    fn answering_wakes_the_asker_and_reports_the_answer() {
        let mut s = store();
        let a = ok(&mut s, |tx| open(tx, "w-a1", OpenAsk { resume_note: Some("继续".into()), ..q("用哪家？") }));
        let (got, ch) = ok_at(&mut s, NOW + MIN, |tx| answer(tx, "crew-a", &a.ask, "用甲"));
        assert_eq!(got.state, AskStateView::Answered);
        assert_eq!(got.answer.as_deref(), Some("用甲"));
        assert!(ch.contains(&Change::AskAnswered { crew_id: "crew-a".into(), ask: "H1".into(), asked_by: Some("w-a1".into()), cleared: vec![] }));
        let w = crate::wakes::list(s.conn(), Some("w-a1"), 10).unwrap();
        assert_eq!(w.len(), 1);
        assert_eq!((w[0].kind, w[0].priority, w[0].source.as_str()), (WakeKind::AskAnswer, WakePriority::Human, "ask:crew-a/H1"));
        assert!(matches!(err(&mut s, |tx| answer(tx, "crew-a", "H1", "再答一次")), CollabError::Work(Rejected::AskNotOpen(_))));
        assert!(matches!(err(&mut s, |tx| answer(tx, "crew-a", "X1", "x")), CollabError::Invalid(_)));
    }

    #[test]
    fn an_unrecoverable_asker_is_relayed_through_its_captain() {
        let mut s = store();
        let a = ok(&mut s, |tx| open(tx, "w-a1", q("要不要回滚？")));
        ok(&mut s, |tx| crate::sessions::set_resume_failed(tx, "w-a1", true));
        ok(&mut s, |tx| answer(tx, "crew-a", &a.ask, "回滚"));
        assert!(crate::wakes::list(s.conn(), Some("w-a1"), 10).unwrap().is_empty());
        assert_eq!(crate::wakes::list(s.conn(), Some("cap-a"), 10).unwrap()[0].source, "ask:crew-a/H1");
    }

    #[test]
    fn withdraw_follows_the_book_permissions() {
        let mut s = store();
        let a = ok(&mut s, |tx| open(tx, "w-a1", q("还要吗？")));
        assert!(matches!(err(&mut s, |tx| withdraw(tx, &Sender::Session("w-a2".into()), "crew-a", &a.ask, "不是我的")), CollabError::Work(Rejected::Forbidden(_))));
        let w = ok(&mut s, |tx| withdraw(tx, &Sender::Session("cap-a".into()), "crew-a", &a.ask, "不需要了"));
        assert_eq!((w.state, w.withdraw_reason.as_deref()), (AskStateView::Withdrawn, Some("不需要了")));
        assert!(waits(s.conn(), NOW, None).unwrap().is_empty());
    }

    #[test]
    fn defaults_apply_when_due_and_wake_the_asker() {
        let mut s = store();
        let a = ok(&mut s, |tx| open(tx, "w-a1", OpenAsk { default: Some(AskDefault { text: "按周三".into(), after_minutes: 30 }), ..q("周几发版？") }));
        assert!(ok(&mut s, |tx| apply_due_defaults(tx, "crew-a")).is_empty(), "没到点");
        let (done, _) = ok_at(&mut s, NOW + 31 * MIN, |tx| apply_due_defaults(tx, "crew-a"));
        assert_eq!(done, [a.ask.clone()]);
        assert_eq!(get(s.conn(), "crew-a", &a.ask).unwrap().state, AskStateView::Defaulted);
        assert_eq!(crate::wakes::list(s.conn(), Some("w-a1"), 10).unwrap().len(), 1);
    }

    #[test]
    fn the_builtin_crew_does_not_ask() {
        let mut s = store();
        s.conn().execute("INSERT INTO sessions(id,crew_id,role,executor,updated_at) VALUES('chief-1','chief','captain','claude','t')", []).unwrap();
        assert!(matches!(err(&mut s, |tx| open(tx, "chief-1", q("?"))), CollabError::Forbidden(_)));
    }
}
```

`crates/collab/src/approvals.rs`（文件头 + 类型 + 测试）：

```rust
//! 权限审批台账（spec §22.2：同步闸，随这次工具调用失效，不发 H 号）。等待与超时在 M3b 的 pre-tool-use hook。
use crate::messages::{self, NewPost};
use crate::{Change, CollabError, Result, Sender, Tx, channels, crews, prompts, sessions, settings};
use crewd_proto::{Approval, Decision, Mention, MentionKind};
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewApproval {
    pub session_id: String,
    pub tool_name: String,
    pub summary: String,
    pub tool_use_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{err, ok, store};

    fn req() -> NewApproval {
        NewApproval { session_id: "w-b1".into(), tool_name: "computer-use".into(), summary: "点击系统设置".into(), tool_use_id: Some("tu-1".into()) }
    }

    #[test]
    fn request_records_and_announces_in_the_root_human_channel() {
        let mut s = store();
        let a = ok(&mut s, |tx| request(tx, req()));
        assert!(a.id.starts_with("ap-"));
        assert_eq!(a.crew_id, "crew-b");
        assert_eq!(a.decision, None);
        let last = crate::messages::list(s.conn(), "crew-a/human", None, None, 10).unwrap();
        assert_eq!(last.len(), 1);
        assert!(last[0].text.contains("computer-use") && last[0].text.contains("点击系统设置"));
        assert_eq!(last[0].category.as_deref(), Some("approval"));
        assert_eq!(list_pending(s.conn()).unwrap().len(), 1);
    }

    #[test]
    fn decide_once_by_a_known_decider() {
        let mut s = store();
        let a = ok(&mut s, |tx| request(tx, req()));
        assert!(matches!(err(&mut s, |tx| decide(tx, &a.id, true, "someone")), CollabError::Invalid(_)));
        let d = ok(&mut s, |tx| decide(tx, &a.id, true, "human"));
        assert_eq!((d.decision, d.decided_by.as_deref()), (Some(Decision::Allow), Some("human")));
        assert!(matches!(err(&mut s, |tx| decide(tx, &a.id, false, "timeout")), CollabError::Conflict(_)));
        assert!(list_pending(s.conn()).unwrap().is_empty());
        assert!(matches!(err(&mut s, |tx| decide(tx, "ap-nope", true, "human")), CollabError::NotFound(_)));
    }
}
```

`crates/collab/src/restart.rs`（文件头 + 测试）：

```rust
//! 重启后问人是否恢复上次在跑的 session（§22.7 Q1）。这里只记账与公告；恢复由 M3c 订阅 `RestartAnswered` 后做。
use crate::messages::{self, NewPost};
use crate::{Change, CollabError, Result, Sender, Tx, channels, crews, prompts, sessions, settings};
use crewd_proto::{Mention, MentionKind, RestartAnswer, RestartPrompt};
use rusqlite::{Connection, OptionalExtension, params};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{err, ok, ok_at, store, NOW};

    #[test]
    fn open_announces_per_root_and_nothing_to_resume_means_no_prompt() {
        let mut s = store();
        assert_eq!(ok(&mut s, |tx| open(tx, vec![])), None);
        let p = ok(&mut s, |tx| open(tx, vec!["w-a1".into(), "w-b1".into(), "cap-x".into(), "w-a1".into()])).unwrap();
        assert_eq!(p.session_ids, ["w-a1", "w-b1", "cap-x"], "去重、保持顺序");
        let a = crate::messages::list(s.conn(), "crew-a/human", None, None, 10).unwrap();
        assert_eq!(a.len(), 1, "crew-a 与 crew-b 同根，一条公告");
        assert!(a[0].text.contains("名字-w-a1") && a[0].text.contains("名字-w-b1"));
        assert_eq!(crate::messages::list(s.conn(), "crew-x/human", None, None, 10).unwrap().len(), 1);
        assert_eq!(pending(s.conn()).unwrap().unwrap().id, p.id);
        assert!(matches!(err(&mut s, |tx| open(tx, vec!["ghost".into()])), CollabError::NotFound(_)));
    }

    #[test]
    fn later_can_become_resume_but_not_the_other_way() {
        let mut s = store();
        let p = ok(&mut s, |tx| open(tx, vec!["w-a1".into()])).unwrap();
        let (l, ch) = ok_at(&mut s, NOW, |tx| answer(tx, p.id, RestartAnswer::Later));
        assert_eq!(l.answer, Some(RestartAnswer::Later));
        assert!(ch.contains(&Change::RestartAnswered { id: p.id, answer: RestartAnswer::Later, session_ids: vec!["w-a1".into()] }));
        assert_eq!(pending(s.conn()).unwrap().map(|x| x.id), Some(p.id), "「先不」之后人仍可整批恢复");
        let r = ok(&mut s, |tx| answer(tx, p.id, RestartAnswer::Resume));
        assert_eq!(r.answer, Some(RestartAnswer::Resume));
        assert!(pending(s.conn()).unwrap().is_none());
        assert!(matches!(err(&mut s, |tx| answer(tx, p.id, RestartAnswer::Later)), CollabError::Conflict(_)));
    }

    #[test]
    fn a_new_restart_replaces_an_unanswered_prompt() {
        let mut s = store();
        let a = ok(&mut s, |tx| open(tx, vec!["w-a1".into()])).unwrap();
        let b = ok(&mut s, |tx| open(tx, vec!["w-a2".into()])).unwrap();
        assert!(matches!(err(&mut s, |tx| get(tx.conn(), a.id)), CollabError::NotFound(_)));
        assert_eq!(pending(s.conn()).unwrap().unwrap().id, b.id);
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-collab`
Expected: 编译失败。

- [ ] **Step 4: 实现**

`asks.rs`：

```rust
fn ts(t: Ts) -> String {
    crewd_store::ts_from_ms(t.0)
}

fn load(conn: &Connection, crew: &str) -> Result<WorkBook> {
    let state: Option<String> = conn
        .query_row("SELECT state FROM work_books WHERE crew_id=?1", [crew], |r| r.get(0))
        .optional()?;
    match state {
        None => Ok(WorkBook::new(CrewId::new(crew), Config::default())),
        Some(s) => {
            let st: BookState = serde_json::from_str(&s)?;
            WorkBook::restore(st, Config::default())
                .map_err(|v| CollabError::Invalid(format!("机组 {crew} 的工作账本读回不合法：{v}")))
        }
    }
}

/// 在同一个事务里读整本账、跑一条命令、整份写回（M4a 的持久化边界）。M4b 的工作项命令也走这里。
/// 事件日志此时丢弃（M4b 加 v4 的事件表再落）。
pub fn with_book<R>(
    tx: &mut Tx<'_>,
    crew: &str,
    f: impl FnOnce(&mut WorkBook, Ts) -> std::result::Result<R, Rejected>,
) -> Result<R> {
    crews::get(tx.conn(), crew)?;
    let mut book = load(tx.conn(), crew)?;
    let r = f(&mut book, Ts(tx.now_ms())).map_err(CollabError::Work)?;
    let _ = book.take_log();
    tx.conn().execute(
        "INSERT INTO work_books(crew_id,state,updated_at) VALUES(?1,?2,?3)
         ON CONFLICT(crew_id) DO UPDATE SET state=excluded.state, updated_at=excluded.updated_at",
        params![crew, serde_json::to_string(book.state())?, tx.now_ts()],
    )?;
    Ok(r)
}

fn actor_of(conn: &Connection, session: &str) -> Result<Actor> {
    Ok(match sessions::role_of(conn, session)? {
        crewd_proto::Role::Captain => Actor::captain(session),
        crewd_proto::Role::Worker => Actor::worker(session),
    })
}

fn parse_ask(ask: &str) -> Result<AskId> {
    ask.parse().map_err(|e: crewd_work::ParseIdError| CollabError::Invalid(e.to_string()))
}

fn view(conn: &Connection, crew: &str, a: &HumanAsk) -> Result<Ask> {
    let meta: Option<(Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT resume_note, message_id FROM ask_meta WHERE crew_id=?1 AND ask=?2",
            params![crew, a.id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (resume_note, message_id) = meta.unwrap_or((None, None));
    let (state, answer, closed_at, withdraw_reason) = match &a.state {
        AskState::Open => (AskStateView::Open, None, None, None),
        AskState::Answered { text, at } => (AskStateView::Answered, Some(text.clone()), Some(ts(*at)), None),
        AskState::Defaulted { at } => (AskStateView::Defaulted, a.default.as_ref().map(|d| d.text.clone()), Some(ts(*at)), None),
        AskState::Withdrawn { reason, at } => (AskStateView::Withdrawn, None, Some(ts(*at)), Some(reason.clone())),
    };
    Ok(Ask {
        crew_id: crew.into(),
        ask: a.id.to_string(),
        question: a.question.clone(),
        asked_by: a.asked_by.session().map(|s| s.to_string()),
        asked_at: ts(a.asked_at),
        state,
        answer,
        closed_at,
        withdraw_reason,
        default: a.default.as_ref().map(|d| AskDefault { text: d.text.clone(), after_minutes: d.after_ms / 60_000 }),
        item: a.item.map(|w| w.to_string()),
        resume_note,
        message_id,
    })
}

pub fn get(conn: &Connection, crew: &str, ask: &str) -> Result<Ask> {
    let h = parse_ask(ask)?;
    let book = load(conn, crew)?;
    let a = book.ask(h).ok_or_else(|| CollabError::NotFound(format!("机组 {crew} 没有 {ask}")))?;
    view(conn, crew, a)
}

pub fn open(tx: &mut Tx<'_>, asker: &str, o: OpenAsk) -> Result<Ask> {
    let s = sessions::get(tx.conn(), asker)?;
    let crew = crews::get(tx.conn(), &s.crew_id)?;
    if crew.builtin {
        return Err(CollabError::Forbidden("内建总机组不提问（同 McpServer.swift:745-753）".into()));
    }
    let actor = actor_of(tx.conn(), asker)?;
    let question = o.question.trim().to_string();
    let default = o.default.map(|d| DefaultOption { text: d.text, after_ms: d.after_minutes.saturating_mul(60_000) });
    let h = with_book(tx, &crew.id, |b, now| b.open_ask(&actor, now, &question, default, None).map(|a| a.value))?;
    let root = crews::root_of(tx.conn(), &crew.id)?;
    let ch = channels::ensure_human_channel(tx, &root)?;
    let posted = messages::post_internal(
        tx,
        Sender::Session(asker.into()),
        NewPost {
            channel_id: ch.id,
            text: question,
            mentions: vec![Mention { kind: MentionKind::Human, target_id: None }],
            category: Some("ask".into()),
            ..Default::default()
        },
    )?;
    tx.conn().execute(
        "INSERT INTO ask_meta(crew_id,ask,resume_note,message_id) VALUES(?1,?2,?3,?4)",
        params![crew.id, h.to_string(), o.resume_note, posted.message.id],
    )?;
    tx.note(Change::AskChanged { crew_id: crew.id.clone(), ask: h.to_string() });
    get(tx.conn(), &crew.id, &h.to_string())
}

/// 直接开的问被答 / 按默认办了：叫醒提问者；它已无法恢复（或已不在）→ 叫它机组的机长转达（§22.4）。
fn wake_asker(tx: &mut Tx<'_>, crew: &str, a: &Ask) -> Result<()> {
    let Some(asker) = &a.asked_by else { return Ok(()) };
    let target = match crewd_store::sessions::get(tx.conn(), asker)? {
        Some(s) if s.resume_failed_at.is_none() => Some(s.id),
        _ => crews::captain_of(tx.conn(), crew)?.filter(|c| c != asker),
    };
    if let Some(t) = target {
        wakes::enqueue(
            tx,
            WakeRequest {
                session: t,
                kind: WakeKind::AskAnswer,
                source: wakes::source::ask(crew, &a.ask),
                priority: WakePriority::Human,
                not_before_ms: None,
                note: None,
                origin: None,
            },
        )?;
    }
    Ok(())
}

pub fn answer(tx: &mut Tx<'_>, crew: &str, ask: &str, text: &str) -> Result<Ask> {
    let h = parse_ask(ask)?;
    let (applied, standalone) = with_book(tx, crew, |b, now| {
        let standalone = b.ask(h).map(|a| a.standalone).unwrap_or(false);
        b.answer_ask(&Actor::Human, now, h, text).map(|ap| (ap, standalone))
    })?;
    let cleared: Vec<(String, Option<String>)> = applied
        .events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::BlockerCleared { id, owner, .. } => Some((id.to_string(), owner.as_ref().map(|o| o.to_string()))),
            _ => None,
        })
        .collect();
    let a = get(tx.conn(), crew, ask)?;
    if standalone {
        wake_asker(tx, crew, &a)?;
    }
    tx.note(Change::AskChanged { crew_id: crew.into(), ask: a.ask.clone() });
    tx.note(Change::AskAnswered { crew_id: crew.into(), ask: a.ask.clone(), asked_by: a.asked_by.clone(), cleared });
    Ok(a)
}

pub fn withdraw(tx: &mut Tx<'_>, by: &Sender, crew: &str, ask: &str, reason: &str) -> Result<Ask> {
    let h = parse_ask(ask)?;
    let actor = match by {
        Sender::Human => Actor::Human,
        Sender::Session(s) => actor_of(tx.conn(), s)?,
        Sender::System => return Err(CollabError::Forbidden("系统不撤回问".into())),
    };
    with_book(tx, crew, |b, now| b.withdraw_ask(&actor, now, h, reason))?;
    tx.note(Change::AskChanged { crew_id: crew.into(), ask: ask.into() });
    get(tx.conn(), crew, ask)
}

/// 默认方案到点的问按默认办（调度层调用；谁来定时调它见 spec 问题 5）。返回办了的 H 号。
pub fn apply_due_defaults(tx: &mut Tx<'_>, crew: &str) -> Result<Vec<String>> {
    let done: Vec<(AskId, bool)> = with_book(tx, crew, |b, now| {
        let mut out = Vec::new();
        for h in b.defaults_due(now) {
            let standalone = b.ask(h).map(|a| a.standalone).unwrap_or(false);
            b.apply_default(&Actor::System, now, h)?;
            out.push((h, standalone));
        }
        Ok(out)
    })?;
    let mut ids = Vec::new();
    for (h, standalone) in done {
        let a = get(tx.conn(), crew, &h.to_string())?;
        if standalone {
            wake_asker(tx, crew, &a)?;
        }
        tx.note(Change::AskChanged { crew_id: crew.into(), ask: a.ask.clone() });
        ids.push(a.ask);
    }
    Ok(ids)
}

/// 人的待确认清单（`ask.list`）：`root` 给了就只看这棵树里的机组。等得久的在前。
pub fn waits(conn: &Connection, now_ms: i64, root: Option<&str>) -> Result<Vec<HumanWait>> {
    let mut st = conn.prepare("SELECT crew_id FROM work_books ORDER BY crew_id")?;
    let crews_with_books: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let mut out = Vec::new();
    for crew in crews_with_books {
        if let Some(r) = root {
            if crews::root_of(conn, &crew)? != r {
                continue;
            }
        }
        let book = load(conn, &crew)?;
        for w in book.human_waits(Ts(now_ms)) {
            let a = book.ask(w.ask).expect("human_waits 只列账本里的问");
            out.push(HumanWait {
                ask: view(conn, &crew, a)?,
                items: w.items.iter().map(|i| i.to_string()).collect(),
                owners: w.owners.iter().map(|o| o.to_string()).collect(),
                waited_ms: w.waited_ms,
                tier: match w.tier {
                    WaitTier::Fresh => WaitTierView::Fresh,
                    WaitTier::Pinned => WaitTierView::Pinned,
                    WaitTier::StillWanted => WaitTierView::StillWanted,
                },
                default_due_at: w.default_due_at.map(ts),
            });
        }
    }
    out.sort_by(|a, b| b.waited_ms.cmp(&a.waited_ms).then(a.ask.crew_id.cmp(&b.ask.crew_id)).then(a.ask.ask.cmp(&b.ask.ask)));
    Ok(out)
}
```

`approvals.rs`：

```rust
const COLS: &str = "id,session_id,crew_id,tool_name,summary,tool_use_id,requested_at,decision,decided_at,decided_by";
const DECIDERS: &[&str] = &["human", "timeout", "unreachable"];

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Approval> {
    let decision: Option<String> = r.get(7)?;
    Ok(Approval {
        id: r.get(0)?,
        session_id: r.get(1)?,
        crew_id: r.get(2)?,
        tool_name: r.get(3)?,
        summary: r.get(4)?,
        tool_use_id: r.get(5)?,
        requested_at: r.get(6)?,
        decision: decision.as_deref().and_then(Decision::parse),
        decided_at: r.get(8)?,
        decided_by: r.get(9)?,
    })
}

pub fn get(conn: &Connection, id: &str) -> Result<Approval> {
    conn.query_row(&format!("SELECT {COLS} FROM approvals WHERE id=?1"), [id], row)
        .optional()?
        .ok_or_else(|| CollabError::NotFound(format!("没有审批 {id}")))
}

pub fn list_pending(conn: &Connection) -> Result<Vec<Approval>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM approvals WHERE decision IS NULL ORDER BY requested_at, id"))?;
    Ok(st.query_map([], row)?.collect::<rusqlite::Result<_>>()?)
}

pub fn request(tx: &mut Tx<'_>, n: NewApproval) -> Result<Approval> {
    let s = sessions::get(tx.conn(), &n.session_id)?;
    if n.tool_name.trim().is_empty() {
        return Err(CollabError::Invalid("工具名不能为空".into()));
    }
    let id = format!("ap-{}", uuid::Uuid::new_v4().simple());
    tx.conn().execute(
        "INSERT INTO approvals(id,session_id,crew_id,tool_name,summary,tool_use_id,requested_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![id, s.id, s.crew_id, n.tool_name, n.summary, n.tool_use_id, tx.now_ts()],
    )?;
    let root = crews::root_of(tx.conn(), &s.crew_id)?;
    let ch = channels::ensure_human_channel(tx, &root)?;
    let f = prompts::render_file(settings::locale(tx.conn())?);
    let name = sessions::display_name(tx.conn(), &s.id)?;
    let text = prompts::fill(f.get("notice.approval"), &[("session", &name), ("tool", &n.tool_name), ("summary", &n.summary)]);
    messages::post_internal(
        tx,
        Sender::System,
        NewPost {
            channel_id: ch.id,
            text,
            mentions: vec![Mention { kind: MentionKind::Human, target_id: None }],
            category: Some("approval".into()),
            ..Default::default()
        },
    )?;
    tx.note(Change::ApprovalChanged { id: id.clone() });
    get(tx.conn(), &id)
}

pub fn decide(tx: &mut Tx<'_>, id: &str, allow: bool, by: &str) -> Result<Approval> {
    if !DECIDERS.contains(&by) {
        return Err(CollabError::Invalid(format!("decided_by 只能是 {DECIDERS:?}")));
    }
    let a = get(tx.conn(), id)?;
    if a.decision.is_some() {
        return Err(CollabError::Conflict(format!("审批 {id} 已经有结果")));
    }
    let d = if allow { Decision::Allow } else { Decision::Deny };
    tx.conn().execute(
        "UPDATE approvals SET decision=?2, decided_at=?3, decided_by=?4 WHERE id=?1",
        params![id, d.as_str(), tx.now_ts(), by],
    )?;
    tx.note(Change::ApprovalChanged { id: id.into() });
    get(tx.conn(), id)
}
```

`restart.rs`：

```rust
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RestartPrompt> {
    let ids: String = r.get(2)?;
    let answer: Option<String> = r.get(3)?;
    Ok(RestartPrompt {
        id: r.get(0)?,
        created_at: r.get(1)?,
        session_ids: serde_json::from_str(&ids).map_err(|e| rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e)))?,
        answer: answer.as_deref().and_then(RestartAnswer::parse),
        answered_at: r.get(4)?,
    })
}

const COLS: &str = "id,created_at,session_ids,answer,answered_at";

pub fn get(conn: &Connection, id: i64) -> Result<RestartPrompt> {
    conn.query_row(&format!("SELECT {COLS} FROM restart_prompts WHERE id=?1"), [id], row)
        .optional()?
        .ok_or_else(|| CollabError::NotFound(format!("没有重启询问 {id}")))
}

/// 还要人拍板的那一条（没答，或答了「先不」）。
pub fn pending(conn: &Connection) -> Result<Option<RestartPrompt>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLS} FROM restart_prompts WHERE answer IS NULL OR answer='later' ORDER BY id DESC LIMIT 1"),
            [],
            row,
        )
        .optional()?)
}

pub fn open(tx: &mut Tx<'_>, session_ids: Vec<String>) -> Result<Option<RestartPrompt>> {
    let mut ids: Vec<String> = Vec::new();
    for id in session_ids {
        sessions::get(tx.conn(), &id)?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return Ok(None);
    }
    tx.conn().execute("DELETE FROM restart_prompts WHERE answer IS NULL", [])?;
    tx.conn().execute(
        "INSERT INTO restart_prompts(created_at,session_ids) VALUES(?1,?2)",
        params![tx.now_ts(), serde_json::to_string(&ids)?],
    )?;
    let id = tx.conn().last_insert_rowid();
    // 每个涉及的根机组人类群各一条，只列那棵树里的 session。
    let f = prompts::render_file(settings::locale(tx.conn())?);
    let mut by_root: Vec<(String, Vec<String>)> = Vec::new();
    for sid in &ids {
        let root = crews::root_of(tx.conn(), &sessions::crew_of(tx.conn(), sid)?)?;
        match by_root.iter_mut().find(|(r, _)| *r == root) {
            Some((_, v)) => v.push(sid.clone()),
            None => by_root.push((root, vec![sid.clone()])),
        }
    }
    for (root, sids) in by_root {
        let ch = channels::ensure_human_channel(tx, &root)?;
        let mut text = prompts::fill(f.get("notice.restart"), &[("count", &sids.len().to_string())]);
        for sid in &sids {
            text.push('\n');
            text.push_str(&prompts::fill(f.get("notice.restart.item"), &[("name", &sessions::display_name(tx.conn(), sid)?)]));
        }
        messages::post_internal(
            tx,
            Sender::System,
            NewPost {
                channel_id: ch.id,
                text,
                mentions: vec![Mention { kind: MentionKind::Human, target_id: None }],
                category: Some("restart".into()),
                ..Default::default()
            },
        )?;
    }
    tx.note(Change::RestartChanged { id });
    Ok(Some(get(tx.conn(), id)?))
}

pub fn answer(tx: &mut Tx<'_>, id: i64, a: RestartAnswer) -> Result<RestartPrompt> {
    let p = get(tx.conn(), id)?;
    match (p.answer, a) {
        (Some(RestartAnswer::Resume), _) => return Err(CollabError::Conflict("已经答过「恢复」".into())),
        (Some(RestartAnswer::Later), RestartAnswer::Later) => return Ok(p),
        _ => {}
    }
    tx.conn().execute(
        "UPDATE restart_prompts SET answer=?2, answered_at=?3 WHERE id=?1",
        params![id, a.as_str(), tx.now_ts()],
    )?;
    tx.note(Change::RestartChanged { id });
    tx.note(Change::RestartAnswered { id, answer: a, session_ids: p.session_ids.clone() });
    get(tx.conn(), id)
}
```

（`restart.rs` 的测试 `a_new_restart_replaces_an_unanswered_prompt` 里 `err(&mut s, |tx| get(tx.conn(), a.id))`：`get` 是读函数，借 `tx.conn()` 调用即可。`approvals.rs` / `restart.rs` 的 `use` 里没用到的项按 clippy 删。）

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-collab`
Expected: 57 + prompts 3 + asks 8 + approvals 2 + restart 3 = 73 passed。

- [ ] **Step 6: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/collab/Cargo.toml crates/collab/src/lib.rs crates/collab/src/asks.rs crates/collab/src/approvals.rs crates/collab/src/restart.rs crates/collab/src/prompts.rs crates/collab/prompts/render.zh.md crates/collab/prompts/render.en.md Cargo.lock
git diff --cached --stat
git commit -m "crewd-collab: human asks via the work book (H numbers), approvals ledger, restart prompt; prompt files

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 11: 渲染（注入块、唤醒正文、出队现渲染）+ 世界观 / 回合收尾的签名桩

`render` 只做两件事：把库里的东西整理成结构（`blocks_for`、`build_batch`，读库），再按 md 模板拼字符串（`injection`、`wake_text`，纯函数）。M3b 的 hook 调 `unread_for` → `blocks_for` → `injection`，注入成功后 `advance`；M3c 出队时 `build_batch` → `wake_text`，`unusable` 里的行 `mark_dropped`。`worldview` / `turn_trace` 只定签名（§22.5「M3a 先放 stub」），正文归 M3b。

**Files:**
- Create: `crewd/crates/collab/src/render.rs`、`crewd/crates/collab/src/worldview.rs`、`crewd/crates/collab/src/turn_trace.rs`
- Modify: `crewd/crates/collab/src/lib.rs`（`pub mod render; pub mod turn_trace; pub mod worldview;`）、`crewd/crates/collab/src/prompts.rs`（`REQUIRED` 加节）、`crewd/crates/collab/prompts/render.zh.md`、`crewd/crates/collab/prompts/render.en.md`

**Interfaces:**
- Consumes: Task 5–10。
- Produces：
  - `render::{MsgLine { seq, at, sender, text, directed_to: Option<Vec<String>>, reply_to: Option<String> }, ChannelBlock { channel_id, kind, name, crew_title, lines }, WakeBody, WakeItem { wake_ids: Vec<i64>, kind, note, body }, WakeBatch { session_id, items, unusable: Vec<(i64, String)> }, blocks_for(conn, session, &[(String, Vec<Message>)]) -> Result<Vec<ChannelBlock>>, injection(Locale, &[ChannelBlock]) -> String, wake_text(Locale, &WakeBatch) -> String, build_batch(conn, session, &[WakeRow]) -> Result<WakeBatch>}`。`WakeBody = Messages(Vec<ChannelBlock>) | Timer | AskAnswered { ask, question, answer, resume_note } | AskDefaulted { ask, question, answer, resume_note } | Restart | System | Checkpoint | StopCheck`。
  - `worldview::{WorldCtx { locale, session_id, role, executor, brief, crew_id, crew_title, working_directory, captain_name, parent_titles, child_titles, channels, human_watching }, context_for(conn, session, Locale) -> Result<WorldCtx>, render(&WorldCtx) -> String}`（M3a 的 `render` 返回空串，M3b 实现）。
  - `turn_trace::{TurnEnd { Nothing, Posted { message_id }, AlreadyCalled }, on_turn_end(tx, session, prompt_id, last_text) -> Result<TurnEnd>}`（M3a 恒返回 `Nothing`，M3b 实现）。

- [ ] **Step 1: 加提示词节（两份都加，节名一致）**

`render.zh.md` 末尾追加：

```markdown
## sender.human
人类

## sender.system
crewd

## inject.header
以下是你所在的群里的新消息（只含与你相关、你看得见的）：

## inject.channel.lobby
【{crew} · 大厅】

## inject.channel.human
【{crew} · 人类群】

## inject.channel.topic
【群：{name}】

## inject.channel.dm
【私聊】

## line
[{at}] {sender}：{text}

## line.directed
（发给 {targets} 的，不是派给你的）

## line.reply
（回复 {reply}）

## wake.header
你被叫醒了，原因如下。

## wake.message
群里有需要你看的消息：

## wake.timer
你之前约的提醒到了。

## wake.ask_answer
人回答了你的问题 {ask}。
问题：{question}
答复：{answer}

## wake.ask_defaulted
你的问题 {ask} 到时没人回复，按默认方案办：{answer}
问题：{question}
到点后的动作照常走权限审批；一旦弹出审批框就停下，转回人。

## wake.resume_note
你当时留的接续说明：{note}

## wake.restart
crewd 重启打断了你上一轮的工作。先核对当前状态（工作区改动、群里的新消息），再继续。

## wake.system
系统通知。

## wake.checkpoint
检查点到了。

## wake.stop_check
停下之前，请确认名下工作项都有下一步。

## wake.note
附言：{note}
```

`render.en.md` 末尾追加同名各节：

```markdown
## sender.human
Human

## sender.system
crewd

## inject.header
New messages in your channels (only those addressed to or visible to you):

## inject.channel.lobby
[{crew} · lobby]

## inject.channel.human
[{crew} · human channel]

## inject.channel.topic
[channel: {name}]

## inject.channel.dm
[direct message]

## line
[{at}] {sender}: {text}

## line.directed
(addressed to {targets}, not to you)

## line.reply
(reply to {reply})

## wake.header
You were woken up for the following.

## wake.message
Messages that need your attention:

## wake.timer
A reminder you scheduled is due.

## wake.ask_answer
The human answered your question {ask}.
Question: {question}
Answer: {answer}

## wake.ask_defaulted
Nobody answered your question {ask} in time; going with the default: {answer}
Question: {question}
Actions after the deadline still go through permission approval; if an approval prompt appears, stop and hand it back to the human.

## wake.resume_note
Your note on how to continue: {note}

## wake.restart
A crewd restart interrupted your last turn. Check the current state (working tree, new channel messages) before continuing.

## wake.system
System notice.

## wake.checkpoint
A checkpoint is due.

## wake.stop_check
Before stopping, make sure every work item you own has a next step.

## wake.note
Note: {note}
```

`prompts.rs` 的 `REQUIRED` 追加：

```rust
    ("sender.human", &[]),
    ("sender.system", &[]),
    ("inject.header", &[]),
    ("inject.channel.lobby", &["crew"]),
    ("inject.channel.human", &["crew"]),
    ("inject.channel.topic", &["name"]),
    ("inject.channel.dm", &[]),
    ("line", &["at", "sender", "text"]),
    ("line.directed", &["targets"]),
    ("line.reply", &["reply"]),
    ("wake.header", &[]),
    ("wake.message", &[]),
    ("wake.timer", &[]),
    ("wake.ask_answer", &["ask", "question", "answer"]),
    ("wake.ask_defaulted", &["ask", "question", "answer"]),
    ("wake.resume_note", &["note"]),
    ("wake.restart", &[]),
    ("wake.system", &[]),
    ("wake.checkpoint", &[]),
    ("wake.stop_check", &[]),
    ("wake.note", &["note"]),
```

- [ ] **Step 2: 写失败的测试**

`crates/collab/src/render.rs`（文件头 + 类型 + 测试）：

```rust
//! 给 agent 看的渲染：注入块（hook / codex additionalContext）与唤醒正文（spec §22.2、§22.5）。
//! 文字只来自 prompts/render.*.md；本文件（测试以外）不含任何自然语言字面量（测试钉住）。
use crate::prompts::{fill, render_file};
use crate::wakes::{WakeKind, WakeRow};
use crate::{Locale, Result, asks, channels, crews, cursors, messages, sessions};
use crewd_proto::{AskStateView, ChannelKind, Message, SenderKind};
use rusqlite::Connection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsgLine {
    pub seq: i64,
    pub at: String,
    pub sender: String,
    pub text: String,
    /// 看得见但不是给我的：目标的显示名。
    pub directed_to: Option<Vec<String>>,
    /// 回复的是谁（显示名）。
    pub reply_to: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelBlock {
    pub channel_id: String,
    pub kind: ChannelKind,
    pub name: String,
    pub crew_title: String,
    pub lines: Vec<MsgLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeBody {
    Messages(Vec<ChannelBlock>),
    Timer,
    AskAnswered { ask: String, question: String, answer: String, resume_note: Option<String> },
    AskDefaulted { ask: String, question: String, answer: String, resume_note: Option<String> },
    Restart,
    System,
    Checkpoint,
    StopCheck,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeItem {
    /// 合并进这一项的队列行（消息类会把多行合成一项）。
    pub wake_ids: Vec<i64>,
    pub kind: WakeKind,
    pub note: Option<String>,
    pub body: WakeBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeBatch {
    pub session_id: String,
    pub items: Vec<WakeItem>,
    /// 渲染不出来的行（消息已删、问已撤回……）与原因：M3c 记 `mark_dropped`。
    pub unusable: Vec<(i64, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Sender;
    use crate::messages::{NewPost, post};
    use crate::testkit::{NOW, ok, store};
    use crate::wakes::{Enqueued, WakePriority, WakeRequest, enqueue, list};
    use crewd_proto::{Mention, MentionKind};

    fn say(s: &mut crewd_store::Store, who: Sender, ch: &str, text: &str, mentions: Vec<Mention>, reply: Option<String>) -> Message {
        ok(s, |tx| post(tx, who, NewPost { channel_id: ch.into(), text: text.into(), mentions, in_reply_to: reply, ..Default::default() })).message
    }

    #[test]
    fn injection_names_channels_senders_and_marks_messages_meant_for_others() {
        let mut s = store();
        let q = say(&mut s, Sender::Human, "crew-a/lobby", "今天发版吗？", vec![], None);
        say(&mut s, Sender::Session("w-a1".into()), "crew-a/lobby", "我来查", vec![Mention { kind: MentionKind::Broadcast, target_id: None }, Mention { kind: MentionKind::Session, target_id: Some("w-a2".into()) }], Some(q.id.clone()));
        let unread = ok(&mut s, |tx| crate::cursors::unread_for(tx, "cap-a"));
        let blocks = blocks_for(s.conn(), "cap-a", &unread).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].crew_title, "机组甲");
        assert_eq!(blocks[0].lines[0].sender, "人类");
        assert_eq!(blocks[0].lines[1].directed_to, Some(vec!["名字-w-a2".to_string()]));
        assert_eq!(blocks[0].lines[1].reply_to.as_deref(), Some("人类"));
        let text = injection(Locale::Zh, &blocks);
        assert!(text.contains("【机组甲 · 大厅】"), "{text}");
        assert!(text.contains("人类：今天发版吗？"), "{text}");
        assert!(text.contains("（发给 名字-w-a2 的，不是派给你的）"), "{text}");
        assert!(injection(Locale::En, &blocks).contains("[机组甲 · lobby]"));
        assert_eq!(injection(Locale::Zh, &[]), "", "没有未读 = 不注入");
    }

    #[test]
    fn build_batch_merges_messages_and_resolves_ask_answers() {
        let mut s = store();
        let m1 = say(&mut s, Sender::Session("w-a2".into()), "crew-a/lobby", "第一条", vec![], None);
        let m2 = say(&mut s, Sender::Session("w-a2".into()), "crew-a/lobby", "第二条", vec![], None);
        let a = ok(&mut s, |tx| crate::asks::open(tx, "w-a1", crate::asks::OpenAsk { question: "用哪家？".into(), resume_note: Some("接着接入".into()), ..Default::default() }));
        ok(&mut s, |tx| crate::asks::answer(tx, "crew-a", &a.ask, "用甲"));
        ok(&mut s, |tx| {
            for m in [&m1, &m2] {
                enqueue(tx, WakeRequest { session: "w-a1".into(), kind: WakeKind::Message, source: crate::wakes::source::message(&m.id), priority: WakePriority::Auto, not_before_ms: None, note: None, origin: Some("w-a2".into()) })?;
            }
            let r: Result<Enqueued> = enqueue(tx, WakeRequest { session: "w-a1".into(), kind: WakeKind::Message, source: crate::wakes::source::message("gone"), priority: WakePriority::Auto, not_before_ms: None, note: None, origin: None });
            r
        });
        let rows = crate::wakes::due_for(s.conn(), "w-a1", NOW).unwrap();
        let batch = build_batch(s.conn(), "w-a1", &rows).unwrap();
        assert_eq!(batch.unusable.len(), 1, "消息不存在的那一行");
        let kinds: Vec<WakeKind> = batch.items.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, [WakeKind::AskAnswer, WakeKind::Message], "人回答的优先级高，排前面");
        assert_eq!(batch.items[1].wake_ids.len(), 2, "两条消息合成一项");
        let text = wake_text(Locale::Zh, &batch);
        assert!(text.contains("答复：用甲") && text.contains("接续说明：接着接入"), "{text}");
        assert!(text.contains("第一条") && text.contains("第二条"), "{text}");
        assert_eq!(list(s.conn(), Some("w-a1"), 10).unwrap().len(), 4);
    }

    #[test]
    fn render_code_has_no_prose() {
        let src = include_str!("render.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        let prose: Vec<char> = code
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .flat_map(|l| l.chars())
            .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c) || ('\u{3000}'..='\u{303f}').contains(c) || ('\u{ff00}'..='\u{ffef}').contains(c))
            .collect();
        assert!(prose.is_empty(), "render.rs 代码里出现了中文字面量（应放进 prompts/render.*.md）：{prose:?}");
    }
}
```

`crates/collab/src/worldview.rs`（整个文件）：

```rust
//! 世界观的上下文与渲染签名（spec §22.5：M3a 定签名，M3b 写 prompts/world-model.*.md 与 `render` 的正文）。
//! 变量照 `Sources/Stores/LocalSessionWorldModel.swift:73-90`；M3b 可以往 `WorldCtx` 加字段（文件归 M3b）。
use crate::{Locale, Result, channels, crews, sessions};
use crewd_proto::Role;
use rusqlite::Connection;

#[derive(Debug, Clone, PartialEq)]
pub struct WorldCtx {
    pub locale: Locale,
    pub session_id: String,
    pub role: Role,
    pub executor: String,
    pub brief: String,
    pub crew_id: String,
    pub crew_title: String,
    pub working_directory: String,
    pub captain_name: Option<String>,
    pub parent_titles: Vec<String>,
    pub child_titles: Vec<String>,
    /// 它在的群（id）。
    pub channels: Vec<String>,
    /// 找人类该发到哪：人类正在注视的群 + 根机组人类群（§17）。
    pub human_watching: Vec<String>,
}

pub fn context_for(conn: &Connection, session: &str, locale: Locale) -> Result<WorldCtx> {
    let s = sessions::get(conn, session)?;
    let crew = crews::get(conn, &s.crew_id)?;
    let titles = |ids: Vec<String>| -> Result<Vec<String>> { ids.iter().map(|i| Ok(crews::get(conn, i)?.title)).collect() };
    let mut watching: Vec<String> = {
        let mut st = conn.prepare("SELECT channel_id FROM channel_watch ORDER BY since, channel_id")?;
        st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    let root_human = crewd_store::channels::human_id(&crews::root_of(conn, &crew.id)?);
    if !watching.contains(&root_human) {
        watching.push(root_human);
    }
    Ok(WorldCtx {
        locale,
        session_id: s.id.clone(),
        role: sessions::role_of(conn, &s.id)?,
        executor: s.executor.clone(),
        brief: s.brief.clone().unwrap_or_default(),
        crew_id: crew.id.clone(),
        crew_title: crew.title.clone(),
        working_directory: s.working_directory.clone().or(crew.working_directory.clone()).unwrap_or_default(),
        captain_name: crews::captain_of(conn, &crew.id)?.map(|c| sessions::display_name(conn, &c)).transpose()?,
        parent_titles: titles(crews::parents(conn, &crew.id)?)?,
        child_titles: titles(crews::children(conn, &crew.id)?)?,
        channels: channels::channels_of_session(conn, &s.id)?.into_iter().map(|c| c.id).collect(),
        human_watching: watching,
    })
}

/// M3b 实现（读 prompts/world-model.{zh,en}.md）。M3a 返回空串：claude 的 `--append-system-prompt-file` 得到空文件。
/// M3 集成验收必须核「世界观非空」（见计划 Task 15 交付记录的遗留）。
pub fn render(_ctx: &WorldCtx) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{ok, store};

    #[test]
    fn context_collects_crew_lineage_channels_and_where_to_find_the_human() {
        let mut s = store();
        ok(&mut s, |tx| crate::channels::watch(tx, "crew-b/lobby", true));
        let c = context_for(s.conn(), "w-b1", Locale::Zh).unwrap();
        assert_eq!(c.crew_title, "机组乙");
        assert_eq!(c.role, Role::Worker);
        assert_eq!(c.captain_name.as_deref(), Some("名字-cap-b"));
        assert_eq!(c.parent_titles, ["机组甲"]);
        assert_eq!(c.channels, ["crew-b/lobby"]);
        assert_eq!(c.human_watching, ["crew-b/lobby", "crew-a/human"]);
    }

    /// M3b 实现 `render` 时删掉这条。
    #[test]
    fn render_is_a_stub_until_m3b() {
        let s = store();
        assert_eq!(render(&context_for(s.conn(), "w-a1", Locale::Zh).unwrap()), "");
    }
}
```

`crates/collab/src/turn_trace.rs`（整个文件）：

```rust
//! 回合收尾停在没叫到人的问句上 → 代发（spec §22.3，移植 `Sources/Mcp/SessionTurnTrace.swift`）。
//! M3a 只定签名；M3b 实现（按 prompt_id 去重、永不阻塞、写 `sessions.awaiting_question`）。
use crate::{Result, Tx};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEnd {
    /// 不需要代发。
    Nothing,
    /// 代发了一条（worker → 大厅 @机长；机长 → 根机组人类群 @人）。
    Posted { message_id: String },
    /// 这一轮已经 @ 过能答的人，或同一 prompt_id 已代发过。
    AlreadyCalled,
}

pub fn on_turn_end(_tx: &mut Tx<'_>, _session: &str, _prompt_id: &str, _last_text: &str) -> Result<TurnEnd> {
    Ok(TurnEnd::Nothing)
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-collab render::`
Expected: 编译失败（`blocks_for` 等不存在）。

- [ ] **Step 4: 实现 render**

`render.rs`，类型之后：

```rust
fn sender_name(conn: &Connection, l: Locale, m: &Message) -> Result<String> {
    let f = render_file(l);
    Ok(match m.sender_kind {
        SenderKind::Human => f.get("sender.human").to_string(),
        SenderKind::System => f.get("sender.system").to_string(),
        SenderKind::Session | SenderKind::Captain => match (&m.sender_name, &m.sender_session_id) {
            (Some(n), _) => n.clone(),
            (None, Some(id)) => sessions::display_name(conn, id).unwrap_or_else(|_| id.clone()),
            (None, None) => String::new(),
        },
    })
}

/// 未读（或唤醒带的消息）→ 按群分块，标出「发给别人的」与回复对象。
pub fn blocks_for(conn: &Connection, session: &str, unread: &[(String, Vec<Message>)]) -> Result<Vec<ChannelBlock>> {
    let l = crate::settings::locale(conn)?;
    let mut out = Vec::new();
    for (ch_id, msgs) in unread {
        if msgs.is_empty() {
            continue;
        }
        let ch = channels::get(conn, ch_id)?;
        let crew_title = crews::get(conn, &ch.crew_id)?.title;
        let mut lines = Vec::new();
        for m in msgs {
            let directed_to = match cursors::directed_targets(m, session) {
                None => None,
                Some(ids) => Some(ids.iter().map(|i| sessions::display_name(conn, i).unwrap_or_else(|_| i.clone())).collect()),
            };
            let reply_to = match &m.in_reply_to {
                None => None,
                Some(rid) => match messages::get(conn, rid) {
                    Ok(r) => Some(sender_name(conn, l, &r)?),
                    Err(_) => None,
                },
            };
            lines.push(MsgLine { seq: m.seq, at: m.created_at.clone(), sender: sender_name(conn, l, m)?, text: m.text.clone(), directed_to, reply_to });
        }
        out.push(ChannelBlock { channel_id: ch.id, kind: ch.kind, name: ch.name, crew_title, lines });
    }
    Ok(out)
}

fn block_text(l: Locale, b: &ChannelBlock) -> String {
    let f = render_file(l);
    let head = match b.kind {
        ChannelKind::Lobby => fill(f.get("inject.channel.lobby"), &[("crew", &b.crew_title)]),
        ChannelKind::Human => fill(f.get("inject.channel.human"), &[("crew", &b.crew_title)]),
        ChannelKind::Topic => fill(f.get("inject.channel.topic"), &[("name", &b.name)]),
        ChannelKind::Dm => f.get("inject.channel.dm").to_string(),
    };
    let mut out = head;
    for ln in &b.lines {
        out.push('\n');
        if let Some(t) = &ln.directed_to {
            out.push_str(&fill(f.get("line.directed"), &[("targets", &t.join(", "))]));
        }
        if let Some(r) = &ln.reply_to {
            out.push_str(&fill(f.get("line.reply"), &[("reply", r)]));
        }
        out.push_str(&fill(f.get("line"), &[("at", &ln.at), ("sender", &ln.sender), ("text", &ln.text)]));
    }
    out
}

/// 注入块。没有消息 = 空串（调用方据此不注入）。
pub fn injection(l: Locale, blocks: &[ChannelBlock]) -> String {
    let blocks: Vec<&ChannelBlock> = blocks.iter().filter(|b| !b.lines.is_empty()).collect();
    if blocks.is_empty() {
        return String::new();
    }
    let mut out = render_file(l).get("inject.header").to_string();
    for b in blocks {
        out.push_str("\n\n");
        out.push_str(&block_text(l, b));
    }
    out
}

/// 一次投递的唤醒正文（M3c 合并同一档优先级后调用）。
pub fn wake_text(l: Locale, batch: &WakeBatch) -> String {
    let f = render_file(l);
    let mut out = f.get("wake.header").to_string();
    for it in &batch.items {
        out.push_str("\n\n");
        match &it.body {
            WakeBody::Messages(blocks) => {
                out.push_str(f.get("wake.message"));
                for b in blocks {
                    out.push_str("\n\n");
                    out.push_str(&block_text(l, b));
                }
            }
            WakeBody::AskAnswered { ask, question, answer, resume_note } | WakeBody::AskDefaulted { ask, question, answer, resume_note } => {
                let key = if matches!(it.body, WakeBody::AskAnswered { .. }) { "wake.ask_answer" } else { "wake.ask_defaulted" };
                out.push_str(&fill(f.get(key), &[("ask", ask), ("question", question), ("answer", answer)]));
                if let Some(n) = resume_note {
                    out.push('\n');
                    out.push_str(&fill(f.get("wake.resume_note"), &[("note", n)]));
                }
            }
            WakeBody::Timer => out.push_str(f.get("wake.timer")),
            WakeBody::Restart => out.push_str(f.get("wake.restart")),
            WakeBody::System => out.push_str(f.get("wake.system")),
            WakeBody::Checkpoint => out.push_str(f.get("wake.checkpoint")),
            WakeBody::StopCheck => out.push_str(f.get("wake.stop_check")),
        }
        if let Some(n) = &it.note {
            out.push('\n');
            out.push_str(&fill(f.get("wake.note"), &[("note", n)]));
        }
    }
    out
}

/// 队列行 → 可渲染的批（出队时现渲染，spec §22.4）。行的顺序即输出顺序（`due_for` 已按优先级排好）；
/// 消息类合成一项，放在第一条消息行出现的位置。渲染不了的行进 `unusable`。
pub fn build_batch(conn: &Connection, session: &str, rows: &[WakeRow]) -> Result<WakeBatch> {
    let mut items: Vec<WakeItem> = Vec::new();
    let mut unusable = Vec::new();
    let mut msg_slot: Option<usize> = None;
    let mut msgs: Vec<(String, Vec<Message>)> = Vec::new();
    for r in rows {
        match r.kind {
            WakeKind::Message => {
                let Some(id) = r.source.strip_prefix("msg:") else {
                    unusable.push((r.id, format!("source {:?} is not a message", r.source)));
                    continue;
                };
                let Ok(m) = messages::get(conn, id) else {
                    unusable.push((r.id, format!("message {id} no longer exists")));
                    continue;
                };
                match msgs.iter_mut().find(|(c, _)| *c == m.channel_id) {
                    Some((_, v)) => v.push(m),
                    None => msgs.push((m.channel_id.clone(), vec![m])),
                }
                match msg_slot {
                    Some(i) => items[i].wake_ids.push(r.id),
                    None => {
                        msg_slot = Some(items.len());
                        items.push(WakeItem { wake_ids: vec![r.id], kind: WakeKind::Message, note: None, body: WakeBody::Messages(Vec::new()) });
                    }
                }
            }
            WakeKind::AskAnswer => {
                let parsed = r.source.strip_prefix("ask:").and_then(|s| s.rsplit_once('/'));
                let Some((crew, ask)) = parsed else {
                    unusable.push((r.id, format!("source {:?} is not an ask", r.source)));
                    continue;
                };
                let a = match asks::get(conn, crew, ask) {
                    Ok(a) => a,
                    Err(e) => {
                        unusable.push((r.id, e.to_string()));
                        continue;
                    }
                };
                let answer = a.answer.clone().unwrap_or_default();
                let body = match a.state {
                    AskStateView::Answered => WakeBody::AskAnswered { ask: a.ask, question: a.question, answer, resume_note: a.resume_note },
                    AskStateView::Defaulted => WakeBody::AskDefaulted { ask: a.ask, question: a.question, answer, resume_note: a.resume_note },
                    other => {
                        unusable.push((r.id, format!("{} is {other}, no answer to deliver", a.ask)));
                        continue;
                    }
                };
                items.push(WakeItem { wake_ids: vec![r.id], kind: r.kind, note: r.note.clone(), body });
            }
            k => {
                let body = match k {
                    WakeKind::Timer => WakeBody::Timer,
                    WakeKind::Restart => WakeBody::Restart,
                    WakeKind::System => WakeBody::System,
                    WakeKind::Checkpoint => WakeBody::Checkpoint,
                    _ => WakeBody::StopCheck,
                };
                items.push(WakeItem { wake_ids: vec![r.id], kind: k, note: r.note.clone(), body });
            }
        }
    }
    if let Some(i) = msg_slot {
        items[i].body = WakeBody::Messages(blocks_for(conn, session, &msgs)?);
    }
    Ok(WakeBatch { session_id: session.into(), items, unusable })
}
```

（`build_batch_merges_messages_and_resolves_ask_answers` 里 ask_answer 行由 `asks::answer` 自动入队，human 优先级排在两条 auto 消息前；那条 `msg:gone` 进 `unusable`。`unusable` 的原因是写给日志 / M3c `last_error` 的开发者向英文短句（与 M4a `Rejected` 同口径），不是给 agent 的正文，所以不进 md、也不会被 `render_code_has_no_prose` 判红。）

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-collab`
Expected: 73 + render 3 + worldview 2 = 78 passed；`prompts::both_languages_have_every_section_with_only_allowed_placeholders` 覆盖新加的 21 节。

- [ ] **Step 6: 变异：证明「代码里没有文案」那把尺子会红**

```bash
cp crates/collab/src/render.rs target/render.rs.bak
```

（备份放 `target/`：已被 gitignore，不会误提交。）在 `injection` 函数体第一行加 `let _ = "注入";`，跑 `cargo test -p crewd-collab render_code_has_no_prose`，Expected: FAIL，报出 `['注', '入']`。然后 `cp target/render.rs.bak crates/collab/src/render.rs` 还原（**不要用 `git checkout`**），再跑一次确认 PASS。

- [ ] **Step 7: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/collab/src/lib.rs crates/collab/src/render.rs crates/collab/src/worldview.rs crates/collab/src/turn_trace.rs crates/collab/src/prompts.rs crates/collab/prompts/render.zh.md crates/collab/prompts/render.en.md
git diff --cached --stat
git commit -m "crewd-collab: injection and wake rendering from prompt files; worldview and turn_trace signatures for M3b

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 12: 服务层骨架：`Hub` 双通道与 `emit`、`AppState::collab`、`SessionControl`、`TokenTable`

裁定 2–3、15。`AppState::collab(f)` 是服务层写协作数据的唯一入口：在库线程上 `collab::run`，提交后**仍在库线程上** `hub.emit`（现读数据拼客户端事件 + 原样发 `CollabEvent`）。事件名常量本任务先加进 proto（方法常量在 Task 13）。

**Files:**
- Create: `crewd/crates/server/src/control.rs`、`crewd/crates/server/src/token.rs`
- Modify: `crewd/crates/server/Cargo.toml`、`crewd/crates/server/src/lib.rs`、`crewd/crates/server/src/hub.rs`、`crewd/crates/proto/src/lib.rs`（`events` 模块）、`crewd/Cargo.lock`

**Interfaces:**
- Consumes: Task 1（`Db`）、Task 5–11（collab 全部）。
- Produces：`crewd_server::hub::{Hub, CollabEvent}`（`Hub::{new, publish, subscribe, subscribe_collab, emit}`）；`AppState { db, hub, sessions: Arc<dyn SessionControl>, tokens: TokenTable }`、`AppState::{new, with_sessions, collab}`；`crewd_server::control::{BoxFuture, SessionControl, StartRequest, Inspection, QuotaReport, QuotaLine, ControlError, NoSessions, FakeSessionControl, FakeCall}`；`crewd_server::token::TokenTable`；`crewd_proto::events::{MESSAGE_APPENDED, CHANNEL_CHANGED, CREW_CHANGED, CREW_REMOVED, BADGE_CHANGED, ASK_CHANGED, APPROVAL_CHANGED, SESSION_CHANGED, RESTART_CHANGED, LAGGED, TOPICS}`。

- [ ] **Step 1: 依赖与 proto 事件常量**

```bash
cargo add --offline -p crewd-server --path crates/collab
cargo add --offline -p crewd-server rusqlite --features bundled
cargo add --offline -p crewd-server thiserror
```

`crates/proto/src/lib.rs` 的 `events` 模块整段替换（`WHITEBOARD_APPENDED` 先留着，Task 13 删）：

```rust
pub mod events {
    /// `events.subscribe` 接受的主题（= 事件名第一个点之前的部分）；其他一律 -32602。
    /// `session` 主题的事件由 M3c 发；`wake` 主题由 M3c 加（排障用）。
    pub const TOPICS: &[&str] = &["message", "channel", "crew", "badge", "ask", "approval", "session", "restart", "whiteboard"];
    pub const MESSAGE_APPENDED: &str = "message.appended";
    pub const CHANNEL_CHANGED: &str = "channel.changed";
    pub const CREW_CHANGED: &str = "crew.changed";
    pub const CREW_REMOVED: &str = "crew.removed";
    pub const BADGE_CHANGED: &str = "badge.changed";
    pub const ASK_CHANGED: &str = "ask.changed";
    pub const APPROVAL_CHANGED: &str = "approval.changed";
    /// `SessionLive`，M3c 发。
    pub const SESSION_CHANGED: &str = "session.changed";
    pub const RESTART_CHANGED: &str = "restart.changed";
    /// 旧事件，Task 13 删。
    pub const WHITEBOARD_APPENDED: &str = "whiteboard.appended";
    /// 订阅者处理太慢、漏掉了事件；`data.missed` 是漏掉的条数，客户端应全量重拉。
    pub const LAGGED: &str = "events.lagged";
}
```

- [ ] **Step 2: 写失败的测试**

`crates/server/src/token.rs`（整个文件）：

```rust
//! helper 身份令牌（spec §22.3）：M3c 起会话时 `mint`，M3b 的 `agent.attach` 用 `verify`。
//! 只在内存里：crewd 重启时旧进程已经跟着死了。用途是防串号（配错、复用旧配置），不是挡 agent 本身。
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct TokenTable {
    inner: Mutex<HashMap<String, String>>,
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl TokenTable {
    /// 给 session 发一个新令牌（替换旧的）。64 个十六进制字符。
    pub fn mint(&self, session: &str) -> String {
        let t = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        self.inner.lock().expect("token table poisoned").insert(session.to_string(), t.clone());
        t
    }

    pub fn verify(&self, session: &str, token: &str) -> bool {
        self.inner
            .lock()
            .expect("token table poisoned")
            .get(session)
            .is_some_and(|t| ct_eq(t.as_bytes(), token.as_bytes()))
    }

    pub fn revoke(&self, session: &str) {
        self.inner.lock().expect("token table poisoned").remove(session);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_per_session_and_replaceable() {
        let t = TokenTable::default();
        let a = t.mint("w-1");
        assert_eq!(a.len(), 64);
        assert!(t.verify("w-1", &a));
        assert!(!t.verify("w-2", &a), "别的 session 不能用");
        assert!(!t.verify("w-1", "x"));
        let b = t.mint("w-1");
        assert!(!t.verify("w-1", &a), "重新发放后旧的作废");
        assert!(t.verify("w-1", &b));
        t.revoke("w-1");
        assert!(!t.verify("w-1", &b));
    }
}
```

`crates/server/src/control.rs`（整个文件；实现与测试同在，trait 是本任务冻结的接口）：

```rust
//! 会话控制的窄接口（spec §22.5）：M3c 的 `server::orchestra::Sessions` 实现它；M3b 的工具、M3a 的 RPC 经它调。
//! 参数一律 owned、返回 `BoxFuture`，可做 `dyn`。`NoSessions` 是 M3c 接上之前的默认实现（一律 Unsupported）。
use crewd_proto::{Role, Session, SessionLive};
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartRequest {
    pub crew_id: String,
    pub role: Role,
    /// "claude" | "codex" | "acp:<id>"
    pub executor: String,
    pub brief: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub worktree: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inspection {
    pub live: SessionLive,
    /// 屏幕文本（claude）或结构化记录的尾部（codex）。
    pub screen: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuotaLine {
    pub executor: String,
    pub plan: Option<String>,
    /// 额度用尽时的重置时刻（RFC 3339）。
    pub limited_until: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuotaReport {
    pub lines: Vec<QuotaLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ControlError {
    #[error("{0}")]
    NotFound(String),
    #[error("session {0} 已经在跑")]
    AlreadyRunning(String),
    #[error("不支持：{0}")]
    Unsupported(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Failed(String),
}

pub trait SessionControl: Send + Sync {
    /// 注册 session 行（经 collab）并起进程；返回注册后的行。
    fn start(&self, req: StartRequest) -> BoxFuture<'_, Result<Session, ControlError>>;
    /// `--resume` 一个已有 session（重启询问答「恢复」、`session.resume`）。
    fn resume(&self, session_id: String) -> BoxFuture<'_, Result<(), ControlError>>;
    fn stop(&self, session_id: String, reason: String) -> BoxFuture<'_, Result<(), ControlError>>;
    /// 给在跑的 session 投一段文字（机长的 nudge_session）。
    fn nudge(&self, session_id: String, text: String) -> BoxFuture<'_, Result<(), ControlError>>;
    fn inspect(&self, session_id: String, max_lines: usize) -> BoxFuture<'_, Result<Inspection, ControlError>>;
    fn set_profile(&self, session_id: String, model: Option<String>, effort: Option<String>) -> BoxFuture<'_, Result<(), ControlError>>;
    /// 交接机长：数据（`collab::crews::set_captain`）+ 两边的世界观 / 工具表刷新。
    fn handoff_captain(&self, crew_id: String, to_session_id: String) -> BoxFuture<'_, Result<(), ControlError>>;
    fn quota(&self) -> BoxFuture<'_, Result<QuotaReport, ControlError>>;
    fn live(&self, session_id: String) -> BoxFuture<'_, Result<SessionLive, ControlError>>;
}

/// M3c 接上之前的默认实现。
pub struct NoSessions;

fn unsupported<T: Send + 'static>() -> BoxFuture<'static, Result<T, ControlError>> {
    Box::pin(async { Err(ControlError::Unsupported("会话管理还没接上（M3c）".into())) })
}

impl SessionControl for NoSessions {
    fn start(&self, _: StartRequest) -> BoxFuture<'_, Result<Session, ControlError>> { unsupported() }
    fn resume(&self, _: String) -> BoxFuture<'_, Result<(), ControlError>> { unsupported() }
    fn stop(&self, _: String, _: String) -> BoxFuture<'_, Result<(), ControlError>> { unsupported() }
    fn nudge(&self, _: String, _: String) -> BoxFuture<'_, Result<(), ControlError>> { unsupported() }
    fn inspect(&self, _: String, _: usize) -> BoxFuture<'_, Result<Inspection, ControlError>> { unsupported() }
    fn set_profile(&self, _: String, _: Option<String>, _: Option<String>) -> BoxFuture<'_, Result<(), ControlError>> { unsupported() }
    fn handoff_captain(&self, _: String, _: String) -> BoxFuture<'_, Result<(), ControlError>> { unsupported() }
    fn quota(&self) -> BoxFuture<'_, Result<QuotaReport, ControlError>> { unsupported() }
    fn live(&self, _: String) -> BoxFuture<'_, Result<SessionLive, ControlError>> { unsupported() }
}

/// 测试替身（M3b 的工具测试、M3a 的 RPC 测试用）：记下每次调用，按预设回答。不碰库、不起进程。
#[derive(Debug, Clone, PartialEq)]
pub enum FakeCall {
    Start(StartRequest),
    Resume(String),
    Stop(String, String),
    Nudge(String, String),
    Inspect(String, usize),
    SetProfile(String, Option<String>, Option<String>),
    Handoff(String, String),
    Quota,
    Live(String),
}

#[derive(Default)]
pub struct FakeSessionControl {
    calls: Mutex<Vec<FakeCall>>,
    live: Mutex<std::collections::HashMap<String, SessionLive>>,
    fail_next: Mutex<Option<ControlError>>,
}

impl FakeSessionControl {
    pub fn calls(&self) -> Vec<FakeCall> {
        self.calls.lock().unwrap().clone()
    }
    pub fn set_live(&self, l: SessionLive) {
        self.live.lock().unwrap().insert(l.session_id.clone(), l);
    }
    /// 下一次调用（任何方法）返回这个错误。
    pub fn fail_next(&self, e: ControlError) {
        *self.fail_next.lock().unwrap() = Some(e);
    }
    fn record<T: Send + 'static>(&self, c: FakeCall, ok: T) -> BoxFuture<'_, Result<T, ControlError>> {
        self.calls.lock().unwrap().push(c);
        let r = match self.fail_next.lock().unwrap().take() {
            Some(e) => Err(e),
            None => Ok(ok),
        };
        Box::pin(async move { r })
    }
}

impl SessionControl for FakeSessionControl {
    fn start(&self, req: StartRequest) -> BoxFuture<'_, Result<Session, ControlError>> {
        let n = self.calls.lock().unwrap().len();
        let s = Session {
            id: format!("fake-{n}"),
            crew_id: req.crew_id.clone(),
            role: req.role,
            executor: req.executor.clone(),
            title: req.title.clone(),
            brief: Some(req.brief.clone()),
            desired: crewd_proto::Desired::Running,
            ..Default::default()
        };
        self.record(FakeCall::Start(req), s)
    }
    fn resume(&self, id: String) -> BoxFuture<'_, Result<(), ControlError>> { self.record(FakeCall::Resume(id), ()) }
    fn stop(&self, id: String, reason: String) -> BoxFuture<'_, Result<(), ControlError>> { self.record(FakeCall::Stop(id, reason), ()) }
    fn nudge(&self, id: String, text: String) -> BoxFuture<'_, Result<(), ControlError>> { self.record(FakeCall::Nudge(id, text), ()) }
    fn inspect(&self, id: String, max_lines: usize) -> BoxFuture<'_, Result<Inspection, ControlError>> {
        let live = self.live.lock().unwrap().get(&id).cloned().unwrap_or(SessionLive { session_id: id.clone(), ..Default::default() });
        self.record(FakeCall::Inspect(id, max_lines), Inspection { live, screen: String::new() })
    }
    fn set_profile(&self, id: String, model: Option<String>, effort: Option<String>) -> BoxFuture<'_, Result<(), ControlError>> {
        self.record(FakeCall::SetProfile(id, model, effort), ())
    }
    fn handoff_captain(&self, crew: String, to: String) -> BoxFuture<'_, Result<(), ControlError>> { self.record(FakeCall::Handoff(crew, to), ()) }
    fn quota(&self) -> BoxFuture<'_, Result<QuotaReport, ControlError>> { self.record(FakeCall::Quota, QuotaReport::default()) }
    fn live(&self, id: String) -> BoxFuture<'_, Result<SessionLive, ControlError>> {
        let l = self.live.lock().unwrap().get(&id).cloned().unwrap_or(SessionLive { session_id: id.clone(), ..Default::default() });
        self.record(FakeCall::Live(id), l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn fake_records_calls_and_can_fail_once() {
        let fake = Arc::new(FakeSessionControl::default());
        let dynamic: Arc<dyn SessionControl> = fake.clone();
        let s = dynamic
            .start(StartRequest { crew_id: "crew-a".into(), executor: "claude".into(), brief: "修滚动".into(), ..Default::default() })
            .await
            .unwrap();
        assert_eq!(s.crew_id, "crew-a");
        fake.fail_next(ControlError::AlreadyRunning("w-1".into()));
        assert_eq!(dynamic.resume("w-1".into()).await, Err(ControlError::AlreadyRunning("w-1".into())));
        dynamic.resume("w-1".into()).await.unwrap();
        fake.set_live(SessionLive { session_id: "w-1".into(), running: true, ..Default::default() });
        assert!(dynamic.live("w-1".into()).await.unwrap().running);
        assert_eq!(fake.calls().len(), 4);
        assert!(matches!(fake.calls()[1], FakeCall::Resume(ref id) if id == "w-1"));
    }

    #[tokio::test]
    async fn no_sessions_says_unsupported() {
        let n: Arc<dyn SessionControl> = Arc::new(NoSessions);
        assert!(matches!(n.quota().await, Err(ControlError::Unsupported(_))));
    }
}
```

`crates/server/src/lib.rs` 的测试模块里加（`AppState::collab` 的测试）：

```rust
    #[tokio::test]
    async fn collab_commits_then_publishes_client_and_internal_events() {
        use crewd_proto::{Crew, events};
        let store = Store::open_in_memory().unwrap();
        crewd_store::crews::upsert(store.conn(), &Crew { id: "crew-a".into(), title: "机组甲".into(), created_at: "t".into(), updated_at: "t".into(), ..Default::default() }, None).unwrap();
        crewd_store::schema::backfill_v3(store.conn()).unwrap();
        let state = AppState::new(crewd_store::Db::spawn(store).unwrap());
        let mut client = state.hub.subscribe();
        let mut internal = state.hub.subscribe_collab();
        let m = state
            .collab(|tx| {
                crewd_collab::messages::post(tx, crewd_collab::Sender::Human, crewd_collab::messages::NewPost {
                    channel_id: "crew-a/lobby".into(),
                    text: "大家好".into(),
                    ..Default::default()
                })
            })
            .await
            .unwrap();
        let e1 = client.recv().await.unwrap();
        assert_eq!(e1.event, events::MESSAGE_APPENDED);
        assert_eq!(e1.data["seq"], m.message.seq);
        assert_eq!(client.recv().await.unwrap().event, events::BADGE_CHANGED);
        assert!(matches!(internal.recv().await.unwrap(), CollabEvent::MessageAppended { .. }));

        let r: crewd_collab::Result<()> = state
            .collab(|tx| {
                crewd_collab::messages::post(tx, crewd_collab::Sender::Human, crewd_collab::messages::NewPost { channel_id: "crew-a/lobby".into(), text: "会回滚".into(), ..Default::default() })?;
                Err(crewd_collab::CollabError::Invalid("中途失败".into()))
            })
            .await;
        assert!(r.is_err());
        assert!(matches!(client.try_recv(), Err(tokio::sync::broadcast::error::TryRecvError::Empty)), "回滚 = 不发事件");
    }
```

（`use` 补 `crate::hub::CollabEvent`。）

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-server`
Expected: 编译失败（`subscribe_collab`、`collab`、`control`、`token` 不存在）。

- [ ] **Step 4: 实现**

`crates/server/src/hub.rs`（整个文件替换）：

```rust
//! 进程内事件总线：客户端事件（`Event`，连接层按主题转发）与内部协作事件（`CollabEvent`，M3c / M4b 订阅）。
use crewd_collab::{Change, CollabError};
use crewd_proto::{Event, events};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::json;
use tokio::sync::broadcast;

/// spec §22.5 的 `CollabEvent`：类型定义在 crewd-collab（裁定 3），这里是它在服务层的名字。
pub use crewd_collab::Change as CollabEvent;

#[derive(Clone)]
pub struct Hub {
    tx: broadcast::Sender<Event>,
    collab: broadcast::Sender<CollabEvent>,
}

impl Hub {
    pub fn new() -> Self {
        Self {
            tx: broadcast::channel(1024).0,
            collab: broadcast::channel(4096).0,
        }
    }
    pub fn publish(&self, e: Event) {
        // 没有订阅者时 send 会报错，这正常，忽略。
        let _ = self.tx.send(e);
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
    pub fn subscribe_collab(&self) -> broadcast::Receiver<CollabEvent> {
        self.collab.subscribe()
    }

    /// 提交之后、在库线程上调（`AppState::collab` 保证）：每条变更原样发内部通道，再现读数据发客户端事件。
    /// 取数失败只记日志（数据已经提交，不能让发事件的失败把成功的操作说成失败）。
    pub fn emit(&self, conn: &Connection, changes: &[Change]) {
        for c in changes {
            let _ = self.collab.send(c.clone());
            match client_events(conn, c) {
                Ok(evs) => evs.into_iter().for_each(|e| self.publish(e)),
                Err(e) => eprintln!("crewd: 变更 {c:?} 取数失败，客户端事件没发：{e}"),
            }
        }
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

fn ev<T: Serialize>(name: &str, data: &T) -> crewd_collab::Result<Event> {
    Ok(Event { event: name.into(), data: serde_json::to_value(data)? })
}

/// 删掉了的东西读不到 = 不发（删除另有 `crew.removed`）。
fn gone<T>(r: crewd_collab::Result<T>) -> crewd_collab::Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(CollabError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

fn client_events(conn: &Connection, c: &Change) -> crewd_collab::Result<Vec<Event>> {
    use crewd_collab::{approvals, asks, channels, crews, cursors, messages, restart};
    Ok(match c {
        Change::MessageAppended { message_id, .. } => gone(messages::get(conn, message_id))?
            .map(|m| ev(events::MESSAGE_APPENDED, &m))
            .transpose()?
            .into_iter()
            .collect(),
        Change::BadgeChanged { channel_id } => gone(cursors::badge(conn, channel_id))?
            .map(|b| ev(events::BADGE_CHANGED, &b))
            .transpose()?
            .into_iter()
            .collect(),
        Change::ChannelChanged { channel_id } => gone(channels::summary(conn, channel_id))?
            .map(|s| ev(events::CHANNEL_CHANGED, &s))
            .transpose()?
            .into_iter()
            .collect(),
        Change::CrewChanged { crew_id } => gone(crews::get(conn, crew_id))?
            .map(|x| ev(events::CREW_CHANGED, &x))
            .transpose()?
            .into_iter()
            .collect(),
        Change::CrewRemoved { crew_id } => vec![Event { event: events::CREW_REMOVED.into(), data: json!({ "crewId": crew_id }) }],
        Change::AskChanged { crew_id, ask } => gone(asks::get(conn, crew_id, ask))?
            .map(|a| ev(events::ASK_CHANGED, &a))
            .transpose()?
            .into_iter()
            .collect(),
        Change::ApprovalChanged { id } => gone(approvals::get(conn, id))?
            .map(|a| ev(events::APPROVAL_CHANGED, &a))
            .transpose()?
            .into_iter()
            .collect(),
        Change::RestartChanged { id } => gone(restart::get(conn, *id))?
            .map(|p| ev(events::RESTART_CHANGED, &p))
            .transpose()?
            .into_iter()
            .collect(),
        // 只走内部通道：session.changed（SessionLive）由 M3c 发；入队 / 答复 / 重启答复是给 M3c / M4b 的。
        Change::SessionChanged { .. } | Change::WakeEnqueued { .. } | Change::AskAnswered { .. } | Change::RestartAnswered { .. } => vec![],
    })
}
```

`crates/server/src/lib.rs`：模块加 `pub mod control; pub mod token;`；`AppState` 换成：

```rust
pub struct AppState {
    pub db: Db,
    pub hub: Hub,
    pub sessions: Arc<dyn control::SessionControl>,
    pub tokens: token::TokenTable,
}

impl AppState {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            hub: Hub::new(),
            sessions: Arc::new(control::NoSessions),
            tokens: token::TokenTable::default(),
        }
    }

    /// M3c 在 serve 启动段换上真实现。
    pub fn with_sessions(mut self, s: Arc<dyn control::SessionControl>) -> Self {
        self.sessions = s;
        self
    }

    /// 写协作数据的唯一入口：库线程上跑 `collab::run`，提交后在同一线程上 `hub.emit`（事件顺序 = 提交顺序）。
    pub async fn collab<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut crewd_collab::Tx<'_>) -> crewd_collab::Result<T> + Send + 'static,
    ) -> crewd_collab::Result<T> {
        let hub = self.hub.clone();
        let now = crewd_store::now_ms();
        self.db
            .call(move |s| {
                let (v, changes) = crewd_collab::run(s, now, f)?;
                hub.emit(s.conn(), &changes);
                Ok(v)
            })
            .await
            .map_err(crewd_collab::CollabError::from)?
    }
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-server`
Expected: 新增 token 1、control 2、lib 1 通过；原有测试照旧。

- [ ] **Step 6: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/server/Cargo.toml crates/server/src/lib.rs crates/server/src/hub.rs crates/server/src/control.rs crates/server/src/token.rs crates/proto/src/lib.rs Cargo.lock
git diff --cached --stat
git commit -m "crewd-server: AppState::collab with ordered event emission, CollabEvent channel, SessionControl trait with fake, TokenTable

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 13: 客户端 RPC 与事件全部换新；删 `whiteboard.*`

spec §22.2 的方法表 + §22.7 拍板加的 `channel.setOwner`、`restart.pending` / `restart.answer`。客户端都是人：写操作的 `Sender` 一律 `Human`。参数结构全部 `deny_unknown_fields`；无参数方法只收 `null` 或 `{}`；全可选参数的方法 `null` 当 `{}`。错误码：`-32004` 不存在、`-32003` 不允许、`-32009` 冲突、`-32602` 参数 / 规则不过、`-32603` 内部错误。

**Files:**
- Create: `crewd/crates/proto/src/params.rs`
- Modify: `crewd/crates/proto/src/lib.rs`（`mod params; pub use params::*;`、`methods`、`events::TOPICS` 去掉 whiteboard、删 `WHITEBOARD_APPENDED`、parse 助手、错误码、`API_VERSION = 2`）、`crewd/crates/proto/src/model.rs`（删 `WhiteboardListParams` / `WhiteboardPostParams`，`TodoListParams` / `SubscribeParams` 挪进 params.rs 并加 `deny_unknown_fields`）、`crewd/crates/server/src/dispatch.rs`（整个重写）、`crewd/crates/server/tests/socket.rs`、`crewd/crates/crewd/tests/e2e.rs`、`crewd/schema/crewd-api.schema.json`

**Interfaces:**
- Consumes: Task 12（`AppState::collab`）、collab 全部读写函数。
- Produces：`crewd_proto::methods::*`（下表常量 + `ALL`）；params 类型（下面整段）；`parse_params_or_default`、`parse_no_params`；`RpcError::{NOT_FOUND = -32004, FORBIDDEN = -32003, CONFLICT = -32009}`；`crewd_server::dispatch::collab_err(CollabError) -> RpcError`（M3b 的 agent RPC 复用同一映射）。

| 方法 | params | result |
|---|---|---|
| system.hello | — | Hello |
| crew.list | — | Crew[]（最近有动静在上，带 lastActivityAt） |
| crew.get | CrewIdParams | CrewDetail |
| crew.create | CrewCreateParams | Crew |
| crew.update | CrewUpdateParams | Crew |
| crew.adopt / crew.release / crew.delete | CrewAdoptParams / CrewReleaseParams / CrewIdParams | Crew / Crew / Empty |
| channel.list | CrewIdParams | ChannelSummary[] |
| channel.create | ChannelCreateParams | Channel |
| channel.update | ChannelUpdateParams | Channel |
| channel.addMembers / channel.removeMember | ChannelMembersParams / ChannelMemberParams | Channel |
| channel.watch | ChannelWatchParams | ChannelSummary |
| channel.setOwner | ChannelSetOwnerParams | Channel（人 = 强制，§22.7 Q2） |
| message.list | MessageListParams | Message[] |
| message.post | MessagePostParams | Message |
| message.search | MessageSearchParams | Message[] |
| read.mark | ReadMarkParams | ChannelSummary |
| session.list / session.get | SessionListParams / SessionGetParams | Session[] / Session |
| ask.list / ask.answer / ask.withdraw | AskListParams / AskAnswerParams / AskWithdrawParams | HumanWait[] / Ask / Ask |
| approval.list / approval.decide | — / ApprovalDecideParams | Approval[] / Approval |
| restart.pending / restart.answer | — / RestartAnswerParams | RestartPrompt 或 null / RestartPrompt |
| directory.list | DirectoryListParams | DirectoryRow[] |
| todo.list | TodoListParams | Todo[] |
| events.subscribe | SubscribeParams | { topics } |

- [ ] **Step 1: proto 参数与常量**

`crates/proto/src/params.rs`（整个文件）：

```rust
//! RPC 参数。全部 `deny_unknown_fields`（§14 遗留：客户端拼错字段要报错，不能静默忽略）。
use crate::model::{Attachment, ChannelKind, Ledger, MemberRef, Mention, RestartAnswer};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

macro_rules! params {
    ($(#[$m:meta])* $name:ident { $($(#[$fm:meta])* $f:ident : $t:ty),* $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        pub struct $name { $($(#[$fm])* pub $f: $t,)* }
    };
}

params!(/// 无参数方法只收 null 或 {}。
    NoParams {});
params!(CrewIdParams { crew_id: String });
params!(CrewCreateParams { title: String, working_directory: String, #[serde(default)] parent_id: Option<String> });
params!(CrewUpdateParams {
    crew_id: String,
    #[serde(default)] title: Option<String>,
    #[serde(default)] working_directory: Option<String>,
    #[serde(default)] hidden: Option<bool>,
    /// "" = 清掉。
    #[serde(default)] status_line: Option<String>,
});
params!(CrewAdoptParams { crew_id: String, parent_id: String });
params!(CrewReleaseParams { crew_id: String, from_parent_id: String, #[serde(default)] to_parent_id: Option<String> });
params!(ChannelCreateParams {
    crew_id: String,
    /// 只能是 topic 或 dm。
    kind: ChannelKind,
    #[serde(default)] name: String,
    #[serde(default)] purpose: Option<String>,
    #[serde(default)] members: Vec<MemberRef>,
});
params!(ChannelUpdateParams {
    channel_id: String,
    #[serde(default)] name: Option<String>,
    #[serde(default)] purpose: Option<String>,
    #[serde(default)] archived: Option<bool>,
});
params!(ChannelMembersParams { channel_id: String, members: Vec<MemberRef> });
params!(ChannelMemberParams { channel_id: String, member: MemberRef });
params!(ChannelWatchParams { channel_id: String, on: bool });
params!(ChannelSetOwnerParams { channel_id: String, owner: MemberRef });
params!(MessageListParams {
    channel_id: String,
    #[serde(default)] after_seq: Option<i64>,
    #[serde(default)] before_seq: Option<i64>,
    /// 缺省 200，上限 1000。
    #[serde(default)] limit: Option<u32>,
});
params!(MessagePostParams {
    channel_id: String,
    text: String,
    #[serde(default)] in_reply_to: Option<String>,
    /// captain 不带 targetId（发送时解析）；session 必带。
    #[serde(default)] mentions: Vec<Mention>,
    /// 非空一律拒（上传协议未定，裁定 14）。
    #[serde(default)] attachments: Vec<Attachment>,
});
params!(MessageSearchParams {
    query: String,
    #[serde(default)] crew_id: Option<String>,
    #[serde(default)] channel_ids: Option<Vec<String>>,
    #[serde(default)] after: Option<String>,
    #[serde(default)] before: Option<String>,
    /// 缺省 50，上限 500。
    #[serde(default)] limit: Option<u32>,
});
params!(ReadMarkParams { channel_id: String, seq: i64 });
params!(SessionListParams { #[serde(default)] crew_id: Option<String> });
params!(SessionGetParams { session_id: String });
params!(AskListParams { #[serde(default)] root_crew_id: Option<String> });
params!(AskAnswerParams { crew_id: String, ask: String, text: String });
params!(AskWithdrawParams { crew_id: String, ask: String, reason: String });
params!(ApprovalDecideParams { id: String, allow: bool });
params!(RestartAnswerParams { id: i64, answer: RestartAnswer });
params!(DirectoryListParams { #[serde(default)] query: Option<String> });
params!(TodoListParams { crew_id: String, ledger: Ledger });
params!(SubscribeParams {
    /// 主题 = 事件名第一个点之前的部分，例如 "message"。
    topics: Vec<String>,
});
```

（`Ledger` 若没有 `Default`，给它加 `#[derive(Default)]` + `#[default] Agent`；`ChannelKind` / `RestartAnswer` / `MemberRef` 需要 `Default`：`MemberRef` 手写 `impl Default for MemberRef { fn default() -> Self { MemberRef::Human } }`。宏里的 `Default` 只为 `parse_params_or_default`，无参 / 全可选结构以外用不到。）

`crates/proto/src/lib.rs`：`API_VERSION = 2`；`methods` 整段替换：

```rust
pub mod methods {
    pub const HELLO: &str = "system.hello";
    pub const CREW_LIST: &str = "crew.list";
    pub const CREW_GET: &str = "crew.get";
    pub const CREW_CREATE: &str = "crew.create";
    pub const CREW_UPDATE: &str = "crew.update";
    pub const CREW_ADOPT: &str = "crew.adopt";
    pub const CREW_RELEASE: &str = "crew.release";
    pub const CREW_DELETE: &str = "crew.delete";
    pub const CHANNEL_LIST: &str = "channel.list";
    pub const CHANNEL_CREATE: &str = "channel.create";
    pub const CHANNEL_UPDATE: &str = "channel.update";
    pub const CHANNEL_ADD_MEMBERS: &str = "channel.addMembers";
    pub const CHANNEL_REMOVE_MEMBER: &str = "channel.removeMember";
    pub const CHANNEL_WATCH: &str = "channel.watch";
    pub const CHANNEL_SET_OWNER: &str = "channel.setOwner";
    pub const MESSAGE_LIST: &str = "message.list";
    pub const MESSAGE_POST: &str = "message.post";
    pub const MESSAGE_SEARCH: &str = "message.search";
    pub const READ_MARK: &str = "read.mark";
    pub const SESSION_LIST: &str = "session.list";
    pub const SESSION_GET: &str = "session.get";
    pub const ASK_LIST: &str = "ask.list";
    pub const ASK_ANSWER: &str = "ask.answer";
    pub const ASK_WITHDRAW: &str = "ask.withdraw";
    pub const APPROVAL_LIST: &str = "approval.list";
    pub const APPROVAL_DECIDE: &str = "approval.decide";
    pub const RESTART_PENDING: &str = "restart.pending";
    pub const RESTART_ANSWER: &str = "restart.answer";
    pub const DIRECTORY_LIST: &str = "directory.list";
    pub const TODO_LIST: &str = "todo.list";
    pub const SUBSCRIBE: &str = "events.subscribe";
    // M3b / M3c 的方法常量写在各自的段落里，并加进 ALL（契约 json 由后合者重新生成）。

    /// 客户端可调的全部方法（契约测试与 `crewd rpc` 端到端都按它逐个核）。
    pub const ALL: &[&str] = &[
        HELLO, CREW_LIST, CREW_GET, CREW_CREATE, CREW_UPDATE, CREW_ADOPT, CREW_RELEASE, CREW_DELETE,
        CHANNEL_LIST, CHANNEL_CREATE, CHANNEL_UPDATE, CHANNEL_ADD_MEMBERS, CHANNEL_REMOVE_MEMBER, CHANNEL_WATCH, CHANNEL_SET_OWNER,
        MESSAGE_LIST, MESSAGE_POST, MESSAGE_SEARCH, READ_MARK, SESSION_LIST, SESSION_GET,
        ASK_LIST, ASK_ANSWER, ASK_WITHDRAW, APPROVAL_LIST, APPROVAL_DECIDE, RESTART_PENDING, RESTART_ANSWER,
        DIRECTORY_LIST, TODO_LIST, SUBSCRIBE,
    ];
}
```

`events::TOPICS` 去掉 `"whiteboard"`，删 `WHITEBOARD_APPENDED`。`RpcError` 加：

```rust
    pub const FORBIDDEN: i32 = -32003;
    pub const NOT_FOUND: i32 = -32004;
    pub const CONFLICT: i32 = -32009;
```

parse 助手（`parse_params` 之后）：

```rust
/// 参数全可选的方法：`null` 当 `{}`。
pub fn parse_params_or_default<T: DeserializeOwned + Default>(params: Value) -> Result<T, RpcError> {
    if params.is_null() { Ok(T::default()) } else { parse_params(params) }
}

/// 无参数的方法：只收 `null` 或 `{}`。
pub fn parse_no_params(params: Value) -> Result<(), RpcError> {
    parse_params_or_default::<NoParams>(params).map(|_| ())
}
```

proto 测试里 `parse_params_reports_invalid_params` 改成 `parse_params::<MessageListParams>(json!({"limit": 3}))`；`api_schema_lists_methods_and_types` 暂时改成断言 `s["methods"]["message.post"]` 存在（Task 14 重写 `api_schema`）；加：

```rust
    #[test]
    fn params_reject_unknown_fields_and_no_param_methods_accept_null_or_empty() {
        let e = parse_params::<ReadMarkParams>(json!({"channelId":"c","seq":1,"bogus":true})).unwrap_err();
        assert_eq!(e.code, RpcError::INVALID_PARAMS);
        assert!(parse_no_params(Value::Null).is_ok());
        assert!(parse_no_params(json!({})).is_ok());
        assert!(parse_no_params(json!({"x":1})).is_err());
        let p: SessionListParams = parse_params_or_default(Value::Null).unwrap();
        assert_eq!(p.crew_id, None);
    }
```

`api_schema()` 里的方法表此刻先照新表机械改（`"params": "CrewIdParams"` 这类字符串），Task 14 再整体重写成 `$ref`。

- [ ] **Step 2: 写失败的测试（socket）**

`crates/server/tests/socket.rs`：保留 `Client` 与两条和白板无关的测试（`second_instance_on_same_dir_is_refused_and_stale_socket_is_ignored`、`invalid_utf8_gets_parse_error_and_connection_survives`；后者若用到 `whiteboard.*` 只把方法名换成 `message.list`）；`start` 里建数据那段换成 `seed(dir)`；其余测试函数全部删掉，换成下面这些。`Client` 加一个返回错误码的助手：

```rust
impl Client {
    async fn call_err(&mut self, id: u64, method: &str, params: Value) -> i32 {
        self.send_raw(&json!({"id":id,"method":method,"params":params}).to_string()).await;
        match self.next().await {
            ServerFrame::Response(r) => r.error.unwrap_or_else(|| panic!("{method} 应当失败，却返回 {:?}", r.result)).code,
            ServerFrame::Event(e) => panic!("意外的事件 {e:?}"),
        }
    }
}

/// 合成数据：机组 a（1 号，机长 cap-a、worker w-a1）；w-a1 开了一条问、申请了一次审批；有一条重启询问。
fn seed(dir: &Path) {
    use crewd_proto::{Role, Session};
    let mut s = crewd_store::Store::open(&dir.join("crewd.db")).unwrap();
    crewd_store::crews::upsert(
        s.conn(),
        &Crew {
            id: "a".into(),
            title: "A".into(),
            number: Some(1),
            working_directory: Some("/tmp/example".into()),
            created_at: "2026-10-01T00:00:00.000Z".into(),
            updated_at: "2026-10-01T00:00:00.000Z".into(),
            ..Default::default()
        },
        None,
    )
    .unwrap();
    for (id, role, ext) in [("cap-a", Role::Captain, None), ("w-a1", Role::Worker, Some(2))] {
        crewd_store::sessions::upsert(
            s.conn(),
            &Session { id: id.into(), crew_id: "a".into(), role, executor: "claude".into(), display_name: Some(format!("名字-{id}")), ext, updated_at: "2026-10-01T00:00:00.000Z".into(), ..Default::default() },
            None,
        )
        .unwrap();
    }
    crewd_store::schema::backfill_v3(s.conn()).unwrap();
    crewd_collab::run(&mut s, crewd_store::now_ms(), |tx| {
        crewd_collab::asks::open(tx, "w-a1", crewd_collab::asks::OpenAsk { question: "用哪家支付？".into(), ..Default::default() })?;
        crewd_collab::approvals::request(tx, crewd_collab::approvals::NewApproval { session_id: "w-a1".into(), tool_name: "computer-use".into(), summary: "点击设置".into(), tool_use_id: None })?;
        crewd_collab::restart::open(tx, vec!["w-a1".into()])
    })
    .unwrap();
}

#[tokio::test]
async fn hello_crews_channels_and_messages() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut c = Client::connect(dir.path()).await;
    assert_eq!(c.call(1, "system.hello", Value::Null).await["apiVersion"], 2);
    assert_eq!(c.call(2, "crew.list", json!({})).await[0]["id"], "a");
    let chans = c.call(3, "channel.list", json!({"crewId":"a"})).await;
    let ids: Vec<&str> = chans.as_array().unwrap().iter().map(|x| x["channel"]["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["a/lobby", "a/human"]);
    let text = "发布 0.1.44 🚀，中文没问题";
    let m = c.call(4, "message.post", json!({"channelId":"a/lobby","text":text})).await;
    assert_eq!(m["senderKind"], "human");
    assert_eq!(m["channelId"], "a/lobby");
    let list = c.call(5, "message.list", json!({"channelId":"a/lobby"})).await;
    assert_eq!(list[0]["text"], text);
    assert_eq!(c.call(6, "message.list", json!({"channelId":"a/lobby","afterSeq":m["seq"]})).await, json!([]));
    let found = c.call(7, "message.search", json!({"query":"0.1.44"})).await;
    assert_eq!(found.as_array().unwrap().len(), 1);
    assert_eq!(c.call(8, "todo.list", json!({"crewId":"a","ledger":"agent"})).await, json!([]));
    let dir_rows = c.call(9, "directory.list", Value::Null).await;
    let nums: Vec<&str> = dir_rows.as_array().unwrap().iter().map(|r| r["number"].as_str().unwrap()).collect();
    assert_eq!(nums, ["1", "1-1", "1-2"]);
    assert_eq!(c.call(10, "session.get", json!({"sessionId":"w-a1"})).await["ext"], 2);
}

#[tokio::test]
async fn params_are_strict_and_errors_are_typed() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut c = Client::connect(dir.path()).await;
    assert_eq!(c.call_err(1, "message.list", json!({"channelId":"a/lobby","bogus":1})).await, -32602);
    assert_eq!(c.call_err(2, "system.hello", json!({"x":1})).await, -32602);
    assert_eq!(c.call_err(3, "whiteboard.list", json!({"crewId":"a"})).await, -32601, "旧方法直接删，不留别名");
    assert_eq!(c.call_err(4, "message.post", json!({"channelId":"zz","text":"x"})).await, -32004);
    assert_eq!(c.call_err(5, "message.post", json!({"channelId":"a/lobby","text":"  "})).await, -32602);
    assert_eq!(c.call_err(6, "channel.update", json!({"channelId":"a/lobby","archived":true})).await, -32602);
    assert_eq!(c.call_err(7, "approval.decide", json!({"id":"ap-none","allow":true})).await, -32004);
    // 连接还活着
    assert_eq!(c.call(8, "system.hello", Value::Null).await["server"], "crewd");
}

#[tokio::test]
async fn subscribers_receive_message_and_badge_events_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut sub = Client::connect(dir.path()).await;
    assert_eq!(sub.call(1, "events.subscribe", json!({"topics":["message","badge"]})).await["topics"], json!(["message","badge"]));
    let mut c = Client::connect(dir.path()).await;
    c.call(1, "message.post", json!({"channelId":"a/lobby","text":"有人吗"})).await;
    let names: Vec<String> = [sub.next().await, sub.next().await]
        .into_iter()
        .map(|f| match f {
            ServerFrame::Event(e) => e.event,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(names, ["message.appended", "badge.changed"]);
    assert_eq!(sub.call_err(2, "events.subscribe", json!({"topics":["whiteboard"]})).await, -32602, "whiteboard 主题已删");
    assert_eq!(sub.call_err(3, "events.subscribe", json!({"topics":[]})).await, -32602);
}

#[tokio::test]
async fn asks_approvals_and_restart_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut c = Client::connect(dir.path()).await;
    let waits = c.call(1, "ask.list", Value::Null).await;
    assert_eq!(waits[0]["ask"]["question"], "用哪家支付？");
    let a = c.call(2, "ask.answer", json!({"crewId":"a","ask":"H1","text":"用甲"})).await;
    assert_eq!(a["state"], "answered");
    assert_eq!(c.call(3, "ask.list", json!({"rootCrewId":"a"})).await, json!([]));
    assert_eq!(c.call_err(4, "ask.answer", json!({"crewId":"a","ask":"H1","text":"再答"})).await, -32602);
    let aps = c.call(5, "approval.list", Value::Null).await;
    let id = aps[0]["id"].as_str().unwrap().to_string();
    assert_eq!(c.call(6, "approval.decide", json!({"id":id,"allow":true})).await["decision"], "allow");
    assert_eq!(c.call_err(7, "approval.decide", json!({"id":id,"allow":false})).await, -32009);
    let p = c.call(8, "restart.pending", Value::Null).await;
    let rid = p["id"].as_i64().unwrap();
    assert_eq!(c.call(9, "restart.answer", json!({"id":rid,"answer":"later"})).await["answer"], "later");
    assert_eq!(c.call(10, "restart.pending", Value::Null).await["id"], rid, "「先不」之后仍可整批恢复");
    c.call(11, "restart.answer", json!({"id":rid,"answer":"resume"})).await;
    assert_eq!(c.call(12, "restart.pending", Value::Null).await, Value::Null);
}

#[tokio::test]
async fn read_mark_clears_the_human_channel_badge() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut c = Client::connect(dir.path()).await;
    let chans = c.call(1, "channel.list", json!({"crewId":"a"})).await;
    let human = chans.as_array().unwrap().iter().find(|x| x["channel"]["id"] == "a/human").unwrap().clone();
    assert_eq!(human["unreadForHuman"], 3, "问、审批、重启询问各一条公告");
    assert_eq!(human["humanMentions"], 3);
    let last = c.call(2, "message.list", json!({"channelId":"a/human"})).await;
    let seq = last.as_array().unwrap().last().unwrap()["seq"].clone();
    let s = c.call(3, "read.mark", json!({"channelId":"a/human","seq":seq})).await;
    assert_eq!((s["unreadForHuman"].clone(), s["humanMentions"].clone()), (json!(0), json!(0)));
}

#[tokio::test]
async fn channels_can_be_created_watched_and_handed_over() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = start(dir.path()).await;
    let mut c = Client::connect(dir.path()).await;
    let ch = c.call(1, "channel.create", json!({"crewId":"a","kind":"topic","name":"发版","members":[{"kind":"session","id":"w-a1"}]})).await;
    assert_eq!(ch["owner"], json!({"kind":"human"}));
    let id = ch["id"].as_str().unwrap().to_string();
    assert_eq!(c.call(2, "channel.watch", json!({"channelId":id,"on":true})).await["watched"], true);
    let ch = c.call(3, "channel.setOwner", json!({"channelId":id,"owner":{"kind":"session","id":"w-a1"}})).await;
    assert_eq!(ch["owner"], json!({"kind":"session","id":"w-a1"}));
    assert_eq!(c.call_err(4, "channel.create", json!({"crewId":"a","kind":"lobby","name":"x"})).await, -32602);
    let made = c.call(5, "crew.create", json!({"title":"新机组","workingDirectory":"/tmp/example","parentId":"a"})).await;
    let detail = c.call(6, "crew.get", json!({"crewId":made["id"]})).await;
    assert_eq!(detail["rootId"], "a");
    c.call(7, "crew.release", json!({"crewId":made["id"],"fromParentId":"a"})).await;
    assert_eq!(c.call(8, "crew.delete", json!({"crewId":made["id"]})).await, json!({}));
}
```

`crates/crewd/tests/e2e.rs` 的 `import_then_serve_then_rpc`：`whiteboard.list` → `message.list` + `{"channelId":"local-aaaa/lobby"}`；`whiteboard.post` → `message.post` + `{"channelId":"local-aaaa/lobby","text":"切换后第一条 ✅"}`（去掉 senderKind）。

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-server --test socket`
Expected: 编译失败或大量 -32601（方法未实现）。

- [ ] **Step 4: 重写 dispatch**

`crates/server/src/dispatch.rs`（整个文件）：

```rust
//! 客户端 RPC（spec §22.2）。客户端都是人：写操作的发送者一律 `Sender::Human`。
//! 写操作经 `AppState::collab`（同一事务 + 提交后发事件）；纯读直接在库线程上读。
use crate::AppState;
use crewd_collab::{CollabError, Sender, approvals, asks, channels, crews, cursors, directory, messages, restart, sessions};
use crewd_proto::*;
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

pub async fn handle(state: &AppState, req: Request) -> Response {
    let id = req.id;
    match route(state, req).await {
        Ok(v) => Response::ok(id, v),
        Err(e) => Response::err(id, e),
    }
}

fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::new(RpcError::INTERNAL, e.to_string())
}

fn to_value<T: Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(internal)
}

/// collab 错误 → RPC 错误码（M3b 的 agent RPC 复用同一张表）。
pub fn collab_err(e: CollabError) -> RpcError {
    match e {
        CollabError::NotFound(m) => RpcError::new(RpcError::NOT_FOUND, m),
        CollabError::Invalid(m) => RpcError::new(RpcError::INVALID_PARAMS, m),
        CollabError::Forbidden(m) => RpcError::new(RpcError::FORBIDDEN, m),
        CollabError::Conflict(m) => RpcError::new(RpcError::CONFLICT, m),
        CollabError::Work(crewd_work::Rejected::Forbidden(a)) => RpcError::new(RpcError::FORBIDDEN, format!("not allowed: {a}")),
        CollabError::Work(r) => RpcError::new(RpcError::INVALID_PARAMS, r.to_string()),
        CollabError::Store(e) => internal(e),
    }
}

async fn write<T: Serialize + Send + 'static>(
    state: &AppState,
    f: impl FnOnce(&mut crewd_collab::Tx<'_>) -> crewd_collab::Result<T> + Send + 'static,
) -> Result<Value, RpcError> {
    to_value(state.collab(f).await.map_err(collab_err)?)
}

async fn read<T: Serialize + Send + 'static>(
    state: &AppState,
    f: impl FnOnce(&Connection) -> crewd_collab::Result<T> + Send + 'static,
) -> Result<Value, RpcError> {
    let v = state.db.call(move |s| f(s.conn())).await.map_err(internal)?.map_err(collab_err)?;
    to_value(v)
}

async fn route(state: &AppState, req: Request) -> Result<Value, RpcError> {
    let p = req.params;
    match req.method.as_str() {
        methods::HELLO => {
            parse_no_params(p)?;
            to_value(Hello { server: "crewd".into(), version: env!("CARGO_PKG_VERSION").into(), api_version: API_VERSION })
        }
        methods::CREW_LIST => {
            parse_no_params(p)?;
            read(state, crews::list_by_activity).await
        }
        methods::CREW_GET => {
            let p: CrewIdParams = parse_params(p)?;
            write(state, move |tx| crews::detail(tx, &p.crew_id)).await
        }
        methods::CREW_CREATE => {
            let p: CrewCreateParams = parse_params(p)?;
            write(state, move |tx| {
                crews::create(tx, crews::NewCrew { title: p.title, working_directory: p.working_directory, parent_id: p.parent_id, title_source: Some("human".into()) })
            })
            .await
        }
        methods::CREW_UPDATE => {
            let p: CrewUpdateParams = parse_params(p)?;
            write(state, move |tx| {
                let title_source = p.title.as_ref().map(|_| "human".to_string());
                crews::update(tx, &p.crew_id, crews::CrewPatch { title: p.title, title_source, working_directory: p.working_directory, hidden: p.hidden, status_line: p.status_line, policy: None })
            })
            .await
        }
        methods::CREW_ADOPT => {
            let p: CrewAdoptParams = parse_params(p)?;
            write(state, move |tx| crews::adopt(tx, &p.crew_id, &p.parent_id)).await
        }
        methods::CREW_RELEASE => {
            let p: CrewReleaseParams = parse_params(p)?;
            write(state, move |tx| crews::release(tx, &p.crew_id, &p.from_parent_id, p.to_parent_id.as_deref())).await
        }
        methods::CREW_DELETE => {
            let p: CrewIdParams = parse_params(p)?;
            write(state, move |tx| crews::delete(tx, &p.crew_id).map(|()| Empty {})).await
        }
        methods::CHANNEL_LIST => {
            let p: CrewIdParams = parse_params(p)?;
            write(state, move |tx| channels::list_for_crew(tx, &p.crew_id)).await
        }
        methods::CHANNEL_CREATE => {
            let p: ChannelCreateParams = parse_params(p)?;
            write(state, move |tx| {
                channels::create(tx, &Sender::Human, channels::NewChannel { crew_id: p.crew_id, kind: p.kind, name: p.name, purpose: p.purpose, members: p.members })
            })
            .await
        }
        methods::CHANNEL_UPDATE => {
            let p: ChannelUpdateParams = parse_params(p)?;
            write(state, move |tx| {
                channels::update(tx, &Sender::Human, &p.channel_id, channels::ChannelPatch { name: p.name, purpose: p.purpose, archived: p.archived })
            })
            .await
        }
        methods::CHANNEL_ADD_MEMBERS => {
            let p: ChannelMembersParams = parse_params(p)?;
            write(state, move |tx| channels::add_members(tx, &Sender::Human, &p.channel_id, &p.members)).await
        }
        methods::CHANNEL_REMOVE_MEMBER => {
            let p: ChannelMemberParams = parse_params(p)?;
            write(state, move |tx| channels::remove_member(tx, &Sender::Human, &p.channel_id, &p.member)).await
        }
        methods::CHANNEL_WATCH => {
            let p: ChannelWatchParams = parse_params(p)?;
            write(state, move |tx| channels::watch(tx, &p.channel_id, p.on)).await
        }
        methods::CHANNEL_SET_OWNER => {
            let p: ChannelSetOwnerParams = parse_params(p)?;
            write(state, move |tx| channels::force_owner(tx, &Sender::Human, &p.channel_id, &p.owner)).await
        }
        methods::MESSAGE_LIST => {
            let p: MessageListParams = parse_params(p)?;
            read(state, move |c| messages::list(c, &p.channel_id, p.after_seq, p.before_seq, p.limit.unwrap_or(200))).await
        }
        methods::MESSAGE_POST => {
            let p: MessagePostParams = parse_params(p)?;
            write(state, move |tx| {
                messages::post(tx, Sender::Human, messages::NewPost { channel_id: p.channel_id, text: p.text, in_reply_to: p.in_reply_to, mentions: p.mentions, attachments: p.attachments, category: None })
                    .map(|posted| posted.message)
            })
            .await
        }
        methods::MESSAGE_SEARCH => {
            let p: MessageSearchParams = parse_params(p)?;
            read(state, move |c| {
                messages::search(c, &messages::Search { query: p.query, crew_id: p.crew_id, channel_ids: p.channel_ids, after: p.after, before: p.before, limit: p.limit.unwrap_or(50) })
            })
            .await
        }
        methods::READ_MARK => {
            let p: ReadMarkParams = parse_params(p)?;
            write(state, move |tx| {
                cursors::mark_read(tx, &p.channel_id, p.seq)?;
                channels::summary(tx.conn(), &p.channel_id)
            })
            .await
        }
        methods::SESSION_LIST => {
            let p: SessionListParams = parse_params_or_default(p)?;
            read(state, move |c| sessions::list(c, p.crew_id.as_deref())).await
        }
        methods::SESSION_GET => {
            let p: SessionGetParams = parse_params(p)?;
            read(state, move |c| sessions::get(c, &p.session_id)).await
        }
        methods::ASK_LIST => {
            let p: AskListParams = parse_params_or_default(p)?;
            let now = crewd_store::now_ms();
            read(state, move |c| asks::waits(c, now, p.root_crew_id.as_deref())).await
        }
        methods::ASK_ANSWER => {
            let p: AskAnswerParams = parse_params(p)?;
            write(state, move |tx| asks::answer(tx, &p.crew_id, &p.ask, &p.text)).await
        }
        methods::ASK_WITHDRAW => {
            let p: AskWithdrawParams = parse_params(p)?;
            write(state, move |tx| asks::withdraw(tx, &Sender::Human, &p.crew_id, &p.ask, &p.reason)).await
        }
        methods::APPROVAL_LIST => {
            parse_no_params(p)?;
            read(state, approvals::list_pending).await
        }
        methods::APPROVAL_DECIDE => {
            let p: ApprovalDecideParams = parse_params(p)?;
            write(state, move |tx| approvals::decide(tx, &p.id, p.allow, "human")).await
        }
        methods::RESTART_PENDING => {
            parse_no_params(p)?;
            read(state, restart::pending).await
        }
        methods::RESTART_ANSWER => {
            let p: RestartAnswerParams = parse_params(p)?;
            write(state, move |tx| restart::answer(tx, p.id, p.answer)).await
        }
        methods::DIRECTORY_LIST => {
            let p: DirectoryListParams = parse_params_or_default(p)?;
            read(state, move |c| directory::list(c, p.query.as_deref())).await
        }
        methods::TODO_LIST => {
            let p: TodoListParams = parse_params(p)?;
            read(state, move |c| Ok(crewd_store::todos::list(c, &p.crew_id, p.ledger)?)).await
        }
        other => Err(RpcError::new(RpcError::METHOD_NOT_FOUND, format!("未知方法: {other}"))),
    }
}
```

（`crewd-server` 依赖 `crewd-work` 才能匹配 `Rejected::Forbidden`：`cargo add --offline -p crewd-server --path crates/work`，`Cargo.toml` 与 `Cargo.lock` 一并提交。`read(state, crews::list_by_activity)` 这类直接传函数名的写法若推不出生命周期，换成闭包 `|c| crews::list_by_activity(c)`。）

`crates/server/src/conn.rs` 不用改（它读 `events::TOPICS`）。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-server && cargo test -p crewd --test e2e`
Expected: socket 新测试 6 条 + 保留的 2 条通过；e2e 里 `committed_schema_matches_generated` 会红（契约变了）——下一步重新生成。

- [ ] **Step 6: 重新生成契约、全量检查、提交**

```bash
cargo run -q -p crewd -- schema > schema/crewd-api.schema.json
./scripts/check.sh
git add crates/proto/src/params.rs crates/proto/src/lib.rs crates/proto/src/model.rs crates/server/Cargo.toml crates/server/src/dispatch.rs crates/server/tests/socket.rs crates/crewd/tests/e2e.rs schema/crewd-api.schema.json Cargo.lock
git diff --cached --stat
git commit -m "crewd: client RPC on channels/messages/asks/approvals/restart; strict params; typed error codes; drop whiteboard.*

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 14: 契约整理（单一 `$defs`、类型化方法 / 事件表）+ `crewd rpc` 驱动全部方法

§14 遗留：契约还不能给 Swift 生成代码（结果 / 事件是字符串描述、缺 ServerFrame、每个类型各自一份 `$defs`、参数没 deny_unknown_fields、枚举是字符串）。本任务把 `api_schema()` 改成一个 `SchemaGenerator` 生成：每个方法 `{params: $ref, result: $ref | array | nullable}`、每个事件 `{$ref}`、帧 `Request` / `ServerFrame`，全部类型只在顶层 `$defs` 出现一次。

**Files:**
- Modify: `crewd/crates/proto/src/lib.rs`（`api_schema` 重写 + 测试）、`crewd/crates/proto/src/model.rs`（`CrewRef`、`Lagged`、`Subscribed` 三个小结构）、`crewd/crates/server/src/hub.rs` / `conn.rs`（`crew.removed`、`events.lagged`、订阅回执改用这三个结构，线上 JSON 不变）、`crewd/crates/crewd/tests/e2e.rs`、`crewd/schema/crewd-api.schema.json`

**Interfaces:**
- Consumes: Task 13 的全部类型与 `methods::ALL`。
- Produces：`api_schema() -> Value`，形如 `{apiVersion: 2, transport, frames: {Request, ServerFrame}, methods: {<name>: {params, result}}, events: {<name>: schema}, $defs: {...}}`；`crewd_proto::{CrewRef { crew_id }, Lagged { missed: u64 }, Subscribed { topics: Vec<String> }}`。

- [ ] **Step 1: 写失败的测试**

`crates/proto/src/lib.rs` 测试模块：删掉 `api_schema_lists_methods_and_types`，加：

```rust
    fn walk_refs(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                if let Some(Value::String(r)) = m.get("$ref") {
                    out.push(r.clone());
                }
                m.values().for_each(|x| walk_refs(x, out));
            }
            Value::Array(a) => a.iter().for_each(|x| walk_refs(x, out)),
            _ => {}
        }
    }

    #[test]
    fn schema_lists_exactly_the_client_methods() {
        let s = api_schema();
        let mut listed: Vec<&str> = s["methods"].as_object().unwrap().keys().map(|k| k.as_str()).collect();
        listed.sort();
        let mut all: Vec<&str> = methods::ALL.to_vec();
        all.sort();
        assert_eq!(listed, all);
        assert_eq!(s["apiVersion"], 2);
    }

    #[test]
    fn every_ref_resolves_to_a_single_top_level_def() {
        let s = api_schema();
        let defs = s["$defs"].as_object().unwrap();
        let mut refs = Vec::new();
        walk_refs(&s, &mut refs);
        assert!(!refs.is_empty());
        for r in refs {
            let name = r.strip_prefix("#/$defs/").unwrap_or_else(|| panic!("不是顶层 $defs 引用：{r}"));
            assert!(defs.contains_key(name), "悬空引用 {r}");
        }
        for (name, d) in defs {
            assert!(d.get("$defs").is_none(), "{name} 自带了一份 $defs");
        }
    }

    #[test]
    fn every_params_type_denies_unknown_fields_and_vocabularies_are_enums() {
        let s = api_schema();
        let defs = s["$defs"].as_object().unwrap();
        for (m, spec) in s["methods"].as_object().unwrap() {
            let r = spec["params"]["$ref"].as_str().unwrap_or_else(|| panic!("{m} 的 params 不是 $ref"));
            let def = &defs[r.trim_start_matches("#/$defs/")];
            assert_eq!(def["additionalProperties"], json!(false), "{m} 的参数没有 deny_unknown_fields");
        }
        for e in ["SenderKind", "MentionKind", "Role", "ChannelKind", "Desired", "AskStateView", "RestartAnswer"] {
            assert!(defs[e].get("enum").is_some() || defs[e].get("oneOf").is_some(), "{e} 不是封闭枚举：{}", defs[e]);
        }
        for e in [events::MESSAGE_APPENDED, events::SESSION_CHANGED, events::LAGGED] {
            assert!(s["events"][e].is_object(), "缺事件 {e}");
        }
    }
```

`crates/crewd/tests/e2e.rs` 加：

```rust
/// 每个客户端方法都能经 `crewd rpc` 走到处理函数（不是 -32601）：能成功的就要求成功；需要 agent 先造数据的
/// （回答问、审批、重启询问）用不存在的号，要求得到「不存在 / 规则不过」而不是「没有这个方法」。
#[test]
fn rpc_drives_every_client_method() {
    let data = tempfile::tempdir().unwrap();
    let out = crewd(data.path()).args(["import", "--from"]).arg(fixture()).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let _d = start(data.path());
    enum Want {
        Ok,
        Code(i32),
    }
    use Want::*;
    let cases: Vec<(&str, &str, Want)> = vec![
        ("system.hello", "null", Ok),
        ("crew.list", "null", Ok),
        ("crew.get", r#"{"crewId":"local-aaaa"}"#, Ok),
        ("crew.create", r#"{"title":"端到端","workingDirectory":"/tmp/example"}"#, Ok),
        ("crew.update", r#"{"crewId":"local-bbbb","statusLine":"跑端到端"}"#, Ok),
        ("crew.adopt", r#"{"crewId":"local-bbbb","parentId":"local-aaaa"}"#, Ok),
        ("crew.release", r#"{"crewId":"local-bbbb","fromParentId":"local-aaaa","toParentId":"local-aaaa"}"#, Ok),
        ("crew.delete", r#"{"crewId":"nope"}"#, Code(-32004)),
        ("channel.list", r#"{"crewId":"local-aaaa"}"#, Ok),
        ("channel.create", r#"{"crewId":"local-aaaa","kind":"topic","name":"端到端群"}"#, Ok),
        ("channel.update", r#"{"channelId":"local-aaaa/human","purpose":"找人"}"#, Ok),
        ("channel.addMembers", r#"{"channelId":"local-aaaa/human","members":[{"kind":"session","id":"worker-2222bbbb"}]}"#, Ok),
        ("channel.removeMember", r#"{"channelId":"local-aaaa/human","member":{"kind":"session","id":"worker-2222bbbb"}}"#, Ok),
        ("channel.watch", r#"{"channelId":"local-aaaa/lobby","on":true}"#, Ok),
        ("channel.setOwner", r#"{"channelId":"local-aaaa/lobby","owner":{"kind":"human"}}"#, Code(-32602)),
        ("message.list", r#"{"channelId":"local-aaaa/lobby"}"#, Ok),
        ("message.post", r#"{"channelId":"local-aaaa/lobby","text":"切换后第一条 ✅"}"#, Ok),
        ("message.search", r#"{"query":"切换后"}"#, Ok),
        ("read.mark", r#"{"channelId":"local-aaaa/lobby","seq":1}"#, Ok),
        ("session.list", "null", Ok),
        ("session.get", r#"{"sessionId":"captain-1111aaaa"}"#, Ok),
        ("ask.list", "null", Ok),
        ("ask.answer", r#"{"crewId":"local-aaaa","ask":"H1","text":"x"}"#, Code(-32602)),
        ("ask.withdraw", r#"{"crewId":"local-aaaa","ask":"H1","reason":"x"}"#, Code(-32602)),
        ("approval.list", "null", Ok),
        ("approval.decide", r#"{"id":"ap-none","allow":true}"#, Code(-32004)),
        ("restart.pending", "null", Ok),
        ("restart.answer", r#"{"id":99,"answer":"later"}"#, Code(-32004)),
        ("directory.list", "null", Ok),
        ("todo.list", r#"{"crewId":"local-aaaa","ledger":"agent"}"#, Ok),
        ("events.subscribe", r#"{"topics":["message"]}"#, Ok),
    ];
    let mut covered: Vec<&str> = cases.iter().map(|c| c.0).collect();
    covered.sort();
    let mut all = crewd_proto::methods::ALL.to_vec();
    all.sort();
    assert_eq!(covered, all, "加了新方法就在这里加一条");
    for (method, params, want) in cases {
        let out: Output = crewd(data.path()).args(["rpc", method, params]).output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        match want {
            Ok => assert!(out.status.success(), "{method}: {err}"),
            Code(c) => {
                assert!(!out.status.success(), "{method} 应当失败");
                assert!(err.contains(&format!("({c})")), "{method}: 期望 {c}，实际 {err}");
            }
        }
    }
}
```

（`crewd` 二进制 crate 的测试要能用 `crewd_proto`：它已是 `crewd` 的依赖，集成测试可以直接 `use`。）

`import_then_serve_then_rpc` 里 `message.post` 已在 Task 13 改好；本任务不再动它。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-proto && cargo test -p crewd --test e2e rpc_drives_every_client_method`
Expected: proto 三条新测试红（方法表还是字符串、没有 `$defs`）；e2e 这条应当已经绿（Task 13 已实现全部方法）——若红，先修 Task 13 的遗漏。

- [ ] **Step 3: 重写 `api_schema`**

`crates/proto/src/model.rs` 末尾加：

```rust
/// `crew.removed` 的数据。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CrewRef {
    pub crew_id: String,
}

/// `events.lagged`：漏掉的事件条数，客户端应全量重拉。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Lagged {
    pub missed: u64,
}

/// `events.subscribe` 的回执。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Subscribed {
    pub topics: Vec<String>,
}
```

`hub.rs` 的 `CrewRemoved` 分支改成 `ev(events::CREW_REMOVED, &CrewRef { crew_id: crew_id.clone() })?`；`conn.rs` 里 `json!({ "missed": n })` → `serde_json::to_value(Lagged { missed: n }).unwrap()`、`json!({ "topics": p.topics })` → `serde_json::to_value(Subscribed { topics: p.topics.clone() }).unwrap()`（线上 JSON 不变，只是有了类型）。

`crates/proto/src/lib.rs`，`api_schema` 整个替换：

```rust
fn sub<T: JsonSchema>(g: &mut schemars::SchemaGenerator) -> Value {
    serde_json::to_value(g.subschema_for::<T>()).expect("schema 可序列化")
}

fn many(v: Value) -> Value {
    json!({ "type": "array", "items": v })
}

fn nullable(v: Value) -> Value {
    json!({ "anyOf": [v, { "type": "null" }] })
}

fn m(params: Value, result: Value) -> Value {
    json!({ "params": params, "result": result })
}

/// 整份 API 契约（`crewd schema` 打印它；仓库里提交一份，测试保证两者一致）。
/// 全部类型只在顶层 `$defs` 出现一次；方法 / 事件 / 帧都是 `$ref`，Swift 端可以直接据此生成代码。
pub fn api_schema() -> Value {
    let mut gen = schemars::SchemaGenerator::default();
    let g = &mut gen;
    let methods = json!({
        methods::HELLO: m(sub::<NoParams>(g), sub::<Hello>(g)),
        methods::CREW_LIST: m(sub::<NoParams>(g), many(sub::<Crew>(g))),
        methods::CREW_GET: m(sub::<CrewIdParams>(g), sub::<CrewDetail>(g)),
        methods::CREW_CREATE: m(sub::<CrewCreateParams>(g), sub::<Crew>(g)),
        methods::CREW_UPDATE: m(sub::<CrewUpdateParams>(g), sub::<Crew>(g)),
        methods::CREW_ADOPT: m(sub::<CrewAdoptParams>(g), sub::<Crew>(g)),
        methods::CREW_RELEASE: m(sub::<CrewReleaseParams>(g), sub::<Crew>(g)),
        methods::CREW_DELETE: m(sub::<CrewIdParams>(g), sub::<Empty>(g)),
        methods::CHANNEL_LIST: m(sub::<CrewIdParams>(g), many(sub::<ChannelSummary>(g))),
        methods::CHANNEL_CREATE: m(sub::<ChannelCreateParams>(g), sub::<Channel>(g)),
        methods::CHANNEL_UPDATE: m(sub::<ChannelUpdateParams>(g), sub::<Channel>(g)),
        methods::CHANNEL_ADD_MEMBERS: m(sub::<ChannelMembersParams>(g), sub::<Channel>(g)),
        methods::CHANNEL_REMOVE_MEMBER: m(sub::<ChannelMemberParams>(g), sub::<Channel>(g)),
        methods::CHANNEL_WATCH: m(sub::<ChannelWatchParams>(g), sub::<ChannelSummary>(g)),
        methods::CHANNEL_SET_OWNER: m(sub::<ChannelSetOwnerParams>(g), sub::<Channel>(g)),
        methods::MESSAGE_LIST: m(sub::<MessageListParams>(g), many(sub::<Message>(g))),
        methods::MESSAGE_POST: m(sub::<MessagePostParams>(g), sub::<Message>(g)),
        methods::MESSAGE_SEARCH: m(sub::<MessageSearchParams>(g), many(sub::<Message>(g))),
        methods::READ_MARK: m(sub::<ReadMarkParams>(g), sub::<ChannelSummary>(g)),
        methods::SESSION_LIST: m(sub::<SessionListParams>(g), many(sub::<Session>(g))),
        methods::SESSION_GET: m(sub::<SessionGetParams>(g), sub::<Session>(g)),
        methods::ASK_LIST: m(sub::<AskListParams>(g), many(sub::<HumanWait>(g))),
        methods::ASK_ANSWER: m(sub::<AskAnswerParams>(g), sub::<Ask>(g)),
        methods::ASK_WITHDRAW: m(sub::<AskWithdrawParams>(g), sub::<Ask>(g)),
        methods::APPROVAL_LIST: m(sub::<NoParams>(g), many(sub::<Approval>(g))),
        methods::APPROVAL_DECIDE: m(sub::<ApprovalDecideParams>(g), sub::<Approval>(g)),
        methods::RESTART_PENDING: m(sub::<NoParams>(g), nullable(sub::<RestartPrompt>(g))),
        methods::RESTART_ANSWER: m(sub::<RestartAnswerParams>(g), sub::<RestartPrompt>(g)),
        methods::DIRECTORY_LIST: m(sub::<DirectoryListParams>(g), many(sub::<DirectoryRow>(g))),
        methods::TODO_LIST: m(sub::<TodoListParams>(g), many(sub::<Todo>(g))),
        methods::SUBSCRIBE: m(sub::<SubscribeParams>(g), sub::<Subscribed>(g)),
    });
    let events = json!({
        events::MESSAGE_APPENDED: sub::<Message>(g),
        events::CHANNEL_CHANGED: sub::<ChannelSummary>(g),
        events::CREW_CHANGED: sub::<Crew>(g),
        events::CREW_REMOVED: sub::<CrewRef>(g),
        events::BADGE_CHANGED: sub::<Badge>(g),
        events::ASK_CHANGED: sub::<Ask>(g),
        events::APPROVAL_CHANGED: sub::<Approval>(g),
        events::SESSION_CHANGED: sub::<SessionLive>(g),
        events::RESTART_CHANGED: sub::<RestartPrompt>(g),
        events::LAGGED: sub::<Lagged>(g),
    });
    let frames = json!({
        "Request": sub::<Request>(g),
        "ServerFrame": sub::<ServerFrame>(g),
    });
    json!({
        "apiVersion": API_VERSION,
        "transport": "NDJSON over Unix socket; one Request per line, server replies ServerFrame per line",
        "topics": events::TOPICS,
        "frames": frames,
        "methods": methods,
        "events": events,
        "$defs": gen.take_definitions(true),
    })
}
```

（`json!` 里依次求值，`g` 的可变借用一次一个。`gen` 在最后 `take_definitions` 前不能还被 `g` 借着——`g` 的最后一次使用在 `frames` 之后，借用检查器接受；若报错，把三个 `json!` 结果先各自绑定再取定义。`schemars` 1.2 的 `SchemaGenerator::default()` 用 draft 2020-12、`$defs` 路径，`subschema_for` 返回 `{"$ref": "#/$defs/Name"}`——已在 `~/.cargo/registry/src/*/schemars-1.2.2/src/generate.rs` 核过 `subschema_for` / `take_definitions(apply_transforms: bool)` 的签名。）

- [ ] **Step 4: 重新生成契约并跑全量**

```bash
cargo run -q -p crewd -- schema > schema/crewd-api.schema.json
./scripts/check.sh
```

Expected: 全绿（含 e2e 的 `committed_schema_matches_generated` 与 `rpc_drives_every_client_method`）。

- [ ] **Step 5: 变异：证明 deny_unknown_fields 那把尺子会红**

```bash
cp crates/proto/src/params.rs target/params.rs.bak
```

把 `params!` 宏里的 `deny_unknown_fields` 删掉，跑 `cargo test -p crewd-proto every_params_type_denies_unknown_fields`，Expected: FAIL（第一个方法名就报）。`cp target/params.rs.bak crates/proto/src/params.rs` 还原，再跑确认 PASS。

- [ ] **Step 6: 提交**

```bash
git add crates/proto/src/lib.rs crates/proto/src/model.rs crates/server/src/hub.rs crates/server/src/conn.rs crates/crewd/tests/e2e.rs schema/crewd-api.schema.json
git diff --cached --stat
git commit -m "crewd-proto: contract with one \$defs, typed method/event/frame tables; e2e drives every client method via crewd rpc

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 15: 真实数据只读演练、README、spec 交付记录

spec §22.2 验收第一条：在**副本**上跑 v2→v3（照 M1 的做法）。**这一步读真实数据目录**：开跑前先在 crew 群里报一句（「M3a 要在真实数据的只读副本上跑 v2→v3 演练，只报计数」），不需要等回复，但要留痕。源目录只读；副本与产出都在 scratchpad（或 `target/`），**不进仓库**，报告里只写计数，不写任何消息正文、机组名、session 名。

**Files:**
- Modify: `crewd/README.md`、`docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`

- [ ] **Step 1: 准备 v2 库（用 M3a 之前的 crewd）**

在 scratchpad（下称 `$S`，用会话给的 scratchpad 绝对路径；没有就用 `crewd/target/m3a-dryrun`）：

```bash
mkdir "$S/m3a-dryrun"                               # 已存在就报错停下，别 rm -rf
git archive <M3a 开工前的 main 提交> crewd | tar -x -C "$S/m3a-dryrun"
cargo build -q --manifest-path "$S/m3a-dryrun/crewd/Cargo.toml" -p crewd     # 这是 v2 的 crewd（不 cd）
cp -R "<旧数据目录>" "$S/m3a-dryrun/legacy-copy"       # 旧数据目录缺省 ~/Library/Application Support/PendingCrew；只读复制
"$S/m3a-dryrun/crewd/target/debug/crewd" --data-dir "$S/m3a-dryrun/v2" import --from "$S/m3a-dryrun/legacy-copy" > "$S/m3a-dryrun/import-report.json"
cp "$S/m3a-dryrun/v2/crewd.db" "$S/m3a-dryrun/v2-before.db"
```

（`<M3a 开工前的 main 提交>` = 本分支 Task 1 之前的那个提交，`git log --oneline` 里第一个 M3a 提交的父提交。）

- [ ] **Step 2: 迁移前的读数**

```bash
DB="$S/m3a-dryrun/v2/crewd.db"
sqlite3 "$DB" "PRAGMA user_version; SELECT COUNT(*) FROM crews; SELECT COUNT(*) FROM sessions; SELECT COUNT(*) FROM messages; SELECT seq FROM sqlite_sequence WHERE name='messages';"
sqlite3 "$DB" "SELECT seq FROM messages ORDER BY seq" > "$S/m3a-dryrun/seq-before.txt"
sqlite3 "$DB" "SELECT sender_kind, COUNT(*) FROM messages GROUP BY 1"
```

- [ ] **Step 3: 用 M3a 的 crewd 打开（触发迁移），再读**

```bash
cargo build -q -p crewd       # 在本 worktree 的 crewd/ 下
./target/debug/crewd --data-dir "$S/m3a-dryrun/v2" serve &   # 记下 $! 这个 pid；看到 socket 出现后用 kill <pid> 停（不按名字杀）
./target/debug/crewd --data-dir "$S/m3a-dryrun/v2" rpc system.hello null
kill <上面记下的 pid>
sqlite3 "$DB" "PRAGMA user_version; SELECT COUNT(*) FROM messages; SELECT seq FROM sqlite_sequence WHERE name='messages';"
sqlite3 "$DB" "SELECT seq FROM messages ORDER BY seq" > "$S/m3a-dryrun/seq-after.txt"
diff "$S/m3a-dryrun/seq-before.txt" "$S/m3a-dryrun/seq-after.txt" && echo SEQ_SAME
sqlite3 "$DB" "SELECT COUNT(*) FROM crews c WHERE (SELECT COUNT(*) FROM channels ch WHERE ch.crew_id=c.id AND ch.kind='lobby') <> 1;"   # 期望 0
sqlite3 "$DB" "SELECT COUNT(*) FROM sessions s LEFT JOIN read_cursors r ON r.reader=s.id AND r.channel_id=s.crew_id||'/lobby' WHERE r.seq IS NULL OR r.seq < COALESCE((SELECT MAX(seq) FROM messages m WHERE m.channel_id=s.crew_id||'/lobby'),0);"   # 期望 0
sqlite3 "$DB" "SELECT COUNT(*) FROM channels ch LEFT JOIN read_cursors r ON r.reader='human' AND r.channel_id=ch.id WHERE r.seq IS NULL OR r.seq < COALESCE((SELECT MAX(seq) FROM messages m WHERE m.channel_id=ch.id),0);"   # 期望 0
sqlite3 "$DB" "SELECT COUNT(*) FROM wakes; SELECT COUNT(*) FROM pragma_foreign_key_check;"   # 期望 0 / 0
sqlite3 "$DB" "SELECT sender_kind, COUNT(*) FROM messages GROUP BY 1"
```

（serve 若因迁移失败拒绝启动，stderr 会列出认不出的 sender / mention 词：**停下，把那些值报给人**，不要改归一表去猜它们的含义。`serve &` 在 zsh 里后台跑；`$!` 立刻读，别跨命令。）

- [ ] **Step 4: 再开一次，确认没有变化**

```bash
cp "$DB" "$S/m3a-dryrun/after-first.db"
./target/debug/crewd --data-dir "$S/m3a-dryrun/v2" serve &   # 同上，记 pid、hello、kill <pid>
sqlite3 "$DB" "SELECT COUNT(*) FROM channels; SELECT COUNT(*) FROM read_cursors; SELECT value FROM meta WHERE key='nextCrewNumber';"
sqlite3 "$S/m3a-dryrun/after-first.db" "SELECT COUNT(*) FROM channels; SELECT COUNT(*) FROM read_cursors; SELECT value FROM meta WHERE key='nextCrewNumber';"
```

Expected: 两组读数逐项相等。

- [ ] **Step 5: 新导入路径（v3 直接导入）也跑一次**

```bash
./target/debug/crewd --data-dir "$S/m3a-dryrun/v3-fresh" import --from "$S/m3a-dryrun/legacy-copy" > "$S/m3a-dryrun/import-v3.json"
```

对 `v3-fresh/crewd.db` 跑 Step 3 的同一组 SQL（大厅、游标、外键），期望全 0；消息条数与 `import-report.json` 的 `counts.messages` 一致，`messagesWithUnknownSender` / `messagesWithUnknownMention` 若非 0，报给人。

- [ ] **Step 6: README 与 spec 交付记录**

`crewd/README.md` 的 crate 列表加一行：`- crates/collab（crewd-collab）：协作层——群、成员、群主、消息与 @ 解析、游标与红标、机组 DAG、人类待确认（H 号经 crewd-work）、审批、重启询问、唤醒队列与熔断台账、渲染（M3a）。`

spec 末尾追加一节（节号 = 当时 spec 里最大的节号 + 1；标题 `M3a 交付记录与遗留（计划 docs/superpowers/plans/2026-10-06-crewd-m3a-data-and-api.md）`），内容按下面的骨架填**实测**数字（不要抄这里的占位，每个数字回原始输出逐字抄）：

```markdown
已交付：schema v3 + v2→v3 迁移（seq 高水位、词表归一、大厅回填、游标钉末尾、机长指针、分机号、机组号计数器）；Db actor；crewd-collab（…按计划「文件结构」列）；客户端 RPC 与事件（…列方法）；契约 v2（单一 $defs）；`crewd rpc` 驱动全部方法。

真实数据只读演练（副本，<日期>）：v2 crewd 导入 → <N> 机组 / <N> session / <N> 消息；M3a 迁移后消息条数与 seq 集合一致（diff 空）；大厅数 = 机组数；session 未读 0、人类红标 0、唤醒 0、外键 0；再开一次各项不变；v3 直接导入同样全 0；认不出的 sender / mention：<列出或「无」>。

与 spec 原文不同 / 补充的决定（理由见计划「裁定」）：CollabEvent 类型放在 collab（裁定 3）；v3 补列 crews.captain_session_id / next_ext、sessions.ext / last_output_at、meta.nextCrewNumber、restart_prompts 表、channels.owner_* / pending_owner_*（裁定 8、§22.7）；wake_latches.scope 改为 edge|session（§22.7 Q5）；ask_answer 的 source 带机组（裁定 13）；SessionControl 多了 resume / handoff_captain（裁定 15）；队列与 session 生命周期的写函数全部在 collab（裁定 16）；ask.answer 的回调点 = Change::AskAnswered（裁定 17）；附件上传未实现（裁定 14）。

遗留：worldview::render 是空串（M3b 实现；M3 集成验收必须核世界观非空）；默认方案到点的 apply_due_defaults 没有定时调用方（spec 问题 5，M3c 或 M4b 认领）；语言只能手工写 meta.locale；message.search 是全表 instr 扫描（量大再加 FTS）；channels.list_for_crew 有写副作用（懒建人类群）所以走写路径。
```

- [ ] **Step 7: 提交**

```bash
./scripts/check.sh
git add README.md ../docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md
git diff --cached --stat
git commit -m "docs: M3a delivery record (real-data dry run counts, decisions, leftovers)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## 覆盖对照（spec → 任务）

| spec 条目 | 任务 |
|---|---|
| §22.2 schema v3 全部表（channels / channel_members / channel_watch / messages 重建 / read_cursors / listens / wakes / wake_latches / approvals / work_books / ask_meta / sessions 与 crews 加列） | 3（+ 4 的线上字段） |
| §22.2 Db actor（专用线程）+「慢查询进行中 hello 照常」验收 | 1 |
| §22.2 迁移：确定性大厅 id、旧消息归大厅、人类群懒建、游标钉末尾、session desired=stopped、extra 不解析（除裁定 8 的两个键） | 3、7 |
| §22.2 群规则：成员资格、只有成员能发言、人类群自动入群、dm 两人、跨机组显示、归档拒收可解档、@ 发送时解析、reply_to 追加、目标非成员拒 | 7、8 |
| §22.2 红标与未读按游标派生、read.mark、Mac / iOS 共用 | 7、9 |
| §22.2 根机组 root_of、收编后人类群归档 | 6、7 |
| §22.2 机组 DAG（禁环、内建、直系子、release-to 无副作用、建父 / 子、删除条件、号码规则） | 6 |
| §22.2 人类待确认经 crewd-work（open / answer / apply_default + withdraw_ask）、开问同事务发公告与 ask_meta、审批不发 H 号 | 2、10 |
| §22.2 客户端 RPC 表、事件表、删 whiteboard.* | 13 |
| §22.2 契约整理（typed result / event、deny_unknown_fields、枚举、单一 $defs、ServerFrame） | 4、13、14 |
| §22.2 collab::render（md 节、include_str!、节缺失测试、无字面量） | 10、11 |
| §22.2 验收：真实数据演练、群规则逐条测试、DAG 先红、H 号重开库 / 连续 / 共用计数 / 撤回条件、契约漂移 + deny + rpc 驱动全部方法、Db 线程 | 15、7–8、6、10、14、1 |
| §22.5 冻结接口（schema、Db、Collab、render、CollabEvent、WakeRequest、TokenTable、SessionControl + Fake、worldview / turn_trace 签名） | 1、3、5–12（`CommsSpec` 归 M3b） |
| §22.6 给 M4b 的口：work_books 读改写事务、H 号唯一发号处、wakes kind checkpoint / stop_check 与渲染节、resume_failed_at、BlockerCleared 回调点 | 10（`with_book`）、5、11、6、10（`AskAnswered.cleared`） |
| §22.7 Q1 重启问人 | 10（`restart`）、13 |
| §22.7 Q2 群主、Jev / 群主兜底、收听 | 7（群主）、9（收听）；叫谁归 M3c |
| §22.7 Q3 不做私有群 / Q4 删工具与侧栏基础序 | 范围；6（`list_by_activity`）、13 |
| §22.7 Q5 不设默认上限、两级熔断的数据 | 5（`settings`、`latches`、`origin`、`recent_for`）、6（`note_output`）、8 |
| §17 「@人类」未注视提示的数据 | 8（`unwatched_human_mention`） |

## 自查记录

- **占位扫描**：两处有意留给执行时按实测定的：Task 3 的 `ts_from_ms` 第二个期望字符串（给了核对命令与唯一允许的改法）、Task 3 Step 6 的 `import_fixture`（指向该测试文件里已有的导入代码）。Task 15 的交付记录骨架里的 `<N>` 是要从实测输出抄的读数，不是计划内容。
- **类型一致**：`Sender`（lib.rs）在 channels / messages / asks / dispatch 里同一个；`MemberRef::{session, to_row, from_row}` 在 Task 4 定义、Task 7 起使用；`Change` 的变体在 Task 5 一次定全，后续任务只 `note`，hub 的 `client_events` 穷举匹配（新增变体会编不过，是好事）。
- **Review Focus**：5 条各有钉住它的测试（写在对应任务）；`render_code_has_no_prose` 与 `every_params_type_denies_unknown_fields` 各有一个变异步骤证明会红。
- **没做过的事**：代码没编译（见开头说明）；`closed_vocab!` 宏、`params!` 宏、`api_schema` 里 `json!` 内连续可变借用这三处最可能要按编译器调整写法；schemars 对 `#[serde(tag = "kind")]` 枚举（`MemberRef`）生成 `oneOf` 而不是 `enum`，契约测试只对平铺枚举断言 `enum | oneOf`。
