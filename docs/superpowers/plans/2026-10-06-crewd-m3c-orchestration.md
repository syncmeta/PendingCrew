# crewd M3c 编排（会话管理 / 唤醒 / 环路熔断 / 重启恢复）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** serve 自己拉起、停掉、接回 agent 会话，并把「谁该醒、什么时候醒、醒来看到什么」做成持久化、可重启、不会打转的唤醒机制：新 crate `crewd-wake`（纯判定）+ `crewd-server::orchestra`（IO 与会话管理），取代 Swift 的 `CrewSessionRunner` 唤醒链、`CrewDeferredWakeQueue`、`AutomaticWakeAdmission`。

**Architecture:** 两层。**纯判定层 `crates/wake`**（不起进程、不碰 tokio / 文件 / 数据库、不读时钟）：唤醒规划（谁醒）、环路熔断（§22.7 Q5）、队列选批与退避、额度 / 重启 / 启动闸策略、待决策通知与 nudge 键表、给 agent 与群看的文字（`prompts/orchestra.{zh,en}.md`）。**IO 层 `crates/server/src/orchestra`**：一个 actor（`Core`）独占全部会话句柄与内存状态，按输入顺序处理「执行器事件 / 协作事件 / 定时拍 / 客户端与 agent 的命令」；数据只经一个 `Port` trait 读写（生产实现 `DbPort` 落在 M3a 的 `Db` / `Collab` 上，测试实现 `MemPort` 在内存里）；执行器只经 `Executor` / `ExecHandle` trait（claude = M2b 的 `start_claude` + `SessionHandle`；codex / acp 回 `Unsupported`，M2c / M2e 合入时各加一个实现）。Jev 的挂钩点是 `Arbiter`（M3c 交付返回 `None` 的桩，M5b 换实现）。

**Tech Stack:** Rust 1.94（edition 2024）、tokio 1.53、crewd-exec（M2b）、crewd-term（M2a，类型）、crewd-store / crewd-collab / crewd-proto（M3a 合入后的实际代码）、rusqlite（经 crewd-store 已在 Cargo.lock）。**不新增第三方 crate**；只新增 workspace 成员 `crewd-wake`，以及给 crewd-server 加 tokio 的 `test-util` 开发特性。

**Spec:** `docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`——§5（跟进引擎的唤醒语义）、§6、§7.1、§17（群 / @人类 / 未注视的群 = 免打扰）、§18（M2b 窄接口与遗留）、§19（M4a：`open_ask`、`reassign_lost` 只认 crewd 自己的恢复失败记录）、§20（M5a：Jev 别名层归 M5b）、**§22 全部，以 §22.7 人在 2026-10-06 的拍板（commit fdc884e）为准**——§22.4 正文里的「全机 12 次/分 准入」「max_live 默认 10」「重启自动接回全部」三处已被 §22.7 Q1 / Q5 取代，本计划按 §22.7 写（见文末「spec 问题」）。Swift 对照（仓库根下）：`Sources/Stores/AutomaticWakeAdmission.swift`、`Sources/Mac/Services/CrewSessionRunner.swift`、`Sources/Mac/LocalRunner/{CrewDeferredWakeQueue,CrewLocalMentionWakeLogic,AgentQuota,SessionPendingDecision,AgentSessionResume}.swift`、`Sources/Mac/Services/CrewLocalMentionWaker.swift`。

> 写计划时**没有**编译或重放本计划的代码（M3a 尚未落地，`DbPort` / `SessionControl` / proto 接线依赖它的实际签名）。纯判定层（Task 1–5）只依赖 Rust 标准库，代码按可直接编译写；IO 层（Task 6–11）的代码按 M2b 合入后的实际 `crewd-exec` API 写（`crewd-m2b@b05a57e` 读过），按 §22.5 的 M3a 名字写的地方都收在 `orchestra/db.rs`、`orchestra/control.rs`、`orchestra/rpc.rs` 三个文件里，Task 10 Step 1 先做对齐。执行时编译不过就按实际签名改调用处，**不改判定、不改测试期望**；对不上的写进交付记录。

## 依赖与前提

- **M3a 已合入 main**（§22.1：M3a 先独立落 main），本计划在从 main 切出的 `crewd-m3c` worktree 上做。M3b 与本计划并行；两边按 §22.5 的文件归属不改对方文件。
- **以 M3a 计划的「冻结给 M3b / M3c 的接口」为准**（`docs/superpowers/plans/2026-10-06-crewd-m3a-data-and-api.md`，与 §22.5 不同处以它为准）。它已包含 §22.7 拍板要的 `channels.owner_*`、`wake_latches.scope ∈ {edge, session}`、`wakes.origin`、`restart_prompts`、`settings::max_live_sessions`，以及控制者裁定补的 `sessions.last_progress_at` + `Change::Progress`（C3）、`wakes::{next_due_after, requeue, by_ids, prune}`、`mark_failed(.., retry_at_ms)`、`cursors::cursor`、`channels::members` 含隐式成员（C6）。M3c 不加迁移、不建表、不读写任何 SQL。
- 用到的 M3a 接口（只在 `db.rs` / `control.rs` / `rpc.rs` 与 serve 启动段里调用；名字与签名逐条见 M3a 计划冻结块）：`Db::{spawn, call}`；`crewd_collab::{run, Tx, Change（含 Progress）, ProgressKind, Sender, Locale, messages::{get, list, post, NewPost}, channels::{get, members（有效成员，含大厅 / 人类群的隐式成员）, owner_of, ensure_human_channel}, cursors::{cursor, unread_for, advance}, listens::active_for_channel, crews::{get, list, captain_of, root_of, set_captain}, sessions::{get, list, register, NewSession, running_desired, set_desired, set_working, set_resume_failed, set_profile}, asks::apply_due_defaults, restart::open, settings::{locale, max_live_sessions, set_max_live_sessions}, wakes::{enqueue, due_for, by_ids, list, mark_inflight, mark_delivered, mark_failed(.., retry_at_ms), requeue, mark_dropped, reset_inflight, sessions_with_due, next_due_after, prune, WakeRequest, WakeRow, Enqueued, source}, wakes::latches::{Scope, Latch, set, clear, list}, render::{build_batch, wake_text}, worldview::{context_for, render}}`；`crewd_store::channels::lobby_id`；`crewd_server::{AppState::{collab, db, hub, tokens, orchestra}, hub::CollabEvent, Hub::{subscribe_collab, publish}, control::{SessionControl, StartRequest, Inspection, QuotaReport, ControlError}, TokenTable::mint}`；`crewd_proto::{Session, SessionLive, HealthView, HealthKindView, DecisionView, Role, RestartAnswer, MemberRef, events::SESSION_CHANGED}`。控制者裁定 C6 之后 M3c **不写任何 SQL**（原「spec 问题」5 的缺口全部由 M3a 补了函数）。
- 用到的 M2b 执行器接口（`crewd/crates/exec`，已读过实际代码）：`driver::{start_claude, ClaudeStart, DriverConfig, SessionHandle, SessionStatus, StartError, SendError, Closed}`、`session::{SessionEvent, ExitInfo, ExitStatusKind, DeliveryOutcome, BRIEF_DELIVERY_ID}`、`delivery::Subject`、`health::{Health, HealthKind}`、`launch::{ClaudeSpec, CommsSpec}`、`env::{compose_child_path, AgentPaths}`、`resume::{AgentIdLedger, MemoryLedger}`、`registry::ProcessRegistry`；`crewd_term::{DecisionEvent, PendingDecision, Snapshot}`；`crewd_server::{reap_orphans, ledger::StoreLedger}`。

## Global Constraints

- 所有命令在 `crewd/` 目录下跑；先 `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`。
- 每个任务结束跑 `./scripts/check.sh`（fmt + clippy -D warnings + test）全绿再提交。**永远不跑 `--ignored`**（`live_claude` 那类只给人点头后手动跑）。
- 提交：只 `git add` 该任务列出的文件（路径相对 `crewd/`，显式列出，绝不 `git add -A` / `git add .` / 目录通配）；提交前 `git diff --cached --stat` 核对文件数；提交信息末尾一行**正好是** `Co-Authored-By: Claude <noreply@anthropic.com>`（不带模型名）；**不 `--amend`**；不 push。
- 依赖：只用 `cargo add --offline`（只取 Cargo.lock 里已有的版本），不手写版本号；`Cargo.lock` 随改动一起提交。
- 依赖方向：`crewd-wake` 不依赖任何 crate（只用 std）；`crewd-server` → `crewd-wake`；`crewd-exec` 不依赖 wake / server / store。
- 纯判定层不调 `SystemTime::now()` / `Instant::now()`；时间一律是调用方给的 `Ms`（Unix 毫秒）。IO 层的时间只从注入的 `Clock` 取（测试用 `ManualClock`）。
- **测试永远不起真 claude**：单元测试用 `StandIn` 执行器；端到端只用 crewd-exec 的 `fake_claude`（`target/<profile>/fake_claude`，找不到就失败并说怎么建，绝不退回 PATH 查找）。库里的 `serve()`（不带选项）**不起任何 agent**（`ClaudeProgram::Disabled`）；`crewd serve` 二进制只在 `--claude PATH` / `CREWD_CLAUDE` 给出时用那个路径，否则才按登录 shell PATH 找——所以每个起 `crewd serve` 的测试都必须设 `CREWD_CLAUDE`（Task 11 有检查步骤）。
- **绝不按名字 / 模式杀进程**（不用 `pkill` / `killall` / `kill $(pgrep …)`）；测试里只按自己 spawn 出来的 pid 发信号。
- 测试只用合成数据（公开仓库）：机组 / session / 消息全是测试里现造的，名字像 `cap` / `w1` / 「合成任务甲」；不读真实 `~/.claude`、不碰真实数据目录。
- 给 agent 或群看的文字只在 `crates/wake/prompts/orchestra.{zh,en}.md`；代码里没有提示词字面量（Task 5 有测试钉住 `crewd-wake` 的判定源码）。错误类型的 `Display`（给客户端 / 日志看）不算提示词。
- zsh 不做词分割：手动跑步骤时照抄命令，别把多个参数塞进一个变量。

## 范围

做：
- `crewd-wake`：唤醒规划（§22.7 Q2：@ 目标 ∪ Jev / 群主 ∪ 收听者，同一条消息一个 session 只叫一次；6h 陈旧闸；自己不叫自己；只在群成员里选）、环路熔断（§22.7 Q5：边 + 目标两级，判据「短时间重复唤醒且没有新产出」，人发起的永不计数、永不被挡，阈值一处常量）、选批（到期、只取最高一档、≤10 条 / ≤8KB）、失败退避（30s → 2min → 放弃并告警）、额度重置唤醒时刻（重置 + 1min，拿不到按 45min 退避）、重启名单排序与「被打断」判定、启动闸（可选上限默认关 + 错开 2s）、待决策通知（按 `numbered` 分叉，键名从键表算）、nudge 键表。
- `crewd-server::orchestra`：会话管理（一个 session 同时只有一个进程；起 / 停 / 接回；`desired` / `was_working` / `last_active_at` / `resume_failed_at` 落库）；执行器抽象与 claude 实现；会话事件 → `session.changed`；续跑被拒的群通知 + 给 agent 的附言；起不来 → `resume_failed_at` + 告警；额度 → 持久化重置唤醒；待决策 → 群通知 + 5 分钟升级；持久唤醒队列的投递（idle 边沿、到点、合并、游标过滤、inflight 回执、退避、放弃告警）；`IdleHook`（M4b 挂「停下即检查」）；`Arbiter`（M5b 挂 Jev，交付桩）；问被回答 → 叫醒提问者 / 机长转达 + `AskAnsweredHook`（M4b 挂 BlockerCleared）；`ContextExtras` 口；重启：回收孤儿 → inflight 退回 pending → **问人要不要恢复**（§22.7 Q1）→ 答「恢复」才错开拉起；积压补救（6h 内的未读 @）；停机。
- 客户端 RPC：`session.start / stop / resume / live`、`crew.setCaptain`、`wake.list`、`admission.recover`（熔断恢复，只有客户端能调）、`orchestra.settings.get / set`（Q5 可选上限）；事件 `session.changed`、`wake.changed`。重启询问的 `restart.pending / restart.answer` 与 `restart.changed` 是 M3a 的（它记账、发人类群公告），M3c 订阅 `Change::RestartAnswered` 后恢复。`SessionControl` 的实现（M3a 的 trait，含 `resume` / `handoff_captain`）。
- `crewd serve --claude PATH`（`CREWD_CLAUDE`）与隐藏的 `--claude-env K=V`（端到端测试给假 claude 传 `FAKE_CLAUDE_LOG`）。

不做：
- codex / ACP 的执行器实现（M2c / M2e 各加一个 `Executor`；codex 会话在 M3c 里 `start` 回 `Unsupported`）。
- `crewd mcp` / `crewd hook`、工具、世界观文案、turn_trace（M3b）；hook 的未读注入（M3b；M3c 只依赖「hook 注入后推进同一个游标」）。
- 中途换模型（`/model` 屏幕驱动，§18 遗留的 `SessionProfileSwitch`）：`SessionControl::set_profile` 只落库、下次启动生效，回执照实说（控制者裁定 C5）。
- 删 git worktree / 删分支：crewd 只建（`isolation: true`，控制者裁定 C8）、永不删，删除是人的动作。
- 侧栏排序（§22.7 Q4 + 控制者裁定 C1：人手动排，`crews.position` / `crew.reorder` 归 M3a，M3c 不碰）；Jev 实现（M5b）；检查点 / 停下即检查 / BlockerCleared 的业务（M4b，只留口）。
- 删 Swift 代码（切换时做）。

## 写计划时定下的裁定（执行时照做，有异议先回报）

1. **新 crate 叫 `crates/wake`**（包 `crewd-wake`），零依赖：§22.4 指定；零依赖让它和 crewd-work 一样能纯算、快测。它不依赖 crewd-collab：§22.5 的 `WakeRequest` 字段类型 M3a 还没定，`crewd-wake` 自带与 `wakes` 表一一对应的 `NewWake` / `QueuedWake`，与 M3a 类型的互转只在 `orchestra/db.rs` 一处。
2. **Swift 的 `AutomaticWakeAdmission` 不移植**。§22.7 Q5（人拍板）明确「绝不按全机总量限流」「30 个 session 正常工作不受影响」，而 Swift 的全机 12 次/分、单机组 5 次/分、单 session 2 次/分会直接挡住 30 个 session 的正常收发。换成 `crewd_wake::breaker::LoopBreaker`（Q5 原文的两级环路熔断）。Swift 里要的两件事仍然保住：①人类唤醒永不被挡（修 `AutomaticWakeAdmission.swift:300` 那个「`window.count >= 12` 排在优先级判断之前」的 bug——新实现里根本没有全机窗口）；②熔断只有人能从客户端解（`admission.recover`，agent 没有入口，同 Swift `:151-160`）。「已受理的唤醒被重放」那条熔断（Swift `:286`）不再需要：队列行一旦 `delivered` 就不会再被选中，重放在结构上不发生。
3. **环路判据（Q5「没有新产出」的落法；控制者裁定 C3）**：目标 T 的「连续无产出投递」= T 上一次有产出之后、10 分钟窗口内送到 T 的非人类唤醒（来源 = `wakes.origin`）。产出 = ① T 调了真干活的工具、② T 名下工作项有变化——两者都是 M3a 的 `sessions::note_progress`（M3b 的 post-tool-use 对非 crewd 工具调、M4b 的工作项命令**必须**调），它写 `sessions.last_progress_at` 并发 `Change::Progress`：M3c 订阅它（`fact_of` → `CollabFact::Progress` → `LoopBreaker::progress`），事件丢了（`Lagged`）时准入前再比一次 `last_progress_at` 兜底。**发言不是产出**（`last_output_at` 只是数据，M3c 不读）；③ T 发了一条消息，**且这条消息叫醒的人里不包含正在叫 T 的那些来源**——回话给叫醒你的人不算。③ 的「回叫不算产出」是必要的：两个 session 互相 @ 打转时每一轮都在发消息（`messages::post` 也会写 `last_output_at`），若「发了消息」就算产出，Q5 点名要截的那种环路永远截不住。边（来源 → T）上超过 8 次、或系统 / 自己定时这类非 session 来源对 T 合计超过 8 次 → 熔断那条边 / T 的自动唤醒，只停这一条环路，告诉人（根机组人类群 @人）和群主（出事的群里 @群主）。数值都在 `LOOP_LIMITS` 一处。
4. **重启不自动接回**（§22.7 Q1；人类 @ 例外见控制者裁定 C4）：serve 起来后 `desired=running` 的 session 全部进「待恢复」名单（内存，按 `desired` 每次启动重新推出来），调 M3a 的 `restart::open`（它在每个相关根机组人类群发一条带名单的 @人公告、客户端经 `restart.pending` / `restart.changed` 弹窗）。人经 M3a 的 `restart.answer{id, answer: resume}` 答「恢复」→ M3c 收 `Change::RestartAnswered`，按机长在前、最近活跃在前给名单里仍在等的各入队一条恢复唤醒（`was_working=1` 的带「被打断」附言），由启动闸错开 2s 逐个 `--resume`；答「先不」或不答 → 都不起。**待恢复名单里的 session 不会被自动唤醒拉起**（Q1「期间给它们的唤醒照常入队」）；之后人可整批（再答 `resume`）或单个（`session.resume`，同样补「被打断」附言）恢复。**例外（控制者裁定 C4）**：询问开出之后，人在群里 @ 了名单里的某一个（新入队的人类消息唤醒，id 大于启动时的水位）→ 只拉起它，等同对它单个 `resume`（被打断的补附言），其余照旧等。启动前就排着的人类唤醒不算（否则「先问再起」形同虚设）。
5. **启动闸**：没有默认上限（Q5）；`orchestra.settings.set{maxLiveSessions}` 可设，存在 M3a 的 `collab::settings::{max_live_sessions, set_max_live_sessions}`（M3c 不自己写 meta 键）。上限只挡**自动**拉起（唤醒拉起、agent 的 `start_session`）；人从客户端起 / 恢复、以及人类唤醒拉起不受上限（§5「人类唤醒不应被全机限流挡」；控制者裁定 C5）。所有拉起之间错开 2s（Q1「错开时间逐个」），客户端单个起 / 恢复不错开。
6. **一个 actor**：`Core` 独占 `live` / `starting` 表，命令与事件串行处理，所以「同一 session 并发 start → 第二个 `AlreadyRunning`」是结构性的（§18 遗留：M2b 只按 claude 会话号挡）。拉起本身（`start_claude` 要落账、写文件、起 PTY，可能几秒）放在派生任务里，结果以 `Input::Launched` 回到 actor；事件泵在同一个任务里、在 `Launched` 之后才开始转发，保证顺序。每次拉起一个 `gen`，旧进程迟到的事件按 gen 丢掉。
7. **投递只在三种时刻发生**：入队当下（目标空闲）、idle 边沿（`SessionEvent::Working(false)`）、定时拍（到点 / 有会话退出腾出位置）。目标在干活、正卡在待决策框、有未结的投递、额度 / 认证类 health 在身 → 不投。额度类 health 在身时只有「额度重置唤醒」到点才先 `clear_quota_health()` 再投（§22.4）。
8. **选批**：只取到期的最高一档优先级（human > system > auto），按 `(created_at, id)`，≤10 条；渲染后超过 8KB 就从尾部减到放得下（至少 1 条）。消息类唤醒若目标游标已越过它（忙时 hook 注入过）→ 直接记 `delivered`、不重投（§22.4、`CrewSessionRunner.swift:479`）。
9. **回执**：投出去记 `inflight`；只有 `Delivery{Delivered}` 才记 `delivered` 并推进消息类唤醒的游标（每个群推到批里最大的 seq，只往前推）。非终局的 `Blocked` / `NotReady` / `Stalled` 不动；终局失败 → `attempts+1`，按 30s / 2min 退避回 pending（退避时刻经 M3a `mark_failed(.., retry_at_ms)` 写进 `not_before`，重启后照样退避），第 3 次放弃记 `dropped`；放弃的批里只要有非 system 优先级的唤醒，就在本机组大厅 @群主（目标自己就是机长时改发根机组人类群 @人）——system 级唤醒失败不告警，免得「告警本身叫不醒」成环（同 `CrewLocalMentionWakeLogic.swift:26-28` 的用意）。
10. **人从客户端停掉一个 session**：`desired=stopped`，并把它此刻还在 pending 的唤醒记 `dropped`（`stopped`）——否则退出事件一到，「腾出位置」的那次定时拍就会被这些旧唤醒立刻拉起它。之后新来的 @ 照样会把它拉起（§22.4「@ 一个已停的 session → 它被 `--resume` 拉起」）。消息本身还在群里，下次醒来照样看得到。
11. **`resume_failed_at` = 「crewd 没能把这个 session 的原对话接回来」**（M4b 的 `reassign_lost` 只认这一列，§19）。写：①拉起报错，或进程在「从没干过活」时带着 `LaunchFailed` 退出（起不来）——告警一次（已经写过就不重复告警）；②`ResumeRefused`（原对话丢了，即使驱动层换新号起来了也算；不另发「起不来」告警，续跑被拒的群通知已经说了）。清：某次拉起**没有**经历续跑被拒、且之后 `Working(true)`（这一次真的接上并干上活了）。
12. **续跑被拒**：事件顺序是 `Spawned{旧号, resumed:true}` → `ResumeRefused{旧号}` → `Spawned{新号, resumed:false}`（M2b 实际代码如此；M2c 计划 2026-10-06 改成 claude / codex 一致）。**以最后一条 `Spawned` 为准**：`Live.agent_session_id` 每条 `Spawned` 都覆盖，`Live.refused` 一旦置上本次进程内不再清。M2b 驱动层已经自动换新号重起、把没投出去的正文搬过去；M3c 补 Swift `AgentSessionResume.swift:223-250` 的两句：群里一条（含 claude 原话与「日志在不在盘上」的诊断），给 agent 一条 system 唤醒（「这是新开的一轮，别假装记得」）；并按裁定 11 写 `resume_failed_at`。
13. **额度**：`Health` 里没有重置时刻（M2b 只认短语），重置时刻由 `QuotaSource` 钩子给（M3c 交付返回 `None` 的桩）→ 一律按 45 分钟退避挂唤醒，群里说明「拿不到可信重置时刻」（Swift `AgentQuota.swift:330-360` 的 fail-loud）。同一 session 已有未投的额度唤醒就不再挂。
14. **待决策**：worker 卡住 → 本机组大厅 @群主（= 机长）；机长卡住 → 根机组人类群 @人（绝不 @ 自己，`SessionPendingDecision.swift:375`）；5 分钟没解 → 升级 @人。同一指纹不重发。键名从 `NUDGE_KEYS` 现算（`SessionPendingDecision.swift:339-366` 的教训：一张表、一份名单）。
15. **问被回答**：叫醒提问者（它已无法恢复 → 叫机长转达）的唤醒由 M3a 的 `asks::answer` 在同一事务里入队（source `ask:<crew>/H<n>`，M3a 计划裁定 13 / 17）；M3c 订阅 `WakeEnqueued` 把它投出去，订阅 `AskAnswered` 把 `cleared`（BlockerCleared 的 (工作项, 负责人)）交给 `AskAnsweredHook`（M4b 的回调点）。默认方案到点（`asks::apply_due_defaults`）由 M3c 的定时拍调（控制者裁定）。别人入队的唤醒（M3b 的 schedule_wakeup、M4b 的检查点、M3a 在人决了审批后入队的 `ask_answer` / source `approval:<id>`，控制者裁定 C2）同样靠 `WakeEnqueued` 触发投递。
16. **积压补救**（Swift `CrewLocalMentionWaker.swift:123` 的 B3）：协作事件只在内存里广播，「消息已落库、规划还没入队」时崩掉会丢唤醒；订阅者落后（`events.lagged`）同理。启动时与收到 `Lagged` 时，对每个 session 的未读消息重新规划一遍（不问 Arbiter，按规则；6h 陈旧闸照旧），靠 `UNIQUE` 去重入队。
17. **非消息类唤醒是至少一次**：kill -9 时正处在 `inflight` 的定时 / 系统唤醒，重启后退回 pending 再投一次（可能重复）；消息类靠游标做到恰好一次。宁可重投不丢（Swift `CrewSessionRunner.swift` `confirmWake` 注释里的论证）。
18. **CommsSpec 只在 `orchestra/exec_claude.rs::comms_for` 一处拼**：M3b 并行改它的形状（加 `token`、去 `crew_id/captain/label/structured_post`）；后合者只改这一个函数。M3b 未合入时 `hooks: false`（helper 还没有 `hook` 子命令）。
19. **测试的确定性**：Core 的单元测试不跑 actor 循环，直接 `core.handle(input)`，时间用 `ManualClock`；派生任务（拉起、事件泵、Arbiter）在 current_thread 运行时上靠 `Rig::settle()`（反复 `yield_now` + 排空收件箱，连续 50 次空才停）推进——不 sleep、不看墙钟。只有 Arbiter 超时那一条用 50ms 真超时。
20. **`isolation: true` 的 worktree 由会话管理器建（控制者裁定 C8，取代 C5 里「`worktree` = 已存在目录」）**：`StartRequest` / `StartSession` / `session.start` 都带显式 `isolation: bool`（不再有路径字符串）。`Core::start_new` 在写 session 行**之前**建：机组工作目录（`Port::crew_workdir`）→ `Worktrees::create`（`orchestra/worktree.rs`，位置 / 分支名 / slug 逐条照 Swift，见 Task 6 Step 6b）→ 路径填进 `NewSession.worktree`（落 `sessions.worktree`）→ session 在那里起；`isolation: false` = 机组共享目录。建失败（不是仓库、detached HEAD、机组没工作目录、git 出错）= `ControlError::Worktree(原因)`：不写行、不起进程、不退回共享目录。之后被 `--resume` 拉起时直接用库里的 `worktree`；目录被人删掉了 = 走既有的「工作目录没了」起不来路径（Review Focus 5），不重建。crewd 永不删 worktree / 分支。

## 人已拍板（§22.7）与控制者裁定（2026-10-06）→ 在哪一处

| 拍板 | 落在 | 改的话只改 |
|---|---|---|
| Q1 重启问人，答「恢复」才错开拉起；待恢复期间唤醒照常入队、不拉起；可整批 / 单个恢复 | 裁定 4；Task 9 | `orchestra/boot.rs::boot` + `crewd_wake::policy::restart_order` |
| Q2 群主；不 @ 的人类发言 → Jev（候选 = 群成员）→ 无 / 低置信交群主；收听者并集；一条消息一个 session 只叫一次 | Task 2 `plan` / `untargeted_human`；Task 8 Arbiter | `crewd_wake::plan::untargeted_human`（Jev 之外的兜底）、`hooks::Arbiter`（M5b） |
| Q5 不设默认上限、可选上限默认关；按「来源 → 目标」与「目标」两级熔断无产出环路；只停那一条、告诉人和群主；阈值一处常量 | 裁定 2、3、5；Task 3、Task 8 | `crewd_wake::breaker::LOOP_LIMITS`；上限 = `orchestra.settings.set` |
| C3 产出信号 = M3a `sessions::note_progress` → `Change::Progress`（工具调用 / 工作项变化；发言不算） | 裁定 3；Task 8（`CollabFact::Progress`）、Task 10（`fact_of`） | `deliver.rs::observe_progress`、`db.rs::fact_of` |
| C4 等重启答复期间，人 @ 名单里的某一个 = 只恢复它（算对它单个 resume）；其余照旧等 | 裁定 4；Task 8 `may_launch` / `launch_for_wakes`、Task 9 `boot`（水位） | `deliver.rs::may_launch` 里 `held` 那几行 |
| C5 人类唤醒绕过可选上限；其余默认照写：环路窗口 10 分钟 / 8 次、人停 session 丢弃它 pending 的唤醒、`set_profile` 只落库下次启动生效、人的唤醒在额度未恢复时也拉起 | 裁定 3、5、10；Task 3、7、8 | `LOOP_LIMITS`、`LaunchGate::check` 的 `bypass_cap`、`core.rs::stop`、`control.rs`、`deliver.rs::may_launch` |
| C7 默认方案到点：M3c 的定时拍调 `asks::apply_due_defaults` | 裁定 15；Task 9 | `on_tick` |
| C8 `start_session` 保留 `isolation`（必填 bool）：`true` = 会话管理器在机组工作目录里新建 git worktree（照 Swift 的位置 / 分支名），记进 `sessions.worktree` 并在那里起；失败 = 起不来、不退回共享目录；crewd 不删 worktree | 裁定 20；Task 6 Step 6b、Task 7、Task 10 | `orchestra/worktree.rs`、`core.rs::start_new` |

## 原「仍待人拍板」1–6 的去向

全部由控制者裁定定下（上表 C4 / C5），不再待人拍板：1 环路判据（10 分钟 / 8 次、回叫不算产出）照写；2 人类唤醒不受可选上限照写；**3 改了**：等重启答复期间人 @ 某一个 = 只恢复它（C4）；4 人停 session 丢弃 pending 唤醒照写；5 人的唤醒在额度未恢复时照样拉起照写；6 `set_profile` 只落库下次生效照写（`worktree` 一条已被控制者裁定 C8 取代：`isolation: bool`，由会话管理器建 worktree，见裁定 20）。每条仍只在一处可改（见上表「改的话只改」）。

## Review Focus

1. 全机同时有大量唤醒（30 个 session 各自被派活、回报、调工具）时，任何 session 都不被熔断；某条边被熔断后，人发的一条 @ 仍立刻送到。（Task 3 / Task 8 `thirty_sessions_working_normally_are_never_tripped`；Task 8 `mutual_mention_loop_is_cut_on_one_edge_and_human_still_gets_through`）
2. 人刚在客户端停掉一个有积压唤醒的 session，它退出后不能被那些积压立刻拉起来。（Task 8 `stop_drops_pending_wakes_and_does_not_relaunch`）
3. worker 忙时被 @，hook 已把那条注入进上下文（游标越过）——idle 边沿、以及 kill -9 重启之后，都不能再投一遍。（Task 8 `already_injected_message_is_settled_not_resent`；Task 9 `boot_requeues_inflight_and_skips_injected_messages`）
4. 目标正卡在对话框（信任框 / 权限菜单）时来了唤醒——绝不能把正文敲进对话框；框答掉之后再投。（Task 8 `pending_decision_holds_delivery_until_cleared`）
5. 目标根本起不来（claude 不在 / codex 会话 / 工作目录没了）——唤醒不能无限重试：3 次后放弃、只告警一次、`resume_failed_at` 只写一次。（Task 8 `launch_failures_back_off_then_drop_with_one_alert`）
6. `isolation: true` 建 worktree 失败（不是仓库、detached HEAD、机组没工作目录）时，不留 session 行、不起进程、也不悄悄退回共享目录（控制者裁定 C8）。（Task 7 `worktree_failure_fails_the_start_with_the_reason_and_creates_nothing`；Task 11 `isolated_start_creates_a_worktree_in_the_crew_repo_and_a_failed_one_leaves_nothing`）

## 文件结构

```
crewd/crates/wake/                       新 crate crewd-wake（零依赖）
  Cargo.toml                                                               (T1)
  src/lib.rs            模块声明（每个任务追加一行）                         (T1–T5)
  src/model.rs          Ms、Priority / WakeKind / WakeState / LatchScope、NewWake、QueuedWake、MessageRef、Latch (T1)
  src/text.rs           Lang、节表 SECTIONS、get / render / fill / placeholders  (T1)
  prompts/orchestra.zh.md, prompts/orchestra.en.md   全部给 agent / 群看的文字 (T1)
  src/plan.rs           唤醒规划（Q2）                                       (T2)
  src/breaker.rs        环路熔断（Q5）                                       (T3)
  src/queue.rs          选批、退避、告警判定                                  (T4)
  src/policy.rs         额度唤醒时刻、重启名单排序、启动闸                      (T4)
  src/notice.rs         待决策通知、nudge 键表；判定源码无字面量的测试          (T5)
crewd/crates/server/
  Cargo.toml            + crewd-wake、crewd-term；dev: tokio test-util          (T6)
  src/lib.rs            只改 serve 启动那一段：ServeOptions / serve_with（T11）。`pub mod orchestra;` 与 `AppState.orchestra` M3a 已建
  src/dispatch.rs       未知方法分支前转给 orchestra::rpc::route（一行）        (T10)
  src/orchestra/mod.rs        替换 M3a 占位：OrchestraConfig、spawn、Handle、actor 循环 (T6, T9)
  src/orchestra/clock.rs      Clock、SystemClock、ManualClock                 (T6)
  src/orchestra/exec.rs       Executor / ExecHandle / Executors / LaunchRequest (T6)
  src/orchestra/exec_claude.rs ClaudeExecutor、ClaudeProgram、comms_for        (T6)
  src/orchestra/hooks.rs      Arbiter（M5b 桩）/ IdleHook / AskAnsweredHook / QuotaSource / ContextExtras (T6)
  src/orchestra/port.rs       Port trait 与它的数据类型                        (T6)
  src/orchestra/worktree.rs   Worktrees trait + GitWorktrees（isolation=true 时建 git worktree，C8） (T6)
  src/orchestra/testkit.rs    #[cfg(test)] MemPort、StandIn、Rig              (T6)
  src/orchestra/core.rs       Core：输入分发、会话生命周期、会话事件            (T7, T8)
  src/orchestra/deliver.rs    唤醒入队、选批投递、回执、退避、idle、问被回答     (T8)
  src/orchestra/boot.rs       启动恢复（Q1 询问）、积压补救、停机、下一拍         (T9)
  src/orchestra/db.rs         DbPort（读写全经 crewd-collab，不写 SQL）、Change → CollabFact 转发 (T10)
  src/orchestra/control.rs    impl SessionControl                             (T10)
  src/orchestra/rpc.rs        M3c 的客户端 RPC                                 (T10)
  tests/orchestra_fake_claude.rs  进程内 serve_with + fake_claude 端到端        (T11)
crewd/crates/proto/src/orchestra.rs   M3c 的方法 / 事件常量与参数结果类型       (T10)
crewd/crates/proto/src/lib.rs         pub mod orchestra + api_schema 登记一行   (T10)
crewd/schema/crewd-api.schema.json    重新生成                                 (T10)
crewd/crates/crewd/src/main.rs        serve --claude / --claude-env            (T11)
crewd/crates/crewd/tests/e2e.rs       起 serve 的测试一律设 CREWD_CLAUDE        (T11)
crewd/crates/crewd/tests/orchestra_restart.rs  kill -9 重启端到端               (T11)
crewd/README.md、spec §22 交付记录                                              (T12)
```

文件归属与先后（与 M3a 计划冻结块末的先后表一致，spec §22.5 已列）：`dispatch.rs` 一行（只有 M3c 改）、`proto/src/lib.rs` 两行、`crates/crewd/src/main.rs` 的 serve 参数、`tests/e2e.rs`、`Cargo.toml` / `Cargo.lock`、契约 json 是与 M3b 共享、各改不相交段的文件——**后合者** rebase 时保留两边、重跑 `cargo add` / 重新生成，不挑边。`AppState` 与 `conn.rs`（含 `wake` 主题）M3c 都不改（M3a 已备好）。`exec_claude.rs::comms_for` 是 M3c 的文件但填 M3b 定的 `CommsSpec`：后合者改这一个函数对上形状（裁定 18）。

## 接口一览（后面的任务只用这些名字）

```rust
// crewd_wake::model
pub struct Ms(pub i64);  pub const SEC: i64; pub const MIN: i64; pub const HOUR: i64;
impl Ms { pub const fn add(self, ms: i64) -> Ms; pub const fn since(self, earlier: Ms) -> i64 }
pub enum Priority { Auto, System, Human }                 // Ord: Auto < System < Human；as_str / parse / ALL
pub enum WakeKind { Message, Timer, AskAnswer, Restart, System, Checkpoint, StopCheck }
pub enum WakeState { Pending, Inflight, Delivered, Dropped }
pub enum LatchScope { Session, Edge }
pub struct NewWake { pub session: String, pub kind: WakeKind, pub source: String, pub priority: Priority, pub not_before: Ms,
                     pub note: Option<String>, pub origin: String }
pub struct QueuedWake { pub id: i64, pub session: String, pub kind: WakeKind, pub source: String, pub priority: Priority,
                        pub not_before: Ms, pub note: Option<String>, pub state: WakeState, pub attempts: u32, pub created_at: Ms,
                        pub origin: String, pub msg: Option<MessageRef> }
pub struct MessageRef { pub channel: String, pub seq: i64 }
pub mod source { message(id) / quota(ms) / restart(tag) / system(tag) / timer(token) }   // 与 crewd_collab::wakes::source 逐字相同
pub struct Latch { pub scope: LatchScope, pub id: String, pub reason: String, pub at: Ms }

// crewd_wake::plan
pub enum Sender { Human, System, Session(String) }        // key(): "human" / "system" / session id
pub enum Mention { Session(String), Broadcast, Human }
pub struct MsgFacts { pub id: String, pub at: MessageRef, pub sender: Sender, pub mentions: Vec<Mention>, pub created_at: Ms }
pub struct Listen { pub session: String, pub until: Ms, pub senders: Option<Vec<String>> }
pub struct ChannelCtx { pub session_members: Vec<String>, pub owner: Option<String>, pub listens: Vec<Listen> }
pub struct WakeTarget { pub session: String, pub priority: Priority }
pub fn needs_arbiter(msg: &MsgFacts, now: Ms) -> bool;
pub fn plan(msg: &MsgFacts, ctx: &ChannelCtx, arbiter: Option<&[String]>, now: Ms) -> Vec<WakeTarget>;
pub fn untargeted_human(ctx: &ChannelCtx) -> Vec<String>;

// crewd_wake::breaker
pub struct LoopLimits { pub window_ms: i64, pub edge_max: usize, pub target_max: usize }
pub const LOOP_LIMITS: LoopLimits;   pub const HUMAN: &str; pub const SYSTEM: &str;
pub fn origin_of(kind: WakeKind, sender: Option<&Sender>, target: &str) -> String;
pub fn edge_id(origin: &str, target: &str) -> String;
pub enum Verdict { Pass, Held(Latch), Tripped(Latch) }
impl LoopBreaker { pub fn new(LoopLimits, Vec<Latch>) -> Self; pub fn held(&self, target: &str, origin: &str) -> Option<&Latch>;
  pub fn admit(&mut self, target: &str, origin: &str, now: Ms) -> Verdict; pub fn progress(&mut self, target: &str);
  pub fn on_post(&mut self, poster: &str, woke: &[String]); pub fn recover(&mut self, scope: LatchScope, id: &str) -> bool;
  pub fn latches(&self) -> Vec<Latch>; pub fn streak_len(&self, target: &str) -> usize }

// crewd_wake::queue
pub struct BatchLimits { pub max_items: usize, pub max_bytes: usize }   // Default 10 / 8192
pub fn is_due(w: &QueuedWake, now: Ms) -> bool;
pub fn select_batch(pending: &[QueuedWake], now: Ms, limits: &BatchLimits) -> Vec<QueuedWake>;
pub const MAX_ATTEMPTS: u32;  pub fn after_failure(attempts: u32, now: Ms) -> Option<Ms>;
pub fn ids(batch: &[QueuedWake]) -> Vec<i64>;  pub fn should_alert(batch: &[QueuedWake]) -> bool;

// crewd_wake::policy
pub struct QuotaPlan { pub fire_at: Ms, pub fallback: bool }  // source() = "quota:<fire_at>"
pub fn quota_plan(reset_at: Option<Ms>, now: Ms) -> QuotaPlan;
pub fn quota_pending(pending: &[QueuedWake]) -> bool;  pub fn quota_due(pending: &[QueuedWake], now: Ms) -> bool;
pub struct RestartRow { pub session: String, pub name: String, pub crew: String, pub captain: bool, pub was_working: bool, pub last_active_at: Option<Ms> }
pub fn restart_order(rows: Vec<RestartRow>) -> Vec<RestartRow>;  pub fn restart_source(boot: &str) -> String;
pub enum Gate { Go, AtCap, WaitUntil(Ms) }
impl LaunchGate { pub fn new(max_live: Option<usize>, stagger_ms: i64) -> Self; pub fn check(&self, live: usize, bypass_cap: bool, now: Ms) -> Gate;
  pub fn launched(&mut self, now: Ms); pub fn set_max_live(&mut self, max: Option<usize>); pub fn max_live(&self) -> Option<usize> }

// crewd_wake::notice
pub struct DecisionFacts { pub prompt: String, pub options: Vec<String>, pub numbered: bool }
pub enum NoticeStage { First, Escalate }   pub const DECISION_ESCALATE_MS: i64;
pub struct Notice { pub text: String, pub to_human: bool }
pub fn decision_notice(lang: Lang, stage: NoticeStage, name: &str, session: &str, is_captain: bool, d: &DecisionFacts, waited_min: i64) -> Notice;
pub fn nudge_bytes(input: &str) -> Vec<u8>;  pub fn nudge_key_names() -> Vec<&'static str>;

// crewd_server::orchestra（Task 6–10）
pub trait Executor { fn launch(&self, LaunchRequest) -> BoxFut<'_, Result<Launched, LaunchError>>; fn locate_transcript(&self, id: &str) -> Option<PathBuf> }
pub trait ExecHandle { send(text, &Context) / accepts_context / raw / stop / clear_quota_health / screen_text / snapshot / status }
pub trait Port { …见 Task 6… }
pub struct Handle; // start / resume / stop / live / nudge / inspect / set_profile / set_captain / wakes / recover /
                   // max_live / set_max_live / note_progress / collab / shutdown
```

---
### Task 1: crate `crewd-wake` 骨架 + 数据形状 + 文字表

**Files:**
- Create: `crewd/crates/wake/Cargo.toml`、`crewd/crates/wake/src/lib.rs`、`crewd/crates/wake/src/model.rs`、`crewd/crates/wake/src/text.rs`、`crewd/crates/wake/prompts/orchestra.zh.md`、`crewd/crates/wake/prompts/orchestra.en.md`
- Modify: `crewd/Cargo.lock`

**Interfaces:**
- Consumes: 无（只用 std）。
- Produces: `model::*`（见「接口一览」，含 `model::source::{message, quota, restart, system, timer}`）；`text::{Lang, SECTIONS, get, render, fill, placeholders}`。

- [ ] **Step 1: 建 crate**

```bash
mkdir crates/wake && mkdir crates/wake/src && mkdir crates/wake/prompts
```

`crewd/crates/wake/Cargo.toml`：

```toml
[package]
name = "crewd-wake"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
```

`crewd/crates/wake/src/lib.rs`：

```rust
//! crewd 的唤醒判定（spec §22.4、§22.7）：谁醒、能不能醒、什么时候醒、醒来看到的附言。
//! sans-IO：不起进程、不碰 tokio / 文件 / 数据库、不读时钟——时间由调用方传入（同 crewd-work）。
pub mod model;
pub mod text;

pub use model::*;
```

- [ ] **Step 2: 写失败的测试（model）**

`crewd/crates/wake/src/model.rs` 先只放测试模块（实现下一步补在它上面）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_round_trip_their_table_words() {
        for p in Priority::ALL {
            assert_eq!(Priority::parse(p.as_str()), Some(*p));
        }
        for k in WakeKind::ALL {
            assert_eq!(WakeKind::parse(k.as_str()), Some(*k));
        }
        for s in WakeState::ALL {
            assert_eq!(WakeState::parse(s.as_str()), Some(*s));
        }
        for s in LatchScope::ALL {
            assert_eq!(LatchScope::parse(s.as_str()), Some(*s));
        }
        assert_eq!(WakeKind::AskAnswer.as_str(), "ask_answer");
        assert_eq!(WakeKind::StopCheck.as_str(), "stop_check");
        assert_eq!(Priority::parse("urgent"), None);
    }

    #[test]
    fn priority_orders_auto_below_system_below_human() {
        assert!(Priority::Auto < Priority::System);
        assert!(Priority::System < Priority::Human);
    }

    #[test]
    fn source_formats_match_the_m3a_contract() {
        assert_eq!(source::message("m-1"), "msg:m-1");
        assert_eq!(source::quota(5), "quota:5");
        assert_eq!(source::restart("7"), "restart:7");
        assert_eq!(source::system("captain:c1:9"), "system:captain:c1:9");
        assert_eq!(source::timer("t"), "timer:t");
    }

    #[test]
    fn ms_arithmetic_saturates() {
        assert_eq!(Ms(i64::MAX).add(1), Ms(i64::MAX));
        assert_eq!(Ms(10).since(Ms(4)), 6);
        assert_eq!(Ms(i64::MIN).since(Ms(1)), i64::MIN);
    }
}
```

Run: `cargo test -p crewd-wake`
Expected: 编译失败（`Priority` 等未定义）。

- [ ] **Step 3: 实现 model**

在 `model.rs` 测试模块上方写：

```rust
//! 唤醒的数据形状：时间、优先级、种类、状态、队列一行、熔断记录。全是纯值。
//! 词表与 schema v3 的 `wakes` / `wake_latches` 表一一对应（spec §22.2；`edge` 见 §22.7 Q5）。
use std::fmt;

/// Unix 毫秒。时间一律由调用方给（同 crewd-work 的 `Ts`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ms(pub i64);

pub const SEC: i64 = 1_000;
pub const MIN: i64 = 60 * SEC;
pub const HOUR: i64 = 60 * MIN;

impl Ms {
    pub const fn add(self, ms: i64) -> Ms {
        Ms(self.0.saturating_add(ms))
    }

    /// `self - earlier`（毫秒，饱和）。
    pub const fn since(self, earlier: Ms) -> i64 {
        self.0.saturating_sub(earlier.0)
    }
}

/// 带表内词的枚举：`as_str` / `parse` / `ALL`，词就是库里存的值。
macro_rules! str_enum {
    ($(#[$m:meta])* $name:ident { $($(#[$vm:meta])* $v:ident = $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name { $($(#[$vm])* $v),+ }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$v),+];

            pub const fn as_str(self) -> &'static str {
                match self { $($name::$v => $s),+ }
            }

            pub fn parse(s: &str) -> Option<Self> {
                match s { $($s => Some($name::$v),)+ _ => None }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

str_enum!(
    /// 唤醒优先级。声明顺序即 `Ord`：Auto < System < Human（合并投递只取最高一档）。
    Priority { Auto = "auto", System = "system", Human = "human" }
);

str_enum!(
    /// `checkpoint` / `stop_check` 是给 M4b 留的，M3c 里没有写者。
    WakeKind {
        Message = "message",
        Timer = "timer",
        AskAnswer = "ask_answer",
        Restart = "restart",
        System = "system",
        Checkpoint = "checkpoint",
        StopCheck = "stop_check",
    }
);

str_enum!(WakeState {
    Pending = "pending",
    Inflight = "inflight",
    Delivered = "delivered",
    Dropped = "dropped",
});

str_enum!(
    /// 环路熔断的两级（§22.7 Q5）：`Session` = 目标的非 session 来源全部停，`Edge` = 「来源 > 目标」这一条停。
    LatchScope { Session = "session", Edge = "edge" }
);

/// 入队请求（= `wakes` 表一行的可写部分）。`UNIQUE(session, kind, source)` 去重。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewWake {
    pub session: String,
    pub kind: WakeKind,
    /// 来源身份（不存渲染好的正文，出队时现渲染：`CrewDeferredWakeQueue.swift:13`）。
    pub source: String,
    pub priority: Priority,
    pub not_before: Ms,
    /// 非消息类唤醒的附言（已渲染好的文字，来自 `text`）。
    pub note: Option<String>,
    /// 是谁引起的（`breaker::origin_of` 的键：`human` / `system` / session id；环路熔断用，§22.7 Q5）。
    pub origin: String,
}

/// 队列里的一行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedWake {
    pub id: i64,
    pub session: String,
    pub kind: WakeKind,
    pub source: String,
    pub priority: Priority,
    pub not_before: Ms,
    pub note: Option<String>,
    pub state: WakeState,
    pub attempts: u32,
    pub created_at: Ms,
    pub origin: String,
    /// 消息类唤醒指向的那条消息（Port 从 `msg:<id>` 解析出来；消息已删 = None）。
    pub msg: Option<MessageRef>,
}

/// 一条消息在群里的位置（游标比的是它）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MessageRef {
    pub channel: String,
    pub seq: i64,
}

/// `wakes.source` 的拼法。**与 `crewd_collab::wakes::source` 逐字相同**（M3a 定死的格式；
/// `render::build_batch` 靠它找消息 / 问）。`orchestra/db.rs` 有一条测试逐个比对两边。
pub mod source {
    pub fn message(message_id: &str) -> String {
        format!("msg:{message_id}")
    }
    pub fn quota(at_ms: i64) -> String {
        format!("quota:{at_ms}")
    }
    pub fn restart(tag: &str) -> String {
        format!("restart:{tag}")
    }
    pub fn system(tag: &str) -> String {
        format!("system:{tag}")
    }
    pub fn timer(token: &str) -> String {
        format!("timer:{token}")
    }
}

/// 一条熔断（`wake_latches` 一行）。只有人能从客户端解（`admission.recover`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Latch {
    pub scope: LatchScope,
    pub id: String,
    /// 诊断短码（如 `no-progress:9/10min`）；给人看的话由 IO 层用 `text` 渲染，不存在这里。
    pub reason: String,
    pub at: Ms,
}
```

Run: `cargo test -p crewd-wake`
Expected: PASS（4 passed）。

- [ ] **Step 4: 写失败的测试（text）**

`crewd/crates/wake/src/text.rs` 先只放测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn names(lang: Lang) -> BTreeSet<&'static str> {
        sections(lang).keys().copied().collect()
    }

    #[test]
    fn both_files_have_exactly_the_declared_sections() {
        let declared: BTreeSet<&str> = SECTIONS.iter().map(|(k, _)| *k).collect();
        assert_eq!(names(Lang::Zh), declared, "zh");
        assert_eq!(names(Lang::En), declared, "en");
    }

    #[test]
    fn every_section_uses_exactly_its_declared_placeholders() {
        for lang in [Lang::Zh, Lang::En] {
            for (key, vars) in SECTIONS {
                let used: BTreeSet<String> = placeholders(get(lang, key)).into_iter().collect();
                let want: BTreeSet<String> = vars.iter().map(|v| v.to_string()).collect();
                assert_eq!(used, want, "{lang:?} ## {key}");
            }
        }
    }

    #[test]
    fn render_fills_every_placeholder() {
        for lang in [Lang::Zh, Lang::En] {
            for (key, vars) in SECTIONS {
                let values: Vec<(&str, &str)> = vars.iter().map(|v| (*v, "X")).collect();
                let out = render(lang, key, &values);
                assert!(placeholders(&out).is_empty(), "{lang:?} ## {key}: {out}");
            }
        }
    }

    #[test]
    fn fill_is_single_pass_and_leaves_unknown_placeholders() {
        // 替换进来的值里的 {…} 不会再被展开（显示名 / claude 原话是外来文本）。
        assert_eq!(fill("{a}|{b}", &[("a", "{b}"), ("b", "x")]), "{b}|x");
        assert_eq!(fill("{zz} {a}", &[("a", "1")]), "{zz} 1");
        assert_eq!(fill("{ 不是 } {}", &[]), "{ 不是 } {}");
    }

    #[test]
    #[should_panic(expected = "缺少一节")]
    fn missing_section_is_a_loud_programmer_error() {
        get(Lang::Zh, "no.such.section");
    }

    #[test]
    fn parse_rejects_empty_and_duplicate_sections() {
        assert!(parse("t", "## a\n\n## b\nx\n").unwrap_err().contains("## a 是空的"));
        assert!(parse("t", "## a\nx\n## a\ny\n").unwrap_err().contains("## a 重复"));
        assert_eq!(parse("t", "注释\n## a\n x \n").unwrap()["a"], "x");
    }
}
```

Run: `cargo test -p crewd-wake text`
Expected: 编译失败（`sections` / `get` 等未定义）。

- [ ] **Step 5: 实现 text**

在 `text.rs` 测试模块上方写：

```rust
//! 给 agent 与群看的文字全在 `prompts/orchestra.{zh,en}.md`（编译期 include_str!），代码里不写字面量。
//! 格式同 `crates/jev/src/prompts.rs`：第一个 `## 名字` 之前是给人看的注释；每节到下一个 `## ` 为止、
//! 去首尾空白；节名重复、节为空都是错误。占位符 `{名字}` 由 [`render`] 单遍替换。
use std::collections::BTreeMap;
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lang {
    #[default]
    Zh,
    En,
}

/// 每一节与它用到的占位符。加一节 = 两份 md 各加一节 + 这里加一行（测试钉住三处一致）。
pub const SECTIONS: &[(&str, &[&str])] = &[
    ("restart.interrupted", &[]),
    ("resume.refused.group", &["name", "said"]),
    ("resume.refused.diag.found", &["path"]),
    ("resume.refused.diag.missing", &[]),
    ("resume.refused.agent", &["id", "said"]),
    ("resume.failed.group", &["name", "detail"]),
    ("quota.scheduled", &["name", "minutes"]),
    ("quota.fallback", &["name", "minutes"]),
    ("quota.wake", &[]),
    ("delivery.failed.group", &["name", "attempts", "error"]),
    ("loop.tripped.human", &["target", "origin", "count", "minutes"]),
    ("loop.tripped.owner", &["target", "origin"]),
    ("origin.system", &[]),
    ("captain.changed.new", &[]),
    ("captain.changed.old", &["name"]),
    ("decision.first.worker", &["name"]),
    ("decision.first.captain", &["name"]),
    ("decision.escalate", &["name", "minutes"]),
    ("decision.options.numbered", &[]),
    ("decision.options.unnumbered", &["keys", "move"]),
    ("decision.move.none", &[]),
    ("decision.move.keys", &["keys"]),
    ("decision.tail.worker", &[]),
    ("decision.tail.captain", &[]),
    ("decision.tail.escalate", &[]),
    ("decision.session", &["session"]),
];

type Sections = BTreeMap<&'static str, &'static str>;

fn parse(name: &'static str, text: &'static str) -> Result<Sections, String> {
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
    Ok(sections)
}

static ZH: LazyLock<Sections> = LazyLock::new(|| {
    parse("orchestra.zh", include_str!("../prompts/orchestra.zh.md")).unwrap_or_else(|e| panic!("{e}"))
});
static EN: LazyLock<Sections> = LazyLock::new(|| {
    parse("orchestra.en", include_str!("../prompts/orchestra.en.md")).unwrap_or_else(|e| panic!("{e}"))
});

fn sections(lang: Lang) -> &'static Sections {
    match lang {
        Lang::Zh => &ZH,
        Lang::En => &EN,
    }
}

/// 缺节 = 程序员错误（`both_files_have_exactly_the_declared_sections` 钉住），直接 panic 并说清是哪一节。
pub fn get(lang: Lang, key: &str) -> &'static str {
    match sections(lang).get(key) {
        Some(text) => text,
        None => panic!("prompts/orchestra.{lang:?}.md 缺少一节：## {key}"),
    }
}

pub fn render(lang: Lang, key: &str, vars: &[(&str, &str)]) -> String {
    fill(get(lang, key), vars)
}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c == '_')
}

/// 单遍替换 `{名字}`（名字只含小写字母与下划线）；`vars` 里没有的占位符原样留下（测试抓得到）。
pub fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if is_ident(&after[..close]) => {
                let key = &after[..close];
                match vars.iter().find(|(k, _)| *k == key) {
                    Some((_, v)) => out.push_str(v),
                    None => {
                        out.push('{');
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 模板里出现的占位符名（去重、按出现顺序）。
pub fn placeholders(template: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if is_ident(&after[..close]) => {
                let key = after[..close].to_string();
                if !found.contains(&key) {
                    found.push(key);
                }
                rest = &after[close + 1..];
            }
            _ => rest = after,
        }
    }
    found
}
```

`crewd/crates/wake/prompts/orchestra.zh.md`：

````markdown
<!--
crewd-wake 的群通知与唤醒附言（spec §22.4、§22.7）。每节一段；占位符见 src/text.rs 的 SECTIONS。
这里的话是写给 agent 或人看的，不写进代码。
-->

## restart.interrupted
crewd 重启打断了你上一轮的工作。先核对现状（git status、你在做的那件事做到哪了、群里有没有新消息），再决定怎么继续；别照着记忆里的半截步骤直接往下做。

## resume.refused.group
「{name}」的原对话接不回来了——claude 拒了这个会话号，原话：`{said}`。这一轮是新开的：群里的上下文它还在，但终端里聊过的细节要重讲。

## resume.refused.diag.found
顺带：这个会话号的日志在盘上是有的（{path}），所以不是「日志没了」，得另查（claude 版本、文件权限、文件本身坏了）。

## resume.refused.diag.missing
顺带：这个会话号的日志在 claude 的 projects 目录下一个都找不到。

## resume.refused.agent
【注意】原会话（{id}）没能续上——claude 自己的原话是「{said}」，所以这是新开的一轮。群里的消息会照常注入，别假装记得终端里聊过的细节。

## resume.failed.group
「{name}」起不来了：{detail}。它名下的事需要有人接手，或者请人看一下。

## quota.scheduled
「{name}」撞到额度上限，已挂额度重置唤醒（约 {minutes} 分钟后）。

## quota.fallback
「{name}」撞到额度上限，拿不到可信的重置时刻，已按退避挂唤醒（约 {minutes} 分钟后）。

## quota.wake
额度应该已经重置。你上次撞到额度上限被迫停下——醒来第一件事是核现状：get_quota 看额度、git log 看 main 到哪了、看自己的分支还在不在。积压里 @ 你的消息可能在你停下时已经被处理掉了，先核实再决定哪几条还要回。若仍受限，用 schedule_wakeup 自己顺延。

## delivery.failed.group
给「{name}」的唤醒连续 {attempts} 次没送进去（最后一次：{error}），已放弃。相关消息仍在群里，它下次醒来会看到；急的话请人看一下这个 session。

## loop.tripped.human
疑似唤醒环路：「{origin}」在 {minutes} 分钟内叫醒「{target}」{count} 次，期间「{target}」没有新产出（没调工具、没动工作项、回话只是回给叫它的人）。已只停这一条：之后「{origin}」不会再自动叫醒「{target}」，人的消息照常送达。确认没问题的话，在客户端对这条熔断点「恢复」。

## loop.tripped.owner
「{origin}」→「{target}」的自动唤醒疑似打转，已被熔断（只停这一条，详情在人类群）。需要它们继续协作的话，请人在客户端恢复。

## origin.system
系统

## captain.changed.new
你现在是这个机组的机长。机长专用的工具已对你开放；先看一眼大厅和人类群的未读，再决定下一步。

## captain.changed.old
你不再是这个机组的机长（新机长：{name}）。手上的事继续做完，派活与对人的汇报交给新机长。

## decision.first.worker
{name} 卡住了，在等人拍板：

## decision.first.captain
{name}（机长）卡住了，在等一个回复：

## decision.escalate
{name} 仍卡着，已等 {minutes} 分钟没人处理：

## decision.options.numbered
怎么答：这一屏的选项带编号，nudge_session 发对应数字即可。

## decision.options.unnumbered
怎么答：这一屏的选项没有编号。发数字无效，而 nudge_session 只认这几个裸按键：{keys}，其余一律当文本发出去并自动补一个回车——那个回车落在当前高亮的那一项上，未必是你想选的（esc 也不是「取消这个框」，它会让 claude 直接退出）。所以别发数字。高亮当下就停在你要的那一项上 → 发 `enter`；{move}

## decision.move.none
不在那一项上 → 交给人。

## decision.move.keys
不在 → 先发 {keys} 把它移到位，再发 `enter`。

## decision.tail.worker
机长可 inspect_session 看现场；按法照上面那一行（它是按当时那一屏算出来的）。拍不了板就 @人。

## decision.tail.captain
需要人打开这个 session 的终端选一下。

## decision.tail.escalate
需要人来定：打开这个 session 的终端直接选，或让机长 nudge_session 代按。

## decision.session
（session: {session}）
````

`crewd/crates/wake/prompts/orchestra.en.md`：

````markdown
<!--
Group notices and wake notes of crewd-wake (spec §22.4, §22.7). One paragraph per section;
placeholders are declared in SECTIONS in src/text.rs. Written for agents and humans; never in code.
-->

## restart.interrupted
A crewd restart interrupted your previous turn. Check the current state first (git status, how far the thing you were doing got, new messages in your channels), then decide how to continue; do not resume half-remembered steps blindly.

## resume.refused.group
"{name}" could not get its previous conversation back — claude refused the session id and said: `{said}`. This is a fresh conversation: it still has the channel context, but details discussed in the terminal must be repeated.

## resume.refused.diag.found
Note: the log for this session id does exist on disk ({path}), so the log is not missing; look elsewhere (claude version, file permissions, a corrupted file).

## resume.refused.diag.missing
Note: no log for this session id exists anywhere under claude's projects directory.

## resume.refused.agent
[Note] Your previous session ({id}) could not be resumed — claude itself said "{said}" — so this is a fresh conversation. Channel messages are injected as usual; do not pretend to remember details from the terminal.

## resume.failed.group
"{name}" cannot be started: {detail}. Someone needs to take over its work, or a human should look at it.

## quota.scheduled
"{name}" hit its usage limit; a wake is scheduled for the quota reset (in about {minutes} minutes).

## quota.fallback
"{name}" hit its usage limit and no reliable reset time is known; a back-off wake is scheduled (in about {minutes} minutes).

## quota.wake
Your quota should have reset. You stopped last time because you hit the usage limit — first check the current state: get_quota for the quota, git log for where main is, whether your branch still exists. Messages that @-ed you meanwhile may already have been handled; verify before deciding which still need an answer. If you are still limited, push this back yourself with schedule_wakeup.

## delivery.failed.group
A wake for "{name}" failed to arrive {attempts} times in a row (last error: {error}) and was given up. The related messages are still in the channel and it will see them when it next wakes; if this is urgent, ask a human to look at the session.

## loop.tripped.human
Possible wake loop: "{origin}" woke "{target}" {count} times within {minutes} minutes and "{target}" produced nothing new meanwhile (no tool call, no work-item change, replies only went back to whoever woke it). Only this one link is stopped: "{origin}" will no longer wake "{target}" automatically; human messages still arrive. If this is fine, press "Recover" on this breaker in the client.

## loop.tripped.owner
Automatic wakes "{origin}" → "{target}" looked like a loop and were stopped (only this link; details in the human channel). If they need to keep working together, ask a human to recover it in the client.

## origin.system
system

## captain.changed.new
You are now the captain of this crew. Captain-only tools are available to you; read the unread messages in the lobby and the human channel before deciding what to do next.

## captain.changed.old
You are no longer the captain of this crew (new captain: {name}). Finish what you are doing; assigning work and reporting to the human now belong to the new captain.

## decision.first.worker
{name} is stuck waiting for a decision:

## decision.first.captain
{name} (captain) is stuck waiting for an answer:

## decision.escalate
{name} is still stuck; nobody has handled it for {minutes} minutes:

## decision.options.numbered
How to answer: the options on this screen are numbered; send the matching number with nudge_session.

## decision.options.unnumbered
How to answer: the options on this screen are not numbered. Numbers do nothing, and nudge_session only knows these bare keys: {keys}; anything else is sent as text followed by an automatic Enter — and that Enter confirms whichever option is highlighted right now, which may not be the one you want (esc does not cancel the dialog either: it makes claude exit). So do not send numbers. If the highlight is already on the option you want → send `enter`; {move}

## decision.move.none
otherwise → hand it to a human.

## decision.move.keys
otherwise → send {keys} to move it there first, then `enter`.

## decision.tail.worker
The captain can look with inspect_session; follow the line above (it was computed from the screen at that moment). If you cannot decide, @ the human.

## decision.tail.captain
A human needs to open this session's terminal and choose.

## decision.tail.escalate
A human needs to decide: open this session's terminal and choose, or have the captain press it with nudge_session.

## decision.session
(session: {session})
````

Run: `cargo test -p crewd-wake`
Expected: PASS（10 passed）。

- [ ] **Step 6: 门禁与提交**

Run: `./scripts/check.sh`
Expected: 全绿。

```bash
git add crates/wake/Cargo.toml crates/wake/src/lib.rs crates/wake/src/model.rs crates/wake/src/text.rs crates/wake/prompts/orchestra.zh.md crates/wake/prompts/orchestra.en.md Cargo.lock
git diff --cached --stat
git commit -m "crewd-wake: crate skeleton, wake data shapes, orchestra text sections

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: 唤醒规划（§22.7 Q2）

移植 `Sources/Mac/LocalRunner/CrewLocalMentionWakeLogic.swift:58-103`，改成：只在群成员里选（§17）；人发、一个 @ 都没有 → 先用 Jev（`arbiter` 参数，候选 = 群成员），没有 / 低置信 → 群主（Q2）；收听者与上面取并集；同一条消息一个 session 只出现一次；自己不叫自己；超过 6h 的不叫（`:53`）；broadcast / @人类 不叫任何 session（`:74`）。

**Files:**
- Create: `crewd/crates/wake/src/plan.rs`
- Modify: `crewd/crates/wake/src/lib.rs`（加 `pub mod plan;`）

**Interfaces:**
- Consumes: Task 1 `Ms`、`HOUR`、`MessageRef`、`Priority`。
- Produces: `plan::{Sender, Mention, MsgFacts, Listen, ChannelCtx, WakeTarget, MAX_WAKE_AGE_MS, priority_of, is_stale, needs_arbiter, plan, untargeted_human}`。

- [ ] **Step 1: 写失败的测试**

`crewd/crates/wake/src/plan.rs` 先放测试（`lib.rs` 加 `pub mod plan;`）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MIN, MessageRef};

    const NOW: Ms = Ms(10 * HOUR);

    fn msg(sender: Sender, mentions: Vec<Mention>) -> MsgFacts {
        MsgFacts {
            id: "m-7".into(),
            at: MessageRef { channel: "c1/lobby".into(), seq: 7 },
            sender,
            mentions,
            created_at: NOW,
        }
    }

    fn ctx(members: &[&str], owner: Option<&str>) -> ChannelCtx {
        ChannelCtx {
            session_members: members.iter().map(|s| s.to_string()).collect(),
            owner: owner.map(str::to_string),
            listens: vec![],
        }
    }

    fn who(targets: &[WakeTarget]) -> Vec<&str> {
        targets.iter().map(|t| t.session.as_str()).collect()
    }

    fn s(id: &str) -> Sender {
        Sender::Session(id.into())
    }

    fn at(id: &str) -> Mention {
        Mention::Session(id.into())
    }

    #[test]
    fn mention_wakes_the_member_with_the_senders_priority() {
        let c = ctx(&["cap", "w1", "w2"], Some("cap"));
        let t = plan(&msg(s("cap"), vec![at("w1")]), &c, None, NOW);
        assert_eq!(t, vec![WakeTarget { session: "w1".into(), priority: Priority::Auto }]);
        let t = plan(&msg(Sender::Human, vec![at("w2")]), &c, None, NOW);
        assert_eq!(t[0].priority, Priority::Human);
        let t = plan(&msg(Sender::System, vec![at("w2")]), &c, None, NOW);
        assert_eq!(t[0].priority, Priority::System);
    }

    #[test]
    fn never_wakes_the_sender_or_a_non_member() {
        let c = ctx(&["cap", "w1"], Some("cap"));
        assert!(plan(&msg(s("w1"), vec![at("w1")]), &c, None, NOW).is_empty());
        assert!(plan(&msg(s("cap"), vec![at("outsider")]), &c, None, NOW).is_empty());
    }

    #[test]
    fn broadcast_and_human_mentions_wake_nobody_even_from_a_human() {
        let c = ctx(&["cap", "w1"], Some("cap"));
        assert!(plan(&msg(s("w1"), vec![Mention::Broadcast]), &c, None, NOW).is_empty());
        assert!(plan(&msg(s("w1"), vec![Mention::Human]), &c, None, NOW).is_empty());
        // 人显式写了 broadcast：不是「没 @ 任何人」，不交群主。
        assert!(plan(&msg(Sender::Human, vec![Mention::Broadcast]), &c, None, NOW).is_empty());
    }

    #[test]
    fn untargeted_human_goes_to_jev_then_to_the_owner() {
        let c = ctx(&["cap", "w1", "w2"], Some("cap"));
        let m = msg(Sender::Human, vec![]);
        assert_eq!(who(&plan(&m, &c, None, NOW)), ["cap"], "Jev 未接入 → 群主");
        let jev = ["w2".to_string()];
        assert_eq!(who(&plan(&m, &c, Some(&jev), NOW)), ["w2"]);
        let outsider = ["zz".to_string()];
        assert_eq!(who(&plan(&m, &c, Some(&outsider), NOW)), ["cap"], "Jev 选了非成员 → 当没答");
        assert_eq!(who(&plan(&m, &c, Some(&[]), NOW)), ["cap"], "空集 = 低置信 → 群主");
    }

    #[test]
    fn owner_that_is_not_a_session_member_wakes_nobody() {
        // 群主是人 / 已离开：没有 session 可叫，只剩收听者。
        let m = msg(Sender::Human, vec![]);
        assert!(plan(&m, &ctx(&["w1", "w2"], Some("gone")), None, NOW).is_empty());
        assert!(plan(&m, &ctx(&["w1", "w2"], None), None, NOW).is_empty());
    }

    #[test]
    fn session_messages_without_mentions_wake_only_listeners() {
        let mut c = ctx(&["cap", "w1", "w2"], Some("cap"));
        assert!(plan(&msg(s("w1"), vec![]), &c, None, NOW).is_empty());
        c.listens.push(Listen { session: "w2".into(), until: NOW.add(MIN), senders: None });
        assert_eq!(who(&plan(&msg(s("w1"), vec![]), &c, None, NOW)), ["w2"]);
    }

    #[test]
    fn listeners_are_unioned_once_and_respect_window_and_sender_filter() {
        let mut c = ctx(&["cap", "w1", "w2", "w3"], Some("cap"));
        c.listens = vec![
            Listen { session: "w1".into(), until: NOW.add(MIN), senders: None },
            Listen { session: "w2".into(), until: NOW, senders: None },
            Listen { session: "w3".into(), until: NOW.add(MIN), senders: Some(vec!["human".into()]) },
        ];
        // w1 既被 @ 又在收听：只出现一次；w2 窗口已过；w3 只收听人。
        assert_eq!(who(&plan(&msg(s("cap"), vec![at("w1")]), &c, None, NOW)), ["w1"]);
        assert_eq!(who(&plan(&msg(Sender::Human, vec![at("w1")]), &c, None, NOW)), ["w1", "w3"]);
        // 人不 @：群主 + 收听者并集。
        assert_eq!(who(&plan(&msg(Sender::Human, vec![]), &c, None, NOW)), ["cap", "w1", "w3"]);
    }

    #[test]
    fn listener_that_is_the_sender_or_not_a_member_is_skipped() {
        let mut c = ctx(&["cap", "w1"], Some("cap"));
        c.listens = vec![
            Listen { session: "w1".into(), until: NOW.add(MIN), senders: None },
            Listen { session: "ghost".into(), until: NOW.add(MIN), senders: None },
        ];
        assert!(plan(&msg(s("w1"), vec![]), &c, None, NOW).is_empty());
    }

    #[test]
    fn messages_older_than_six_hours_wake_nobody() {
        let c = ctx(&["cap", "w1"], Some("cap"));
        let mut m = msg(s("cap"), vec![at("w1")]);
        m.created_at = NOW.add(-MAX_WAKE_AGE_MS - 1);
        assert!(plan(&m, &c, None, NOW).is_empty());
        m.created_at = NOW.add(-MAX_WAKE_AGE_MS);
        assert_eq!(plan(&m, &c, None, NOW).len(), 1, "正好 6h 还叫");
    }

    #[test]
    fn arbiter_is_asked_only_for_fresh_untargeted_human_messages() {
        assert!(needs_arbiter(&msg(Sender::Human, vec![]), NOW));
        assert!(!needs_arbiter(&msg(Sender::Human, vec![at("w1")]), NOW));
        assert!(!needs_arbiter(&msg(Sender::Human, vec![Mention::Broadcast]), NOW));
        assert!(!needs_arbiter(&msg(s("w1"), vec![]), NOW));
        assert!(!needs_arbiter(&msg(Sender::System, vec![]), NOW));
        let mut old = msg(Sender::Human, vec![]);
        old.created_at = NOW.add(-MAX_WAKE_AGE_MS - 1);
        assert!(!needs_arbiter(&old, NOW));
    }
}
```

Run: `cargo test -p crewd-wake plan`
Expected: 编译失败。

- [ ] **Step 2: 实现**

在 `plan.rs` 测试模块上方写：

```rust
//! 唤醒规划（spec §22.4「唤醒规划」+ §22.7 Q2）：一条新消息 → 该叫醒谁、什么优先级。纯函数。
//! 移植 `Sources/Mac/LocalRunner/CrewLocalMentionWakeLogic.swift:58-103`：
//! - 只在群成员里选（§17）；自己不叫自己；超过 6h 的不叫（`:53`）；broadcast / @人类 不叫任何 session（`:74`）。
//! - 人发、一个 @ 都没有：先用调度层（Jev，候选 = 群成员）的答案；没有 / 低置信 → 群主（Q2；Swift 是「默认 @机长」，
//!   大厅与人类群的群主就是机长，语义不变）。
//! - 收听者（`listens`，窗口内、发送者匹配）与上面取并集；同一条消息一个 session 只出现一次。
use crate::model::{HOUR, MessageRef, Ms, Priority};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sender {
    Human,
    System,
    Session(String),
}

impl Sender {
    /// 收听过滤与环路熔断用的来源键：`human` / `system` / session id。
    pub fn key(&self) -> &str {
        match self {
            Self::Human => "human",
            Self::System => "system",
            Self::Session(id) => id,
        }
    }
}

/// 发送时已解析好的 @（`captain` 在发送时已换成具体 session id，§22.2）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mention {
    Session(String),
    Broadcast,
    Human,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MsgFacts {
    /// 消息 id（唤醒的 source 用它，`source::message`）。
    pub id: String,
    pub at: MessageRef,
    pub sender: Sender,
    /// 空 = 一个 @ 都没写。
    pub mentions: Vec<Mention>,
    pub created_at: Ms,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listen {
    pub session: String,
    /// 收听到这一刻为止（不含）。
    pub until: Ms,
    /// `None` = 谁发的都算；否则元素是 [`Sender::key`]。
    pub senders: Option<Vec<String>>,
}

/// 消息所在群此刻的情况（由 IO 层按 M3a 的群规则算好：大厅 = 本机组全部 session，人类群 = 根机组机长 + 显式成员）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelCtx {
    pub session_members: Vec<String>,
    /// 群主（§22.7 Q2；大厅 / 人类群 = 机组机长；人建的群可能不是 session）。
    pub owner: Option<String>,
    pub listens: Vec<Listen>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WakeTarget {
    pub session: String,
    pub priority: Priority,
}

pub const MAX_WAKE_AGE_MS: i64 = 6 * HOUR;

pub fn priority_of(sender: &Sender) -> Priority {
    match sender {
        Sender::Human => Priority::Human,
        Sender::System => Priority::System,
        Sender::Session(_) => Priority::Auto,
    }
}

pub fn is_stale(msg: &MsgFacts, now: Ms) -> bool {
    now.since(msg.created_at) > MAX_WAKE_AGE_MS
}

/// 要不要先问调度层（Arbiter）：人发的、一个 @ 都没有、不陈旧。
pub fn needs_arbiter(msg: &MsgFacts, now: Ms) -> bool {
    msg.sender == Sender::Human && msg.mentions.is_empty() && !is_stale(msg, now)
}

/// 人发、没有 @、调度层没给可用答案时叫谁（§22.7 Q2：群主）。改兜底规则只改这里。
pub fn untargeted_human(ctx: &ChannelCtx) -> Vec<String> {
    ctx.owner
        .iter()
        .filter(|o| ctx.session_members.contains(o))
        .cloned()
        .collect()
}

/// `arbiter`：调度层给的叫醒集合（`None` = 没问 / 没答 / 失败；空集 = 低置信）。
pub fn plan(msg: &MsgFacts, ctx: &ChannelCtx, arbiter: Option<&[String]>, now: Ms) -> Vec<WakeTarget> {
    if is_stale(msg, now) {
        return vec![];
    }
    let is_member = |s: &str| ctx.session_members.iter().any(|m| m == s);
    let sender_session = match &msg.sender {
        Sender::Session(id) => Some(id.as_str()),
        _ => None,
    };
    let mut out: Vec<String> = Vec::new();
    let push = |s: &str, out: &mut Vec<String>| {
        if Some(s) != sender_session && is_member(s) && !out.iter().any(|o| o == s) {
            out.push(s.to_string());
        }
    };
    for m in &msg.mentions {
        if let Mention::Session(id) = m {
            push(id, &mut out);
        }
    }
    if msg.sender == Sender::Human && msg.mentions.is_empty() {
        let chosen: Vec<String> = arbiter
            .map(|a| a.iter().filter(|s| is_member(s)).cloned().collect())
            .unwrap_or_default();
        let chosen = if chosen.is_empty() { untargeted_human(ctx) } else { chosen };
        for s in &chosen {
            push(s, &mut out);
        }
    }
    for l in &ctx.listens {
        let sender_ok = l
            .senders
            .as_ref()
            .is_none_or(|ss| ss.iter().any(|s| s == msg.sender.key()));
        if l.until > now && sender_ok {
            push(&l.session, &mut out);
        }
    }
    let priority = priority_of(&msg.sender);
    out.into_iter().map(|session| WakeTarget { session, priority }).collect()
}
```

Run: `cargo test -p crewd-wake plan`
Expected: PASS（10 passed）。

- [ ] **Step 3: 变异自证（提交前做，改完用编辑器改回，不用 git checkout）**

把 `untargeted_human` 的 `.filter(|o| ctx.session_members.contains(o))` 删掉 → `owner_that_is_not_a_session_member_wakes_nobody` 必须红；把 `push` 里的 `!out.iter().any(|o| o == s)` 删掉 → `listeners_are_unioned_once_and_respect_window_and_sender_filter` 必须红。都红过再改回，`cargo test -p crewd-wake plan` 回到全绿。

- [ ] **Step 4: 门禁与提交**

Run: `./scripts/check.sh`
Expected: 全绿。

```bash
git add crates/wake/src/plan.rs crates/wake/src/lib.rs
git diff --cached --stat
git commit -m "crewd-wake: wake planning — members only, Jev then channel owner, listeners unioned once

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: 环路熔断（§22.7 Q5）

取代 `Sources/Stores/AutomaticWakeAdmission.swift`（裁定 2）。没有任何全机 / 机组 / 单 session 的总量窗口；只截「同一来源 → 同一目标、10 分钟内超过 8 次、目标期间没有新产出」的边，以及「系统 / 自己定时这类非 session 来源对同一目标合计超过 8 次」的目标。人发起的唤醒永不计数、永不被挡（修 `AutomaticWakeAdmission.swift:300`：Swift 的全机滑窗判断排在优先级判断之前，人类唤醒也被挡）。熔断只能由人解（`recover`，IO 层只给客户端开这个口）。

**Files:**
- Create: `crewd/crates/wake/src/breaker.rs`
- Modify: `crewd/crates/wake/src/lib.rs`（加 `pub mod breaker;`）

**Interfaces:**
- Consumes: Task 1 `Ms`、`MIN`、`SEC`、`Latch`、`LatchScope`、`WakeKind`；Task 2 `Sender`。
- Produces: `breaker::{LoopLimits, LOOP_LIMITS, HUMAN, SYSTEM, origin_of, edge_id, Verdict, LoopBreaker}`（方法见「接口一览」）。

- [ ] **Step 1: 写失败的测试**

`crewd/crates/wake/src/breaker.rs` 先放测试（`lib.rs` 加 `pub mod breaker;`）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SEC;

    const T0: Ms = Ms(1_000_000_000);

    fn lb() -> LoopBreaker {
        LoopBreaker::new(LOOP_LIMITS, vec![])
    }

    fn round(i: usize) -> Ms {
        T0.add(i as i64 * 30 * SEC)
    }

    #[test]
    fn ping_pong_between_two_sessions_is_cut_on_one_edge() {
        let mut b = lb();
        let mut tripped = Vec::new();
        'rounds: for i in 0..20 {
            // a 叫醒 b，b 回 @a（回叫，不算产出）；b 叫醒 a，a 回 @b ……
            for (target, origin) in [("b", "a"), ("a", "b")] {
                match b.admit(target, origin, round(i)) {
                    Verdict::Pass => b.on_post(target, &[origin.to_string()]),
                    Verdict::Tripped(l) => {
                        tripped.push(l);
                        break 'rounds;
                    }
                    Verdict::Held(l) => panic!("没熔断过却 Held：{l:?}"),
                }
            }
        }
        assert_eq!(tripped.len(), 1);
        assert_eq!((tripped[0].scope, tripped[0].id.as_str()), (LatchScope::Edge, "a>b"));
        assert_eq!(tripped[0].at, round(LOOP_LIMITS.edge_max));
        assert!(matches!(b.admit("b", "a", round(21)), Verdict::Held(_)));
        assert_eq!(b.admit("b", HUMAN, round(21)), Verdict::Pass, "人照样送到");
        assert_eq!(b.admit("b", "c", round(21)), Verdict::Pass, "别的来源不受影响");
    }

    #[test]
    fn progress_resets_the_streak() {
        let mut b = lb();
        for i in 0..LOOP_LIMITS.edge_max {
            assert_eq!(b.admit("b", "a", round(i)), Verdict::Pass);
        }
        b.progress("b");
        assert_eq!(b.streak_len("b"), 0);
        for i in 0..LOOP_LIMITS.edge_max {
            assert_eq!(b.admit("b", "a", round(10 + i)), Verdict::Pass, "第 {i} 次");
        }
        assert!(matches!(b.admit("b", "a", round(19)), Verdict::Tripped(_)));
    }

    #[test]
    fn posting_to_someone_else_is_progress_replying_back_is_not() {
        let mut b = lb();
        for i in 0..3 {
            b.admit("b", "a", round(i));
        }
        b.on_post("b", &["a".to_string()]);
        assert_eq!(b.streak_len("b"), 3, "回给叫它的人不算产出");
        b.on_post("b", &["a".to_string(), "c".to_string()]);
        assert_eq!(b.streak_len("b"), 3, "只要叫回了来源就不算");
        b.on_post("b", &["c".to_string()]);
        assert_eq!(b.streak_len("b"), 0);
        b.admit("b", "a", round(4));
        b.on_post("b", &[]);
        assert_eq!(b.streak_len("b"), 0, "发了一条谁也不叫的消息（如发给人）算产出");
    }

    #[test]
    fn slow_repeats_outside_the_window_never_trip() {
        let mut b = lb();
        for i in 0..50 {
            assert_eq!(b.admit("b", "a", T0.add(i * 2 * MIN)), Verdict::Pass, "第 {i} 次");
        }
    }

    #[test]
    fn self_timer_loop_trips_its_own_edge() {
        let mut b = lb();
        for i in 0..LOOP_LIMITS.edge_max {
            assert_eq!(b.admit("t", "t", round(i)), Verdict::Pass);
        }
        let Verdict::Tripped(l) = b.admit("t", "t", round(9)) else { panic!("该熔断") };
        assert_eq!((l.scope, l.id.as_str()), (LatchScope::Edge, "t>t"));
    }

    #[test]
    fn system_and_self_wakes_share_the_target_budget() {
        let mut b = lb();
        for i in 0..4 {
            assert_eq!(b.admit("t", SYSTEM, round(2 * i)), Verdict::Pass);
            assert_eq!(b.admit("t", "t", round(2 * i + 1)), Verdict::Pass);
        }
        let Verdict::Tripped(l) = b.admit("t", SYSTEM, round(9)) else { panic!("该熔断目标") };
        assert_eq!((l.scope, l.id.as_str()), (LatchScope::Session, "t"));
        assert!(matches!(b.admit("t", "t", round(10)), Verdict::Held(_)));
        assert_eq!(b.admit("t", "x", round(10)), Verdict::Pass, "别的 session 叫它不受目标级熔断影响");
    }

    #[test]
    fn human_wakes_never_count_and_are_never_held() {
        let latch = Latch { scope: LatchScope::Session, id: "t".into(), reason: "x".into(), at: T0 };
        let mut b = LoopBreaker::new(LOOP_LIMITS, vec![latch]);
        for i in 0..100 {
            assert_eq!(b.admit("t", HUMAN, T0.add(i)), Verdict::Pass);
        }
        assert_eq!(b.streak_len("t"), 0);
        assert!(b.held("t", HUMAN).is_none());
    }

    #[test]
    fn thirty_sessions_working_normally_are_never_tripped() {
        let workers: Vec<String> = (1..=30).map(|i| format!("w{i}")).collect();
        for tools_reported in [true, false] {
            let mut b = lb();
            // 10 分钟里 3 轮：机长给每个 worker 派一件事，worker 干活、回报 @机长，机长接着派下一件。
            // `tools_reported = false`：M3b 接上工具调用信号之前，只有消息这一种产出信号。
            for r in 0..3 {
                for (i, w) in workers.iter().enumerate() {
                    let now = T0.add((r * 30 + i) as i64 * 6 * SEC);
                    assert_eq!(b.admit(w, "cap", now), Verdict::Pass, "{w} 第 {r} 轮");
                    if tools_reported {
                        b.progress(w);
                    }
                    b.on_post(w, &["cap".to_string()]);
                    assert_eq!(b.admit("cap", w, now), Verdict::Pass, "cap ← {w} 第 {r} 轮");
                    b.on_post("cap", &[w.clone()]);
                }
            }
            assert!(b.latches().is_empty());
        }
    }

    #[test]
    fn loaded_latches_hold_until_recovered() {
        let latch = Latch { scope: LatchScope::Edge, id: "a>b".into(), reason: "x".into(), at: T0 };
        let mut b = LoopBreaker::new(LOOP_LIMITS, vec![latch]);
        assert!(matches!(b.admit("b", "a", T0), Verdict::Held(_)));
        assert!(b.recover(LatchScope::Edge, "a>b"));
        assert!(!b.recover(LatchScope::Edge, "a>b"), "第二次没东西可解");
        assert_eq!(b.admit("b", "a", T0), Verdict::Pass);
    }

    #[test]
    fn recovering_an_edge_forgets_that_edges_streak() {
        let mut b = lb();
        for i in 0..=LOOP_LIMITS.edge_max {
            b.admit("b", "a", round(i));
        }
        assert!(b.recover(LatchScope::Edge, "a>b"));
        for i in 0..LOOP_LIMITS.edge_max {
            assert_eq!(b.admit("b", "a", round(10 + i)), Verdict::Pass, "恢复后重新计数，第 {i} 次");
        }
    }

    #[test]
    fn origin_of_maps_each_kind() {
        let w1 = Sender::Session("w1".into());
        assert_eq!(origin_of(WakeKind::Message, Some(&w1), "t"), "w1");
        assert_eq!(origin_of(WakeKind::Message, Some(&Sender::Human), "t"), HUMAN);
        assert_eq!(origin_of(WakeKind::Message, None, "t"), SYSTEM);
        assert_eq!(origin_of(WakeKind::Timer, None, "t"), "t");
        assert_eq!(origin_of(WakeKind::AskAnswer, None, "t"), HUMAN);
        assert_eq!(origin_of(WakeKind::Restart, None, "t"), HUMAN);
        for k in [WakeKind::System, WakeKind::Checkpoint, WakeKind::StopCheck] {
            assert_eq!(origin_of(k, None, "t"), SYSTEM);
        }
    }
}
```

Run: `cargo test -p crewd-wake breaker`
Expected: 编译失败。

- [ ] **Step 2: 实现**

在 `breaker.rs` 测试模块上方写：

```rust
//! 唤醒环路熔断（spec §22.7 Q5，人 2026-10-06 拍板）：**不做任何全机 / 机组总量限流**；只截「短时间内反复叫醒、
//! 却没有新产出」的环路——程序错误让同一 session 被反复叫醒，或两个 session 互相叫醒打转。
//!
//! 两级：
//! - 边（来源 → 目标）：同一来源对同一目标，目标上次产出之后、窗口内已放行 `edge_max` 次，再来就熔断这条边。
//! - 目标：系统 / 目标自己定时这类非 session 来源对同一目标合计已放行 `target_max` 次，再来就熔断目标的这类唤醒。
//!
//! 产出（清零目标的计数）：工具调用、工作项变化（IO 层调 [`LoopBreaker::progress`]）；目标发了一条消息、且它叫醒的
//! 人里不含正在叫它的来源（[`LoopBreaker::on_post`]——回叫不算，否则互相 @ 的环路每轮都「有新消息」，永远截不住）。
//! 人发起的唤醒（来源 `human`）永不计数、永不被挡。熔断只能由人解（[`LoopBreaker::recover`]）。
use std::collections::{BTreeMap, HashMap};

use crate::model::{Latch, LatchScope, MIN, Ms, WakeKind};
use crate::plan::Sender;

/// 判据的唯一一处常量（§22.7 Q5「阈值与判据写成一处常量」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopLimits {
    /// 只看这段时间里的投递。
    pub window_ms: i64,
    /// 同一来源对同一目标、目标没有新产出时最多放行几次。
    pub edge_max: usize,
    /// 非 session 来源（系统、目标自己定时）对同一目标合计最多放行几次。
    pub target_max: usize,
}

pub const LOOP_LIMITS: LoopLimits = LoopLimits {
    window_ms: 10 * MIN,
    edge_max: 8,
    target_max: 8,
};

pub const HUMAN: &str = "human";
pub const SYSTEM: &str = "system";

/// 一条唤醒是谁引起的。消息类 = 发送者；定时 = 目标自己；问被回答 / 重启恢复 = 人（都是人点的头）；其余 = 系统。
pub fn origin_of(kind: WakeKind, sender: Option<&Sender>, target: &str) -> String {
    match kind {
        WakeKind::Message => sender.map_or(SYSTEM, Sender::key).to_string(),
        WakeKind::Timer => target.to_string(),
        WakeKind::AskAnswer | WakeKind::Restart => HUMAN.to_string(),
        WakeKind::System | WakeKind::Checkpoint | WakeKind::StopCheck => SYSTEM.to_string(),
    }
}

pub fn edge_id(origin: &str, target: &str) -> String {
    format!("{origin}>{target}")
}

fn is_other_session(origin: &str, target: &str) -> bool {
    origin != HUMAN && origin != SYSTEM && origin != target
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    /// 已熔断：留在 pending，直到人恢复。
    Held(Latch),
    /// 这一次触发了熔断：调用方落库（`wake_latches`）并告诉人与群主；这一次不投。
    Tripped(Latch),
}

#[derive(Debug)]
pub struct LoopBreaker {
    limits: LoopLimits,
    /// 见过的最晚时间（墙钟回跳不让窗口变宽）。
    clock: Ms,
    /// 目标 → 上次产出之后放行的 (时间, 来源)。
    streak: HashMap<String, Vec<(Ms, String)>>,
    latches: BTreeMap<(LatchScope, String), Latch>,
}

impl LoopBreaker {
    pub fn new(limits: LoopLimits, latches: Vec<Latch>) -> Self {
        Self {
            limits,
            clock: Ms(i64::MIN),
            streak: HashMap::new(),
            latches: latches.into_iter().map(|l| ((l.scope, l.id.clone()), l)).collect(),
        }
    }

    /// 这条来源对这个目标此刻是否被熔断挡着（人永不）。
    pub fn held(&self, target: &str, origin: &str) -> Option<&Latch> {
        if origin == HUMAN {
            return None;
        }
        if let Some(l) = self.latches.get(&(LatchScope::Edge, edge_id(origin, target))) {
            return Some(l);
        }
        if !is_other_session(origin, target) {
            return self.latches.get(&(LatchScope::Session, target.to_string()));
        }
        None
    }

    /// 投递前问一次；`Pass` 即记下这一次。
    pub fn admit(&mut self, target: &str, origin: &str, now: Ms) -> Verdict {
        if origin == HUMAN {
            return Verdict::Pass;
        }
        if let Some(l) = self.held(target, origin) {
            return Verdict::Held(l.clone());
        }
        self.clock = self.clock.max(now);
        let (clock, window) = (self.clock, self.limits.window_ms);
        let (edge_n, target_n) = {
            let streak = self.streak.entry(target.to_string()).or_default();
            streak.retain(|(at, _)| clock.since(*at) < window);
            (
                streak.iter().filter(|(_, o)| o == origin).count(),
                streak.iter().filter(|(_, o)| !is_other_session(o, target)).count(),
            )
        };
        if edge_n >= self.limits.edge_max {
            return Verdict::Tripped(self.trip(LatchScope::Edge, edge_id(origin, target), edge_n + 1));
        }
        if !is_other_session(origin, target) && target_n >= self.limits.target_max {
            return Verdict::Tripped(self.trip(LatchScope::Session, target.to_string(), target_n + 1));
        }
        self.streak
            .get_mut(target)
            .expect("上面 entry 建过")
            .push((clock, origin.to_string()));
        Verdict::Pass
    }

    fn trip(&mut self, scope: LatchScope, id: String, count: usize) -> Latch {
        let latch = Latch {
            scope,
            id: id.clone(),
            reason: format!("no-progress:{count}/{}min", self.limits.window_ms / MIN),
            at: self.clock,
        };
        self.latches.insert((scope, id), latch.clone());
        latch
    }

    /// 目标有了产出（工具调用、工作项变化）。
    pub fn progress(&mut self, target: &str) {
        self.streak.remove(target);
    }

    /// `poster` 发了一条消息、叫醒了 `woke`：叫醒的人里不含正在叫它的来源 → 算产出。
    pub fn on_post(&mut self, poster: &str, woke: &[String]) {
        let replies_back = self
            .streak
            .get(poster)
            .is_some_and(|s| s.iter().any(|(_, o)| woke.iter().any(|w| w == o)));
        if !replies_back {
            self.progress(poster);
        }
    }

    /// 人解开一条熔断；这条环路的计数一并清掉（不然恢复后第一次就又熔断）。
    pub fn recover(&mut self, scope: LatchScope, id: &str) -> bool {
        if self.latches.remove(&(scope, id.to_string())).is_none() {
            return false;
        }
        match scope {
            LatchScope::Session => {
                if let Some(s) = self.streak.get_mut(id) {
                    s.retain(|(_, o)| is_other_session(o, id));
                }
            }
            LatchScope::Edge => {
                if let Some((origin, target)) = id.split_once('>')
                    && let Some(s) = self.streak.get_mut(target)
                {
                    s.retain(|(_, o)| o != origin);
                }
            }
        }
        true
    }

    pub fn latches(&self) -> Vec<Latch> {
        self.latches.values().cloned().collect()
    }

    pub fn streak_len(&self, target: &str) -> usize {
        self.streak.get(target).map_or(0, Vec::len)
    }
}
```

Run: `cargo test -p crewd-wake breaker`
Expected: PASS（11 passed）。

- [ ] **Step 3: 变异自证（提交前做，改完手工改回）**

把 `on_post` 的 `if !replies_back` 改成无条件 `self.progress(poster);` → `ping_pong_between_two_sessions_is_cut_on_one_edge` 与 `posting_to_someone_else_is_progress_replying_back_is_not` 必须红（这就是「发消息就算产出」截不住互相 @ 的证据）。把 `admit` 开头的 `if origin == HUMAN { return Verdict::Pass; }` 删掉 → `human_wakes_never_count_and_are_never_held` 必须红。改回后全绿。

- [ ] **Step 4: 门禁与提交**

Run: `./scripts/check.sh`
Expected: 全绿。

```bash
git add crates/wake/src/breaker.rs crates/wake/src/lib.rs
git diff --cached --stat
git commit -m "crewd-wake: per-edge and per-target wake loop breaker, no global throttle (spec 22.7 Q5)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: 选批、退避、额度唤醒、重启名单、启动闸

**Files:**
- Create: `crewd/crates/wake/src/queue.rs`、`crewd/crates/wake/src/policy.rs`
- Modify: `crewd/crates/wake/src/lib.rs`（加 `pub mod policy;`、`pub mod queue;`）

**Interfaces:**
- Consumes: Task 1 `Ms`、`SEC`、`MIN`、`Priority`、`QueuedWake`、`WakeState`。
- Produces: `queue::{BatchLimits, is_due, select_batch, MAX_ATTEMPTS, RETRY_DELAYS_MS, after_failure, ids, should_alert}`；`policy::{QUOTA_RESET_BUFFER_MS, QUOTA_FALLBACK_MS, QUOTA_SOURCE_PREFIX, QuotaPlan, quota_plan, quota_pending, quota_due, RestartRow, restart_order, restart_source, DEFAULT_STAGGER_MS, Gate, LaunchGate}`。

- [ ] **Step 1: 写失败的测试（queue）**

`crewd/crates/wake/src/queue.rs` 先放测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WakeKind;

    pub(crate) fn w(id: i64, priority: Priority, not_before: i64, created_at: i64) -> QueuedWake {
        QueuedWake {
            id,
            session: "s".into(),
            kind: WakeKind::Message,
            source: format!("msg:m-{id}"),
            priority,
            not_before: Ms(not_before),
            note: None,
            state: WakeState::Pending,
            attempts: 0,
            created_at: Ms(created_at),
            origin: "cap".into(),
            msg: None,
        }
    }

    #[test]
    fn batch_takes_only_the_highest_due_tier_in_creation_order() {
        let pending = vec![
            w(1, Priority::Auto, 0, 1),
            w(2, Priority::Human, 0, 5),
            w(3, Priority::Human, 0, 2),
            w(4, Priority::Human, 99, 0), // 没到点
            w(5, Priority::System, 0, 0),
        ];
        let got: Vec<i64> = select_batch(&pending, Ms(10), &BatchLimits::default()).iter().map(|w| w.id).collect();
        assert_eq!(got, [3, 2]);
    }

    #[test]
    fn non_pending_rows_are_never_selected_and_ties_break_by_id() {
        let mut a = w(7, Priority::Auto, 0, 1);
        a.state = WakeState::Inflight;
        let pending = vec![a, w(9, Priority::Auto, 0, 1), w(8, Priority::Auto, 0, 1)];
        let got: Vec<i64> = select_batch(&pending, Ms(1), &BatchLimits::default()).iter().map(|w| w.id).collect();
        assert_eq!(got, [8, 9]);
        assert!(select_batch(&[], Ms(1), &BatchLimits::default()).is_empty());
    }

    #[test]
    fn batch_is_capped_at_max_items_but_never_empty_when_something_is_due() {
        let pending: Vec<QueuedWake> = (1..=15).map(|i| w(i, Priority::Auto, 0, i)).collect();
        assert_eq!(select_batch(&pending, Ms(0), &BatchLimits::default()).len(), 10);
        let one = BatchLimits { max_items: 0, max_bytes: 1 };
        assert_eq!(select_batch(&pending, Ms(0), &one).len(), 1);
    }

    #[test]
    fn failures_back_off_then_give_up_on_the_third() {
        assert_eq!(after_failure(1, Ms(0)), Some(Ms(30 * SEC)));
        assert_eq!(after_failure(2, Ms(0)), Some(Ms(2 * MIN)));
        assert_eq!(after_failure(3, Ms(0)), None);
        assert_eq!(after_failure(9, Ms(0)), None);
    }

    #[test]
    fn only_non_system_batches_alert_on_give_up() {
        assert!(!should_alert(&[w(1, Priority::System, 0, 0)]));
        assert!(should_alert(&[w(1, Priority::System, 0, 0), w(2, Priority::Auto, 0, 0)]));
        assert!(should_alert(&[w(1, Priority::Human, 0, 0)]));
        assert_eq!(ids(&[w(4, Priority::Auto, 0, 0), w(2, Priority::Auto, 0, 0)]), [4, 2]);
    }
}
```

Run: `cargo test -p crewd-wake queue`
Expected: 编译失败。

- [ ] **Step 2: 实现 queue**

```rust
//! 选批与退避（spec §22.4「队列」）。纯函数。
//! Swift 是一次一条（`CrewDeferredWakeQueue.swift:50`）；这里到期的最高一档合并成一次投递（≤10 条，渲染后 ≤8KB 由 IO 层裁），
//! 少转几轮空转。
use crate::model::{MIN, Ms, Priority, QueuedWake, SEC, WakeState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchLimits {
    pub max_items: usize,
    /// 渲染后的正文上限（IO 层渲染后从尾部减条数直到放得下，至少留 1 条）。
    pub max_bytes: usize,
}

impl Default for BatchLimits {
    fn default() -> Self {
        Self { max_items: 10, max_bytes: 8 * 1024 }
    }
}

pub fn is_due(w: &QueuedWake, now: Ms) -> bool {
    w.state == WakeState::Pending && w.not_before <= now
}

/// 到期的 pending 里只取最高一档优先级，按 `(created_at, id)`，最多 `max_items`（至少 1）条。
pub fn select_batch(pending: &[QueuedWake], now: Ms, limits: &BatchLimits) -> Vec<QueuedWake> {
    let Some(top) = pending.iter().filter(|w| is_due(w, now)).map(|w| w.priority).max() else {
        return vec![];
    };
    let mut tier: Vec<QueuedWake> = pending
        .iter()
        .filter(|w| is_due(w, now) && w.priority == top)
        .cloned()
        .collect();
    tier.sort_by_key(|w| (w.created_at, w.id));
    tier.truncate(limits.max_items.max(1));
    tier
}

pub const MAX_ATTEMPTS: u32 = 3;
pub const RETRY_DELAYS_MS: [i64; 2] = [30 * SEC, 2 * MIN];

/// 一次投递终局失败之后：`attempts` 是算上这次的次数。`None` = 放弃（记 dropped）。
pub fn after_failure(attempts: u32, now: Ms) -> Option<Ms> {
    if attempts >= MAX_ATTEMPTS {
        return None;
    }
    let i = attempts.max(1) as usize - 1;
    RETRY_DELAYS_MS.get(i).map(|d| now.add(*d))
}

pub fn ids(batch: &[QueuedWake]) -> Vec<i64> {
    batch.iter().map(|w| w.id).collect()
}

/// 放弃时要不要在群里告警：批里有非 system 级的唤醒才告（system 级的告警本身叫不醒时不成环，
/// 同 `CrewLocalMentionWakeLogic.swift:26-28` 的用意）。
pub fn should_alert(batch: &[QueuedWake]) -> bool {
    batch.iter().any(|w| w.priority != Priority::System)
}
```

Run: `cargo test -p crewd-wake queue`
Expected: PASS（5 passed）。

- [ ] **Step 3: 写失败的测试（policy）**

`crewd/crates/wake/src/policy.rs` 先放测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Priority, WakeKind};

    fn quota_wake(source: &str, state: WakeState, not_before: i64) -> QueuedWake {
        QueuedWake {
            id: 1,
            session: "s".into(),
            kind: WakeKind::System,
            source: source.into(),
            priority: Priority::System,
            not_before: Ms(not_before),
            note: None,
            state,
            attempts: 0,
            created_at: Ms(0),
            origin: "system".into(),
            msg: None,
        }
    }

    #[test]
    fn quota_wake_fires_a_minute_after_a_future_reset_else_backs_off() {
        let now = Ms(1_000 * MIN);
        let p = quota_plan(Some(now.add(30 * MIN)), now);
        assert_eq!(p, QuotaPlan { fire_at: now.add(31 * MIN), fallback: false });
        assert_eq!(p.source(), format!("quota:{}", now.add(31 * MIN).0));
        for stale in [None, Some(now), Some(now.add(-MIN))] {
            assert_eq!(quota_plan(stale, now), QuotaPlan { fire_at: now.add(45 * MIN), fallback: true });
        }
    }

    #[test]
    fn quota_pending_and_due_look_only_at_quota_sources() {
        let p = vec![quota_wake("quota:5", WakeState::Pending, 50), quota_wake("timer:x", WakeState::Pending, 0)];
        assert!(quota_pending(&p));
        assert!(!quota_due(&p, Ms(49)));
        assert!(quota_due(&p, Ms(50)));
        assert!(!quota_pending(&[quota_wake("quota:5", WakeState::Delivered, 0)]));
        assert!(quota_pending(&[quota_wake("quota:5", WakeState::Inflight, 0)]));
    }

    fn row(session: &str, captain: bool, was_working: bool, last: Option<i64>) -> RestartRow {
        RestartRow {
            session: session.into(),
            name: session.into(),
            crew: "c".into(),
            captain,
            was_working,
            last_active_at: last.map(Ms),
        }
    }

    #[test]
    fn restart_order_puts_captains_first_then_most_recently_active() {
        let got: Vec<String> = restart_order(vec![
            row("w-old", false, false, Some(1)),
            row("w-never", false, false, None),
            row("cap", true, false, Some(0)),
            row("w-new", false, true, Some(9)),
        ])
        .into_iter()
        .map(|r| r.session)
        .collect();
        assert_eq!(got, ["cap", "w-new", "w-old", "w-never"]);
        assert_eq!(restart_source("b1"), "restart:b1");
    }

    #[test]
    fn launch_gate_has_no_cap_by_default_and_staggers_launches() {
        let mut g = LaunchGate::new(None, DEFAULT_STAGGER_MS);
        assert_eq!(g.check(10_000, false, Ms(0)), Gate::Go, "默认不设上限（§22.7 Q5）");
        g.launched(Ms(0));
        assert_eq!(g.check(1, false, Ms(500)), Gate::WaitUntil(Ms(DEFAULT_STAGGER_MS)));
        assert_eq!(g.check(1, true, Ms(500)), Gate::WaitUntil(Ms(DEFAULT_STAGGER_MS)), "错开对谁都生效");
        assert_eq!(g.check(1, false, Ms(DEFAULT_STAGGER_MS)), Gate::Go);
        g.launched(Ms(-5));
        assert_eq!(g.check(1, false, Ms(500)), Gate::WaitUntil(Ms(DEFAULT_STAGGER_MS)), "launched 只往后推");
    }

    #[test]
    fn optional_cap_blocks_only_automatic_launches() {
        let mut g = LaunchGate::new(None, 0);
        g.set_max_live(Some(3));
        assert_eq!(g.max_live(), Some(3));
        assert_eq!(g.check(2, false, Ms(0)), Gate::Go);
        assert_eq!(g.check(3, false, Ms(0)), Gate::AtCap);
        assert_eq!(g.check(3, true, Ms(0)), Gate::Go, "人起的不受上限");
        g.set_max_live(None);
        assert_eq!(g.check(3, false, Ms(0)), Gate::Go);
    }
}
```

Run: `cargo test -p crewd-wake policy`
Expected: 编译失败。

- [ ] **Step 4: 实现 policy**

```rust
//! 会话生命周期的几条策略（纯函数）：额度重置唤醒的时刻、重启后的恢复名单顺序、启动闸（可选上限 + 错开）。
use crate::model::{MIN, Ms, QueuedWake, SEC, WakeState};
use crate::queue::is_due;

/// 重置后再缓 1 分钟（额度窗刚翻新，别掐点撞上仍受限的边缘；`AgentQuota.swift:342`）。
pub const QUOTA_RESET_BUFFER_MS: i64 = MIN;
/// 拿不到可信重置时刻时的退避（`AgentQuota.swift:344`）。
pub const QUOTA_FALLBACK_MS: i64 = 45 * MIN;
pub const QUOTA_SOURCE_PREFIX: &str = "quota:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuotaPlan {
    pub fire_at: Ms,
    /// true = 没拿到可信重置时刻，走的退避（群里要说清，Swift 的 fail-loud）。
    pub fallback: bool,
}

impl QuotaPlan {
    pub fn source(&self) -> String {
        crate::model::source::quota(self.fire_at.0)
    }
}

pub fn quota_plan(reset_at: Option<Ms>, now: Ms) -> QuotaPlan {
    match reset_at {
        Some(r) if r > now => QuotaPlan { fire_at: r.add(QUOTA_RESET_BUFFER_MS), fallback: false },
        _ => QuotaPlan { fire_at: now.add(QUOTA_FALLBACK_MS), fallback: true },
    }
}

fn is_quota(w: &QueuedWake) -> bool {
    w.source.starts_with(QUOTA_SOURCE_PREFIX)
}

/// 已挂着一条还没投完的额度唤醒（pending / inflight）。
pub fn quota_pending(pending: &[QueuedWake]) -> bool {
    pending
        .iter()
        .any(|w| is_quota(w) && matches!(w.state, WakeState::Pending | WakeState::Inflight))
}

pub fn quota_due(pending: &[QueuedWake], now: Ms) -> bool {
    pending.iter().any(|w| is_quota(w) && is_due(w, now))
}

/// 重启时「上次在跑」的一行（`desired=running`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartRow {
    pub session: String,
    pub name: String,
    pub crew: String,
    pub captain: bool,
    /// 上次停在干活中途（`was_working=1`）：恢复时补一条「被打断」附言。
    pub was_working: bool,
    pub last_active_at: Option<Ms>,
}

/// 恢复名单的顺序：机长在前，然后最近活跃的在前，最后按 id（§22.7 Q1：人答「恢复」后按这个顺序错开拉起）。
pub fn restart_order(mut rows: Vec<RestartRow>) -> Vec<RestartRow> {
    rows.sort_by(|a, b| {
        b.captain
            .cmp(&a.captain)
            .then(b.last_active_at.cmp(&a.last_active_at))
            .then(a.session.cmp(&b.session))
    });
    rows
}

/// 重启询问的恢复唤醒：`tag` = M3a `restart_prompts` 的 id。
pub fn restart_source(tag: &str) -> String {
    crate::model::source::restart(tag)
}

/// 两次拉起之间至少隔这么久（§22.7 Q1「错开时间逐个」）。
pub const DEFAULT_STAGGER_MS: i64 = 2 * SEC;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    Go,
    /// 可选上限已满（只挡自动拉起）。
    AtCap,
    /// 离上一次拉起太近，这一刻之后再试。
    WaitUntil(Ms),
}

/// 启动闸。**默认没有上限**（§22.7 Q5）；上限只挡自动拉起，人起的（`bypass_cap`）不挡。错开对谁都生效。
#[derive(Clone, Debug)]
pub struct LaunchGate {
    max_live: Option<usize>,
    stagger_ms: i64,
    last: Option<Ms>,
}

impl LaunchGate {
    pub fn new(max_live: Option<usize>, stagger_ms: i64) -> Self {
        Self { max_live, stagger_ms, last: None }
    }

    /// `live` = 正在跑 + 正在起的会话数。
    pub fn check(&self, live: usize, bypass_cap: bool, now: Ms) -> Gate {
        if !bypass_cap && self.max_live.is_some_and(|m| live >= m) {
            return Gate::AtCap;
        }
        match self.last {
            Some(t) if now.since(t) < self.stagger_ms => Gate::WaitUntil(t.add(self.stagger_ms)),
            _ => Gate::Go,
        }
    }

    pub fn launched(&mut self, now: Ms) {
        self.last = Some(self.last.map_or(now, |t| t.max(now)));
    }

    pub fn set_max_live(&mut self, max: Option<usize>) {
        self.max_live = max;
    }

    pub fn max_live(&self) -> Option<usize> {
        self.max_live
    }
}
```

Run: `cargo test -p crewd-wake`
Expected: PASS（全部 crewd-wake 测试，41 passed）。

- [ ] **Step 5: 门禁与提交**

Run: `./scripts/check.sh`
Expected: 全绿。

```bash
git add crates/wake/src/queue.rs crates/wake/src/policy.rs crates/wake/src/lib.rs
git diff --cached --stat
git commit -m "crewd-wake: batch selection, retry back-off, quota wake time, restart order, launch gate

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: 待决策通知与 nudge 键表

移植 `Sources/Mac/LocalRunner/SessionPendingDecision.swift:339-470`：按屏幕上本来有没有编号分叉（§15「绝不编造编号」）；键名从键表现算（`:347-359` 的教训：一张表、一份名单）；worker 卡住 @机长、机长卡住直接 @人、5 分钟没解升级 @人（`:372-389`）。

**Files:**
- Create: `crewd/crates/wake/src/notice.rs`
- Modify: `crewd/crates/wake/src/lib.rs`（加 `pub mod notice;`）

**Interfaces:**
- Consumes: Task 1 `text::{Lang, get, render}`、`MIN`。
- Produces: `notice::{DecisionFacts, NoticeStage, DECISION_ESCALATE_MS, Notice, decision_notice, NUDGE_KEYS, MOVE_KEY_CANDIDATES, nudge_key_names, selection_move_keys, nudge_bytes}`。

- [ ] **Step 1: 写失败的测试**

`crewd/crates/wake/src/notice.rs` 先放测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn d(numbered: bool) -> DecisionFacts {
        DecisionFacts {
            prompt: "Do you trust the files in this folder?".into(),
            options: vec!["Yes, proceed".into(), "No, exit".into()],
            numbered,
        }
    }

    #[test]
    fn worker_first_notice_goes_to_the_captain_with_numbers_when_the_screen_had_them() {
        let n = decision_notice(Lang::Zh, NoticeStage::First, "w1", "s-1", false, &d(true), 0);
        assert!(!n.to_human);
        assert!(n.text.contains("  1. Yes, proceed") && n.text.contains("  2. No, exit"), "{}", n.text);
        assert!(n.text.contains(get(Lang::Zh, "decision.options.numbered")));
        assert!(n.text.contains(get(Lang::Zh, "decision.tail.worker")));
        assert!(n.text.ends_with(&render(Lang::Zh, "decision.session", &[("session", "s-1")])));
    }

    #[test]
    fn unnumbered_screen_never_gets_invented_numbers_and_lists_keys_from_the_table() {
        let n = decision_notice(Lang::Zh, NoticeStage::First, "w1", "s-1", false, &d(false), 0);
        assert!(!n.text.contains("1. "), "绝不编造编号：{}", n.text);
        for k in nudge_key_names() {
            assert!(n.text.contains(&format!("`{k}`")), "键表里的 {k} 没出现在消息里");
        }
        // 键表里没有能挪高亮的键 → 交给人。
        assert!(selection_move_keys().is_empty());
        assert!(n.text.contains(get(Lang::Zh, "decision.move.none")));
    }

    #[test]
    fn captain_notice_goes_straight_to_the_human_and_escalation_always_does() {
        let n = decision_notice(Lang::En, NoticeStage::First, "cap", "s-0", true, &d(true), 0);
        assert!(n.to_human);
        assert!(n.text.contains(get(Lang::En, "decision.tail.captain")));
        let n = decision_notice(Lang::Zh, NoticeStage::Escalate, "w1", "s-1", false, &d(true), 7);
        assert!(n.to_human);
        assert!(n.text.starts_with(&render(Lang::Zh, "decision.escalate", &[("name", "w1"), ("minutes", "7")])));
    }

    #[test]
    fn empty_prompt_and_options_are_left_out() {
        let n = decision_notice(
            Lang::Zh,
            NoticeStage::First,
            "w1",
            "s-1",
            false,
            &DecisionFacts { prompt: String::new(), options: vec![], numbered: false },
            0,
        );
        assert_eq!(n.text.lines().count(), 3, "{}", n.text);
    }

    #[test]
    fn key_names_are_derived_one_per_byte_sequence() {
        assert_eq!(nudge_key_names(), ["enter", "esc"]);
    }

    #[test]
    fn nudge_maps_aliases_to_bytes_and_text_to_inert_text_plus_enter() {
        assert_eq!(nudge_bytes("enter"), b"\r");
        assert_eq!(nudge_bytes(" ESC "), b"\x1b");
        assert_eq!(nudge_bytes("回车"), b"\r");
        assert_eq!(nudge_bytes("2"), b"2\r");
        assert_eq!(nudge_bytes("a\x1bb\nc"), "a\u{241b}b c\r".as_bytes());
    }

    /// 给 agent / 群看的文字只在 prompts/*.md（Global Constraints）。判定源码里出现中日韩字符只可能是漏网的文案
    /// （键名例外，那一行带 `键名，不是文案` 标记；`expect` / `panic!` 的开发者信息例外）。注释与测试不查。
    #[test]
    fn decision_sources_carry_no_prose_literals() {
        let files = [
            ("plan.rs", include_str!("plan.rs")),
            ("breaker.rs", include_str!("breaker.rs")),
            ("queue.rs", include_str!("queue.rs")),
            ("policy.rs", include_str!("policy.rs")),
            ("notice.rs", include_str!("notice.rs")),
        ];
        let cjk = |c: char| matches!(c, '\u{3000}'..='\u{303f}' | '\u{4e00}'..='\u{9fff}' | '\u{ff00}'..='\u{ffef}');
        for (name, src) in files {
            let code = src.split("#[cfg(test)]").next().expect("split 至少一段");
            for (i, line) in code.lines().enumerate() {
                let t = line.trim_start();
                if t.starts_with("//")
                    || line.contains("键名，不是文案")
                    || line.contains(".expect(")
                    || line.contains("panic!(")
                {
                    continue;
                }
                let code_part = t.split(" //").next().unwrap_or(t);
                assert!(!code_part.chars().any(cjk), "{name}:{} 有文案字面量：{line}", i + 1);
            }
        }
    }
}
```

Run: `cargo test -p crewd-wake notice`
Expected: 编译失败。

- [ ] **Step 2: 实现**

```rust
//! 待决策通知与 nudge 键表（移植 `Sources/Mac/LocalRunner/SessionPendingDecision.swift:339-470`）。
//! - 按法由这一屏现算：有编号说发数字；没编号说「别发数字」，可用的键从 [`NUDGE_KEYS`] 现算（一张表、一份名单）。
//! - 选靶：worker 卡住 → @机长；机长卡住 → 直接 @人（绝不 @ 自己：@机长会触发拉起，成环，`:375`）；升级 → @人。
use crate::model::MIN;
use crate::text::{Lang, get, render};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionFacts {
    pub prompt: String,
    pub options: Vec<String>,
    /// 屏幕上本来有没有编号（事实字段，由 crewd-term 如实填）。
    pub numbered: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeStage {
    First,
    Escalate,
}

/// 首报后多久没解决就升级找人（`SessionPendingDecision.swift:389`）。
pub const DECISION_ESCALATE_MS: i64 = 5 * MIN;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    /// true = 发到根机组人类群并 @人；false = 发到本机组大厅并 @群主（机长）。
    pub to_human: bool,
}

/// 别名 → 发给 PTY 的字节。**不在表里的输入一律当文本发出去、并自动补一个回车**。
pub const NUDGE_KEYS: &[(&str, &[u8])] = &[
    ("enter", b"\r"),
    ("esc", b"\x1b"),
    ("回车", b"\r"), // 键名，不是文案
];

/// 能挪菜单高亮的键名（候选；只有进了 `NUDGE_KEYS` 的才算数）。
pub const MOVE_KEY_CANDIDATES: &[&str] = &["up", "down", "上", "下"]; // 键名，不是文案

fn bytes_of(alias: &str) -> Option<&'static [u8]> {
    NUDGE_KEYS.iter().find(|(n, _)| *n == alias).map(|(_, b)| *b)
}

/// 给人看的键名：从表里算；同一串字节只露字典序最小的那个别名。
pub fn nudge_key_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = NUDGE_KEYS.iter().map(|(n, _)| *n).collect();
    names.sort();
    let mut seen: Vec<&'static [u8]> = Vec::new();
    names
        .into_iter()
        .filter(|n| {
            let b = bytes_of(n).expect("名字来自表本身");
            if seen.contains(&b) {
                false
            } else {
                seen.push(b);
                true
            }
        })
        .collect()
}

pub fn selection_move_keys() -> Vec<&'static str> {
    MOVE_KEY_CANDIDATES.iter().copied().filter(|k| bytes_of(k).is_some()).collect()
}

/// `nudge_session` 的输入 → 写进 PTY 的字节。别名 → 表里的字节；其余 = 去毒后的文本 + 回车
/// （控制字符换成 Unicode 控制图形，换行变空格：别让文本里藏着按键）。
pub fn nudge_bytes(input: &str) -> Vec<u8> {
    let key = input.trim();
    if let Some(b) = bytes_of(&key.to_ascii_lowercase()) {
        return b.to_vec();
    }
    let mut s: String = key
        .chars()
        .map(|c| match c {
            '\r' | '\n' => ' ',
            '\u{7f}' => '\u{2421}',
            c if (c as u32) < 0x20 => char::from_u32(0x2400 + c as u32).expect("U+2400..U+241F 都是合法字符"),
            c => c,
        })
        .collect();
    s.push('\r');
    s.into_bytes()
}

fn backticked(keys: &[&str]) -> String {
    keys.iter().map(|k| format!("`{k}`")).collect::<Vec<_>>().join(" / ")
}

fn render_options(lang: Lang, options: &[String], numbered: bool) -> Vec<String> {
    if options.is_empty() {
        return vec![];
    }
    if numbered {
        let mut v: Vec<String> = options.iter().enumerate().map(|(i, o)| format!("  {}. {o}", i + 1)).collect();
        v.push(get(lang, "decision.options.numbered").to_string());
        return v;
    }
    let moves = selection_move_keys();
    let move_text = if moves.is_empty() {
        get(lang, "decision.move.none").to_string()
    } else {
        render(lang, "decision.move.keys", &[("keys", &backticked(&moves))])
    };
    let mut v: Vec<String> = options.iter().map(|o| format!("  {o}")).collect();
    v.push(render(
        lang,
        "decision.options.unnumbered",
        &[("keys", &backticked(&nudge_key_names())), ("move", &move_text)],
    ));
    v
}

pub fn decision_notice(
    lang: Lang,
    stage: NoticeStage,
    name: &str,
    session: &str,
    is_captain: bool,
    d: &DecisionFacts,
    waited_min: i64,
) -> Notice {
    let head = match stage {
        NoticeStage::First if is_captain => render(lang, "decision.first.captain", &[("name", name)]),
        NoticeStage::First => render(lang, "decision.first.worker", &[("name", name)]),
        NoticeStage::Escalate => render(
            lang,
            "decision.escalate",
            &[("name", name), ("minutes", &waited_min.to_string())],
        ),
    };
    let mut lines = vec![head];
    if !d.prompt.is_empty() {
        lines.push(d.prompt.clone());
    }
    lines.extend(render_options(lang, &d.options, d.numbered));
    lines.push(
        match stage {
            NoticeStage::First if is_captain => get(lang, "decision.tail.captain"),
            NoticeStage::First => get(lang, "decision.tail.worker"),
            NoticeStage::Escalate => get(lang, "decision.tail.escalate"),
        }
        .to_string(),
    );
    lines.push(render(lang, "decision.session", &[("session", session)]));
    Notice {
        text: lines.join("\n"),
        to_human: is_captain || stage == NoticeStage::Escalate,
    }
}
```

Run: `cargo test -p crewd-wake`
Expected: PASS（48 passed）。

- [ ] **Step 3: 变异自证（提交前做，改完手工改回）**

往 `NUDGE_KEYS` 加一行 `("down", b"\x1b[B"),` → `key_names_are_derived_one_per_byte_sequence` 与 `unnumbered_screen_never_gets_invented_numbers_and_lists_keys_from_the_table` 必须红（后者因为 `selection_move_keys()` 不再为空）——证明消息里的键名与「挪高亮」那句都跟着表走。把 `plan.rs` 某个非注释行里临时写一个 `let _x = "测试";` → `decision_sources_carry_no_prose_literals` 必须红。改回后全绿。

- [ ] **Step 4: 门禁与提交**

Run: `./scripts/check.sh`
Expected: 全绿。

```bash
git add crates/wake/src/notice.rs crates/wake/src/lib.rs
git diff --cached --stat
git commit -m "crewd-wake: pending-decision notices and nudge key table derived from one source

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 6: orchestra 骨架——时钟、执行器抽象与 claude 实现、挂钩点（含 Jev 桩）、Port 与测试器材

**Files:**
- Modify: `crewd/crates/server/Cargo.toml`、`crewd/Cargo.lock`、`crewd/crates/server/src/orchestra/mod.rs`（**整文件替换 M3a 的占位**；`lib.rs` 的 `pub mod orchestra;` 与 `AppState.orchestra: OnceLock<orchestra::Handle>` M3a 已有，本计划不改 `AppState`）
- Create: `crewd/crates/server/src/orchestra/{clock,exec,exec_claude,hooks,port,worktree,testkit}.rs`

**Interfaces:**
- Consumes: Task 1–5 全部；M2b `crewd_exec::{driver::*, session::SessionEvent, launch::{ClaudeSpec, CommsSpec}, env::{AgentPaths, compose_child_path}, resume::{AgentIdLedger, MemoryLedger, valid_agent_session_id}, registry::ProcessRegistry}`、`crewd_term::{PendingDecision, Snapshot}`。
- Produces（后面任务用的名字）：
  - `orchestra::OrchestraConfig { batch, loops, stagger_ms, lang, arbiter_timeout, heartbeat_ms, boot_id }`（`Default`；测试用 `OrchestraConfig::for_tests()`）
  - `clock::{Clock, SystemClock, ManualClock}`（`ManualClock::new(Ms)`、`set(Ms)`、`advance(i64)`）
  - `exec::{BoxFut, Context, LaunchRequest, LaunchError, Launched, Executor, ExecHandle, Executors}`
  - `exec_claude::{ClaudeProgram, ClaudeExecutor, ClaudeExecutorConfig, comms_for}`
  - `hooks::{ArbiterQuestion, Arbiter, NoArbiter, IdleHook, NoIdleHook, AskAnsweredHook, NoAskHook, QuotaSource, NoQuotaSource, ContextExtras, NoExtras, Hooks}`（`Hooks` 多一个 `worktrees: Arc<dyn Worktrees>`，默认真实现）
  - `worktree::{Worktrees, GitWorktrees, real}`（C8）
  - `port::{Result, Desired, SessionRow, NewSession, SessionPatch, Settle, Audience, LiveView, Rendered, OrchestraEvent, Port}`
  - `testkit::{MemPort, MemChannel, MemKind, StandIn, StandInWorktrees, LaunchRecord}`（`#[cfg(test)]`）

- [ ] **Step 1: 依赖**

```bash
cargo add --offline -p crewd-server --path crates/wake crewd-wake
cargo add --offline -p crewd-server --path crates/term crewd-term
cargo add --offline -p crewd-server --dev tokio -F test-util,rt,macros
```

确认 `crewd-server` 的 `[dependencies]` 多了 `crewd-wake`、`crewd-term`，`[dev-dependencies]` 的 tokio 带 `test-util`；`git diff Cargo.lock` 只多出 `crewd-wake` 相关的行，没有新的第三方 crate。

- [ ] **Step 2: 写失败的测试**

`crewd/crates/server/src/orchestra/mod.rs` 整文件替换 M3a 的占位（M3a 放的是 `#[derive(Debug, Clone)] pub struct Handle;`，让 `AppState.orchestra` 有类型；真 `Handle` 在 Task 9 才写，**在那之前把占位那两行留在文件末尾**，Task 9 用真定义换掉）：

```rust
//! serve 里的编排：会话管理 + 唤醒（spec §22.4、§22.7）。判定在 crewd-wake，这里只做 IO：
//! 一个 actor（`core::Core`）独占会话句柄与内存状态；数据只经 `port::Port`；执行器只经 `exec::Executor`。
pub mod clock;
pub mod exec;
pub mod exec_claude;
pub mod hooks;
pub mod port;
pub mod worktree;
#[cfg(test)]
mod testkit;

use std::time::Duration;

use crewd_wake::breaker::{LOOP_LIMITS, LoopLimits};
use crewd_wake::policy::DEFAULT_STAGGER_MS;
use crewd_wake::queue::BatchLimits;
use crewd_wake::text::Lang;

#[derive(Clone, Debug)]
pub struct OrchestraConfig {
    pub batch: BatchLimits,
    pub loops: LoopLimits,
    /// 两次拉起之间至少隔多久（§22.7 Q1）。
    pub stagger_ms: i64,
    pub lang: Lang,
    /// 问 Arbiter（M5b 的 Jev）最多等多久；超时 = 没答案 → 群主。
    pub arbiter_timeout: Duration,
    /// 没别的事时多久醒一次兜底（到点唤醒由「下一个 not_before」精确唤醒，这只是保险）。
    pub heartbeat_ms: i64,
    /// 本次 serve 启动的标识（重启询问的 promptId、restart 唤醒的 source）。
    pub boot_id: String,
}

impl Default for OrchestraConfig {
    fn default() -> Self {
        Self {
            batch: BatchLimits::default(),
            loops: LOOP_LIMITS,
            stagger_ms: DEFAULT_STAGGER_MS,
            lang: Lang::Zh,
            arbiter_timeout: Duration::from_secs(15),
            heartbeat_ms: 30_000,
            boot_id: uuid::Uuid::new_v4().to_string(),
        }
    }
}

impl OrchestraConfig {
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self {
            stagger_ms: 0,
            arbiter_timeout: Duration::from_millis(50),
            boot_id: "boot-1".into(),
            ..Self::default()
        }
    }
}

/// M3a 的占位（`AppState.orchestra` 的类型）；Task 9 换成真 `Handle`。
#[derive(Debug, Clone)]
pub struct Handle;
```

测试先写在各文件底部（实现见 Step 3）。`clock.rs` 的测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_moves_only_when_told() {
        let c = ManualClock::new(Ms(5));
        assert_eq!(c.now(), Ms(5));
        c.advance(10);
        assert_eq!(c.now(), Ms(15));
        c.set(Ms(1));
        assert_eq!(c.now(), Ms(1));
        assert!(SystemClock.now().0 > 1_700_000_000_000, "系统时钟是 Unix 毫秒");
    }
}
```

`exec.rs` 的测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executors_reject_kinds_that_are_not_wired_in() {
        let e = Executors::new();
        for kind in ["codex", "acp:opencode", "claude"] {
            let err = e.get(kind).err().expect("空表什么都不认");
            assert!(matches!(&err, LaunchError::Unsupported(k) if k == kind), "{err:?}");
        }
    }
}
```

`exec_claude.rs` 的测试（shell 脚本顶替 claude：把 argv 写进文件、停在那里等被停掉；进程由驱动层 setsid，自成进程组）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crewd_exec::resume::MemoryLedger;
    use crewd_exec::session::SessionEvent;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    fn executor(program: ClaudeProgram, data_dir: &Path) -> ClaudeExecutor {
        ClaudeExecutor::new(ClaudeExecutorConfig {
            program,
            data_dir: data_dir.to_path_buf(),
            helper: PathBuf::from("/nonexistent/crewd-helper"),
            registry: None,
            extra_env: BTreeMap::new(),
            projects_dir: None,
        })
    }

    fn request(session: &str, workdir: &Path) -> LaunchRequest {
        LaunchRequest {
            session_id: session.into(),
            crew_id: "c1".into(),
            workdir: workdir.to_path_buf(),
            model: None,
            effort: None,
            system_prompt: Some("合成世界观".into()),
            initial_prompt: None,
            token: "tok-1".into(),
            ledger: Arc::new(MemoryLedger::default()),
        }
    }

    #[tokio::test]
    async fn disabled_program_never_starts_anything() {
        let dir = tempfile::tempdir().unwrap();
        let err = executor(ClaudeProgram::Disabled, dir.path())
            .launch(request("s1", dir.path()))
            .await
            .err()
            .expect("没配置就不起");
        assert!(matches!(err, LaunchError::NotConfigured(_)), "{err}");
    }

    #[tokio::test]
    async fn launches_the_given_program_with_world_model_and_comms() {
        let dir = tempfile::tempdir().unwrap();
        let argv = dir.path().join("argv.txt");
        let script = dir.path().join("claude.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexec sleep 30\n", argv.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let exec = executor(ClaudeProgram::Path(script), dir.path());
        let (handle, mut events) = exec.launch(request("s1", dir.path())).await.unwrap();
        assert!(matches!(events.recv().await, Some(SessionEvent::Spawned { .. })));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let text = loop {
            if let Ok(t) = std::fs::read_to_string(&argv)
                && t.contains("--permission-mode")
            {
                break t;
            }
            assert!(tokio::time::Instant::now() < deadline, "脚本 10s 内没写出 argv");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        for flag in ["--session-id", "--append-system-prompt-file", "--mcp-config"] {
            assert!(text.lines().any(|l| l == flag), "缺 {flag}：{text}");
        }
        handle.stop().unwrap();
        let exited = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(e) = events.recv().await {
                if matches!(e, SessionEvent::Exited(_)) {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false);
        assert!(exited, "停掉之后 10s 内要见到 Exited");
    }

    #[test]
    fn transcript_lookup_scans_every_project_dir_and_refuses_path_like_ids() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("p1")).unwrap();
        std::fs::create_dir_all(dir.path().join("p2")).unwrap();
        std::fs::write(dir.path().join("p2/abc-1.jsonl"), "{}").unwrap();
        let mut exec = executor(ClaudeProgram::Disabled, dir.path());
        exec.projects_dir = Some(dir.path().to_path_buf());
        assert_eq!(exec.locate_transcript("abc-1"), Some(dir.path().join("p2/abc-1.jsonl")));
        assert_eq!(exec.locate_transcript("nope"), None);
        assert_eq!(exec.locate_transcript("../p2/abc-1"), None);
    }
}
```

`hooks.rs` 的测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crewd_wake::MessageRef;
    use crewd_wake::plan::Sender;

    #[tokio::test]
    async fn default_hooks_are_inert_stubs() {
        let h = Hooks::default();
        let q = ArbiterQuestion {
            message: MsgFacts {
                id: "m-1".into(),
                at: MessageRef { channel: "c1/lobby".into(), seq: 1 },
                sender: Sender::Human,
                mentions: vec![],
                created_at: Ms(0),
            },
            text: "合成消息".into(),
            ctx: ChannelCtx::default(),
        };
        assert_eq!(h.arbiter.wake_set(&q).await, None, "M5b 之前没有 Jev → 交群主");
        assert_eq!(h.idle.on_idle("s1", Ms(0)).await, None);
        assert_eq!(h.quota.reset_at("claude", Ms(0)).await, None);
        assert!(h.extras.extras("s1").await.is_empty());
        h.ask.on_answered("c1", "H1", &[], Ms(0)).await;
    }
}
```

`testkit.rs` 的自测：

```rust
#[cfg(test)]
mod self_tests {
    use super::*;
    use crewd_wake::{Priority, WakeKind};

    fn wake(session: &str, source: &str, priority: Priority) -> NewWake {
        NewWake {
            session: session.into(),
            kind: WakeKind::Message,
            source: source.into(),
            priority,
            not_before: Ms(0),
            note: None,
            origin: "cap".into(),
        }
    }

    #[tokio::test]
    async fn mem_port_dedupes_and_orders_due_sessions_by_priority_then_age() {
        let p = MemPort::new();
        assert!(p.enqueue(wake("a", "x#1", Priority::Auto)).await.unwrap());
        assert!(!p.enqueue(wake("a", "x#1", Priority::Auto)).await.unwrap(), "UNIQUE(session, kind, source)");
        assert!(p.enqueue(wake("b", "x#2", Priority::Human)).await.unwrap());
        assert!(p.enqueue(wake("c", "x#3", Priority::Auto)).await.unwrap());
        assert_eq!(p.due_sessions(Ms(0)).await.unwrap(), ["b", "a", "c"]);
        let ids: Vec<i64> = p.pending("a").await.unwrap().iter().map(|w| w.id).collect();
        p.settle(&ids, Settle::Inflight, Ms(0)).await.unwrap();
        assert!(p.pending("a").await.unwrap().is_empty());
        assert_eq!(p.requeue_inflight().await.unwrap(), 1);
        assert_eq!(p.pending("a").await.unwrap().len(), 1);
    }
}
```

Run: `cargo test -p crewd-server orchestra`
Expected: 编译失败（各模块的实现还没写）。

- [ ] **Step 3: 实现 `clock.rs`**

```rust
//! 时间来源。判定层只吃 `Ms`；IO 层只从这里取「现在」，测试用 `ManualClock`。
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crewd_wake::Ms;

pub trait Clock: Send + Sync {
    fn now(&self) -> Ms;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Ms {
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        Ms(ms)
    }
}

pub struct ManualClock(AtomicI64);

impl ManualClock {
    pub fn new(at: Ms) -> Self {
        Self(AtomicI64::new(at.0))
    }

    pub fn set(&self, at: Ms) {
        self.0.store(at.0, Ordering::SeqCst);
    }

    pub fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Ms {
        Ms(self.0.load(Ordering::SeqCst))
    }
}
```

- [ ] **Step 4: 实现 `exec.rs`**

```rust
//! 执行器抽象（spec §22.4「执行器抽象」；§13 Q12 的窄接口）。claude = M2b 的 `start_claude` + `SessionHandle`；
//! codex（M2c）/ ACP（M2e）合入时各加一个 `Executor` 实现，`Executors` 里登记一行。
//! 事件流直接用 crewd-exec 的 `SessionEvent`：M2c 给它加变体时，`core.rs` 的 match 会编不过——那正是要逐个决定怎么处理的地方。
use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use crewd_exec::driver::{Closed, SendError, SessionHandle, SessionStatus, StartError};
use crewd_exec::resume::AgentIdLedger;
use crewd_exec::session::SessionEvent;
use crewd_term::Snapshot;
use tokio::sync::mpsc;

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// codex 的 `additionalContext`（按 key 组装、去重；§4.3、§22.6 的 `ContextExtras` 口）。
pub type Context = BTreeMap<String, String>;

pub struct LaunchRequest {
    pub session_id: String,
    pub crew_id: String,
    pub workdir: PathBuf,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// 世界观（`collab::worldview::render`）。
    pub system_prompt: Option<String>,
    /// 开场正文（只走输入投递，绝不进 argv）。
    pub initial_prompt: Option<String>,
    /// helper 用来 `agent.attach` 的 token（M3b）。
    pub token: String,
    pub ledger: Arc<dyn AgentIdLedger>,
}

#[derive(Debug)]
pub enum LaunchError {
    /// 这种执行器还没接进 crewd（codex 在 M2c 合入前）。
    Unsupported(String),
    /// 本 crewd 没配置这个执行器的可执行文件。
    NotConfigured(String),
    Start(StartError),
    Failed(String),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(kind) => write!(f, "执行器 {kind} 还没接入 crewd"),
            Self::NotConfigured(why) | Self::Failed(why) => write!(f, "{why}"),
            Self::Start(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LaunchError {}

pub type Launched = (Arc<dyn ExecHandle>, mpsc::UnboundedReceiver<SessionEvent>);

pub trait Executor: Send + Sync {
    fn launch(&self, req: LaunchRequest) -> BoxFut<'_, Result<Launched, LaunchError>>;

    /// 续跑被拒时多说一句诊断：这个会话号的日志在不在盘上（只用来说明，不决定任何事；
    /// `AgentSessionResume.swift:136-163`）。
    fn locate_transcript(&self, _agent_session_id: &str) -> Option<PathBuf> {
        None
    }
}

/// §13 Q12 的窄接口（启动之外）。
pub trait ExecHandle: Send + Sync {
    /// `context` 只能在 `accepts_context()` 为真时非空。
    fn send(&self, text: String, context: &Context) -> Result<u64, SendError>;
    fn accepts_context(&self) -> bool {
        false
    }
    fn raw(&self, bytes: Vec<u8>) -> Result<(), Closed>;
    fn stop(&self) -> Result<(), Closed>;
    fn clear_quota_health(&self) -> Result<(), Closed>;
    fn screen_text(&self, max_lines: usize) -> BoxFut<'_, Result<String, Closed>>;
    fn snapshot(&self) -> BoxFut<'_, Result<Option<Snapshot>, Closed>>;
    fn status(&self) -> BoxFut<'_, Result<SessionStatus, Closed>>;
}

/// claude（M2b）。上下文由 hook 注入（M3b），不收 additionalContext。
impl ExecHandle for SessionHandle {
    fn send(&self, text: String, context: &Context) -> Result<u64, SendError> {
        assert!(context.is_empty(), "claude 不收 additionalContext：调用方必须先看 accepts_context()");
        SessionHandle::send(self, text)
    }

    fn raw(&self, bytes: Vec<u8>) -> Result<(), Closed> {
        SessionHandle::raw(self, bytes)
    }

    fn stop(&self) -> Result<(), Closed> {
        SessionHandle::stop(self)
    }

    fn clear_quota_health(&self) -> Result<(), Closed> {
        SessionHandle::clear_quota_health(self)
    }

    fn screen_text(&self, max_lines: usize) -> BoxFut<'_, Result<String, Closed>> {
        Box::pin(SessionHandle::screen_text(self, max_lines))
    }

    fn snapshot(&self) -> BoxFut<'_, Result<Option<Snapshot>, Closed>> {
        // M2c 合入后 `SessionHandle::snapshot` 自己返回 Option：去掉这里的 `.map(Some)`。
        Box::pin(async move { SessionHandle::snapshot(self).await.map(Some) })
    }

    fn status(&self) -> BoxFut<'_, Result<SessionStatus, Closed>> {
        Box::pin(SessionHandle::status(self))
    }
}

/// 执行器类型（`sessions.executor`：`claude` / `codex` / `acp:<id>`）→ 实现。
#[derive(Clone, Default)]
pub struct Executors(BTreeMap<String, Arc<dyn Executor>>);

impl Executors {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, kind: &str, executor: Arc<dyn Executor>) -> Self {
        self.0.insert(kind.to_string(), executor);
        self
    }

    pub fn get(&self, kind: &str) -> Result<Arc<dyn Executor>, LaunchError> {
        self.0
            .get(kind)
            .cloned()
            .ok_or_else(|| LaunchError::Unsupported(kind.to_string()))
    }
}
```

- [ ] **Step 5: 实现 `exec_claude.rs`**

先看 main 上 `crewd_exec::launch::CommsSpec` 现在的形状（`rg -n 'pub struct CommsSpec' -A14 crates/exec/src/launch.rs`）：有 `token` 字段 = M3b 已合入，`comms_for` 用写法 B；还是 `crew_id / captain / structured_post / label` = M3b 未合入，用写法 A。两种只差 `comms_for` 的函数体。

```rust
//! claude 执行器：M2b 的 `start_claude` 包一层。可执行文件三种来源（裁定见 Global Constraints）：
//! `Disabled`（库里 `serve()` 的缺省：绝不起任何 agent）、`Path`（`crewd serve --claude` / 测试的假 claude）、
//! `Discover`（`crewd serve` 不给路径时按登录 shell 的 PATH 找）。
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crewd_exec::driver::{ClaudeStart, DriverConfig, start_claude};
use crewd_exec::env::{AgentPaths, compose_child_path};
use crewd_exec::launch::{ClaudeSpec, CommsSpec};
use crewd_exec::registry::ProcessRegistry;
use crewd_exec::resume::valid_agent_session_id;

use super::exec::{BoxFut, ExecHandle, Executor, LaunchError, LaunchRequest, Launched};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ClaudeProgram {
    #[default]
    Disabled,
    Discover,
    Path(PathBuf),
}

pub struct ClaudeExecutorConfig {
    pub program: ClaudeProgram,
    pub data_dir: PathBuf,
    /// MCP / hook helper（生产 = crewd 自己的可执行文件）。
    pub helper: PathBuf,
    pub registry: Option<Arc<ProcessRegistry>>,
    /// 额外环境变量（测试给假 claude 传 `FAKE_CLAUDE_LOG`；仍过 crewd-exec 的禁用前缀）。
    pub extra_env: BTreeMap<String, String>,
    /// claude 的 `projects` 目录（认证探测与续跑被拒诊断）。
    pub projects_dir: Option<PathBuf>,
}

pub struct ClaudeExecutor {
    program: ClaudeProgram,
    data_dir: PathBuf,
    helper: PathBuf,
    registry: Option<Arc<ProcessRegistry>>,
    extra_env: BTreeMap<String, String>,
    projects_dir: Option<PathBuf>,
    paths: tokio::sync::OnceCell<AgentPaths>,
}

impl ClaudeExecutor {
    pub fn new(c: ClaudeExecutorConfig) -> Self {
        Self {
            program: c.program,
            data_dir: c.data_dir,
            helper: c.helper,
            registry: c.registry,
            extra_env: c.extra_env,
            projects_dir: c.projects_dir,
            paths: tokio::sync::OnceCell::new(),
        }
    }

    /// (claude 可执行文件, 子进程 PATH)。
    async fn program(&self) -> Result<(PathBuf, String), LaunchError> {
        match &self.program {
            ClaudeProgram::Disabled => Err(LaunchError::NotConfigured(
                "这个 crewd 没配置 claude（serve 没给 --claude，也没开自动查找）".into(),
            )),
            ClaudeProgram::Path(p) => {
                let parent = std::env::var("PATH").ok();
                Ok((p.clone(), compose_child_path(&[], parent.as_deref())))
            }
            ClaudeProgram::Discover => {
                let paths = self.paths.get_or_init(AgentPaths::discover).await;
                let program = paths
                    .resolve("claude")
                    .ok_or_else(|| LaunchError::NotConfigured(format!("PATH 里找不到 claude：{}", paths.child_path)))?;
                Ok((program, paths.child_path.clone()))
            }
        }
    }
}

impl Executor for ClaudeExecutor {
    fn launch(&self, req: LaunchRequest) -> BoxFut<'_, Result<Launched, LaunchError>> {
        Box::pin(async move {
            let (program, child_path) = self.program().await?;
            let mut config = DriverConfig::new(self.data_dir.join("run"));
            config.registry = self.registry.clone();
            config.claude_projects_dir = self.projects_dir.clone();
            let comms = comms_for(&self.helper, &self.data_dir, &req.session_id, &req.crew_id, &req.token);
            let spec = ClaudeSpec {
                workdir: req.workdir,
                model: req.model,
                effort: req.effort,
                fast_mode: None,
                system_prompt: req.system_prompt,
                initial_prompt: req.initial_prompt,
                comms: Some(comms),
                additional_env: self.extra_env.clone(),
            };
            let start = ClaudeStart {
                session_id: req.session_id,
                program,
                child_path,
                spec,
                ledger: req.ledger,
            };
            let (handle, events) = start_claude(start, config).await.map_err(LaunchError::Start)?;
            Ok((Arc::new(handle) as Arc<dyn ExecHandle>, events))
        })
    }

    fn locate_transcript(&self, agent_session_id: &str) -> Option<PathBuf> {
        // 会话号要当路径段用：先过 M2b 的字符集校验。
        if !valid_agent_session_id(agent_session_id) {
            return None;
        }
        let root = self.projects_dir.as_ref()?;
        std::fs::read_dir(root)
            .ok()?
            .flatten()
            .map(|e| e.path().join(format!("{agent_session_id}.jsonl")))
            .find(|p| p.is_file())
    }
}
```

`comms_for` 写法 A（M3b 未合入，`CommsSpec` 仍是 M2b 形状）：

```rust
/// 唯一拼 `CommsSpec` 的地方（裁定 18）。M3b 改了它的形状后，后合者只改这里。
pub fn comms_for(helper: &Path, data_dir: &Path, session_id: &str, crew_id: &str, token: &str) -> CommsSpec {
    let _ = token; // M3b 之前 helper 还不读 token
    CommsSpec {
        helper: helper.to_path_buf(),
        data_dir: data_dir.to_path_buf(),
        crew_id: crew_id.to_string(),
        session_id: session_id.to_string(),
        captain: false,
        structured_post: false,
        label: None,
        // M3b 之前 helper 没有 `hook` 子命令。
        hooks: false,
    }
}
```

`comms_for` 写法 B（M3b 已合入，`CommsSpec { helper, data_dir, session_id, token, hooks, approval_gates }`；审批不阻塞，没有等待上限字段）：

```rust
/// 唯一拼 `CommsSpec` 的地方（裁定 18）。`approval_gates` 来自 M3b 的 `AgentConfig.approval_gates`
/// （`serve_with` 建 `ClaudeExecutorConfig` 时从 `state.agent.config` 抄进 `approval_gates` 字段，调用处传进来）——一处配置，
/// claude 的 PreToolUse matcher 与 serve 的判定用同一份。
pub fn comms_for(helper: &Path, data_dir: &Path, session_id: &str, crew_id: &str, token: &str, approval_gates: &[String]) -> CommsSpec {
    let _ = crew_id; // M3b 起机组由 serve 现查，不进命令行
    CommsSpec {
        helper: helper.to_path_buf(),
        data_dir: data_dir.to_path_buf(),
        session_id: session_id.to_string(),
        token: token.to_string(),
        hooks: true,
        approval_gates: approval_gates.to_vec(),
    }
}
```

（写法 B 多一个参数：后合者同时给 `ClaudeExecutorConfig` 加 `approval_gates: Vec<String>`、在 `serve_with` 里填 `state.agent.config.approval_gates.clone()`、调用处传 `&self.cfg.approval_gates`——仍只动 M3c 的这三处。）

- [ ] **Step 6: 实现 `hooks.rs`**

```rust
//! 给后面的里程碑留的挂钩点（spec §22.4、§22.6）。M3c 交付的都是惰性桩：
//! - `Arbiter`：人发、没有 @ 的消息叫谁（M5b 换成 Jev + 别名层，§20）；`None` / 空集 = 交群主（§22.7 Q2）。
//! - `IdleHook`：idle 边沿、队列空时还要不要叫它一次（M4b 的「停下即检查」）。
//! - `AskAnsweredHook`：问被回答后（M4b 的 BlockerCleared 叫醒；`cleared` = M3a `Change::AskAnswered` 带来的 (工作项, 负责人)）。
//! - `QuotaSource`：额度重置时刻（`Health` 里没有；拿不到就按 45 分钟退避）。
//! - `ContextExtras`：codex 的 `additionalContext` 按 key 组装（M4b 注册 `my_work_items`）。
//! 它们都是 async：Jev 要联网、M4b 要读库。
use std::sync::Arc;

use crewd_wake::plan::{ChannelCtx, MsgFacts};
use crewd_wake::{Ms, NewWake};

use super::exec::{BoxFut, Context};
use super::worktree::{self, Worktrees};

pub struct ArbiterQuestion {
    pub message: MsgFacts,
    pub text: String,
    pub ctx: ChannelCtx,
}

pub trait Arbiter: Send + Sync {
    fn wake_set<'a>(&'a self, q: &'a ArbiterQuestion) -> BoxFut<'a, Option<Vec<String>>>;
}

/// M5b 之前的桩：永远没答案 → 群主。
pub struct NoArbiter;

impl Arbiter for NoArbiter {
    fn wake_set<'a>(&'a self, _q: &'a ArbiterQuestion) -> BoxFut<'a, Option<Vec<String>>> {
        Box::pin(async { None })
    }
}

pub trait IdleHook: Send + Sync {
    fn on_idle<'a>(&'a self, session: &'a str, now: Ms) -> BoxFut<'a, Option<NewWake>>;
}

pub struct NoIdleHook;

impl IdleHook for NoIdleHook {
    fn on_idle<'a>(&'a self, _session: &'a str, _now: Ms) -> BoxFut<'a, Option<NewWake>> {
        Box::pin(async { None })
    }
}

pub trait AskAnsweredHook: Send + Sync {
    fn on_answered<'a>(
        &'a self,
        crew: &'a str,
        ask: &'a str,
        cleared: &'a [(String, Option<String>)],
        now: Ms,
    ) -> BoxFut<'a, ()>;
}

pub struct NoAskHook;

impl AskAnsweredHook for NoAskHook {
    fn on_answered<'a>(
        &'a self,
        _crew: &'a str,
        _ask: &'a str,
        _cleared: &'a [(String, Option<String>)],
        _now: Ms,
    ) -> BoxFut<'a, ()> {
        Box::pin(async {})
    }
}

pub trait QuotaSource: Send + Sync {
    fn reset_at<'a>(&'a self, executor: &'a str, now: Ms) -> BoxFut<'a, Option<Ms>>;
}

pub struct NoQuotaSource;

impl QuotaSource for NoQuotaSource {
    fn reset_at<'a>(&'a self, _executor: &'a str, _now: Ms) -> BoxFut<'a, Option<Ms>> {
        Box::pin(async { None })
    }
}

pub trait ContextExtras: Send + Sync {
    fn extras<'a>(&'a self, session: &'a str) -> BoxFut<'a, Context>;
}

pub struct NoExtras;

impl ContextExtras for NoExtras {
    fn extras<'a>(&'a self, _session: &'a str) -> BoxFut<'a, Context> {
        Box::pin(async { Context::new() })
    }
}

#[derive(Clone)]
pub struct Hooks {
    pub arbiter: Arc<dyn Arbiter>,
    pub idle: Arc<dyn IdleHook>,
    pub ask: Arc<dyn AskAnsweredHook>,
    pub quota: Arc<dyn QuotaSource>,
    pub extras: Arc<dyn ContextExtras>,
    /// `isolation: true` 时建 git worktree（控制者裁定 C8）；默认真 git，测试换 `StandInWorktrees`。
    pub worktrees: Arc<dyn Worktrees>,
}

impl Default for Hooks {
    fn default() -> Self {
        Self {
            arbiter: Arc::new(NoArbiter),
            idle: Arc::new(NoIdleHook),
            ask: Arc::new(NoAskHook),
            quota: Arc::new(NoQuotaSource),
            extras: Arc::new(NoExtras),
            worktrees: worktree::real(),
        }
    }
}
```

- [ ] **Step 6b: 实现 `worktree.rs`（控制者裁定 C8：`isolation: true` 时新建 git worktree；crewd 只建、永不删）**

照 Swift：`Sources/Mac/LocalRunner/SessionWorkspace.swift:20-34`（流程与分支名）、`GitWorktreeService.swift:124-158`（位置与目录名）、`:166-185`（`.git/info/exclude`）、`:197-220`（slug）。逐条：机组目录必须是 git 仓库（`rev-parse --is-inside-work-tree` 为 `true`），否则报错；当前分支取 `symbolic-ref --quiet --short HEAD`，detached HEAD 报错；目录 = `<repo 根>/.pendingcrew/worktrees/<slug>-<8 位 hex>`（repo 根 = `rev-parse --show-toplevel`；slug = `hint` 里 `[A-Za-z0-9_-]` 以外的字符换成 `-`、连续 `-` 合一、头尾去 `-`、截 32 字符、空则 `crew`；`hint` = 开场 brief，空串则 `session`）；新分支 `pendingcrew/session-<6 位 hex>`，从机组目录的当前分支起：`git worktree add -b <新分支> <目录> <当前分支>`，在 repo 根里跑；容器目录 `mkdir -p`，并把 `/.pendingcrew/` 追加进 `<repo 根>/.git/info/exclude`（文件存在且还没有这一行才写；本地、不污染仓库）。任何一步失败都回 `Err(原因)`，**绝不退回共享目录**（Swift `SessionWorkspace.swift:7-9`、`McpServer.swift:568` 的同一条原则）。

```rust
//! 为 `isolation: true` 的 session 新建 git worktree（控制者裁定 C8，照 Swift 的 `SessionWorkspace` / `GitWorktreeService`）。
//! 只建、永不删：删除是人的动作（crewd 不碰 `git worktree remove`，也不删分支）。
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use super::exec::BoxFut;

pub trait Worktrees: Send + Sync {
    /// 在 `crew_dir`（机组工作目录）所在的仓库里新建 worktree；`hint` = 目录名前缀（开场 brief，空串则 `session`）。
    /// 返回新目录的绝对路径。失败 = `Err(给人看的原因)`，什么都不该留下来要人收拾的（除了 git 自己在 `worktree add` 中途失败时的残留，那归 git）。
    fn create<'a>(&'a self, crew_dir: &'a Path, hint: &'a str) -> BoxFut<'a, Result<PathBuf, String>>;
}

/// 生产实现：shell 出 `git`。同步的 `Command` 放进 `spawn_blocking`，不堵 actor。
pub struct GitWorktrees;

impl Worktrees for GitWorktrees {
    fn create<'a>(&'a self, crew_dir: &'a Path, hint: &'a str) -> BoxFut<'a, Result<PathBuf, String>> {
        let (dir, hint) = (crew_dir.to_path_buf(), hint.to_string());
        Box::pin(async move {
            tokio::task::spawn_blocking(move || create_blocking(&dir, &hint))
                .await
                .map_err(|e| format!("新建 worktree 的任务崩了：{e}"))?
        })
    }
}

struct Out {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn git(cwd: &Path, args: &[&str]) -> Result<Out, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        // 从 git hook / 别的仓库里被调起时，这几个会把 git 指到错的仓库。
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .map_err(|e| format!("跑不了 git（{e}）；请确认 git 在 PATH 上"))?;
    Ok(Out {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
    })
}

fn create_blocking(crew_dir: &Path, hint: &str) -> Result<PathBuf, String> {
    let shown = crew_dir.display();
    let inside = git(crew_dir, &["rev-parse", "--is-inside-work-tree"])?;
    if !(inside.ok && inside.stdout == "true") {
        return Err(format!("无法为 session 新建 worktree：{shown} 不是 git 仓库"));
    }
    let head = git(crew_dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if !head.ok || head.stdout.is_empty() {
        return Err(format!("无法为 session 新建 worktree：{shown} 当前是 detached HEAD"));
    }
    let top = git(crew_dir, &["rev-parse", "--show-toplevel"])?;
    if !top.ok || top.stdout.is_empty() {
        return Err(format!("无法为 session 新建 worktree：查不到 {shown} 的仓库根（{}）", top.stderr));
    }
    let root = PathBuf::from(top.stdout);
    let container = root.join(".pendingcrew").join("worktrees");
    std::fs::create_dir_all(&container).map_err(|e| format!("建不了 {}：{e}", container.display()))?;
    ensure_local_exclude(&root);
    let hex = |n: usize| uuid::Uuid::new_v4().simple().to_string()[..n].to_string();
    let hint = if hint.is_empty() { "session" } else { hint };
    let target = container.join(format!("{}-{}", slug(hint), hex(8)));
    let branch = format!("pendingcrew/session-{}", hex(6));
    let add = git(&root, &["worktree", "add", "-b", &branch, &target.display().to_string(), &head.stdout])?;
    if !add.ok {
        return Err(format!("git worktree add 失败：{}", add.stderr));
    }
    Ok(target)
}

/// 把 `/.pendingcrew/` 记进仓库本地的 exclude（Swift `ensureLocalGitignore`：文件不存在就跳过，不冒险写错地方）。
fn ensure_local_exclude(root: &Path) {
    use std::io::Write;
    let exclude = root.join(".git").join("info").join("exclude");
    let needle = "/.pendingcrew/";
    if !exclude.is_file() || std::fs::read_to_string(&exclude).map(|c| c.contains(needle)).unwrap_or(false) {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&exclude) {
        let _ = f.write_all(format!("\n# Added by crewd — worktree scratch dir.\n{needle}\n").as_bytes());
    }
}

fn slug(raw: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
            last_dash = ch == '-';
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let s: String = out.trim_matches('-').chars().take(32).collect();
    if s.is_empty() { "crew".into() } else { s }
}

/// 默认挂真实现（`Hooks::default()`）。
pub fn real() -> Arc<dyn Worktrees> {
    Arc::new(GitWorktrees)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 合成仓库：临时目录里 `git init` + 一次空提交；绝不碰真仓库。
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let o = git(dir.path(), args).unwrap();
            assert!(o.ok, "git {args:?}：{}", o.stderr);
        };
        run(&["init", "-b", "main"]);
        run(&["-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "--allow-empty", "-m", "init"]);
        dir
    }

    #[tokio::test]
    async fn creates_a_worktree_on_a_new_branch_under_dot_pendingcrew() {
        let r = repo();
        let root = r.path().canonicalize().unwrap(); // macOS：/var → /private/var，git 报的是真路径
        let p = GitWorktrees.create(&root, "fix scroll 补测试").await.unwrap();
        assert!(p.starts_with(root.join(".pendingcrew").join("worktrees")), "{p:?}");
        assert!(p.file_name().unwrap().to_string_lossy().starts_with("fix-scroll-"), "{p:?}");
        assert!(p.is_dir());
        let b = git(&p, &["symbolic-ref", "--short", "HEAD"]).unwrap().stdout;
        assert!(b.starts_with("pendingcrew/session-") && b.len() == "pendingcrew/session-".len() + 6, "{b}");
        assert!(std::fs::read_to_string(root.join(".git/info/exclude")).unwrap().contains("/.pendingcrew/"));
        let again = GitWorktrees.create(&root, "fix scroll 补测试").await.unwrap();
        assert_ne!(p, again, "目录名带随机后缀，不撞");
        assert_eq!(git(&root, &["status", "--porcelain"]).unwrap().stdout, "", "exclude 生效：主工作区不脏");
    }

    #[tokio::test]
    async fn starts_from_a_subdirectory_of_the_repo() {
        let r = repo();
        let root = r.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let p = GitWorktrees.create(&root.join("sub"), "").await.unwrap();
        assert!(p.starts_with(root.join(".pendingcrew/worktrees")) && p.file_name().unwrap().to_string_lossy().starts_with("session-"), "{p:?}");
    }

    #[tokio::test]
    async fn refuses_non_repos_and_detached_head_and_never_falls_back() {
        let plain = tempfile::tempdir().unwrap();
        let e = GitWorktrees.create(plain.path(), "x").await.unwrap_err();
        assert!(e.contains("不是 git 仓库"), "{e}");
        assert!(!plain.path().join(".pendingcrew").exists());
        let r = repo();
        let root = r.path().canonicalize().unwrap();
        assert!(git(&root, &["checkout", "--detach"]).unwrap().ok);
        let e = GitWorktrees.create(&root, "x").await.unwrap_err();
        assert!(e.contains("detached HEAD"), "{e}");
        assert!(!root.join(".pendingcrew").exists(), "失败不留半成品");
    }

    #[test]
    fn slug_follows_the_swift_rules() {
        assert_eq!(slug("fix scroll 补测试"), "fix-scroll");
        assert_eq!(slug("补测试"), "crew");
        assert_eq!(slug("--a__b--"), "a__b");
        assert_eq!(slug(&"a".repeat(40)).len(), 32);
        assert_eq!(slug("a   b"), "a-b");
    }
}
```

`Cargo.toml`：`uuid`（`v4`）已在 `crewd-server` 依赖里则不动；没有就 `cargo add --offline -p crewd-server uuid -F v4`（锁里已有该 crate，M3a collab 在用）；`tempfile` 已是 dev 依赖（M3a）。`GitWorktrees` 的测试要求 `git` 在 PATH 上。

- [ ] **Step 7: 实现 `port.rs`**

```rust
//! 编排读写数据的唯一出口。生产实现 `db::DbPort`（全部写经 M3a 冻结的 `crewd_collab` 函数；M3c 不建表），
//! 测试实现 `testkit::MemPort`。M3a 的实际名字只在 `db.rs` 里出现。
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use crewd_exec::health::Health;
use crewd_exec::resume::AgentIdLedger;
use crewd_term::PendingDecision;
use crewd_wake::plan::{ChannelCtx, MsgFacts};
use crewd_wake::{Latch, LatchScope, MessageRef, Ms, NewWake, QueuedWake};

pub type Result<T> = anyhow::Result<T>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desired {
    Running,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRow {
    pub id: String,
    pub crew_id: String,
    pub captain: bool,
    /// `claude` / `codex` / `acp:<id>`（§16）。
    pub executor: String,
    pub display_name: Option<String>,
    pub agent_session_id: Option<String>,
    /// 解析好的工作目录：`worktree` → session 自己的 → 机组的；都没有 = `None`（起不了）。
    pub workdir: Option<PathBuf>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub brief: Option<String>,
    pub desired: Desired,
    pub was_working: bool,
    pub last_active_at: Option<Ms>,
    pub resume_failed_at: Option<Ms>,
    pub awaiting_question: Option<String>,
    /// 最近一次「产出」（M3a `sessions.last_progress_at`：工具调用 / 工作项变化，`note_progress` 写；发言不写，C3）。
    pub last_progress_at: Option<Ms>,
}

impl SessionRow {
    pub fn name(&self) -> String {
        self.display_name
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| self.id.chars().take(8).collect())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewSession {
    pub crew_id: String,
    pub captain: bool,
    pub executor: String,
    pub brief: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// crewd 为它新建的 git worktree 的绝对路径（`isolation: true`，控制者裁定 C8；`Core::start_new` 建好后填）；None = 共享机组目录。
    pub worktree: Option<PathBuf>,
}

/// 只改给了的列（§18 遗留：`sessions::upsert` 传 None 会抹掉会话号——这里不碰没给的列）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionPatch {
    pub desired: Option<Desired>,
    pub was_working: Option<bool>,
    pub last_active_at: Option<Ms>,
    pub resume_failed_at: Option<Option<Ms>>,
    pub model: Option<Option<String>>,
    pub effort: Option<Option<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settle {
    Inflight,
    Delivered,
    /// 终局失败、还要再试：回 pending、次数 +1、`not_before = retry_at`（M3a `mark_failed(.., Some(retry_at))`，持久化退避）。
    Retry { attempts: u32, error: String, retry_at: Ms },
    Dropped { error: String },
    /// inflight → pending，次数不变（进程没了、重启）。
    Requeue,
}

/// 系统消息发到哪、@ 谁。群主由 Port 按 M3a 的规则解析（大厅 / 人类群的群主 = 机组机长）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Audience {
    /// 这个机组所属根机组的人类群，@人。
    HumanChannel { crew: String },
    Lobby { crew: String, mention_owner: bool },
    Channel { channel: String, mention_owner: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveView {
    pub session_id: String,
    pub running: bool,
    pub working: bool,
    pub health: Option<Health>,
    pub pending_decision: Option<PendingDecision>,
    pub awaiting_question: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    /// 渲染不出来、已被记 dropped 的唤醒 id。
    pub dropped: Vec<i64>,
}

/// M3c 自己发的事件（M3a 的协作事件由 collab 发）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrchestraEvent {
    SessionChanged(LiveView),
    WakeChanged { session: String },
}

pub trait Port: Send + Sync + 'static {
    // —— sessions 行
    fn session(&self, id: &str) -> impl Future<Output = Result<Option<SessionRow>>> + Send;
    fn sessions_desired_running(&self) -> impl Future<Output = Result<Vec<SessionRow>>> + Send;
    fn all_session_ids(&self) -> impl Future<Output = Result<Vec<String>>> + Send;
    fn insert_session(&self, s: NewSession) -> impl Future<Output = Result<SessionRow>> + Send;
    /// 机组的工作目录（`isolation: true` 建 worktree 的起点，C8）；没有 = None。
    fn crew_workdir(&self, crew: &str) -> impl Future<Output = Result<Option<PathBuf>>> + Send;
    fn patch_session(&self, id: &str, p: SessionPatch) -> impl Future<Output = Result<()>> + Send;
    /// 换机长；返回原机长（没有 / 就是它 → None）。
    fn set_captain(&self, crew: &str, session: &str) -> impl Future<Output = Result<Option<String>>> + Send;
    /// 机组此刻的机长（= 大厅群主）。
    fn captain(&self, crew: &str) -> impl Future<Output = Result<Option<String>>> + Send;
    fn ledger(&self, session: &str) -> Arc<dyn AgentIdLedger>;
    fn worldview(&self, session: &str) -> impl Future<Output = Result<String>> + Send;
    fn mint_token(&self, session: &str) -> String;
    // —— 唤醒队列与熔断
    /// 插入成功 = true；`UNIQUE(session, kind, source)` 撞了 = false。
    fn enqueue(&self, w: NewWake) -> impl Future<Output = Result<bool>> + Send;
    /// 该 session 所有 `pending` 的唤醒（不论到没到点）。
    fn pending(&self, session: &str) -> impl Future<Output = Result<Vec<QueuedWake>>> + Send;
    fn settle(&self, ids: &[i64], s: Settle, now: Ms) -> impl Future<Output = Result<()>> + Send;
    fn requeue_inflight(&self) -> impl Future<Output = Result<usize>> + Send;
    /// 有到期 pending 的 session：按其中最高优先级降序、再按最早一条的 id 升序。
    fn due_sessions(&self, now: Ms) -> impl Future<Output = Result<Vec<String>>> + Send;
    /// 晚于 `now` 的最早 `not_before`（已到期的不算，免得忙等）。
    fn next_not_before_after(&self, now: Ms) -> impl Future<Output = Result<Option<Ms>>> + Send;
    fn list_wakes(&self, session: Option<&str>) -> impl Future<Output = Result<Vec<QueuedWake>>> + Send;
    fn latches(&self) -> impl Future<Output = Result<Vec<Latch>>> + Send;
    fn put_latch(&self, l: &Latch) -> impl Future<Output = Result<()>> + Send;
    fn clear_latch(&self, scope: LatchScope, id: &str) -> impl Future<Output = Result<()>> + Send;
    // —— 设置（M3a `collab::settings`）
    /// 可选并发上限（§22.7 Q5：缺省 None = 不设）。
    fn max_live(&self) -> impl Future<Output = Result<Option<usize>>> + Send;
    fn set_max_live(&self, max: Option<usize>) -> impl Future<Output = Result<()>> + Send;
    fn locale(&self) -> impl Future<Output = Result<crewd_wake::text::Lang>> + Send;
    // —— 重启询问与默认方案（M3a `collab::restart` / `collab::asks`）
    /// 开一次「是否恢复这些 session」的询问（M3a 在相关根机组人类群各发一条 @人）。返回询问 id；名单空 = None。
    fn open_restart(&self, session_ids: Vec<String>) -> impl Future<Output = Result<Option<i64>>> + Send;
    /// 各机组到点的默认方案按默认办（M3a `asks::apply_due_defaults`；控制者裁定由 M3c 的定时拍调）。
    fn apply_due_defaults(&self) -> impl Future<Output = Result<()>> + Send;
    /// 删掉 `before` 之前就已 delivered / dropped 的队列行（M3a `wakes::prune`；控制者裁定 C6：清理归 M3c 的定时拍）。
    fn prune_wakes(&self, before: Ms) -> impl Future<Output = Result<usize>> + Send;
    // —— 协作数据（M3a 的 Collab）
    /// 消息的规划要素 + 正文（正文只给 Arbiter）。
    fn message(&self, r: &MessageRef) -> impl Future<Output = Result<Option<(MsgFacts, String)>>> + Send;
    fn channel_ctx(&self, channel: &str) -> impl Future<Output = Result<Option<ChannelCtx>>> + Send;
    /// 该 session 所在各群里游标之后的消息。
    fn unread(&self, session: &str) -> impl Future<Output = Result<Vec<MsgFacts>>> + Send;
    fn cursor(&self, session: &str, channel: &str) -> impl Future<Output = Result<i64>> + Send;
    /// 只往前推。
    fn advance(&self, session: &str, channel: &str, seq: i64) -> impl Future<Output = Result<()>> + Send;
    fn post_system(&self, to: Audience, text: String) -> impl Future<Output = Result<()>> + Send;
    /// 唤醒正文（`collab::render::{build_batch, wake_text}`）。渲染不出来的行（消息已删、问已撤回）由实现记 dropped，
    /// 不出现在正文里，并在 `Rendered::dropped` 里报回（调用方把它们从这一批里拿掉）。
    fn render_wake(&self, session: &str, batch: &[QueuedWake]) -> impl Future<Output = Result<Rendered>> + Send;
    fn publish(&self, e: OrchestraEvent);
}
```

- [ ] **Step 8: 实现 `testkit.rs`（测试器材，`#[cfg(test)]`）**

```rust
//! 测试器材：内存里的 Port 与顶替执行器。只在 `cargo test` 里编译。
#![allow(dead_code)]
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crewd_exec::driver::{Closed, SendError, SessionStatus};
use crewd_exec::resume::{AgentIdLedger, MemoryLedger};
use crewd_exec::session::SessionEvent;
use crewd_term::Snapshot;
use crewd_wake::plan::{ChannelCtx, Listen, MsgFacts};
use crewd_wake::queue::is_due;
use crewd_wake::{Latch, LatchScope, MessageRef, Ms, NewWake, QueuedWake, WakeKind, WakeState};
use tokio::sync::mpsc;

use super::exec::{BoxFut, Context, ExecHandle, Executor, LaunchError, LaunchRequest, Launched};
use super::port::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemKind {
    Lobby,
    Human,
    Other,
}

#[derive(Clone, Debug)]
pub struct MemChannel {
    pub crew: String,
    pub kind: MemKind,
    /// 只对 `Other` 有意义；大厅 / 人类群的群主 = 机组机长。
    pub owner: Option<String>,
    pub members: Vec<String>,
}

#[derive(Default)]
pub struct MemState {
    pub sessions: BTreeMap<String, SessionRow>,
    pub wakes: Vec<QueuedWake>,
    pub latches: Vec<Latch>,
    pub channels: BTreeMap<String, MemChannel>,
    pub messages: BTreeMap<(String, i64), (MsgFacts, String)>,
    pub cursors: BTreeMap<(String, String), i64>,
    pub listens: BTreeMap<String, Vec<Listen>>,
    pub posts: Vec<(Audience, String)>,
    pub events: Vec<OrchestraEvent>,
    pub max_live: Option<usize>,
    pub restarts: Vec<Vec<String>>,
    pub defaults_applied: usize,
    pub prunes: usize,
    pub ledgers: BTreeMap<String, Arc<MemoryLedger>>,
    /// 机组 → 工作目录（`Port::crew_workdir`）。
    pub crew_dirs: BTreeMap<String, PathBuf>,
    pub next_wake: i64,
    pub next_session: u64,
    /// 测试里的「现在」（入队时间）。
    pub now: Ms,
}

pub struct MemPort {
    pub st: Mutex<MemState>,
}

impl MemPort {
    pub fn new() -> Self {
        Self { st: Mutex::new(MemState::default()) }
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, MemState> {
        self.st.lock().expect("测试器材锁中毒")
    }

    fn captain_of(st: &MemState, crew: &str) -> Option<String> {
        st.sessions.values().find(|s| s.crew_id == crew && s.captain).map(|s| s.id.clone())
    }

    fn ctx_of(st: &MemState, channel: &str) -> Option<ChannelCtx> {
        let ch = st.channels.get(channel)?;
        let captain = Self::captain_of(st, &ch.crew);
        let (members, owner) = match ch.kind {
            MemKind::Lobby => (
                st.sessions.values().filter(|s| s.crew_id == ch.crew).map(|s| s.id.clone()).collect(),
                captain,
            ),
            MemKind::Human => {
                let mut m: Vec<String> = captain.iter().cloned().collect();
                m.extend(ch.members.iter().cloned());
                (m, captain)
            }
            MemKind::Other => (ch.members.clone(), ch.owner.clone()),
        };
        Some(ChannelCtx {
            session_members: members,
            owner,
            listens: st.listens.get(channel).cloned().unwrap_or_default(),
        })
    }

    pub fn wakes_of(&self, session: &str) -> Vec<QueuedWake> {
        self.lock().wakes.iter().filter(|w| w.session == session).cloned().collect()
    }

    pub fn posts(&self) -> Vec<(Audience, String)> {
        self.lock().posts.clone()
    }

    pub fn row(&self, id: &str) -> SessionRow {
        self.lock().sessions.get(id).cloned().expect("测试里先建 session")
    }
}

impl Port for MemPort {
    async fn session(&self, id: &str) -> Result<Option<SessionRow>> {
        Ok(self.lock().sessions.get(id).cloned())
    }

    async fn sessions_desired_running(&self) -> Result<Vec<SessionRow>> {
        Ok(self.lock().sessions.values().filter(|s| s.desired == Desired::Running).cloned().collect())
    }

    async fn all_session_ids(&self) -> Result<Vec<String>> {
        Ok(self.lock().sessions.keys().cloned().collect())
    }

    async fn crew_workdir(&self, crew: &str) -> Result<Option<PathBuf>> {
        Ok(self.lock().crew_dirs.get(crew).cloned())
    }

    async fn insert_session(&self, s: NewSession) -> Result<SessionRow> {
        let mut st = self.lock();
        st.next_session += 1;
        let shared = st.crew_dirs.get(&s.crew_id).cloned();
        let row = SessionRow {
            id: format!("new-{}", st.next_session),
            crew_id: s.crew_id,
            captain: s.captain,
            executor: s.executor,
            display_name: s.title,
            agent_session_id: None,
            // worktree（isolation=true 建好的）→ 机组共享目录。
            workdir: Some(s.worktree.or(shared).unwrap_or_else(|| PathBuf::from("/nonexistent/wd"))),
            model: s.model,
            effort: s.effort,
            brief: Some(s.brief),
            desired: Desired::Stopped,
            was_working: false,
            last_active_at: None,
            resume_failed_at: None,
            awaiting_question: None,
            last_progress_at: None,
        };
        st.sessions.insert(row.id.clone(), row.clone());
        Ok(row)
    }

    async fn patch_session(&self, id: &str, p: SessionPatch) -> Result<()> {
        let mut st = self.lock();
        let row = st.sessions.get_mut(id).ok_or_else(|| anyhow::anyhow!("没有 session {id}"))?;
        if let Some(v) = p.desired {
            row.desired = v;
        }
        if let Some(v) = p.was_working {
            row.was_working = v;
        }
        if let Some(v) = p.last_active_at {
            row.last_active_at = Some(v);
        }
        if let Some(v) = p.resume_failed_at {
            row.resume_failed_at = v;
        }
        if let Some(v) = p.model {
            row.model = v;
        }
        if let Some(v) = p.effort {
            row.effort = v;
        }
        Ok(())
    }

    async fn set_captain(&self, crew: &str, session: &str) -> Result<Option<String>> {
        let mut st = self.lock();
        let prev = Self::captain_of(&st, crew).filter(|p| p != session);
        for s in st.sessions.values_mut().filter(|s| s.crew_id == crew) {
            s.captain = s.id == session;
        }
        Ok(prev)
    }

    async fn captain(&self, crew: &str) -> Result<Option<String>> {
        Ok(Self::captain_of(&self.lock(), crew))
    }

    fn ledger(&self, session: &str) -> Arc<dyn AgentIdLedger> {
        self.lock().ledgers.entry(session.to_string()).or_default().clone()
    }

    async fn worldview(&self, session: &str) -> Result<String> {
        Ok(format!("WORLD {session}"))
    }

    fn mint_token(&self, session: &str) -> String {
        format!("tok-{session}")
    }

    async fn enqueue(&self, w: NewWake) -> Result<bool> {
        let mut st = self.lock();
        if st.wakes.iter().any(|q| q.session == w.session && q.kind == w.kind && q.source == w.source) {
            return Ok(false);
        }
        st.next_wake += 1;
        let (id, now) = (st.next_wake, st.now);
        // `msg:<id>` → 那条消息的位置（同 M3a：消息 id 唯一）。
        let msg = w.source.strip_prefix("msg:").and_then(|mid| {
            st.messages.values().find(|(m, _)| m.id == mid).map(|(m, _)| m.at.clone())
        });
        st.wakes.push(QueuedWake {
            id,
            session: w.session,
            kind: w.kind,
            source: w.source,
            priority: w.priority,
            not_before: w.not_before,
            note: w.note,
            state: WakeState::Pending,
            attempts: 0,
            created_at: now,
            origin: w.origin,
            msg,
        });
        Ok(true)
    }

    async fn pending(&self, session: &str) -> Result<Vec<QueuedWake>> {
        Ok(self
            .lock()
            .wakes
            .iter()
            .filter(|w| w.session == session && w.state == WakeState::Pending)
            .cloned()
            .collect())
    }

    async fn settle(&self, ids: &[i64], s: Settle, _now: Ms) -> Result<()> {
        let mut st = self.lock();
        for w in st.wakes.iter_mut().filter(|w| ids.contains(&w.id)) {
            match &s {
                Settle::Inflight => w.state = WakeState::Inflight,
                Settle::Delivered => w.state = WakeState::Delivered,
                Settle::Retry { attempts, retry_at, .. } => {
                    w.state = WakeState::Pending;
                    w.attempts = *attempts;
                    w.not_before = *retry_at;
                }
                Settle::Dropped { .. } => w.state = WakeState::Dropped,
                Settle::Requeue => {
                    if w.state == WakeState::Inflight {
                        w.state = WakeState::Pending;
                    }
                }
            }
        }
        Ok(())
    }

    async fn requeue_inflight(&self) -> Result<usize> {
        let mut st = self.lock();
        let mut n = 0;
        for w in st.wakes.iter_mut().filter(|w| w.state == WakeState::Inflight) {
            w.state = WakeState::Pending;
            n += 1;
        }
        Ok(n)
    }

    async fn due_sessions(&self, now: Ms) -> Result<Vec<String>> {
        let st = self.lock();
        let mut best: BTreeMap<String, (crewd_wake::Priority, i64)> = BTreeMap::new();
        for w in st.wakes.iter().filter(|w| is_due(w, now)) {
            let e = best.entry(w.session.clone()).or_insert((w.priority, w.id));
            e.0 = e.0.max(w.priority);
            e.1 = e.1.min(w.id);
        }
        let mut v: Vec<(String, (crewd_wake::Priority, i64))> = best.into_iter().collect();
        v.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.1.1.cmp(&b.1.1)));
        Ok(v.into_iter().map(|(s, _)| s).collect())
    }

    async fn next_not_before_after(&self, now: Ms) -> Result<Option<Ms>> {
        Ok(self
            .lock()
            .wakes
            .iter()
            .filter(|w| w.state == WakeState::Pending && w.not_before > now)
            .map(|w| w.not_before)
            .min())
    }

    async fn list_wakes(&self, session: Option<&str>) -> Result<Vec<QueuedWake>> {
        Ok(self.lock().wakes.iter().filter(|w| session.is_none_or(|s| w.session == s)).cloned().collect())
    }

    async fn latches(&self) -> Result<Vec<Latch>> {
        Ok(self.lock().latches.clone())
    }

    async fn put_latch(&self, l: &Latch) -> Result<()> {
        let mut st = self.lock();
        st.latches.retain(|x| (x.scope, &x.id) != (l.scope, &l.id));
        st.latches.push(l.clone());
        Ok(())
    }

    async fn clear_latch(&self, scope: LatchScope, id: &str) -> Result<()> {
        self.lock().latches.retain(|x| !(x.scope == scope && x.id == id));
        Ok(())
    }

    async fn max_live(&self) -> Result<Option<usize>> {
        Ok(self.lock().max_live)
    }

    async fn set_max_live(&self, max: Option<usize>) -> Result<()> {
        self.lock().max_live = max;
        Ok(())
    }

    async fn locale(&self) -> Result<crewd_wake::text::Lang> {
        Ok(crewd_wake::text::Lang::Zh)
    }

    async fn open_restart(&self, session_ids: Vec<String>) -> Result<Option<i64>> {
        let mut st = self.lock();
        if session_ids.is_empty() {
            return Ok(None);
        }
        st.restarts.push(session_ids);
        Ok(Some(st.restarts.len() as i64))
    }

    async fn apply_due_defaults(&self) -> Result<()> {
        self.lock().defaults_applied += 1;
        Ok(())
    }

    async fn prune_wakes(&self, _before: Ms) -> Result<usize> {
        // MemPort 不记 settled 时刻：只数调用次数（测试只核「定时拍会调」）。
        self.lock().prunes += 1;
        Ok(0)
    }

    async fn message(&self, r: &MessageRef) -> Result<Option<(MsgFacts, String)>> {
        Ok(self.lock().messages.get(&(r.channel.clone(), r.seq)).cloned())
    }

    async fn channel_ctx(&self, channel: &str) -> Result<Option<ChannelCtx>> {
        Ok(Self::ctx_of(&self.lock(), channel))
    }

    async fn unread(&self, session: &str) -> Result<Vec<MsgFacts>> {
        let st = self.lock();
        Ok(st
            .messages
            .values()
            .filter(|(m, _)| {
                let ctx = Self::ctx_of(&st, &m.at.channel);
                let member = ctx.is_some_and(|c| c.session_members.iter().any(|s| s == session));
                let cursor = st.cursors.get(&(session.to_string(), m.at.channel.clone())).copied().unwrap_or(0);
                member && m.at.seq > cursor
            })
            .map(|(m, _)| m.clone())
            .collect())
    }

    async fn cursor(&self, session: &str, channel: &str) -> Result<i64> {
        Ok(self.lock().cursors.get(&(session.to_string(), channel.to_string())).copied().unwrap_or(0))
    }

    async fn advance(&self, session: &str, channel: &str, seq: i64) -> Result<()> {
        let mut st = self.lock();
        let c = st.cursors.entry((session.to_string(), channel.to_string())).or_insert(0);
        *c = (*c).max(seq);
        Ok(())
    }

    async fn post_system(&self, to: Audience, text: String) -> Result<()> {
        self.lock().posts.push((to, text));
        Ok(())
    }

    async fn render_wake(&self, _session: &str, batch: &[QueuedWake]) -> Result<Rendered> {
        let st = self.lock();
        let parts: Vec<String> = batch
            .iter()
            .filter_map(|w| match w.kind {
                WakeKind::Message => w
                    .msg
                    .as_ref()
                    .and_then(|r| st.messages.get(&(r.channel.clone(), r.seq)))
                    .map(|(_, text)| text.clone()),
                _ => Some(w.note.clone().unwrap_or_else(|| format!("{}:{}", w.kind, w.source))),
            })
            .collect();
        Ok(Rendered { text: parts.join("\n---\n"), dropped: vec![] })
    }

    fn publish(&self, e: OrchestraEvent) {
        self.lock().events.push(e);
    }
}

#[derive(Clone, Debug)]
pub struct LaunchRecord {
    pub session: String,
    pub workdir: PathBuf,
    pub initial: Option<String>,
    pub system_prompt: Option<String>,
    pub token: String,
}

#[derive(Default)]
pub struct StandInState {
    pub launches: Vec<LaunchRecord>,
    /// 依次让下几次 launch 失败（原因）。
    pub fail_next: Vec<String>,
    pub sends: BTreeMap<String, Vec<(u64, String)>>,
    pub raws: BTreeMap<String, Vec<Vec<u8>>>,
    pub stops: Vec<String>,
    pub cleared_quota: Vec<String>,
    pub events: BTreeMap<String, mpsc::UnboundedSender<SessionEvent>>,
    pub transcripts: BTreeMap<String, PathBuf>,
    pub next_id: u64,
}

/// 顶替执行器：记下收到的一切；事件由测试经 [`StandIn::emit`] 推。launch 成功时自带一条 `Spawned`。
#[derive(Clone, Default)]
pub struct StandIn(pub Arc<Mutex<StandInState>>);

impl StandIn {
    pub fn lock(&self) -> std::sync::MutexGuard<'_, StandInState> {
        self.0.lock().expect("测试器材锁中毒")
    }

    pub fn emit(&self, session: &str, e: SessionEvent) {
        let tx = self.lock().events.get(session).cloned().expect("先 launch 再推事件");
        tx.send(e).expect("事件泵还在");
    }

    pub fn sent(&self, session: &str) -> Vec<String> {
        self.lock().sends.get(session).map(|v| v.iter().map(|(_, t)| t.clone()).collect()).unwrap_or_default()
    }

    pub fn last_send_id(&self, session: &str) -> u64 {
        self.lock().sends.get(session).and_then(|v| v.last()).map(|(id, _)| *id).expect("还没 send 过")
    }

    pub fn launches(&self, session: &str) -> Vec<LaunchRecord> {
        self.lock().launches.iter().filter(|l| l.session == session).cloned().collect()
    }
}

/// 顶替 `Worktrees`：不碰 git，记下调用；可让下一次失败。
#[derive(Clone, Default)]
pub struct StandInWorktrees(pub Arc<Mutex<StandInWtState>>);

#[derive(Default)]
pub struct StandInWtState {
    pub calls: Vec<(PathBuf, String)>,
    pub fail_next: Option<String>,
}

impl StandInWorktrees {
    pub fn calls(&self) -> Vec<(PathBuf, String)> {
        self.0.lock().expect("测试器材锁中毒").calls.clone()
    }

    pub fn fail_next(&self, why: &str) {
        self.0.lock().expect("测试器材锁中毒").fail_next = Some(why.into());
    }
}

impl super::worktree::Worktrees for StandInWorktrees {
    fn create<'a>(&'a self, crew_dir: &'a std::path::Path, hint: &'a str) -> BoxFut<'a, Result<PathBuf, String>> {
        let mut st = self.0.lock().expect("测试器材锁中毒");
        st.calls.push((crew_dir.to_path_buf(), hint.to_string()));
        let n = st.calls.len();
        let r = match st.fail_next.take() {
            Some(why) => Err(why),
            None => Ok(PathBuf::from(format!("/nonexistent/worktrees/w-{n}"))),
        };
        Box::pin(async move { r })
    }
}

impl Executor for StandIn {
    fn launch(&self, req: LaunchRequest) -> BoxFut<'_, Result<Launched, LaunchError>> {
        let st = self.0.clone();
        Box::pin(async move {
            let mut s = st.lock().expect("测试器材锁中毒");
            if !s.fail_next.is_empty() {
                let why = s.fail_next.remove(0);
                return Err(LaunchError::Failed(why));
            }
            s.launches.push(LaunchRecord {
                session: req.session_id.clone(),
                workdir: req.workdir.clone(),
                initial: req.initial_prompt.clone(),
                system_prompt: req.system_prompt.clone(),
                token: req.token.clone(),
            });
            let (tx, rx) = mpsc::unbounded_channel();
            tx.send(SessionEvent::Spawned {
                pid: 4242,
                agent_session_id: format!("agent-{}", req.session_id),
                resumed: false,
            })
            .expect("刚建的通道");
            s.events.insert(req.session_id.clone(), tx);
            drop(s);
            let handle = StandInHandle { session: req.session_id, st };
            Ok((Arc::new(handle) as Arc<dyn ExecHandle>, rx))
        })
    }

    fn locate_transcript(&self, id: &str) -> Option<PathBuf> {
        self.lock().transcripts.get(id).cloned()
    }
}

pub struct StandInHandle {
    session: String,
    st: Arc<Mutex<StandInState>>,
}

impl StandInHandle {
    fn lock(&self) -> std::sync::MutexGuard<'_, StandInState> {
        self.st.lock().expect("测试器材锁中毒")
    }
}

impl ExecHandle for StandInHandle {
    fn send(&self, text: String, context: &Context) -> Result<u64, SendError> {
        assert!(context.is_empty());
        if text.trim().is_empty() {
            return Err(SendError::EmptyText);
        }
        let mut s = self.lock();
        s.next_id += 1;
        let id = s.next_id;
        s.sends.entry(self.session.clone()).or_default().push((id, text));
        Ok(id)
    }

    fn raw(&self, bytes: Vec<u8>) -> Result<(), Closed> {
        self.lock().raws.entry(self.session.clone()).or_default().push(bytes);
        Ok(())
    }

    /// 像真驱动层一样：叫停之后发一条 `Exited { stopped_by_user: true }`。
    fn stop(&self) -> Result<(), Closed> {
        let mut s = self.lock();
        s.stops.push(self.session.clone());
        if let Some(tx) = s.events.get(&self.session) {
            let info = crewd_exec::session::ExitInfo {
                status: crewd_exec::session::ExitStatusKind::Signal(15),
                stopped_by_user: true,
                hit_limit: false,
            };
            let _ = tx.send(SessionEvent::Exited(info));
        }
        Ok(())
    }

    fn clear_quota_health(&self) -> Result<(), Closed> {
        self.lock().cleared_quota.push(self.session.clone());
        Ok(())
    }

    fn screen_text(&self, _max_lines: usize) -> BoxFut<'_, Result<String, Closed>> {
        Box::pin(async { Ok("合成屏幕".to_string()) })
    }

    fn snapshot(&self) -> BoxFut<'_, Result<Option<Snapshot>, Closed>> {
        Box::pin(async { Ok(None) })
    }

    fn status(&self) -> BoxFut<'_, Result<SessionStatus, Closed>> {
        Box::pin(async { Err(Closed) })
    }
}
```

（MemPort 用 `async fn` 实现 RPITIT 方法：trait 里写的是 `-> impl Future + Send`，实现里写 `async fn` 合法；std `MutexGuard` 不跨 await，future 是 Send。编不过就把方法体改成 `async move { … }` 块。）

Run: `cargo test -p crewd-server orchestra`
Expected: PASS（clock 1、exec 1、exec_claude 3、hooks 1、testkit 1，共 7 条）。

- [ ] **Step 9: 门禁与提交**

Run: `./scripts/check.sh`
Expected: 全绿。

```bash
git add crates/server/Cargo.toml Cargo.lock crates/server/src/orchestra/mod.rs crates/server/src/orchestra/clock.rs crates/server/src/orchestra/exec.rs crates/server/src/orchestra/exec_claude.rs crates/server/src/orchestra/hooks.rs crates/server/src/orchestra/port.rs crates/server/src/orchestra/worktree.rs crates/server/src/orchestra/testkit.rs
git diff --cached --stat
git commit -m "crewd-server: orchestra skeleton — clock, executor trait with claude impl, hook points incl. Arbiter stub, Port and test kit

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 7: Core——会话起停与会话事件

一个 actor 独占会话表（裁定 6）。本任务：起（新建 / 接回）、停、`AlreadyRunning`、拉起在派生任务里做并按 gen 回到 actor、会话事件落库与 `session.changed`、退出、续跑被拒（裁定 12）、起不来（裁定 11）、额度（裁定 13）、待决策通知与升级（裁定 14）、nudge、set_profile、可选上限对 agent 起会话的约束（裁定 5）。投递在 Task 8 接。

**Files:**
- Create: `crewd/crates/server/src/orchestra/core.rs`
- Modify: `crewd/crates/server/src/orchestra/mod.rs`（加 `pub mod core;`）、`crewd/crates/server/src/orchestra/testkit.rs`（加 `Rig`）

**Interfaces:**
- Consumes: Task 6 全部；Task 4 `policy::{quota_plan, quota_pending, Gate, LaunchGate}`、`queue::ids`；Task 5 `notice::*`；Task 1 `text`。
- Produces:
  - `core::{Input, Origin, StartSession（带 isolation: bool，C8）, ControlError（含 Worktree(String)）, Reply, Cmd, Inflight, Live, Starting, Raised, Core}`
  - `Core::new(port: Arc<P>, execs: Executors, hooks: Hooks, clock: Arc<dyn Clock>, cfg: OrchestraConfig, inbox: mpsc::UnboundedSender<Input>) -> anyhow::Result<Core<P>>`、`Core::handle(&mut self, Input)`、`pub(crate)`：`resume`、`launch(row, exec, text, wakes)`、`launch_failed(&row, detail)`、`alert_audience(crew, is_captain)`、`live_view`、`publish_live`、`on_tick`、`schedule_quota_for`
  - `testkit::Rig { core, port, exec, clock, rx }`：`new()`、`with(Hooks)`、`crew(crew, captain)`、`worker(crew, id)`、`channel(id, crew, owner, members)`、`settle()`、`cmd(make)`、`run(session)`、`emit(session, event)`、`tick(ms)`、`live(session)`

- [ ] **Step 1: 写测试器材 `Rig`（追加到 `testkit.rs`）**

```rust
use crewd_wake::plan::Sender;
use tokio::sync::oneshot;

use super::OrchestraConfig;
use super::clock::{Clock, ManualClock};
use super::core::{Cmd, Core, Input, Live, Origin};
use super::exec::Executors;
use super::hooks::Hooks;

pub const T0: Ms = Ms(1_000_000_000);

pub struct Rig {
    pub core: Core<MemPort>,
    pub port: Arc<MemPort>,
    pub exec: StandIn,
    pub clock: Arc<ManualClock>,
    pub worktrees: StandInWorktrees,
    pub rx: mpsc::UnboundedReceiver<Input>,
}

impl Rig {
    pub async fn new() -> Self {
        Self::with(Hooks::default()).await
    }

    pub async fn with(mut hooks: Hooks) -> Self {
        let port = Arc::new(MemPort::new());
        let exec = StandIn::default();
        let worktrees = StandInWorktrees::default();
        hooks.worktrees = Arc::new(worktrees.clone());
        let clock = Arc::new(ManualClock::new(T0));
        port.lock().now = T0;
        let (tx, rx) = mpsc::unbounded_channel();
        let execs = Executors::new().with("claude", Arc::new(exec.clone()));
        let core = Core::new(port.clone(), execs, hooks, clock.clone(), OrchestraConfig::for_tests(), tx)
            .await
            .expect("内存 Port 不会失败");
        Self { core, port, exec, clock, worktrees, rx }
    }

    fn row(id: &str, crew: &str, captain: bool) -> SessionRow {
        SessionRow {
            id: id.into(),
            crew_id: crew.into(),
            captain,
            executor: "claude".into(),
            display_name: Some(id.into()),
            agent_session_id: None,
            workdir: Some(PathBuf::from("/nonexistent/wd")),
            model: None,
            effort: None,
            brief: None,
            desired: Desired::Stopped,
            was_working: false,
            last_active_at: None,
            resume_failed_at: None,
            awaiting_question: None,
            last_progress_at: None,
        }
    }

    /// 机组 + 机长 + 大厅 `<crew>/lobby` + 人类群 `<crew>/human`。session 都没在跑、desired=stopped。
    pub fn crew(&self, crew: &str, captain: &str) {
        let mut st = self.port.lock();
        st.sessions.insert(captain.into(), Self::row(captain, crew, true));
        st.crew_dirs.insert(crew.into(), PathBuf::from("/nonexistent/wd"));
        for (suffix, kind) in [("lobby", MemKind::Lobby), ("human", MemKind::Human)] {
            st.channels.insert(
                format!("{crew}/{suffix}"),
                MemChannel { crew: crew.into(), kind, owner: None, members: vec![] },
            );
        }
    }

    pub fn worker(&self, crew: &str, id: &str) {
        self.port.lock().sessions.insert(id.into(), Self::row(id, crew, false));
    }

    pub fn channel(&self, id: &str, crew: &str, owner: Option<&str>, members: &[&str]) {
        self.port.lock().channels.insert(
            id.into(),
            MemChannel {
                crew: crew.into(),
                kind: MemKind::Other,
                owner: owner.map(str::to_string),
                members: members.iter().map(|s| s.to_string()).collect(),
            },
        );
    }

    /// 推进派生任务（拉起、事件泵、Arbiter）并处理它们发回来的输入，直到连续 50 次让步都没有新输入。
    pub async fn settle(&mut self) {
        let mut quiet = 0;
        while quiet < 50 {
            tokio::task::yield_now().await;
            match self.rx.try_recv() {
                Ok(input) => {
                    self.core.handle(input).await;
                    quiet = 0;
                }
                Err(_) => quiet += 1,
            }
        }
    }

    pub async fn cmd<T>(&mut self, make: impl FnOnce(oneshot::Sender<T>) -> Cmd) -> T {
        let (tx, rx) = oneshot::channel();
        self.core.handle(Input::Cmd(make(tx))).await;
        self.settle().await;
        rx.await.expect("命令要有回执")
    }

    /// 人从客户端把它起起来（不错开、不受上限），并等到 `Spawned` 处理完。
    pub async fn run(&mut self, session: &str) {
        let s = session.to_string();
        self.cmd(|reply| Cmd::Resume { session: s, origin: Origin::Human, reply }).await.expect("起得来");
    }

    pub async fn emit(&mut self, session: &str, e: SessionEvent) {
        self.exec.emit(session, e);
        self.settle().await;
    }

    pub async fn tick(&mut self, advance_ms: i64) {
        self.clock.advance(advance_ms);
        self.port.lock().now = self.clock.now();
        self.core.handle(Input::Tick).await;
        self.settle().await;
    }

    pub fn live(&self, session: &str) -> Option<&Live> {
        self.core.live.get(session)
    }

    pub fn sender(id: &str) -> Sender {
        Sender::Session(id.into())
    }
}
```

- [ ] **Step 2: 写失败的测试**

`crewd/crates/server/src/orchestra/core.rs` 先放测试（`mod.rs` 加 `pub mod core;`）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestra::hooks::QuotaSource;
    use crate::orchestra::exec::BoxFut;
    use crate::orchestra::testkit::{Rig, T0};
    use crewd_exec::health::Health;
    use crewd_exec::session::ExitStatusKind;
    use crewd_wake::{HOUR, WakeState};

    fn exited(hit_limit: bool) -> SessionEvent {
        SessionEvent::Exited(ExitInfo { status: ExitStatusKind::Code(0), stopped_by_user: false, hit_limit })
    }

    fn decision(prompt: &str) -> PendingDecision {
        PendingDecision { prompt: prompt.into(), options: vec!["Yes".into(), "No".into()], numbered: true }
    }

    fn start(executor: &str) -> StartSession {
        StartSession {
            crew_id: "c1".into(),
            captain: false,
            executor: executor.into(),
            brief: "合成任务甲".into(),
            title: Some("w-new".into()),
            model: None,
            effort: None,
            isolation: false,
        }
    }

    #[tokio::test]
    async fn concurrent_resume_of_one_session_starts_exactly_one_process() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let (a, ra) = oneshot::channel();
        let (b, rb) = oneshot::channel();
        rig.core.handle(Input::Cmd(Cmd::Resume { session: "cap".into(), origin: Origin::Human, reply: a })).await;
        rig.core.handle(Input::Cmd(Cmd::Resume { session: "cap".into(), origin: Origin::Human, reply: b })).await;
        rig.settle().await;
        assert_eq!(ra.await.unwrap(), Ok(()));
        assert_eq!(rb.await.unwrap(), Err(ControlError::AlreadyRunning("cap".into())));
        assert_eq!(rig.exec.launches("cap").len(), 1);
        let l = &rig.exec.launches("cap")[0];
        assert_eq!((l.system_prompt.as_deref(), l.token.as_str()), (Some("WORLD cap"), "tok-cap"));
        assert_eq!(rig.live("cap").map(|l| l.agent_session_id.as_str()), Some("agent-cap"));
        assert_eq!(rig.port.row("cap").desired, Desired::Running);
        let again = rig.cmd(|reply| Cmd::Resume { session: "cap".into(), origin: Origin::Human, reply }).await;
        assert_eq!(again, Err(ControlError::AlreadyRunning("cap".into())));
    }

    #[tokio::test]
    async fn start_creates_a_row_and_opens_with_the_brief() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let row = rig.cmd(|reply| Cmd::Start { req: start("claude"), origin: Origin::Human, reply }).await.unwrap();
        assert_eq!(rig.exec.launches(&row.id)[0].initial.as_deref(), Some("合成任务甲"));
        assert!(rig.live(&row.id).is_some());
    }

    #[tokio::test]
    async fn isolation_false_uses_the_shared_crew_directory_and_never_touches_git() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let row = rig.cmd(|reply| Cmd::Start { req: start("claude"), origin: Origin::Human, reply }).await.unwrap();
        assert!(rig.worktrees.calls().is_empty());
        assert_eq!(rig.exec.launches(&row.id)[0].workdir, PathBuf::from("/nonexistent/wd"), "机组共享目录");
    }

    #[tokio::test]
    async fn isolation_true_creates_the_worktree_before_the_row_and_launches_there() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let req = StartSession { isolation: true, ..start("claude") };
        let row = rig.cmd(|reply| Cmd::Start { req, origin: Origin::Agent, reply }).await.unwrap();
        // 在机组工作目录里建，hint = 开场 brief（Swift `SessionWorkspace.resolve(hint: brief)`）。
        assert_eq!(rig.worktrees.calls(), vec![(PathBuf::from("/nonexistent/wd"), "合成任务甲".to_string())]);
        assert_eq!(row.workdir, Some(PathBuf::from("/nonexistent/worktrees/w-1")));
        assert_eq!(rig.exec.launches(&row.id)[0].workdir, PathBuf::from("/nonexistent/worktrees/w-1"), "进程在 worktree 里起");
    }

    #[tokio::test]
    async fn worktree_failure_fails_the_start_with_the_reason_and_creates_nothing() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worktrees.fail_next("无法为 session 新建 worktree：/x 当前是 detached HEAD");
        let req = StartSession { isolation: true, ..start("claude") };
        let err = rig.cmd(|reply| Cmd::Start { req, origin: Origin::Agent, reply }).await.unwrap_err();
        assert_eq!(err, ControlError::Worktree("无法为 session 新建 worktree：/x 当前是 detached HEAD".into()));
        assert_eq!(rig.port.lock().sessions.len(), 1, "没写 session 行（只有 cap）");
        assert!(rig.exec.lock().launches.is_empty(), "什么都没起，也没退回共享目录");
    }

    #[tokio::test]
    async fn isolation_without_a_crew_directory_is_a_worktree_error_not_a_silent_fallback() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.port.lock().crew_dirs.remove("c1");
        let req = StartSession { isolation: true, ..start("claude") };
        let err = rig.cmd(|reply| Cmd::Start { req, origin: Origin::Human, reply }).await.unwrap_err();
        assert!(matches!(err, ControlError::Worktree(_)), "{err}");
        assert!(rig.worktrees.calls().is_empty());
        assert_eq!(rig.port.lock().sessions.len(), 1);
    }

    #[tokio::test]
    async fn codex_sessions_are_unsupported_until_m2c_and_nothing_is_created() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let err = rig.cmd(|reply| Cmd::Start { req: start("codex"), origin: Origin::Human, reply }).await.unwrap_err();
        assert!(matches!(err, ControlError::Unsupported(_)), "{err}");
        assert_eq!(rig.port.lock().sessions.len(), 1, "没建行");
    }

    #[tokio::test]
    async fn optional_cap_refuses_agent_starts_but_never_human_ones() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        rig.core.gate.set_max_live(Some(1));
        let err = rig.cmd(|reply| Cmd::Start { req: start("claude"), origin: Origin::Agent, reply }).await.unwrap_err();
        assert_eq!(err, ControlError::AtCap { max: 1 });
        assert!(rig.cmd(|reply| Cmd::Start { req: start("claude"), origin: Origin::Human, reply }).await.is_ok());
    }

    #[tokio::test]
    async fn working_edges_persist_was_working_and_last_active() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        rig.emit("cap", SessionEvent::Working(true)).await;
        let row = rig.port.row("cap");
        assert!(row.was_working);
        assert_eq!(row.last_active_at, Some(T0));
        rig.emit("cap", SessionEvent::Working(false)).await;
        assert!(!rig.port.row("cap").was_working);
    }

    #[tokio::test]
    async fn exit_clears_the_live_entry_and_publishes_not_running() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        rig.emit("cap", exited(false)).await;
        assert!(rig.live("cap").is_none());
        let last = rig.port.lock().events.last().cloned();
        assert!(matches!(last, Some(OrchestraEvent::SessionChanged(v)) if !v.running && v.session_id == "cap"));
    }

    #[tokio::test]
    async fn stop_sets_desired_stopped_drops_pending_wakes_and_stops_the_process() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        rig.port
            .enqueue(NewWake {
                session: "cap".into(),
                kind: WakeKind::Timer,
                source: source::timer("t1"),
                priority: Priority::Auto,
                not_before: T0.add(HOUR),
                note: Some("x".into()),
                origin: "cap".into(),
            })
            .await
            .unwrap();
        rig.cmd(|reply| Cmd::Stop { session: "cap".into(), reply }).await.unwrap();
        assert_eq!(rig.port.row("cap").desired, Desired::Stopped);
        assert_eq!(rig.port.wakes_of("cap")[0].state, WakeState::Dropped);
        assert_eq!(rig.exec.lock().stops, ["cap"]);
    }

    #[tokio::test]
    async fn refused_resume_notifies_queues_a_note_and_counts_as_resume_failure() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.exec.lock().transcripts.insert("old-1".into(), "/synthetic/p/old-1.jsonl".into());
        rig.run("cap").await;
        let said = "No conversation found with session ID: old-1";
        rig.emit("cap", SessionEvent::Spawned { pid: 1, agent_session_id: "old-1".into(), resumed: true }).await;
        rig.emit("cap", SessionEvent::ResumeRefused { id: "old-1".into(), said: said.into() }).await;
        rig.emit("cap", SessionEvent::Spawned { pid: 2, agent_session_id: "new-1".into(), resumed: false }).await;
        assert_eq!(rig.live("cap").unwrap().agent_session_id, "new-1", "以最后一条 Spawned 为准");
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].0, Audience::Lobby { crew: "c1".into(), mention_owner: false });
        assert!(posts[0].1.contains(said) && posts[0].1.contains("/synthetic/p/old-1.jsonl"), "{}", posts[0].1);
        let w = rig.port.wakes_of("cap");
        assert_eq!(w.len(), 1);
        assert_eq!((w[0].kind, w[0].source.as_str(), w[0].priority), (WakeKind::System, "system:resume-refused:old-1", Priority::System));
        assert!(w[0].note.as_deref().unwrap().contains("old-1"));
        assert!(rig.port.row("cap").resume_failed_at.is_some());
        rig.emit("cap", SessionEvent::Working(true)).await;
        assert!(rig.port.row("cap").resume_failed_at.is_some(), "这次拉起经历过续跑被拒：原对话仍算丢了");
    }

    #[tokio::test]
    async fn launch_error_marks_resume_failed_and_alerts_the_owner_once() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.exec.lock().fail_next = vec!["合成：claude 不在".into(), "合成：claude 不在".into()];
        rig.run("w1").await;
        rig.run("w1").await;
        assert!(rig.live("w1").is_none());
        assert!(rig.port.row("w1").resume_failed_at.is_some());
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 1, "已经记过就不再告警：{posts:?}");
        assert_eq!(posts[0].0, Audience::Lobby { crew: "c1".into(), mention_owner: true });
        assert!(posts[0].1.contains("合成：claude 不在"));
    }

    #[tokio::test]
    async fn captain_launch_failure_goes_to_the_human_channel_never_to_itself() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.exec.lock().fail_next = vec!["合成：起不来".into()];
        rig.run("cap").await;
        assert_eq!(rig.port.posts()[0].0, Audience::HumanChannel { crew: "c1".into() });
    }

    #[tokio::test]
    async fn dying_with_launch_failed_before_any_work_is_a_resume_failure_and_later_work_clears_it() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("w1").await;
        rig.emit("w1", SessionEvent::Health(Some(Health::new(HealthKind::LaunchFailed, "合成：起来即死")))).await;
        rig.emit("w1", exited(false)).await;
        assert!(rig.port.row("w1").resume_failed_at.is_some());
        rig.run("w1").await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        assert_eq!(rig.port.row("w1").resume_failed_at, None);
    }

    #[tokio::test]
    async fn quota_without_a_reset_time_schedules_one_fallback_wake_and_says_so() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("w1").await;
        for _ in 0..2 {
            rig.emit("w1", SessionEvent::Health(Some(Health::new(HealthKind::UsageLimit, "x")))).await;
        }
        let w = rig.port.wakes_of("w1");
        assert_eq!(w.len(), 1, "已有一条没投的额度唤醒就不再挂");
        assert_eq!(w[0].not_before, T0.add(45 * MIN));
        assert_eq!(w[0].note.as_deref(), Some(text::get(text::Lang::Zh, "quota.wake")));
        let want = text::render(text::Lang::Zh, "quota.fallback", &[("name", "w1"), ("minutes", "45")]);
        assert!(rig.port.posts().iter().any(|(_, t)| *t == want));
    }

    struct ResetIn30;
    impl QuotaSource for ResetIn30 {
        fn reset_at<'a>(&'a self, _e: &'a str, now: Ms) -> BoxFut<'a, Option<Ms>> {
            Box::pin(async move { Some(now.add(30 * MIN)) })
        }
    }

    #[tokio::test]
    async fn quota_with_a_reset_time_fires_a_minute_after_it_and_exit_by_limit_schedules_too() {
        let mut rig = Rig::with(Hooks { quota: Arc::new(ResetIn30), ..Hooks::default() }).await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("w1").await;
        rig.emit("w1", exited(true)).await;
        let w = rig.port.wakes_of("w1");
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].not_before, T0.add(31 * MIN));
        let want = text::render(text::Lang::Zh, "quota.scheduled", &[("name", "w1"), ("minutes", "31")]);
        assert!(rig.port.posts().iter().any(|(_, t)| *t == want));
    }

    #[tokio::test]
    async fn worker_decision_goes_to_the_owner_once_then_escalates_to_the_human() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("w1").await;
        for _ in 0..2 {
            rig.emit("w1", SessionEvent::Decision(DecisionEvent::Appeared(decision("Trust?")))).await;
        }
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 1, "同一指纹不重发");
        assert_eq!(posts[0].0, Audience::Lobby { crew: "c1".into(), mention_owner: true });
        rig.tick(4 * MIN).await;
        assert_eq!(rig.port.posts().len(), 1);
        rig.tick(MIN).await;
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 2);
        assert_eq!(posts[1].0, Audience::HumanChannel { crew: "c1".into() });
        rig.tick(10 * MIN).await;
        assert_eq!(rig.port.posts().len(), 2, "只升级一次");
    }

    #[tokio::test]
    async fn captain_decision_goes_straight_to_the_human_and_is_not_escalated_again() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.run("cap").await;
        rig.emit("cap", SessionEvent::Decision(DecisionEvent::Appeared(decision("Trust?")))).await;
        rig.tick(6 * MIN).await;
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].0, Audience::HumanChannel { crew: "c1".into() });
    }

    #[tokio::test]
    async fn nudge_sends_alias_bytes_or_text_plus_enter_and_needs_a_running_session() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        let err = rig.cmd(|reply| Cmd::Nudge { session: "cap".into(), input: "enter".into(), reply }).await;
        assert_eq!(err, Err(ControlError::NotRunning("cap".into())));
        rig.run("cap").await;
        rig.cmd(|reply| Cmd::Nudge { session: "cap".into(), input: "enter".into(), reply }).await.unwrap();
        rig.cmd(|reply| Cmd::Nudge { session: "cap".into(), input: "2".into(), reply }).await.unwrap();
        assert_eq!(rig.exec.lock().raws["cap"], [b"\r".to_vec(), b"2\r".to_vec()]);
    }

    #[tokio::test]
    async fn set_profile_only_persists_for_the_next_start() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.cmd(|reply| Cmd::SetProfile { session: "cap".into(), model: Some("opus".into()), effort: None, reply }).await.unwrap();
        assert_eq!(rig.port.row("cap").model.as_deref(), Some("opus"));
    }
}
```

Run: `cargo test -p crewd-server orchestra::core`
Expected: 编译失败（`Core` 等未定义）。

- [ ] **Step 3: 实现**

在 `core.rs` 测试模块上方写：

```rust
//! 编排核心（spec §22.4）：一个 actor 独占全部会话句柄与内存状态，按输入顺序一条条处理（裁定 6）。
//! 判定在 crewd-wake；这里只调 Port、调执行器、发事件。
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use crewd_exec::health::{Health, HealthKind};
use crewd_exec::session::{BRIEF_DELIVERY_ID, ExitInfo, SessionEvent};
use crewd_term::{DecisionEvent, PendingDecision};
use crewd_wake::notice::{self, DECISION_ESCALATE_MS, DecisionFacts, NoticeStage};
use crewd_wake::policy::{self, Gate, LaunchGate};
use crewd_wake::queue::ids;
use crewd_wake::breaker::SYSTEM;
use crewd_wake::text;
use crewd_wake::{MIN, Ms, NewWake, Priority, QueuedWake, WakeKind, source};
use tokio::sync::{mpsc, oneshot};

use super::OrchestraConfig;
use super::clock::Clock;
use super::exec::{ExecHandle, Executor, Executors, LaunchRequest};
use super::hooks::Hooks;
use super::port::*;

pub enum Input {
    Exec { session: String, gen: u64, event: SessionEvent },
    Launched { session: String, gen: u64, result: Result<Arc<dyn ExecHandle>, String> },
    Tick,
    Cmd(Cmd),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// 人从客户端（不受可选上限、不错开）。
    Human,
    /// agent 经 `SessionControl`（受可选上限）。
    Agent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartSession {
    pub crew_id: String,
    pub captain: bool,
    pub executor: String,
    pub brief: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// 控制者裁定 C8：true = 在机组工作目录里新建 git worktree 并在那里起；false = 机组共享目录。
    pub isolation: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlError {
    NotFound(String),
    AlreadyRunning(String),
    NotRunning(String),
    AtCap { max: usize },
    Unsupported(String),
    NoWorkdir(String),
    /// 建 worktree 失败（C8）：原因给人看；什么都没留下、没起、没退回共享目录。
    Worktree(String),
    Internal(String),
}

impl ControlError {
    pub fn internal(e: impl std::fmt::Display) -> Self {
        Self::Internal(e.to_string())
    }
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "没有 session {id}"),
            Self::AlreadyRunning(id) => write!(f, "session {id} 已经在跑或正在起"),
            Self::NotRunning(id) => write!(f, "session {id} 没在跑"),
            Self::AtCap { max } => write!(f, "已达设置里的并发会话上限 {max}"),
            Self::Unsupported(why) | Self::Internal(why) | Self::Worktree(why) => write!(f, "{why}"),
            Self::NoWorkdir(id) => write!(f, "session {id} 没有可用的工作目录"),
        }
    }
}

impl std::error::Error for ControlError {}

pub type Reply<T> = oneshot::Sender<Result<T, ControlError>>;

pub enum Cmd {
    Start { req: StartSession, origin: Origin, reply: Reply<SessionRow> },
    Resume { session: String, origin: Origin, reply: Reply<()> },
    Stop { session: String, reply: Reply<()> },
    Live { session: String, reply: Reply<LiveView> },
    Nudge { session: String, input: String, reply: Reply<()> },
    Handle { session: String, reply: oneshot::Sender<Option<Arc<dyn ExecHandle>>> },
    SetProfile { session: String, model: Option<String>, effort: Option<String>, reply: Reply<()> },
}

/// 一笔还没回执的投递。`delivery` = `ExecHandle::send` 的编号（拉起时带的正文 = `BRIEF_DELIVERY_ID`）。
pub struct Inflight {
    pub delivery: u64,
    pub wakes: Vec<QueuedWake>,
}

pub struct Live {
    pub gen: u64,
    pub handle: Arc<dyn ExecHandle>,
    pub crew: String,
    pub captain: bool,
    pub name: String,
    pub executor: String,
    /// 以最后一条 `Spawned` 为准（裁定 12）。
    pub agent_session_id: String,
    pub working: bool,
    /// 这次拉起之后干过活。
    pub worked: bool,
    /// 这次拉起经历过续跑被拒。
    pub refused: bool,
    pub health: Option<Health>,
    pub decision: Option<PendingDecision>,
    pub inflight: Option<Inflight>,
    /// 人从客户端叫停的（不是停机）。
    pub stopping: bool,
}

pub struct Starting {
    pub gen: u64,
    pub row: SessionRow,
    /// 随开场正文一起送的唤醒（非空时一定有正文）。
    pub wakes: Vec<QueuedWake>,
    pub stop_requested: bool,
}

pub struct Raised {
    pub at: Ms,
    pub fingerprint: String,
    pub escalated: bool,
}

pub struct Core<P: Port> {
    pub(crate) port: Arc<P>,
    pub(crate) execs: Executors,
    pub(crate) hooks: Hooks,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) cfg: OrchestraConfig,
    pub(crate) inbox: mpsc::UnboundedSender<Input>,
    pub(crate) live: HashMap<String, Live>,
    pub(crate) starting: HashMap<String, Starting>,
    pub(crate) next_gen: u64,
    pub(crate) gate: LaunchGate,
    pub(crate) decisions: HashMap<String, Raised>,
    /// 重启后等人答「恢复」的 session（§22.7 Q1）：唤醒照常入队，但不被自动拉起（人 @ 它例外，C4）。
    pub(crate) held: BTreeSet<String>,
    /// `boot` 结束时队列里最大的唤醒 id：比它新的人类消息唤醒 = 询问开出之后人 @ 的（C4）。
    pub(crate) held_watermark: i64,
}

impl<P: Port> Core<P> {
    pub async fn new(
        port: Arc<P>,
        execs: Executors,
        hooks: Hooks,
        clock: Arc<dyn Clock>,
        cfg: OrchestraConfig,
        inbox: mpsc::UnboundedSender<Input>,
    ) -> anyhow::Result<Self> {
        let mut cfg = cfg;
        cfg.lang = port.locale().await?;
        // 可选并发上限（§22.7 Q5：缺省不设），存在 M3a 的 `collab::settings`。
        let gate = LaunchGate::new(port.max_live().await?, cfg.stagger_ms);
        Ok(Self {
            port,
            execs,
            hooks,
            clock,
            cfg,
            inbox,
            live: HashMap::new(),
            starting: HashMap::new(),
            next_gen: 0,
            gate,
            decisions: HashMap::new(),
            held: BTreeSet::new(),
            held_watermark: 0,
        })
    }

    pub async fn handle(&mut self, input: Input) {
        let r = match input {
            Input::Exec { session, gen, event } => self.on_exec(&session, gen, event).await,
            Input::Launched { session, gen, result } => self.on_launched(&session, gen, result).await,
            Input::Tick => self.on_tick().await,
            Input::Cmd(c) => {
                self.on_cmd(c).await;
                Ok(())
            }
        };
        if let Err(e) = r {
            eprintln!("crewd: 编排：{e:#}");
        }
    }

    pub(crate) fn now(&self) -> Ms {
        self.clock.now()
    }

    pub(crate) fn lang(&self) -> text::Lang {
        self.cfg.lang
    }

    // —— 命令

    async fn on_cmd(&mut self, c: Cmd) {
        match c {
            Cmd::Start { req, origin, reply } => {
                let r = self.start_new(req, origin).await;
                let _ = reply.send(r);
            }
            Cmd::Resume { session, origin, reply } => {
                let r = self.resume(&session, origin).await;
                let _ = reply.send(r);
            }
            Cmd::Stop { session, reply } => {
                let r = self.stop(&session).await;
                let _ = reply.send(r);
            }
            Cmd::Live { session, reply } => {
                let r = self.live_view(&session).await;
                let _ = reply.send(r);
            }
            Cmd::Nudge { session, input, reply } => {
                let r = match self.live.get(&session) {
                    Some(l) => l
                        .handle
                        .raw(notice::nudge_bytes(&input))
                        .map_err(|_| ControlError::NotRunning(session.clone())),
                    None => Err(ControlError::NotRunning(session.clone())),
                };
                let _ = reply.send(r);
            }
            Cmd::Handle { session, reply } => {
                let _ = reply.send(self.live.get(&session).map(|l| l.handle.clone()));
            }
            Cmd::SetProfile { session, model, effort, reply } => {
                let patch = SessionPatch { model: Some(model), effort: Some(effort), ..Default::default() };
                let r = self.port.patch_session(&session, patch).await.map_err(ControlError::internal);
                let _ = reply.send(r);
            }
        }
    }

    /// 显式起会话的上限检查：人起的不受上限；显式起的不错开（裁定 5）。
    fn check_cap(&self, origin: Origin) -> Result<(), ControlError> {
        let live = self.live.len() + self.starting.len();
        match self.gate.check(live, origin == Origin::Human, self.now()) {
            Gate::AtCap => Err(ControlError::AtCap { max: self.gate.max_live().unwrap_or(live) }),
            Gate::Go | Gate::WaitUntil(_) => Ok(()),
        }
    }

    async fn start_new(&mut self, req: StartSession, origin: Origin) -> Result<SessionRow, ControlError> {
        let exec = self.execs.get(&req.executor).map_err(|e| ControlError::Unsupported(e.to_string()))?;
        self.check_cap(origin)?;
        // 控制者裁定 C8：isolation=true → 先建 worktree（写行之前；失败 = 什么都不留、不起、不退回共享目录）。
        let worktree = if req.isolation {
            let base = self
                .port
                .crew_workdir(&req.crew_id)
                .await
                .map_err(ControlError::internal)?
                .ok_or_else(|| ControlError::Worktree(format!("机组 {} 没有工作目录，无法为 session 新建 worktree", req.crew_id)))?;
            let hint = req.brief.as_str();
            Some(self.hooks.worktrees.create(&base, hint).await.map_err(ControlError::Worktree)?)
        } else {
            None
        };
        let brief = Some(req.brief.clone()).filter(|b| !b.trim().is_empty());
        let row = self
            .port
            .insert_session(NewSession {
                crew_id: req.crew_id,
                captain: req.captain,
                executor: req.executor,
                brief: req.brief,
                title: req.title,
                model: req.model,
                effort: req.effort,
                worktree: worktree.clone(),
            })
            .await
            .map_err(|e| match &worktree {
                // 行没写成但 worktree 已经建好：crewd 不删它，把路径告诉人去收。
                Some(p) => ControlError::internal(format!("{e}（已建好的 worktree {} 不会被自动删除）", p.display())),
                None => ControlError::internal(e),
            })?;
        if row.workdir.is_none() {
            return Err(ControlError::NoWorkdir(row.id));
        }
        self.launch(row.clone(), exec, brief, vec![]).await.map_err(ControlError::internal)?;
        Ok(row)
    }

    pub(crate) async fn resume(&mut self, session: &str, origin: Origin) -> Result<(), ControlError> {
        if self.live.contains_key(session) || self.starting.contains_key(session) {
            return Err(ControlError::AlreadyRunning(session.into()));
        }
        let row = self
            .port
            .session(session)
            .await
            .map_err(ControlError::internal)?
            .ok_or_else(|| ControlError::NotFound(session.into()))?;
        let exec = self.execs.get(&row.executor).map_err(|e| ControlError::Unsupported(e.to_string()))?;
        if row.workdir.is_none() {
            return Err(ControlError::NoWorkdir(row.id));
        }
        self.check_cap(origin)?;
        // 单个恢复一个等人答复的 session（§22.7 Q1）：上次停在干活中途的，开场补一句「被打断」。
        let interrupted = self.held.remove(session) && row.was_working;
        let text = interrupted.then(|| text::get(self.lang(), "restart.interrupted").to_string());
        self.launch(row, exec, text, vec![]).await.map_err(ControlError::internal)
    }

    /// 人停掉（裁定 10）：desired=stopped，丢掉此刻 pending 的唤醒，叫停进程。
    async fn stop(&mut self, session: &str) -> Result<(), ControlError> {
        if self.port.session(session).await.map_err(ControlError::internal)?.is_none() {
            return Err(ControlError::NotFound(session.into()));
        }
        let now = self.now();
        let pending = self.port.pending(session).await.map_err(ControlError::internal)?;
        if !pending.is_empty() {
            self.port
                .settle(&ids(&pending), Settle::Dropped { error: "stopped".into() }, now)
                .await
                .map_err(ControlError::internal)?;
        }
        let patch = SessionPatch { desired: Some(Desired::Stopped), was_working: Some(false), ..Default::default() };
        self.port.patch_session(session, patch).await.map_err(ControlError::internal)?;
        self.held.remove(session);
        if let Some(l) = self.live.get_mut(session) {
            l.stopping = true;
            let _ = l.handle.stop();
        } else if let Some(st) = self.starting.get_mut(session) {
            st.stop_requested = true;
        }
        Ok(())
    }

    // —— 拉起

    /// 拉起一个 session：`text` = 开场正文（新会话的 brief，或唤醒批的正文），`wakes` = 随正文送的唤醒（记 inflight；
    /// 非空时 `text` 必须有）。
    /// 真正的启动在派生任务里做，结果以 `Input::Launched` 回来（裁定 6）。
    pub(crate) async fn launch(
        &mut self,
        row: SessionRow,
        exec: Arc<dyn Executor>,
        text: Option<String>,
        wakes: Vec<QueuedWake>,
    ) -> anyhow::Result<()> {
        let now = self.now();
        let workdir = row
            .workdir
            .clone()
            .ok_or_else(|| anyhow::anyhow!("session {} 没有工作目录", row.id))?;
        let system_prompt = Some(self.port.worldview(&row.id).await?);
        let token = self.port.mint_token(&row.id);
        if !wakes.is_empty() {
            self.port.settle(&ids(&wakes), Settle::Inflight, now).await?;
        }
        let patch = SessionPatch { desired: Some(Desired::Running), ..Default::default() };
        self.port.patch_session(&row.id, patch).await?;
        self.gate.launched(now);
        self.next_gen += 1;
        let gen = self.next_gen;
        let session = row.id.clone();
        let req = LaunchRequest {
            session_id: row.id.clone(),
            crew_id: row.crew_id.clone(),
            workdir,
            model: row.model.clone(),
            effort: row.effort.clone(),
            system_prompt,
            initial_prompt: text.clone(),
            token,
            ledger: self.port.ledger(&row.id),
        };
        self.starting.insert(
            session.clone(),
            Starting { gen, row, wakes, stop_requested: false },
        );
        let tx = self.inbox.clone();
        let task_session = session.clone();
        tokio::spawn(async move {
            match exec.launch(req).await {
                Ok((handle, mut events)) => {
                    // 先报 Launched、再转事件：同一个发送端按序到达，actor 先建好 Live 再收 Spawned。
                    let launched = Input::Launched { session: task_session.clone(), gen, result: Ok(handle) };
                    if tx.send(launched).is_err() {
                        return;
                    }
                    while let Some(event) = events.recv().await {
                        if tx.send(Input::Exec { session: task_session.clone(), gen, event }).is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Input::Launched { session: task_session, gen, result: Err(e.to_string()) });
                }
            }
        });
        self.publish_live(&session).await;
        Ok(())
    }

    async fn on_launched(
        &mut self,
        session: &str,
        gen: u64,
        result: Result<Arc<dyn ExecHandle>, String>,
    ) -> anyhow::Result<()> {
        if self.starting.get(session).is_none_or(|s| s.gen != gen) {
            return Ok(());
        }
        let st = self.starting.remove(session).expect("上面查过");
        match result {
            Ok(handle) => {
                if st.stop_requested {
                    let _ = handle.stop();
                }
                let inflight = (!st.wakes.is_empty())
                    .then(|| Inflight { delivery: BRIEF_DELIVERY_ID, wakes: st.wakes.clone() });
                self.live.insert(
                    session.to_string(),
                    Live {
                        gen,
                        handle,
                        crew: st.row.crew_id.clone(),
                        captain: st.row.captain,
                        name: st.row.name(),
                        executor: st.row.executor.clone(),
                        agent_session_id: String::new(),
                        working: false,
                        worked: false,
                        refused: false,
                        health: None,
                        decision: None,
                        inflight,
                        stopping: st.stop_requested,
                    },
                );
            }
            Err(e) => {
                self.launch_failed(&st.row, &e).await?;
            }
        }
        self.publish_live(session).await;
        Ok(())
    }

    /// 起不来（裁定 11）：写 `resume_failed_at`；第一次才告警。
    pub(crate) async fn launch_failed(&mut self, row: &SessionRow, detail: &str) -> anyhow::Result<()> {
        let already = self.port.session(&row.id).await?.and_then(|r| r.resume_failed_at).is_some();
        let patch = SessionPatch { resume_failed_at: Some(Some(self.now())), ..Default::default() };
        self.port.patch_session(&row.id, patch).await?;
        if already {
            return Ok(());
        }
        let text = text::render(self.lang(), "resume.failed.group", &[("name", &row.name()), ("detail", detail)]);
        self.port.post_system(self.alert_audience(&row.crew_id, row.captain), text).await
    }

    /// 告警发给谁：worker 的事 → 本机组大厅 @群主；机长自己的事 → 根机组人类群 @人（绝不 @ 自己）。
    pub(crate) fn alert_audience(&self, crew: &str, is_captain: bool) -> Audience {
        if is_captain {
            Audience::HumanChannel { crew: crew.into() }
        } else {
            Audience::Lobby { crew: crew.into(), mention_owner: true }
        }
    }

    // —— 会话事件

    async fn on_exec(&mut self, session: &str, gen: u64, event: SessionEvent) -> anyhow::Result<()> {
        let now = self.clock.now();
        let Some(l) = self.live.get_mut(session) else { return Ok(()) };
        if l.gen != gen {
            return Ok(());
        }
        match event {
            SessionEvent::Spawned { agent_session_id, .. } => {
                l.agent_session_id = agent_session_id;
            }
            SessionEvent::Working(on) => {
                l.working = on;
                let mut patch = SessionPatch { was_working: Some(on), last_active_at: Some(now), ..Default::default() };
                if on {
                    l.worked = true;
                    if !l.refused {
                        patch.resume_failed_at = Some(None);
                    }
                }
                self.port.patch_session(session, patch).await?;
            }
            SessionEvent::Decision(DecisionEvent::Appeared(d)) => {
                l.decision = Some(d.clone());
                self.raise_decision(session, &d, now).await?;
            }
            SessionEvent::Decision(DecisionEvent::Cleared) => {
                l.decision = None;
                self.decisions.remove(session);
            }
            SessionEvent::Delivery { .. } => {}
            SessionEvent::Health(h) => {
                let quota = h.as_ref().is_some_and(|h| h.kind.is_quota());
                l.health = h;
                if quota {
                    self.schedule_quota(session).await?;
                }
            }
            SessionEvent::ResumeRefused { id, said } => {
                l.refused = true;
                self.on_resume_refused(session, &id, &said).await?;
            }
            SessionEvent::Exited(info) => return self.on_exited(session, info).await,
        }
        self.publish_live(session).await;
        Ok(())
    }

    async fn on_exited(&mut self, session: &str, info: ExitInfo) -> anyhow::Result<()> {
        let Some(l) = self.live.remove(session) else { return Ok(()) };
        self.decisions.remove(session);
        let now = self.now();
        if let Some(inf) = &l.inflight {
            self.port.settle(&ids(&inf.wakes), Settle::Requeue, now).await?;
        }
        let mut patch = SessionPatch { last_active_at: Some(now), ..Default::default() };
        if l.stopping {
            patch.was_working = Some(false);
        }
        self.port.patch_session(session, patch).await?;
        if info.hit_limit {
            self.schedule_quota_for(session, &l.executor, &l.name, &l.crew).await?;
        }
        let launch_failed = l.health.as_ref().is_some_and(|h| h.kind == HealthKind::LaunchFailed);
        if launch_failed
            && !l.worked
            && let Some(row) = self.port.session(session).await?
        {
            let detail = l.health.as_ref().map(|h| h.detail.clone()).unwrap_or_default();
            self.launch_failed(&row, &detail).await?;
        }
        self.publish_live(session).await;
        Ok(())
    }

    async fn on_resume_refused(&mut self, session: &str, id: &str, said: &str) -> anyhow::Result<()> {
        let Some(l) = self.live.get(session) else { return Ok(()) };
        let (crew, name, executor) = (l.crew.clone(), l.name.clone(), l.executor.clone());
        let lang = self.lang();
        let diag = match self.execs.get(&executor).ok().and_then(|e| e.locate_transcript(id)) {
            Some(p) => text::render(lang, "resume.refused.diag.found", &[("path", &p.display().to_string())]),
            None => text::get(lang, "resume.refused.diag.missing").to_string(),
        };
        let group = text::render(lang, "resume.refused.group", &[("name", &name), ("said", said)]);
        self.port
            .post_system(Audience::Lobby { crew, mention_owner: false }, format!("{group}\n{diag}"))
            .await?;
        let now = self.now();
        let patch = SessionPatch { resume_failed_at: Some(Some(now)), ..Default::default() };
        self.port.patch_session(session, patch).await?;
        self.port
            .enqueue(NewWake {
                session: session.into(),
                kind: WakeKind::System,
                source: source::system(&format!("resume-refused:{id}")),
                priority: Priority::System,
                not_before: now,
                note: Some(text::render(lang, "resume.refused.agent", &[("id", id), ("said", said)])),
                origin: SYSTEM.into(),
            })
            .await?;
        Ok(())
    }

    // —— 额度（裁定 13）

    async fn schedule_quota(&mut self, session: &str) -> anyhow::Result<()> {
        let Some(l) = self.live.get(session) else { return Ok(()) };
        let (executor, name, crew) = (l.executor.clone(), l.name.clone(), l.crew.clone());
        self.schedule_quota_for(session, &executor, &name, &crew).await
    }

    pub(crate) async fn schedule_quota_for(
        &mut self,
        session: &str,
        executor: &str,
        name: &str,
        crew: &str,
    ) -> anyhow::Result<()> {
        if policy::quota_pending(&self.port.pending(session).await?) {
            return Ok(());
        }
        let now = self.now();
        let reset = self.hooks.quota.reset_at(executor, now).await;
        let plan = policy::quota_plan(reset, now);
        let lang = self.lang();
        self.port
            .enqueue(NewWake {
                session: session.into(),
                kind: WakeKind::System,
                source: plan.source(),
                priority: Priority::System,
                not_before: plan.fire_at,
                note: Some(text::get(lang, "quota.wake").to_string()),
                origin: SYSTEM.into(),
            })
            .await?;
        let minutes = (plan.fire_at.since(now) + MIN - 1) / MIN;
        let key = if plan.fallback { "quota.fallback" } else { "quota.scheduled" };
        let msg = text::render(lang, key, &[("name", name), ("minutes", &minutes.to_string())]);
        self.port.post_system(Audience::Lobby { crew: crew.into(), mention_owner: false }, msg).await
    }

    // —— 待决策（裁定 14）

    async fn raise_decision(&mut self, session: &str, d: &PendingDecision, now: Ms) -> anyhow::Result<()> {
        let fingerprint = d.fingerprint();
        if self.decisions.get(session).is_some_and(|r| r.fingerprint == fingerprint) {
            return Ok(());
        }
        self.decisions.insert(session.into(), Raised { at: now, fingerprint, escalated: false });
        self.post_decision(session, d, NoticeStage::First, 0).await
    }

    async fn post_decision(&self, session: &str, d: &PendingDecision, stage: NoticeStage, waited_min: i64) -> anyhow::Result<()> {
        let Some(l) = self.live.get(session) else { return Ok(()) };
        let facts = DecisionFacts { prompt: d.prompt.clone(), options: d.options.clone(), numbered: d.numbered };
        let n = notice::decision_notice(self.lang(), stage, &l.name, session, l.captain, &facts, waited_min);
        let to = if n.to_human {
            Audience::HumanChannel { crew: l.crew.clone() }
        } else {
            Audience::Lobby { crew: l.crew.clone(), mention_owner: true }
        };
        self.port.post_system(to, n.text).await
    }

    async fn escalate_decisions(&mut self) -> anyhow::Result<()> {
        let now = self.now();
        let due: Vec<String> = self
            .decisions
            .iter()
            .filter(|(_, r)| !r.escalated && now.since(r.at) >= DECISION_ESCALATE_MS)
            .map(|(s, _)| s.clone())
            .collect();
        for s in due {
            let Some(r) = self.decisions.get_mut(&s) else { continue };
            r.escalated = true;
            let waited = now.since(r.at) / MIN;
            let Some((d, captain)) = self.live.get(&s).and_then(|l| l.decision.clone().map(|d| (d, l.captain))) else {
                continue;
            };
            if !captain {
                // 机长的首报已经 @人了。
                self.post_decision(&s, &d, NoticeStage::Escalate, waited).await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn on_tick(&mut self) -> anyhow::Result<()> {
        self.escalate_decisions().await
    }

    // —— 状态面

    pub(crate) async fn live_view(&self, session: &str) -> Result<LiveView, ControlError> {
        let row = self
            .port
            .session(session)
            .await
            .map_err(ControlError::internal)?
            .ok_or_else(|| ControlError::NotFound(session.into()))?;
        let l = self.live.get(session);
        Ok(LiveView {
            session_id: session.into(),
            running: l.is_some(),
            working: l.is_some_and(|l| l.working),
            health: l.and_then(|l| l.health.clone()),
            pending_decision: l.and_then(|l| l.decision.clone()),
            awaiting_question: row.awaiting_question,
        })
    }

    pub(crate) async fn publish_live(&self, session: &str) {
        if let Ok(v) = self.live_view(session).await {
            self.port.publish(OrchestraEvent::SessionChanged(v));
        }
    }
}
```

Run: `cargo test -p crewd-server orchestra`
Expected: PASS（Task 6 的 7 条 + core 17 条）。若 `on_exec` 里 `l` 与 `self.xxx(..)` 的借用冲突编不过：在调用 `self` 方法之前先把要用的字段从 `l` 里取出来（`let d2 = d.clone();` 后不再碰 `l`），不改逻辑。

- [ ] **Step 4: 变异自证（提交前做，改完手工改回）**

把 `on_exec` 里 `if !l.refused { patch.resume_failed_at = Some(None); }` 改成无条件清 → `refused_resume_notifies_queues_a_note_and_counts_as_resume_failure` 必须红。把 `launch_failed` 里 `if already { return Ok(()); }` 删掉 → `launch_error_marks_resume_failed_and_alerts_the_owner_once` 必须红。改回后全绿。

- [ ] **Step 5: 门禁与提交**

Run: `./scripts/check.sh`
Expected: 全绿。

```bash
git add crates/server/src/orchestra/core.rs crates/server/src/orchestra/mod.rs crates/server/src/orchestra/testkit.rs
git diff --cached --stat
git commit -m "crewd-server: orchestra core — one process per session, launch in tasks, session events, resume refusal, quota, decisions

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 8: 唤醒入队与投递——规划、Arbiter、环路熔断、选批、回执、退避、idle 边沿、问被回答

**Files:**
- Create: `crewd/crates/server/src/orchestra/deliver.rs`
- Modify: `crewd/crates/server/src/orchestra/core.rs`（字段、`Input` / `Cmd` 新变体、几个事件分支接上投递）、`crewd/crates/server/src/orchestra/mod.rs`（加 `pub mod deliver;`）、`crewd/crates/server/src/orchestra/testkit.rs`（`Rig::post` / `ack` / `ack_brief`）

**Interfaces:**
- Consumes: Task 2 `plan::{plan, needs_arbiter, MsgFacts, ChannelCtx, Sender, Mention}`；Task 3 `breaker::{LoopBreaker, Verdict, origin_of, SYSTEM}`；Task 4 `queue::{select_batch, is_due, after_failure, ids, should_alert, MAX_ATTEMPTS}`、`policy::{quota_due, quota_pending, Gate}`；Task 6 `hooks::ArbiterQuestion`、`exec::Context`；Task 7 `Core`、`Input`、`Cmd`、`Inflight`、`launch`、`launch_failed`、`alert_audience`。
- Produces:
  - `core::CollabFact { MessageAppended { channel, seq }, WakeEnqueued { session }, AskAnswered { crew, ask, cleared }, Progress { session }, Lagged }`（Task 9 加 `RestartAnswered`）
  - `Input` 新增 `Collab(CollabFact)`、`Arbitrated { msg: MsgFacts, ctx: ChannelCtx, answer: Option<Vec<String>> }`、`Progress { session: String }`（测试与 `Handle::note_progress` 直接喂；生产路径是 `CollabFact::Progress`）
  - `Cmd` 新增 `SetCaptain { crew, session, reply: Reply<()> }`、`Wakes { session: Option<String>, reply: Reply<Vec<QueuedWake>> }`、`Recover { scope: LatchScope, id: String, reply: Reply<()> }`
  - `Core`（`deliver.rs`）：`pub(crate) async fn try_deliver(&mut self, session: &str) -> anyhow::Result<bool>`、`on_collab`、`enqueue_targets`、`on_delivery`、`fail_batch`、`on_idle`、`rescan`、`rescue_backlog`、`set_captain`、`recover`

- [ ] **Step 1: 改 `core.rs`（先让新变体能编过，投递逻辑下一步写在 `deliver.rs`）**

1. `use` 加：`use crewd_wake::breaker::LoopBreaker;`、`use crewd_wake::plan::{ChannelCtx, MsgFacts};`、`use crewd_wake::LatchScope;`、`use std::collections::BTreeSet;`（已有则略）。
2. 在 `Input` 前加：

```rust
/// 协作层发来的事实（M3a 的 `CollabEvent` = `crewd_collab::Change` 由 `db.rs` 换成这个）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollabFact {
    MessageAppended { channel: String, seq: i64 },
    /// 别人（M3a 的问被回答、M3b 的 schedule_wakeup、M4b 的检查点）入了队：试着投。
    WakeEnqueued { session: String },
    /// 问被回答（叫醒提问者的唤醒 M3a 已在同一事务里入队）；`cleared` = (工作项, 负责人)，交给 M4b。
    AskAnswered { crew: String, ask: String, cleared: Vec<(String, Option<String>)> },
    /// 这个 session 有了产出（M3a `sessions::note_progress` → `Change::Progress`：工具调用 / 工作项变化；发言不算，C3）。
    Progress { session: String },
    /// 订阅落后、漏了事件：按未读重新规划一遍（裁定 16）。
    Lagged,
}
```

3. `Input` 加三个变体：

```rust
    Collab(CollabFact),
    /// Arbiter（Jev）答完了（或超时）。
    Arbitrated { msg: MsgFacts, ctx: ChannelCtx, answer: Option<Vec<String>> },
    /// 这个 session 有了产出（与 `CollabFact::Progress` 同效；测试与 `Handle::note_progress` 用）。
    Progress { session: String },
```

4. `Cmd` 加三个变体：

```rust
    SetCaptain { crew: String, session: String, reply: Reply<()> },
    Wakes { session: Option<String>, reply: Reply<Vec<QueuedWake>> },
    /// 人解熔断（只有客户端能调）。
    Recover { scope: LatchScope, id: String, reply: Reply<()> },
```

5. `Core` 加字段（`use std::collections::HashMap` 已有）：

```rust
    pub(crate) breaker: LoopBreaker,
    /// 退避 / 错开产生的「这一刻再看一次」。
    pub(crate) timers: BTreeSet<Ms>,
    /// 失败重试的退避：唤醒 id → 不早于。同一时刻也经 `Settle::Retry { retry_at }` 写进库里的 `not_before`
    /// （M3a `mark_failed(.., retry_at_ms)`），重启后照样退避；内存这份只为选批时不必回读。
    pub(crate) backoff: HashMap<i64, Ms>,
    /// 已经看过的 `last_progress_at`（`Change::Progress` 丢了时的兜底，裁定 3）。
    pub(crate) seen_progress: HashMap<String, Ms>,
```

`Core::new` 里在 `Ok(Self { … })` 之前加 `let latches = port.latches().await?;`，构造时加 `breaker: LoopBreaker::new(cfg.loops, latches), timers: BTreeSet::new(), backoff: HashMap::new(), seen_progress: HashMap::new(),`（`cfg.loops` 是 `Copy`，先 `let loops = cfg.loops;` 再 move `cfg`）。

6. `handle` 的 match 加：

```rust
            Input::Collab(f) => self.on_collab(f).await,
            Input::Arbitrated { msg, ctx, answer } => self.enqueue_targets(&msg, &ctx, answer.as_deref()).await,
            Input::Progress { session } => {
                self.breaker.progress(&session);
                Ok(())
            }
```

7. `on_cmd` 的 match 加：

```rust
            Cmd::SetCaptain { crew, session, reply } => {
                let r = self.set_captain(&crew, &session).await;
                let _ = reply.send(r);
            }
            Cmd::Wakes { session, reply } => {
                let r = self.port.list_wakes(session.as_deref()).await.map_err(ControlError::internal);
                let _ = reply.send(r);
            }
            Cmd::Recover { scope, id, reply } => {
                let r = self.recover(scope, &id).await;
                let _ = reply.send(r);
            }
```

8. 会话事件接上投递：
   - `on_exec` 的 `SessionEvent::Working(on)` 分支末尾（`patch_session` 之后）加：`if !on { self.on_idle(session).await?; }`
   - `SessionEvent::Decision(DecisionEvent::Cleared)` 分支末尾加：`self.try_deliver(session).await?;`
   - `SessionEvent::Delivery { .. } => {}` 换成：`SessionEvent::Delivery { id, outcome, .. } => self.on_delivery(session, id, outcome).await?,`
   - `SessionEvent::Health(h)` 分支：在 `if quota { … }` 后加 `else if l_health_cleared { self.try_deliver(session).await?; }`——具体写法：先 `let cleared = h.is_none();`，分支末尾 `if cleared { self.try_deliver(session).await?; }`。
   - `on_exited` 末尾 `self.publish_live(session).await;` 之后加：`self.rescan().await?;`（腾出位置 / 错开到点）。
   - `on_launched` 的 `Err(e)` 分支改成（先记恢复失败再处理唤醒，放弃时就不会再告第二次警，裁定 9 / RF5）：

```rust
            Err(e) => {
                self.launch_failed(&st.row, &e).await?;
                self.fail_batch(session, st.wakes, &e).await?;
            }
```

   - `on_tick` 改成：

```rust
    pub(crate) async fn on_tick(&mut self) -> anyhow::Result<()> {
        let now = self.now();
        self.timers.retain(|t| *t > now);
        self.escalate_decisions().await?;
        self.rescan().await
    }
```

`mod.rs` 加 `pub mod deliver;`。

- [ ] **Step 2: `Rig` 加三个帮手（`testkit.rs`）**

```rust
use crewd_exec::delivery::Subject;
use crewd_exec::session::{BRIEF_DELIVERY_ID, DeliveryOutcome};
use crewd_wake::plan::Mention;

use super::core::CollabFact;

impl Rig {
    /// 往群里落一条消息（时间 = 现在），再把 `MessageAppended` 交给 actor。
    pub async fn post(&mut self, channel: &str, sender: Sender, mentions: Vec<Mention>, text: &str) -> MessageRef {
        let r = {
            let mut st = self.port.lock();
            let seq = st.messages.keys().filter(|(c, _)| c == channel).map(|(_, s)| *s).max().unwrap_or(0) + 1;
            let r = MessageRef { channel: channel.into(), seq };
            let facts = MsgFacts { id: format!("{channel}#{seq}"), at: r.clone(), sender, mentions, created_at: st.now };
            st.messages.insert((channel.into(), seq), (facts, text.into()));
            r
        };
        let fact = CollabFact::MessageAppended { channel: r.channel.clone(), seq: r.seq };
        self.core.handle(Input::Collab(fact)).await;
        self.settle().await;
        r
    }

    /// 最后一笔 send 投进去了。
    pub async fn ack(&mut self, session: &str) {
        let id = self.exec.last_send_id(session);
        self.emit(session, SessionEvent::Delivery { id, subject: Subject::Steer, outcome: DeliveryOutcome::Delivered }).await;
    }

    /// 拉起时带的开场正文投进去了。
    pub async fn ack_brief(&mut self, session: &str) {
        let e = SessionEvent::Delivery { id: BRIEF_DELIVERY_ID, subject: Subject::Brief, outcome: DeliveryOutcome::Delivered };
        self.emit(session, e).await;
    }
}
```

- [ ] **Step 3: 写失败的测试**

`crewd/crates/server/src/orchestra/deliver.rs` 先放测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestra::core::Cmd;
    use crate::orchestra::exec::BoxFut;
    use crate::orchestra::hooks::{Arbiter, AskAnsweredHook, Hooks, IdleHook};
    use crate::orchestra::testkit::{Rig, T0};
    use crewd_exec::delivery::Subject;
    use crewd_exec::health::Health;
    use crewd_exec::session::{ExitInfo, ExitStatusKind, SessionEvent};
    use crewd_term::{DecisionEvent, PendingDecision};
    use crewd_wake::breaker::LOOP_LIMITS;
    use crewd_wake::plan::Mention;
    use crewd_wake::{MIN, SEC, WakeState};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    const LOBBY: &str = "c1/lobby";

    fn at(id: &str) -> Vec<Mention> {
        vec![Mention::Session(id.into())]
    }

    async fn crew_with(rig: &mut Rig, workers: &[&str], running: &[&str]) {
        rig.crew("c1", "cap");
        for w in workers {
            rig.worker("c1", w);
        }
        for s in running {
            rig.run(s).await;
        }
    }

    async fn fail_last(rig: &mut Rig, session: &str) {
        let id = rig.exec.last_send_id(session);
        let outcome = DeliveryOutcome::Undelivered("合成：没投进去".into());
        rig.emit(session, SessionEvent::Delivery { id, subject: Subject::Steer, outcome }).await;
    }

    #[tokio::test]
    async fn mention_wakes_an_idle_running_target_at_once() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲").await;
        assert_eq!(rig.exec.sent("w1"), ["合成派活甲"]);
        let w = rig.port.wakes_of("w1");
        assert_eq!((w[0].state, w[0].priority, w[0].source.as_str()), (WakeState::Inflight, Priority::Auto, "msg:c1/lobby#1"));
        assert_eq!(w[0].origin, "cap");
    }

    #[tokio::test]
    async fn busy_target_waits_for_the_idle_edge() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲").await;
        assert!(rig.exec.sent("w1").is_empty());
        rig.emit("w1", SessionEvent::Working(false)).await;
        assert_eq!(rig.exec.sent("w1"), ["合成派活甲"]);
    }

    #[tokio::test]
    async fn already_injected_message_is_settled_not_resent() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        let r = rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲").await;
        // 忙时 hook 已把它注入上下文（M3b 推进同一个游标）。
        rig.port.advance("w1", &r.channel, r.seq).await.unwrap();
        rig.emit("w1", SessionEvent::Working(false)).await;
        assert!(rig.exec.sent("w1").is_empty());
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Delivered);
    }

    #[tokio::test]
    async fn delivered_settles_and_advances_the_cursor() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        let r = rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲").await;
        rig.ack("w1").await;
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Delivered);
        assert_eq!(rig.port.cursor("w1", LOBBY).await.unwrap(), r.seq);
    }

    #[tokio::test]
    async fn failures_back_off_30s_then_2min_then_drop_with_one_alert() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲").await;
        fail_last(&mut rig, "w1").await;
        let w = rig.port.wakes_of("w1")[0].clone();
        assert_eq!((w.state, w.attempts), (WakeState::Pending, 1));
        assert_eq!(rig.core.backoff[&w.id], T0.add(30 * SEC));
        rig.tick(29 * SEC).await;
        assert_eq!(rig.exec.sent("w1").len(), 1, "退避没到不重投");
        rig.tick(SEC).await;
        assert_eq!(rig.exec.sent("w1").len(), 2);
        fail_last(&mut rig, "w1").await;
        assert_eq!(rig.core.backoff[&w.id], T0.add(30 * SEC + 2 * MIN));
        rig.tick(2 * MIN).await;
        fail_last(&mut rig, "w1").await;
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Dropped);
        let posts = rig.port.posts();
        assert_eq!(posts.len(), 1, "{posts:?}");
        assert_eq!(posts[0].0, Audience::Lobby { crew: "c1".into(), mention_owner: true });
    }

    #[tokio::test]
    async fn due_wakes_merge_into_one_delivery_of_the_highest_tier_only() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        rig.post(LOBBY, Sender::Human, at("w1"), "人甲").await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "机长乙").await;
        rig.post(LOBBY, Sender::Human, at("w1"), "人丙").await;
        rig.emit("w1", SessionEvent::Working(false)).await;
        assert_eq!(rig.exec.sent("w1"), ["人甲\n---\n人丙"]);
        rig.ack("w1").await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        rig.emit("w1", SessionEvent::Working(false)).await;
        assert_eq!(rig.exec.sent("w1")[1], "机长乙");
    }

    #[tokio::test]
    async fn stopped_session_is_resumed_by_a_mention_with_the_wake_as_opening_text() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &[]).await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲").await;
        assert_eq!(rig.exec.launches("w1")[0].initial.as_deref(), Some("合成派活甲"));
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Inflight);
        rig.ack_brief("w1").await;
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Delivered);
    }

    #[tokio::test]
    async fn stop_drops_pending_wakes_and_does_not_relaunch() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲").await;
        rig.cmd(|reply| Cmd::Stop { session: "w1".into(), reply }).await.unwrap();
        let info = ExitInfo { status: ExitStatusKind::Signal(15), stopped_by_user: true, hit_limit: false };
        rig.emit("w1", SessionEvent::Exited(info)).await;
        rig.tick(MIN).await;
        assert_eq!(rig.exec.launches("w1").len(), 1, "积压的唤醒不能把刚停掉的它拉起来");
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活乙").await;
        assert_eq!(rig.exec.launches("w1").len(), 2, "新来的 @ 照样拉起它（§22.4）");
    }

    #[tokio::test]
    async fn pending_decision_holds_delivery_until_cleared() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        let d = PendingDecision { prompt: "Trust?".into(), options: vec!["Yes".into()], numbered: true };
        rig.emit("w1", SessionEvent::Decision(DecisionEvent::Appeared(d))).await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲").await;
        assert!(rig.exec.sent("w1").is_empty(), "绝不把正文敲进对话框");
        rig.emit("w1", SessionEvent::Decision(DecisionEvent::Cleared)).await;
        assert_eq!(rig.exec.sent("w1"), ["合成派活甲"]);
    }

    /// 机长与 w1 互相 @ 打转，直到某一轮 cap → w1 没送到。返回送到的轮数。
    async fn ping_pong_until_cut(rig: &mut Rig) -> usize {
        for round in 0..20 {
            rig.post(LOBBY, Rig::sender("cap"), at("w1"), &format!("合成派活 {round}")).await;
            if rig.exec.sent("w1").len() <= round {
                return round;
            }
            rig.ack("w1").await;
            rig.post(LOBBY, Rig::sender("w1"), at("cap"), &format!("合成回报 {round}")).await;
            rig.ack("cap").await;
        }
        panic!("20 轮都没截住");
    }

    #[tokio::test]
    async fn mutual_mention_loop_is_cut_on_one_edge_and_human_still_gets_through() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["cap", "w1"]).await;
        assert_eq!(ping_pong_until_cut(&mut rig).await, LOOP_LIMITS.edge_max);
        let latches = rig.port.lock().latches.clone();
        assert_eq!(latches.len(), 1);
        assert_eq!((latches[0].scope, latches[0].id.as_str()), (LatchScope::Edge, "cap>w1"));
        let posts = rig.port.posts();
        assert!(posts.iter().any(|(to, _)| *to == Audience::HumanChannel { crew: "c1".into() }), "告诉人");
        assert!(posts.iter().any(|(to, _)| *to == Audience::Channel { channel: LOBBY.into(), mention_owner: true }), "告诉群主");
        let before = rig.exec.sent("w1").len();
        rig.post(LOBBY, Sender::Human, at("w1"), "人来了").await;
        assert_eq!(rig.exec.sent("w1").len(), before + 1, "人的唤醒永不被挡");
        assert_eq!(rig.exec.sent("w1").last().map(String::as_str), Some("人来了"));
    }

    #[tokio::test]
    async fn recover_clears_the_latch_and_delivers_what_was_held() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["cap", "w1"]).await;
        ping_pong_until_cut(&mut rig).await;
        let before = rig.exec.sent("w1").len();
        rig.cmd(|reply| Cmd::Recover { scope: LatchScope::Edge, id: "cap>w1".into(), reply }).await.unwrap();
        assert!(rig.port.lock().latches.is_empty());
        assert_eq!(rig.exec.sent("w1").len(), before + 1, "被挡的那条在恢复后送到");
        let again = rig.cmd(|reply| Cmd::Recover { scope: LatchScope::Edge, id: "cap>w1".into(), reply }).await;
        assert!(matches!(again, Err(ControlError::NotFound(_))));
    }

    #[tokio::test]
    async fn thirty_sessions_working_normally_are_never_tripped() {
        let workers: Vec<String> = (1..=30).map(|i| format!("w{i}")).collect();
        let names: Vec<&str> = workers.iter().map(String::as_str).collect();
        let mut rig = Rig::new().await;
        let mut running = names.clone();
        running.push("cap");
        crew_with(&mut rig, &names, &running).await;
        for round in 0..3 {
            for w in &names {
                rig.post(LOBBY, Rig::sender("cap"), at(w), &format!("合成派活 {w} {round}")).await;
                rig.ack(w).await;
                rig.core.handle(Input::Progress { session: w.to_string() }).await; // 工具调用（M3b 报）
                rig.post(LOBBY, Rig::sender(w), at("cap"), &format!("合成回报 {w} {round}")).await;
                rig.ack("cap").await;
            }
        }
        assert!(rig.port.lock().latches.is_empty());
        for w in &names {
            assert_eq!(rig.exec.sent(w).len(), 3, "{w}");
        }
        assert_eq!(rig.exec.sent("cap").len(), 90);
    }

    struct Picks(Vec<String>);
    impl Arbiter for Picks {
        fn wake_set<'a>(&'a self, _q: &'a ArbiterQuestion) -> BoxFut<'a, Option<Vec<String>>> {
            Box::pin(async move { Some(self.0.clone()) })
        }
    }

    struct Hangs;
    impl Arbiter for Hangs {
        fn wake_set<'a>(&'a self, _q: &'a ArbiterQuestion) -> BoxFut<'a, Option<Vec<String>>> {
            Box::pin(std::future::pending())
        }
    }

    #[tokio::test]
    async fn arbiter_answer_picks_who_wakes() {
        let mut rig = Rig::with(Hooks { arbiter: Arc::new(Picks(vec!["w2".into()])), ..Hooks::default() }).await;
        crew_with(&mut rig, &["w1", "w2"], &["cap", "w1", "w2"]).await;
        rig.post(LOBBY, Sender::Human, vec![], "合成随口一句").await;
        assert_eq!(rig.exec.sent("w2"), ["合成随口一句"]);
        assert!(rig.exec.sent("cap").is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn hanging_arbiter_times_out_to_the_owner() {
        let mut rig = Rig::with(Hooks { arbiter: Arc::new(Hangs), ..Hooks::default() }).await;
        crew_with(&mut rig, &["w1"], &["cap", "w1"]).await;
        rig.post(LOBBY, Sender::Human, vec![], "合成随口一句").await;
        assert!(rig.exec.sent("cap").is_empty(), "还在等 Arbiter");
        tokio::time::advance(std::time::Duration::from_millis(60)).await;
        rig.settle().await;
        assert_eq!(rig.exec.sent("cap"), ["合成随口一句"], "超时 = 没答案 → 群主");
    }

    struct OnceStopCheck(AtomicBool);
    impl IdleHook for OnceStopCheck {
        fn on_idle<'a>(&'a self, session: &'a str, now: Ms) -> BoxFut<'a, Option<NewWake>> {
            let fire = !self.0.swap(true, Ordering::SeqCst);
            Box::pin(async move {
                fire.then(|| NewWake {
                    session: session.into(),
                    kind: WakeKind::StopCheck,
                    source: source::system("sc-1"),
                    priority: Priority::System,
                    not_before: now,
                    note: Some("合成停下即检查".into()),
                    origin: "system".into(),
                })
            })
        }
    }

    #[tokio::test]
    async fn idle_hook_runs_only_when_the_queue_had_nothing_to_deliver() {
        let mut rig = Rig::with(Hooks { idle: Arc::new(OnceStopCheck(AtomicBool::new(false))), ..Hooks::default() }).await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        rig.emit("w1", SessionEvent::Working(false)).await;
        assert_eq!(rig.exec.sent("w1"), ["合成停下即检查"]);
    }

    #[derive(Default)]
    struct RecordAsks(Mutex<Vec<String>>);
    impl AskAnsweredHook for RecordAsks {
        fn on_answered<'a>(
            &'a self,
            crew: &'a str,
            ask: &'a str,
            cleared: &'a [(String, Option<String>)],
            _now: Ms,
        ) -> BoxFut<'a, ()> {
            self.0.lock().unwrap().push(format!("{crew}/{ask} {cleared:?}"));
            Box::pin(async {})
        }
    }

    #[tokio::test]
    async fn wakes_queued_by_others_are_delivered_and_answered_asks_reach_the_m4b_hook() {
        let asks = Arc::new(RecordAsks::default());
        let mut rig = Rig::with(Hooks { ask: asks.clone(), ..Hooks::default() }).await;
        crew_with(&mut rig, &["w1"], &["cap", "w1"]).await;
        // M3a 的 asks::answer 在同一事务里入队叫醒提问者，再发 WakeEnqueued / AskAnswered。
        let w = NewWake {
            session: "w1".into(),
            kind: WakeKind::AskAnswer,
            source: "ask:c1/H1".into(),
            priority: Priority::Human,
            not_before: T0,
            note: None,
            origin: "human".into(),
        };
        rig.port.enqueue(w).await.unwrap();
        rig.core.handle(Input::Collab(CollabFact::WakeEnqueued { session: "w1".into() })).await;
        rig.settle().await;
        assert_eq!(rig.exec.sent("w1"), ["ask_answer:ask:c1/H1"]);
        let cleared = vec![("W3".to_string(), Some("w1".to_string()))];
        rig.core.handle(Input::Collab(CollabFact::AskAnswered { crew: "c1".into(), ask: "H1".into(), cleared })).await;
        rig.settle().await;
        assert_eq!(*asks.0.lock().unwrap(), [r#"c1/H1 [("W3", Some("w1"))]"#]);
    }

    #[tokio::test]
    async fn tool_output_between_wakes_counts_as_progress() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["cap", "w1"]).await;
        for round in 0..(LOOP_LIMITS.edge_max + 4) {
            rig.post(LOBBY, Rig::sender("cap"), at("w1"), &format!("合成派活 {round}")).await;
            assert_eq!(rig.exec.sent("w1").len(), round + 1, "第 {round} 轮被截了");
            rig.ack("w1").await;
            // 两边在回话之间都调过工具（M3a `note_progress` → `Change::Progress` → `CollabFact::Progress`）。
            for s in ["w1", "cap"] {
                rig.core.handle(Input::Collab(CollabFact::Progress { session: s.into() })).await;
            }
            rig.post(LOBBY, Rig::sender("w1"), at("cap"), &format!("合成回报 {round}")).await;
            rig.ack("cap").await;
        }
        assert!(rig.port.lock().latches.is_empty());
    }

    #[tokio::test]
    async fn quota_wake_clears_quota_health_before_delivering_and_holds_others_until_then() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        rig.emit("w1", SessionEvent::Health(Some(Health::new(crewd_exec::health::HealthKind::UsageLimit, "x")))).await;
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲").await;
        assert!(rig.exec.sent("w1").is_empty(), "额度没恢复不投");
        rig.tick(45 * MIN).await;
        assert_eq!(rig.exec.lock().cleared_quota, ["w1"]);
        assert_eq!(rig.exec.sent("w1"), [crewd_wake::text::get(crewd_wake::text::Lang::Zh, "quota.wake")]);
    }

    #[tokio::test]
    async fn launch_failures_back_off_then_drop_with_one_alert() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &[]).await;
        rig.exec.lock().fail_next = vec!["合成：起不来".into(); 3];
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲").await;
        rig.tick(30 * SEC).await;
        rig.tick(2 * MIN).await;
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Dropped);
        assert!(rig.port.row("w1").resume_failed_at.is_some());
        assert_eq!(rig.port.posts().len(), 1, "只告警一次：{:?}", rig.port.posts());
        rig.tick(10 * MIN).await;
        assert!(rig.exec.launches("w1").is_empty(), "三次都没起来，没有第四次");
    }

    #[tokio::test]
    async fn wakes_for_an_unsupported_executor_are_dropped_with_one_alert() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &[]).await;
        rig.port.lock().sessions.get_mut("w1").unwrap().executor = "codex".into();
        rig.post(LOBBY, Rig::sender("cap"), at("w1"), "合成派活甲").await;
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Dropped);
        assert_eq!(rig.port.posts().len(), 1);
    }

    #[tokio::test]
    async fn set_captain_swaps_roles_and_tells_both() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["cap", "w1"]).await;
        rig.cmd(|reply| Cmd::SetCaptain { crew: "c1".into(), session: "w1".into(), reply }).await.unwrap();
        assert!(rig.port.row("w1").captain && !rig.port.row("cap").captain);
        assert!(rig.live("w1").unwrap().captain);
        assert_eq!(rig.exec.sent("w1"), [crewd_wake::text::get(crewd_wake::text::Lang::Zh, "captain.changed.new")]);
        assert_eq!(rig.exec.sent("cap").len(), 1);
    }

    #[tokio::test]
    async fn lagged_subscription_replans_unread_mentions() {
        let mut rig = Rig::new().await;
        crew_with(&mut rig, &["w1"], &["w1"]).await;
        // 消息落库了，但 MessageAppended 丢了（订阅落后）。
        rig.port.lock().messages.insert(
            (LOBBY.into(), 1),
            (
                MsgFacts {
                    id: "m-lag".into(),
                    at: MessageRef { channel: LOBBY.into(), seq: 1 },
                    sender: Rig::sender("cap"),
                    mentions: at("w1"),
                    created_at: T0,
                },
                "合成派活甲".into(),
            ),
        );
        rig.core.handle(Input::Collab(CollabFact::Lagged)).await;
        rig.settle().await;
        assert_eq!(rig.exec.sent("w1"), ["合成派活甲"]);
    }
}
```

Run: `cargo test -p crewd-server orchestra::deliver`
Expected: 编译失败（`on_collab` 等未定义）。

- [ ] **Step 4: 实现 `deliver.rs`**

在测试模块上方写：

```rust
//! 唤醒的入队与投递（spec §22.4「唤醒规划」「队列」「idle 边沿」「问被回答」；§22.7 Q2 / Q5）。
use std::collections::BTreeMap;

use crewd_exec::health::HealthKind;
use crewd_exec::session::DeliveryOutcome;
use crewd_wake::breaker::{self, Verdict};
use crewd_wake::plan::{self, ChannelCtx, MsgFacts, Sender};
use crewd_wake::policy::{self, Gate};
use crewd_wake::queue::{self, ids};
use crewd_wake::{Latch, LatchScope, MIN, MessageRef, Ms, NewWake, Priority, QueuedWake, WakeKind, source, text};

use super::core::{CollabFact, ControlError, Core, Inflight, Input};
use super::exec::Context;
use super::hooks::ArbiterQuestion;
use super::port::*;

/// 不需要正文的唤醒（`restart` 且没有附言）与其余的分开：前者只要「人在跑」就算送到，不依赖渲染结果是否为空。
fn split_silent(batch: Vec<QueuedWake>) -> (Vec<QueuedWake>, Vec<QueuedWake>) {
    batch.into_iter().partition(|w| w.kind == WakeKind::Restart && w.note.is_none())
}

impl<P: Port> Core<P> {
    pub(crate) async fn on_collab(&mut self, f: CollabFact) -> anyhow::Result<()> {
        match f {
            CollabFact::MessageAppended { channel, seq } => self.on_message(MessageRef { channel, seq }).await,
            CollabFact::WakeEnqueued { session } => self.try_deliver(&session).await.map(|_| ()),
            CollabFact::AskAnswered { crew, ask, cleared } => {
                // 叫醒提问者（或机长转达）的唤醒由 M3a 的 `asks::answer` 在同一事务里入队，它的 `WakeEnqueued`
                // 会把它投出去；M3c 这里只把 BlockerCleared 交给 M4b 的回调点（§22.6）。
                self.hooks.ask.on_answered(&crew, &ask, &cleared, self.now()).await;
                Ok(())
            }
            CollabFact::Progress { session } => {
                self.breaker.progress(&session);
                Ok(())
            }
            CollabFact::Lagged => self.rescue_backlog().await,
        }
    }

    async fn on_message(&mut self, r: MessageRef) -> anyhow::Result<()> {
        let Some((msg, text)) = self.port.message(&r).await? else { return Ok(()) };
        let Some(ctx) = self.port.channel_ctx(&r.channel).await? else { return Ok(()) };
        if plan::needs_arbiter(&msg, self.now()) {
            // Jev 要联网：放到派生任务里问，答案（或超时）以 `Input::Arbitrated` 回来，不卡 actor。
            let (arbiter, tx, limit) = (self.hooks.arbiter.clone(), self.inbox.clone(), self.cfg.arbiter_timeout);
            tokio::spawn(async move {
                let q = ArbiterQuestion { message: msg, text, ctx };
                let answer = tokio::time::timeout(limit, arbiter.wake_set(&q)).await.ok().flatten();
                let _ = tx.send(Input::Arbitrated { msg: q.message, ctx: q.ctx, answer });
            });
            return Ok(());
        }
        self.enqueue_targets(&msg, &ctx, None).await
    }

    pub(crate) async fn enqueue_targets(
        &mut self,
        msg: &MsgFacts,
        ctx: &ChannelCtx,
        answer: Option<&[String]>,
    ) -> anyhow::Result<()> {
        let now = self.now();
        let targets = plan::plan(msg, ctx, answer, now);
        if let Sender::Session(poster) = &msg.sender {
            let woke: Vec<String> = targets.iter().map(|t| t.session.clone()).collect();
            self.breaker.on_post(poster, &woke); // 裁定 3：这条消息算不算产出（回叫叫醒自己的人不算）
        }
        for t in targets {
            let w = NewWake {
                session: t.session.clone(),
                kind: WakeKind::Message,
                source: source::message(&msg.id),
                priority: t.priority,
                not_before: now,
                note: None,
                origin: msg.sender.key().to_string(),
            };
            if self.port.enqueue(w).await? {
                self.try_deliver(&t.session).await?;
            }
        }
        Ok(())
    }

    /// 能投就投（裁定 7、8）。返回这次有没有投出 / 拉起。
    pub(crate) async fn try_deliver(&mut self, session: &str) -> anyhow::Result<bool> {
        let now = self.now();
        // 每轮可能把「已注入」的消息类唤醒直接记 delivered，然后重选；最多 4 轮。
        for _ in 0..4 {
            if self.starting.contains_key(session) {
                return Ok(false);
            }
            let pending = self.port.pending(session).await?;
            if !pending.iter().any(|w| queue::is_due(w, now)) {
                return Ok(false);
            }
            if !self.live_ready(session, &pending, now) {
                return Ok(false);
            }
            let mut origins: BTreeMap<i64, String> = BTreeMap::new();
            let mut candidates = Vec::new();
            for w in pending.iter().filter(|w| queue::is_due(w, now)) {
                let backing_off = self.backoff.get(&w.id).is_some_and(|t| *t > now);
                if !backing_off && self.breaker.held(session, &w.origin).is_none() {
                    origins.insert(w.id, w.origin.clone());
                    candidates.push(w.clone());
                }
            }
            let batch = queue::select_batch(&candidates, now, &self.cfg.batch);
            if batch.is_empty() {
                return Ok(false);
            }
            let mut fresh = Vec::new();
            let mut injected = Vec::new();
            for w in batch {
                if self.injected(session, &w).await? {
                    injected.push(w.id);
                } else {
                    fresh.push(w);
                }
            }
            if !injected.is_empty() {
                self.port.settle(&injected, Settle::Delivered, now).await?;
                self.port.publish(OrchestraEvent::WakeChanged { session: session.into() });
            }
            if fresh.is_empty() {
                continue;
            }
            let running = self.live.contains_key(session);
            if !running && !self.may_launch(session, &fresh, &pending, now) {
                return Ok(false);
            }
            let fresh = self.admit(session, fresh, &origins, now).await?;
            if fresh.is_empty() {
                continue;
            }
            return if running {
                self.send_batch(session, fresh).await
            } else {
                self.launch_for_wakes(session, fresh).await
            };
        }
        Ok(false)
    }

    /// 在跑的目标此刻能不能投（裁定 7）。额度类 health 在身且额度唤醒到点 → 先清 health。没在跑 → true（交给拉起）。
    fn live_ready(&mut self, session: &str, pending: &[QueuedWake], now: Ms) -> bool {
        let Some(l) = self.live.get_mut(session) else { return true };
        if l.inflight.is_some() || l.working || l.decision.is_some() || l.stopping {
            return false;
        }
        match &l.health {
            Some(h) if h.kind.is_quota() => {
                if !policy::quota_due(pending, now) {
                    return false;
                }
                let _ = l.handle.clear_quota_health();
                l.health = None;
                true
            }
            Some(h) if matches!(h.kind, HealthKind::AuthRequired | HealthKind::LaunchFailed) => false,
            _ => true,
        }
    }

    /// 没在跑的目标要不要为这批唤醒拉起（裁定 4、5、13）。
    fn may_launch(&mut self, session: &str, batch: &[QueuedWake], pending: &[QueuedWake], now: Ms) -> bool {
        if self.held.contains(session) {
            // 等人答重启询问（§22.7 Q1）：不被自动拉起。例外（控制者裁定 C4）：询问开出之后人新 @ 了它
            // → 只拉起它（`launch_for_wakes` 把它移出名单，算对它单个 resume）；启动前就排着的人类唤醒不算。
            let called_by_human = batch
                .iter()
                .any(|w| w.priority == Priority::Human && w.kind == WakeKind::Message && w.id > self.held_watermark);
            if !called_by_human {
                return false;
            }
        }
        let human = batch.first().is_some_and(|w| w.priority == Priority::Human);
        if !human && policy::quota_pending(pending) && !policy::quota_due(pending, now) {
            return false; // 额度没恢复：自动唤醒别把它拉起来又撞墙
        }
        match self.gate.check(self.live.len() + self.starting.len(), human, now) {
            Gate::Go => true,
            Gate::AtCap => false,
            Gate::WaitUntil(t) => {
                self.timers.insert(t);
                false
            }
        }
    }

    async fn injected(&self, session: &str, w: &QueuedWake) -> anyhow::Result<bool> {
        let (WakeKind::Message, Some(r)) = (w.kind, &w.msg) else { return Ok(false) };
        Ok(self.port.cursor(session, &r.channel).await? >= r.seq)
    }

    /// 产出的兜底信号（裁定 3）：正路是 `CollabFact::Progress`；事件丢了（`Lagged`）时，`last_progress_at`
    /// （M3a `note_progress` 写，发言不写）比上次看到的新 = 有过产出。
    async fn observe_progress(&mut self, session: &str) -> anyhow::Result<()> {
        let Some(at) = self.port.session(session).await?.and_then(|r| r.last_progress_at) else { return Ok(()) };
        if self.seen_progress.get(session) == Some(&at) {
            return Ok(());
        }
        let first = self.seen_progress.insert(session.to_string(), at).is_none();
        if !first {
            self.breaker.progress(session);
        }
        Ok(())
    }

    /// 环路熔断（§22.7 Q5）：每个来源问一次；被挡的那几条留在 pending。
    async fn admit(
        &mut self,
        session: &str,
        batch: Vec<QueuedWake>,
        origins: &BTreeMap<i64, String>,
        now: Ms,
    ) -> anyhow::Result<Vec<QueuedWake>> {
        self.observe_progress(session).await?;
        let mut pass: BTreeMap<String, bool> = BTreeMap::new();
        for w in &batch {
            let o = &origins[&w.id];
            if pass.contains_key(o) {
                continue;
            }
            let ok = match self.breaker.admit(session, o, now) {
                Verdict::Pass => true,
                Verdict::Held(_) => false,
                Verdict::Tripped(latch) => {
                    self.port.put_latch(&latch).await?;
                    self.notify_loop(session, &latch, o, w).await?;
                    false
                }
            };
            pass.insert(o.clone(), ok);
        }
        Ok(batch.into_iter().filter(|w| pass[&origins[&w.id]]).collect())
    }

    async fn notify_loop(&mut self, target: &str, latch: &Latch, origin: &str, w: &QueuedWake) -> anyhow::Result<()> {
        let lang = self.lang();
        let Some(row) = self.port.session(target).await? else { return Ok(()) };
        let origin_name = if origin == breaker::SYSTEM {
            text::get(lang, "origin.system").to_string()
        } else {
            self.port.session(origin).await?.map_or_else(|| origin.to_string(), |r| r.name())
        };
        let count = match latch.scope {
            LatchScope::Edge => self.cfg.loops.edge_max,
            LatchScope::Session => self.cfg.loops.target_max,
        } + 1;
        let minutes = self.cfg.loops.window_ms / MIN;
        let vars = [
            ("target", row.name()),
            ("origin", origin_name.clone()),
            ("count", count.to_string()),
            ("minutes", minutes.to_string()),
        ];
        let vars: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let human = text::render(lang, "loop.tripped.human", &vars);
        self.port.post_system(Audience::HumanChannel { crew: row.crew_id.clone() }, human).await?;
        let owner = text::render(lang, "loop.tripped.owner", &[("target", &row.name()), ("origin", &origin_name)]);
        let to = match (w.kind, &w.msg) {
            (WakeKind::Message, Some(r)) => Audience::Channel { channel: r.channel.clone(), mention_owner: true },
            _ => Audience::Lobby { crew: row.crew_id, mention_owner: true },
        };
        self.port.post_system(to, owner).await
    }

    /// 渲染；超过 8KB 就从尾部减条数（至少留 1 条；减下的留在 pending）。渲染不出来的（Port 已记 dropped）拿掉。
    async fn render_fitted(&self, session: &str, mut batch: Vec<QueuedWake>) -> anyhow::Result<(Vec<QueuedWake>, String)> {
        loop {
            let r = self.port.render_wake(session, &batch).await?;
            batch.retain(|w| !r.dropped.contains(&w.id));
            if r.text.len() <= self.cfg.batch.max_bytes || batch.len() <= 1 {
                return Ok((batch, r.text));
            }
            batch.pop();
        }
    }

    async fn send_batch(&mut self, session: &str, batch: Vec<QueuedWake>) -> anyhow::Result<bool> {
        let now = self.now();
        let (silent, speak) = split_silent(batch);
        if !silent.is_empty() {
            // 不需要正文的唤醒（无附言的恢复）：在跑就算送到。
            self.port.settle(&ids(&silent), Settle::Delivered, now).await?;
        }
        if speak.is_empty() {
            return Ok(true);
        }
        let (batch, text) = self.render_fitted(session, speak).await?;
        if batch.is_empty() {
            return Ok(false);
        }
        let batch_ids = ids(&batch);
        let accepts = self.live.get(session).is_some_and(|l| l.handle.accepts_context());
        let extras = if accepts { self.hooks.extras.extras(session).await } else { Context::new() };
        self.port.settle(&batch_ids, Settle::Inflight, now).await?;
        let Some(l) = self.live.get_mut(session) else {
            self.port.settle(&batch_ids, Settle::Requeue, now).await?;
            return Ok(false);
        };
        match l.handle.send(text, &extras) {
            Ok(delivery) => {
                l.inflight = Some(Inflight { delivery, wakes: batch });
                self.port.publish(OrchestraEvent::WakeChanged { session: session.into() });
                Ok(true)
            }
            Err(e) => {
                self.fail_batch(session, batch, &e.to_string()).await?;
                Ok(false)
            }
        }
    }

    async fn launch_for_wakes(&mut self, session: &str, batch: Vec<QueuedWake>) -> anyhow::Result<bool> {
        let now = self.now();
        let Some(row) = self.port.session(session).await? else {
            self.port.settle(&ids(&batch), Settle::Dropped { error: "no such session".into() }, now).await?;
            return Ok(false);
        };
        let exec = match self.execs.get(&row.executor) {
            Ok(e) if row.workdir.is_some() => e,
            other => {
                // 重试也起不来（执行器没接入 / 没有工作目录）：记恢复失败、告警一次，唤醒直接放弃。
                let detail = match other {
                    Err(e) => e.to_string(),
                    Ok(_) => ControlError::NoWorkdir(row.id.clone()).to_string(),
                };
                self.launch_failed(&row, &detail).await?;
                self.port.settle(&ids(&batch), Settle::Dropped { error: detail }, now).await?;
                return Ok(false);
            }
        };
        // C4：等重启答复的 session 被人 @ 拉起 = 对它单个 resume；上次停在干活中途的，开场补一句「被打断」。
        let interrupted = (self.held.remove(session) && row.was_working)
            .then(|| text::get(self.lang(), "restart.interrupted").to_string());
        let (silent, speak) = split_silent(batch);
        if !silent.is_empty() {
            // 无附言的恢复：拉起本身就是它要的（起不来由 launch_failed 告警）。
            self.port.settle(&ids(&silent), Settle::Delivered, now).await?;
        }
        if speak.is_empty() {
            self.launch(row, exec, interrupted, vec![]).await?;
            return Ok(true);
        }
        let (batch, text) = self.render_fitted(session, speak).await?;
        if batch.is_empty() {
            return Ok(false);
        }
        let text = match interrupted {
            Some(note) => format!("{note}\n\n{text}"),
            None => text,
        };
        self.launch(row, exec, Some(text), batch).await?;
        Ok(true)
    }

    pub(crate) async fn on_delivery(&mut self, session: &str, id: u64, outcome: DeliveryOutcome) -> anyhow::Result<()> {
        if !outcome.is_final() {
            return Ok(()); // Blocked / NotReady / Stalled：驱动层自己会补投，等终局
        }
        let Some(l) = self.live.get_mut(session) else { return Ok(()) };
        if l.inflight.as_ref().is_none_or(|f| f.delivery != id) {
            return Ok(());
        }
        let inf = l.inflight.take().expect("上面查过");
        let now = self.now();
        if outcome == DeliveryOutcome::Delivered {
            l.worked = true;
            self.port.settle(&ids(&inf.wakes), Settle::Delivered, now).await?;
            let mut tops: BTreeMap<String, i64> = BTreeMap::new();
            for r in inf.wakes.iter().filter(|w| w.kind == WakeKind::Message).filter_map(|w| w.msg.as_ref()) {
                let top = tops.entry(r.channel.clone()).or_insert(r.seq);
                *top = (*top).max(r.seq);
            }
            for (channel, seq) in tops {
                self.port.advance(session, &channel, seq).await?;
            }
            self.port.publish(OrchestraEvent::WakeChanged { session: session.into() });
        } else {
            self.fail_batch(session, inf.wakes, &format!("{outcome:?}")).await?;
        }
        Ok(())
    }

    /// 终局失败：30s / 2min 退避，第 3 次放弃（裁定 9）。已记过「起不来」的 session 放弃时不再告警。
    pub(crate) async fn fail_batch(&mut self, session: &str, wakes: Vec<QueuedWake>, error: &str) -> anyhow::Result<()> {
        if wakes.is_empty() {
            return Ok(());
        }
        let now = self.now();
        let mut gave_up = Vec::new();
        for w in wakes {
            let attempts = w.attempts + 1;
            match queue::after_failure(attempts, now) {
                Some(t) => {
                    let s = Settle::Retry { attempts, error: error.into(), retry_at: t };
                    self.port.settle(&[w.id], s, now).await?;
                    self.backoff.insert(w.id, t);
                    self.timers.insert(t);
                }
                None => {
                    self.port.settle(&[w.id], Settle::Dropped { error: error.into() }, now).await?;
                    self.backoff.remove(&w.id);
                    gave_up.push(w);
                }
            }
        }
        if !gave_up.is_empty()
            && queue::should_alert(&gave_up)
            && let Some(row) = self.port.session(session).await?
            && row.resume_failed_at.is_none()
        {
            let attempts = queue::MAX_ATTEMPTS.to_string();
            let msg = text::render(
                self.lang(),
                "delivery.failed.group",
                &[("name", &row.name()), ("attempts", &attempts), ("error", error)],
            );
            self.port.post_system(self.alert_audience(&row.crew_id, row.captain), msg).await?;
        }
        self.port.publish(OrchestraEvent::WakeChanged { session: session.into() });
        Ok(())
    }

    /// idle 边沿（取代 `CrewSessionRunner.swift:490-541 runBecameIdle`）：①到期唤醒合并投递，投了就结束；
    /// ②`IdleHook`（M4b 的停下即检查）；③到此为止（机长 60s 检查点、continue_work、Todo sweep 都不再有）。
    pub(crate) async fn on_idle(&mut self, session: &str) -> anyhow::Result<()> {
        if self.try_deliver(session).await? {
            return Ok(());
        }
        let now = self.now();
        if let Some(w) = self.hooks.idle.on_idle(session, now).await
            && self.port.enqueue(w).await?
        {
            self.try_deliver(session).await?;
        }
        Ok(())
    }

    pub(crate) async fn rescan(&mut self) -> anyhow::Result<()> {
        let now = self.now();
        for s in self.port.due_sessions(now).await? {
            self.try_deliver(&s).await?;
        }
        Ok(())
    }

    /// 积压补救（裁定 16）：每个 session 的未读重新规划一遍（不问 Arbiter；6h 陈旧闸照旧；UNIQUE 去重）。
    pub(crate) async fn rescue_backlog(&mut self) -> anyhow::Result<()> {
        let now = self.now();
        let mut ctxs: BTreeMap<String, Option<ChannelCtx>> = BTreeMap::new();
        for s in self.port.all_session_ids().await? {
            for msg in self.port.unread(&s).await? {
                if !ctxs.contains_key(&msg.at.channel) {
                    let ctx = self.port.channel_ctx(&msg.at.channel).await?;
                    ctxs.insert(msg.at.channel.clone(), ctx);
                }
                let Some(Some(ctx)) = ctxs.get(&msg.at.channel) else { continue };
                if let Some(t) = plan::plan(&msg, ctx, None, now).into_iter().find(|t| t.session == s) {
                    let w = NewWake {
                        session: s.clone(),
                        kind: WakeKind::Message,
                        source: source::message(&msg.id),
                        priority: t.priority,
                        not_before: now,
                        note: None,
                        origin: msg.sender.key().to_string(),
                    };
                    self.port.enqueue(w).await?;
                }
            }
        }
        self.rescan().await
    }

    pub(crate) async fn set_captain(&mut self, crew: &str, session: &str) -> Result<(), ControlError> {
        let row = self
            .port
            .session(session)
            .await
            .map_err(ControlError::internal)?
            .filter(|r| r.crew_id == crew)
            .ok_or_else(|| ControlError::NotFound(format!("{crew}/{session}")))?;
        let prev = self.port.set_captain(crew, session).await.map_err(ControlError::internal)?;
        let (now, lang) = (self.now(), self.lang());
        if let Some(l) = self.live.get_mut(session) {
            l.captain = true;
        }
        let mut tell = vec![(session.to_string(), text::get(lang, "captain.changed.new").to_string())];
        if let Some(p) = prev {
            if let Some(l) = self.live.get_mut(&p) {
                l.captain = false;
            }
            tell.push((p, text::render(lang, "captain.changed.old", &[("name", &row.name())])));
        }
        for (s, note) in tell {
            let w = NewWake {
                session: s.clone(),
                kind: WakeKind::System,
                source: source::system(&format!("captain:{crew}:{}", now.0)),
                priority: Priority::System,
                not_before: now,
                note: Some(note),
                origin: breaker::SYSTEM.into(),
            };
            self.port.enqueue(w).await.map_err(ControlError::internal)?;
            self.try_deliver(&s).await.map_err(ControlError::internal)?;
        }
        Ok(())
    }

    pub(crate) async fn recover(&mut self, scope: LatchScope, id: &str) -> Result<(), ControlError> {
        if !self.breaker.recover(scope, id) {
            return Err(ControlError::NotFound(id.to_string()));
        }
        self.port.clear_latch(scope, id).await.map_err(ControlError::internal)?;
        self.rescan().await.map_err(ControlError::internal)
    }
}
```

Run: `cargo test -p crewd-server orchestra`
Expected: PASS（Task 6–7 的 24 条 + deliver 22 条）。若 `DeliveryOutcome` 没有 `PartialEq`：把 `outcome == DeliveryOutcome::Delivered` 换成 `matches!(outcome, DeliveryOutcome::Delivered)`。

- [ ] **Step 5: 变异自证（提交前做，改完手工改回）**

1. `live_ready` 里删掉 `|| l.decision.is_some()` → `pending_decision_holds_delivery_until_cleared` 必须红。
2. `try_deliver` 里删掉 `if self.injected(session, &w).await? { … } else` 那一支（全当 fresh）→ `already_injected_message_is_settled_not_resent` 必须红。
3. `core.rs::stop` 里删掉丢弃 pending 的那段 → `stop_drops_pending_wakes_and_does_not_relaunch` 必须红。
4. `admit` 里把 `Verdict::Tripped` 分支改成 `true` → `mutual_mention_loop_is_cut_on_one_edge_and_human_still_gets_through` 必须红。
5. `fail_batch` 的 `&& row.resume_failed_at.is_none()` 删掉 → `launch_failures_back_off_then_drop_with_one_alert` 必须红。

每条都看清红的是**那一条用例名**（不是别的用例顺带红）。全部改回后 `cargo test -p crewd-server orchestra` 全绿。

- [ ] **Step 6: 门禁与提交**

Run: `./scripts/check.sh`
Expected: 全绿。

```bash
git add crates/server/src/orchestra/deliver.rs crates/server/src/orchestra/core.rs crates/server/src/orchestra/mod.rs crates/server/src/orchestra/testkit.rs
git diff --cached --stat
git commit -m "crewd-server: wake intake and delivery — planning with Arbiter stub, loop breaker, merged batches, receipts, back-off, idle edge, asks

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 9: 重启恢复（§22.7 Q1 问人）、定时拍、停机、actor 循环与 `Handle`

**Files:**
- Create: `crewd/crates/server/src/orchestra/boot.rs`
- Modify: `crewd/crates/server/src/orchestra/core.rs`（`Input::Shutdown`、`CollabFact::RestartAnswered`、`Cmd::{MaxLive, SetMaxLive}`、`shutdown` 字段、`on_tick` 调默认方案、`try_deliver` 停机闸）、`crewd/crates/server/src/orchestra/mod.rs`（`pub mod boot;`、`Handle`、`spawn`、`run`）、`crewd/crates/server/src/orchestra/testkit.rs`（`Rig::with_cfg`）

**Interfaces:**
- Consumes: Task 4 `policy::{restart_order, restart_source, RestartRow}`；Task 7 / 8 的 `Core` 全部。
- Produces:
  - `Core::boot(&mut self) -> anyhow::Result<()>`（`pub(crate)`）、`Core::next_deadline(&self) -> Ms`、`Core::shutdown_done(&mut self) -> bool`
  - `CollabFact::RestartAnswered { prompt: i64, resume: bool, sessions: Vec<String> }`、`Input::Shutdown(oneshot::Sender<()>)`、`Cmd::MaxLive { reply: Reply<Option<usize>> }`、`Cmd::SetMaxLive { max: Option<usize>, reply: Reply<()> }`
  - `orchestra::spawn<P: Port>(port: Arc<P>, execs: Executors, hooks: Hooks, clock: Arc<dyn Clock>, cfg: OrchestraConfig) -> anyhow::Result<Handle>`
  - `orchestra::Handle`（`Clone`）：`start(StartSession, Origin) -> Result<SessionRow, ControlError>`、`resume(&str, Origin) -> Result<(), ControlError>`、`stop(&str)`、`live(&str) -> Result<LiveView, ControlError>`、`nudge(&str, String)`、`inspect(&str, usize) -> Result<(LiveView, String), ControlError>`、`set_profile(&str, Option<String>, Option<String>)`、`set_captain(&str, &str)`、`wakes(Option<String>) -> Result<Vec<QueuedWake>, ControlError>`、`recover(LatchScope, String)`、`max_live() -> Result<Option<usize>, ControlError>`、`set_max_live(Option<usize>)`、`note_progress(&str)`、`collab(CollabFact)`、`shutdown(Duration)`

- [ ] **Step 1: 改 `core.rs`**

1. `CollabFact` 加：

```rust
    /// 人答了重启询问（M3a `restart::answer` 的 `Change::RestartAnswered`）。`resume = false` = 「先不」。
    RestartAnswered { prompt: i64, resume: bool, sessions: Vec<String> },
```

2. `Input` 加 `Shutdown(oneshot::Sender<()>),`；`Cmd` 加：

```rust
    MaxLive { reply: Reply<Option<usize>> },
    SetMaxLive { max: Option<usize>, reply: Reply<()> },
```

3. `Core` 加字段 `pub(crate) shutdown: Option<oneshot::Sender<()>>,`（`new` 里 `shutdown: None`）。
4. `handle` 的 match 加：

```rust
            Input::Shutdown(reply) => {
                self.begin_shutdown(reply);
                Ok(())
            }
```

5. `on_cmd` 的 match 加：

```rust
            Cmd::MaxLive { reply } => {
                let _ = reply.send(Ok(self.gate.max_live()));
            }
            Cmd::SetMaxLive { max, reply } => {
                let r = self.set_max_live(max).await;
                let _ = reply.send(r);
            }
```

6. `on_tick` 在 `self.escalate_decisions().await?;` 之后加 `self.port.apply_due_defaults().await?;`（控制者裁定 C7：默认方案到点由 M3c 的定时拍调），再加队列清理（控制者裁定 C6：保留 7 天，最多一小时清一次）：

```rust
        if now.since(self.last_prune) >= HOUR {
            self.last_prune = now;
            let n = self.port.prune_wakes(Ms(now.0 - WAKE_RETENTION_MS)).await?;
            if n > 0 {
                eprintln!("crewd: 编排：清掉 {n} 条已结的旧唤醒");
            }
        }
```

`core.rs` 顶部加 `/// 已结（delivered / dropped）的唤醒行保留多久（排障用 wake.list 看得到最近一周）。\npub(crate) const WAKE_RETENTION_MS: i64 = 7 * 24 * HOUR;`，`Core` 加字段 `pub(crate) last_prune: Ms`（`new` 里 `Ms(0)`，所以启动后第一拍就清一次）；`MemPort` 的状态加 `pub prunes: usize`。`HOUR` 从 `crewd_wake` 取。
7. `deliver.rs` 的 `on_collab` 加一支：`CollabFact::RestartAnswered { prompt, resume, sessions } => self.on_restart_answered(prompt, resume, sessions).await,`；`try_deliver` 第一行加 `if self.shutdown.is_some() { return Ok(false); }`（停机中不投、不拉起）。

- [ ] **Step 2: `Rig::with_cfg`（`testkit.rs`）**

把 `Rig::with` 改成调 `with_cfg(OrchestraConfig::for_tests(), hooks)`，新增：

```rust
    pub async fn with_cfg(cfg: OrchestraConfig, hooks: Hooks) -> Self {
        let port = Arc::new(MemPort::new());
        let exec = StandIn::default();
        let clock = Arc::new(ManualClock::new(T0));
        port.lock().now = T0;
        let (tx, rx) = mpsc::unbounded_channel();
        let execs = Executors::new().with("claude", Arc::new(exec.clone()));
        let core = Core::new(port.clone(), execs, hooks, clock.clone(), cfg, tx).await.expect("内存 Port 不会失败");
        Self { core, port, exec, clock, rx }
    }
```

（`with` 的函数体换成 `Self::with_cfg(OrchestraConfig::for_tests(), hooks).await`。）

- [ ] **Step 3: 写失败的测试**

`crewd/crates/server/src/orchestra/boot.rs` 先放测试（`mod.rs` 加 `pub mod boot;`）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestra::OrchestraConfig;
    use crate::orchestra::core::{Cmd, CollabFact, Input, Origin};
    use crate::orchestra::exec::Executors;
    use crate::orchestra::hooks::Hooks;
    use crate::orchestra::testkit::{Rig, T0};
    use crewd_exec::session::SessionEvent;
    use crewd_wake::plan::{Mention, MsgFacts, Sender};
    use crewd_wake::{Latch, LatchScope, MIN, MessageRef, SEC, WakeState, source};
    use tokio::sync::oneshot;

    fn desired_running(rig: &Rig, id: &str, was_working: bool) {
        let mut st = rig.port.lock();
        let row = st.sessions.get_mut(id).unwrap();
        row.desired = Desired::Running;
        row.was_working = was_working;
    }

    async fn human_mention(rig: &Rig, target: &str, text: &str) {
        let r = MessageRef { channel: "c1/lobby".into(), seq: 1 };
        let facts = MsgFacts {
            id: "m-1".into(),
            at: r.clone(),
            sender: Sender::Human,
            mentions: vec![Mention::Session(target.into())],
            created_at: T0,
        };
        rig.port.lock().messages.insert((r.channel.clone(), r.seq), (facts, text.into()));
        let w = NewWake {
            session: target.into(),
            kind: WakeKind::Message,
            source: source::message("m-1"),
            priority: Priority::Human,
            not_before: T0,
            note: None,
            origin: "human".into(),
        };
        rig.port.enqueue(w).await.unwrap();
    }

    #[tokio::test]
    async fn boot_requeues_inflight_and_skips_injected_messages() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        human_mention(&rig, "w1", "合成派活甲").await;
        rig.port.advance("w1", "c1/lobby", 1).await.unwrap(); // 崩之前 hook 已注入过
        let timer = NewWake {
            session: "w1".into(),
            kind: WakeKind::Timer,
            source: source::timer("t1"),
            priority: Priority::Auto,
            not_before: T0,
            note: Some("合成定时".into()),
            origin: "w1".into(),
        };
        rig.port.enqueue(timer).await.unwrap();
        let all: Vec<i64> = rig.port.wakes_of("w1").iter().map(|w| w.id).collect();
        rig.port.settle(&all, Settle::Inflight, T0).await.unwrap();
        rig.core.boot().await.unwrap();
        rig.settle().await;
        let w = rig.port.wakes_of("w1");
        assert_eq!(w[0].state, WakeState::Delivered, "消息类：游标已越过，不重投");
        assert_eq!(rig.exec.launches("w1")[0].initial.as_deref(), Some("合成定时"), "定时类：至少一次");
    }

    #[tokio::test]
    async fn boot_asks_the_human_and_starts_nothing_until_answered() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        desired_running(&rig, "w1", true);
        desired_running(&rig, "cap", false);
        human_mention(&rig, "w1", "人甲").await;
        rig.core.boot().await.unwrap();
        rig.settle().await;
        assert_eq!(rig.port.lock().restarts, [vec!["cap".to_string(), "w1".to_string()]], "机长在前");
        assert!(rig.exec.lock().launches.is_empty());
        rig.tick(10 * MIN).await;
        assert!(rig.exec.lock().launches.is_empty(), "没答就都不起，唤醒照常排着");
        assert_eq!(rig.port.wakes_of("w1")[0].state, WakeState::Pending);
    }

    #[tokio::test]
    async fn answering_resume_starts_them_staggered_with_the_interrupted_note() {
        let cfg = OrchestraConfig { stagger_ms: 2 * SEC, ..OrchestraConfig::for_tests() };
        let mut rig = Rig::with_cfg(cfg, Hooks::default()).await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        desired_running(&rig, "w1", true);
        desired_running(&rig, "cap", false);
        human_mention(&rig, "w1", "人甲").await;
        rig.core.boot().await.unwrap();
        let answer = CollabFact::RestartAnswered { prompt: 1, resume: true, sessions: vec!["w1".into(), "cap".into()] };
        rig.core.handle(Input::Collab(answer)).await;
        rig.settle().await;
        assert_eq!(rig.exec.launches("cap").len(), 1, "机长先起");
        assert_eq!(rig.exec.launches("cap")[0].initial, None, "没被打断：无附言的恢复不带开场正文");
        assert!(rig.exec.launches("w1").is_empty(), "错开 2s");
        rig.tick(2 * SEC).await;
        let initial = rig.exec.launches("w1")[0].initial.clone().unwrap();
        assert!(initial.contains("人甲"), "{initial}");
        assert!(initial.contains(text::get(text::Lang::Zh, "restart.interrupted")), "{initial}");
    }

    #[tokio::test]
    async fn answering_later_keeps_them_held_and_single_resume_still_works() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        desired_running(&rig, "w1", true);
        desired_running(&rig, "cap", false);
        rig.core.boot().await.unwrap();
        let later = CollabFact::RestartAnswered { prompt: 1, resume: false, sessions: vec!["cap".into(), "w1".into()] };
        rig.core.handle(Input::Collab(later)).await;
        rig.settle().await;
        assert!(rig.exec.lock().launches.is_empty());
        rig.cmd(|reply| Cmd::Resume { session: "w1".into(), origin: Origin::Human, reply }).await.unwrap();
        assert_eq!(
            rig.exec.launches("w1")[0].initial.as_deref(),
            Some(text::get(text::Lang::Zh, "restart.interrupted")),
            "单个恢复也补「被打断」"
        );
        human_mention(&rig, "cap", "人乙").await;
        rig.core.handle(Input::Collab(CollabFact::WakeEnqueued { session: "cap".into() })).await;
        rig.settle().await;
        assert_eq!(rig.exec.launches("cap").len(), 1, "C4：询问之后人 @ 它 = 只恢复它");
        assert!(rig.exec.launches("cap")[0].initial.as_deref().unwrap_or("").contains("人乙"));
    }

    #[tokio::test]
    async fn human_mention_while_held_resumes_only_that_session() {
        // 控制者裁定 C4：其余仍等答复；启动前就排着的人类 @ 不算（boot 测试钉住）。
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.worker("c1", "w2");
        desired_running(&rig, "w1", true);
        desired_running(&rig, "w2", false);
        desired_running(&rig, "cap", false);
        rig.core.boot().await.unwrap();
        rig.settle().await;
        human_mention(&rig, "w1", "人丙").await;
        rig.core.handle(Input::Collab(CollabFact::WakeEnqueued { session: "w1".into() })).await;
        rig.settle().await;
        let initial = rig.exec.launches("w1")[0].initial.clone().unwrap();
        assert!(initial.contains("人丙") && initial.contains(text::get(text::Lang::Zh, "restart.interrupted")), "{initial}");
        assert!(rig.exec.launches("w2").is_empty() && rig.exec.launches("cap").is_empty(), "其余照旧等");
        assert!(!rig.core.held.contains("w1") && rig.core.held.contains("w2"));
    }

    #[tokio::test]
    async fn shutdown_stops_live_sessions_and_keeps_desired_and_was_working() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("cap").await;
        rig.emit("cap", SessionEvent::Working(true)).await;
        let (tx, rx) = oneshot::channel();
        rig.core.handle(Input::Shutdown(tx)).await;
        rig.settle().await;
        assert!(rig.core.shutdown_done());
        rx.await.expect("停机回执");
        let row = rig.port.row("cap");
        assert_eq!((row.desired, row.was_working), (Desired::Running, true), "下次启动问人时知道它被打断了");
        human_mention(&rig, "w1", "人甲").await;
        rig.core.handle(Input::Collab(CollabFact::WakeEnqueued { session: "w1".into() })).await;
        rig.settle().await;
        assert!(rig.exec.launches("w1").is_empty(), "停机中不拉起");
    }

    #[tokio::test]
    async fn latches_and_the_optional_cap_survive_a_restart() {
        let rig = Rig::new().await;
        rig.port.lock().latches.push(Latch { scope: LatchScope::Edge, id: "cap>w1".into(), reason: "x".into(), at: T0 });
        rig.port.lock().max_live = Some(3);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let core = Core::new(rig.port.clone(), Executors::new(), Hooks::default(), rig.clock.clone(), OrchestraConfig::for_tests(), tx)
            .await
            .unwrap();
        assert!(core.breaker.held("w1", "cap").is_some());
        assert_eq!(core.gate.max_live(), Some(3));
    }

    #[tokio::test]
    async fn tick_applies_due_ask_defaults_and_prunes_old_wakes() {
        let mut rig = Rig::new().await;
        rig.tick(MIN).await;
        assert!(rig.port.lock().defaults_applied >= 1);
        assert_eq!(rig.port.lock().prunes, 1, "启动后第一拍清一次，一小时内不再清");
    }

    #[tokio::test]
    async fn next_deadline_skips_due_but_blocked_wakes_and_finds_the_next_future_one() {
        let mut rig = Rig::new().await;
        rig.crew("c1", "cap");
        rig.worker("c1", "w1");
        rig.run("w1").await;
        rig.emit("w1", SessionEvent::Working(true)).await;
        human_mention(&rig, "w1", "人甲").await; // 到期了但目标在忙：不能让定时器空转
        assert_eq!(rig.core.next_deadline().await, T0.add(rig.core.cfg.heartbeat_ms));
        let later = NewWake {
            session: "w1".into(),
            kind: WakeKind::Timer,
            source: source::timer("t2"),
            priority: Priority::Auto,
            not_before: T0.add(5 * SEC),
            note: Some("x".into()),
            origin: "w1".into(),
        };
        rig.port.enqueue(later).await.unwrap();
        assert_eq!(rig.core.next_deadline().await, T0.add(5 * SEC));
    }

    #[tokio::test]
    async fn set_max_live_persists_through_the_port() {
        let mut rig = Rig::new().await;
        rig.cmd(|reply| Cmd::SetMaxLive { max: Some(4), reply }).await.unwrap();
        assert_eq!(rig.port.lock().max_live, Some(4));
        assert_eq!(rig.cmd(|reply| Cmd::MaxLive { reply }).await, Ok(Some(4)));
    }
}
```

`mod.rs` 底部的测试（真 actor 循环 + `Handle`）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestra::clock::ManualClock;
    use crate::orchestra::testkit::{MemChannel, MemKind, MemPort, StandIn, T0};

    #[tokio::test]
    async fn handle_drives_the_actor_and_shutdown_waits_for_exits() {
        let port = Arc::new(MemPort::new());
        {
            let mut st = port.lock();
            st.now = T0;
            st.channels.insert("c1/lobby".into(), MemChannel { crew: "c1".into(), kind: MemKind::Lobby, owner: None, members: vec![] });
        }
        let row = port
            .insert_session(port::NewSession {
                crew_id: "c1".into(),
                captain: true,
                executor: "claude".into(),
                brief: String::new(),
                title: Some("cap".into()),
                model: None,
                effort: None,
                worktree: None,
            })
            .await
            .unwrap();
        let exec = StandIn::default();
        let h = spawn(
            port.clone(),
            Executors::new().with("claude", Arc::new(exec.clone())),
            Hooks::default(),
            Arc::new(ManualClock::new(T0)),
            OrchestraConfig::for_tests(),
        )
        .await
        .unwrap();
        h.resume(&row.id, Origin::Human).await.unwrap();
        let mut running = false;
        for _ in 0..200 {
            if h.live(&row.id).await.unwrap().running {
                running = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(running, "拉起结果经事件回到 actor");
        assert_eq!(h.resume(&row.id, Origin::Human).await, Err(ControlError::AlreadyRunning(row.id.clone())));
        h.shutdown(Duration::from_secs(5)).await;
        assert_eq!(exec.lock().stops, [row.id.clone()]);
    }
}
```

Run: `cargo test -p crewd-server orchestra`
Expected: 编译失败（`boot` / `spawn` / `Handle` 未定义）。

- [ ] **Step 4: 实现 `boot.rs`**

```rust
//! 重启恢复与停机（spec §22.4「重启恢复」；§22.7 Q1：问人，答「恢复」才错开拉起）。
use crewd_wake::breaker::HUMAN;
use crewd_wake::notice::DECISION_ESCALATE_MS;
use crewd_wake::policy::{self, RestartRow};
use crewd_wake::{Ms, NewWake, Priority, WakeKind, text};
use tokio::sync::oneshot;

use super::core::{ControlError, Core};
use super::port::*;

fn restart_row(r: &SessionRow) -> RestartRow {
    RestartRow {
        session: r.id.clone(),
        name: r.name(),
        crew: r.crew_id.clone(),
        captain: r.captain,
        was_working: r.was_working,
        last_active_at: r.last_active_at,
    }
}

impl<P: Port> Core<P> {
    /// serve 启动时调一次（孤儿已由 serve 先回收）：
    /// ① 上次没确认的 inflight 全部退回 pending（消息类靠游标防重放，裁定 17）；
    /// ② 上次在跑的（`desired=running`）一个都不起，进「待恢复」名单，开一次询问（M3a 发人类群公告、客户端弹窗）；
    /// ③ 积压补救（裁定 16）；④ 能投的投出去（待恢复的不会被拉起）。
    pub(crate) async fn boot(&mut self) -> anyhow::Result<()> {
        let requeued = self.port.requeue_inflight().await?;
        if requeued > 0 {
            eprintln!("crewd: 编排：{requeued} 条投递中的唤醒退回待投");
        }
        let rows = policy::restart_order(self.port.sessions_desired_running().await?.iter().map(restart_row).collect());
        self.held = rows.iter().map(|r| r.session.clone()).collect();
        self.port.open_restart(rows.into_iter().map(|r| r.session).collect()).await?;
        self.rescue_backlog().await?;
        // C4 的水位：到此为止排着的（含积压补救刚补的旧 @）都不算「询问之后人新 @ 的」。
        self.held_watermark = self.port.list_wakes(None).await?.iter().map(|w| w.id).max().unwrap_or(0);
        Ok(())
    }

    /// 人答了重启询问。「恢复」→ 名单里仍在等的，按机长在前、最近活跃在前入队一条恢复唤醒（被打断的带附言），
    /// 再逐个试投；错开由启动闸做（裁定 5）。「先不」→ 什么都不做，名单留着。
    pub(crate) async fn on_restart_answered(&mut self, prompt: i64, resume: bool, sessions: Vec<String>) -> anyhow::Result<()> {
        if !resume {
            return Ok(());
        }
        let mut rows = Vec::new();
        for s in sessions.iter().filter(|s| self.held.contains(*s)) {
            if let Some(r) = self.port.session(s).await? {
                rows.push(restart_row(&r));
            }
        }
        let ordered = policy::restart_order(rows);
        let (now, lang) = (self.now(), self.lang());
        for r in &ordered {
            self.held.remove(&r.session);
            let w = NewWake {
                session: r.session.clone(),
                kind: WakeKind::Restart,
                source: policy::restart_source(&prompt.to_string()),
                priority: Priority::Human,
                not_before: now,
                note: r.was_working.then(|| text::get(lang, "restart.interrupted").to_string()),
                origin: HUMAN.into(),
            };
            self.port.enqueue(w).await?;
        }
        for r in &ordered {
            self.try_deliver(&r.session).await?;
        }
        Ok(())
    }

    pub(crate) async fn set_max_live(&mut self, max: Option<usize>) -> Result<(), ControlError> {
        self.port.set_max_live(max).await.map_err(ControlError::internal)?;
        self.gate.set_max_live(max);
        self.rescan().await.map_err(ControlError::internal)
    }

    /// 停机：叫停全部会话（不改 desired / was_working：下次启动问人时要知道谁在跑、谁被打断），等它们都退出再回执。
    pub(crate) fn begin_shutdown(&mut self, reply: oneshot::Sender<()>) {
        self.shutdown = Some(reply);
        for l in self.live.values() {
            let _ = l.handle.stop();
        }
        for st in self.starting.values_mut() {
            st.stop_requested = true;
        }
    }

    /// 停机完成（没有会话还活着）就回执，返回 true。
    pub(crate) fn shutdown_done(&mut self) -> bool {
        if self.shutdown.is_none() || !self.live.is_empty() || !self.starting.is_empty() {
            return false;
        }
        if let Some(reply) = self.shutdown.take() {
            let _ = reply.send(());
        }
        true
    }

    /// 下一次定时拍：最早的「未来」not_before、退避 / 错开的时刻、待决策升级时刻，都没有就心跳。
    /// 已到期却投不了的（目标在忙）不算——那些由 idle 边沿接，不能让定时器空转。
    pub(crate) async fn next_deadline(&self) -> Ms {
        let now = self.now();
        let mut t = now.add(self.cfg.heartbeat_ms);
        if let Ok(Some(nb)) = self.port.next_not_before_after(now).await {
            t = t.min(nb);
        }
        if let Some(first) = self.timers.iter().next() {
            t = t.min((*first).max(now));
        }
        for r in self.decisions.values().filter(|r| !r.escalated) {
            t = t.min(r.at.add(DECISION_ESCALATE_MS).max(now));
        }
        t
    }
}
```


- [ ] **Step 5: `mod.rs` 加 `spawn` / `Handle` / actor 循环**

在 `mod.rs` 的模块声明里加 `pub mod boot;`、`pub mod core;`、`pub mod deliver;`（Task 7 / 8 已加的不重复），**删掉 Task 6 留在文件末尾的 M3a 占位 `pub struct Handle;`**，并加：

```rust
use std::sync::Arc;

use crewd_wake::{LatchScope, QueuedWake};
use tokio::sync::{mpsc, oneshot};

use self::clock::Clock;
use self::core::{Cmd, CollabFact, ControlError, Core, Input, Origin, Reply, StartSession};
use self::exec::Executors;
use self::hooks::Hooks;
use self::port::{LiveView, Port, SessionRow};

/// 编排 actor 的句柄。可克隆；客户端 RPC、`SessionControl`、协作事件转发都经它。
#[derive(Clone)]
pub struct Handle {
    tx: mpsc::UnboundedSender<Input>,
}

/// 建 Core、跑一次启动恢复（`boot`）、起 actor 循环。
pub async fn spawn<P: Port>(
    port: Arc<P>,
    execs: Executors,
    hooks: Hooks,
    clock: Arc<dyn Clock>,
    cfg: OrchestraConfig,
) -> anyhow::Result<Handle> {
    let (tx, rx) = mpsc::unbounded_channel();
    let mut core = Core::new(port, execs, hooks, clock, cfg, tx.clone()).await?;
    core.boot().await?;
    tokio::spawn(run(core, rx));
    Ok(Handle { tx })
}

async fn run<P: Port>(mut core: Core<P>, mut rx: mpsc::UnboundedReceiver<Input>) {
    loop {
        let wait = core.next_deadline().await.since(core.now()).max(0) as u64;
        tokio::select! {
            input = rx.recv() => match input {
                Some(i) => core.handle(i).await,
                None => return,
            },
            _ = tokio::time::sleep(Duration::from_millis(wait)) => core.handle(Input::Tick).await,
        }
        if core.shutdown_done() {
            return;
        }
    }
}

fn stopped() -> ControlError {
    ControlError::Internal("编排已停止".into())
}

impl Handle {
    async fn call<T>(&self, make: impl FnOnce(Reply<T>) -> Cmd) -> Result<T, ControlError> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Input::Cmd(make(tx))).map_err(|_| stopped())?;
        rx.await.map_err(|_| stopped())?
    }

    pub async fn start(&self, req: StartSession, origin: Origin) -> Result<SessionRow, ControlError> {
        self.call(|reply| Cmd::Start { req, origin, reply }).await
    }

    pub async fn resume(&self, session: &str, origin: Origin) -> Result<(), ControlError> {
        let session = session.to_string();
        self.call(|reply| Cmd::Resume { session, origin, reply }).await
    }

    pub async fn stop(&self, session: &str) -> Result<(), ControlError> {
        let session = session.to_string();
        self.call(|reply| Cmd::Stop { session, reply }).await
    }

    pub async fn live(&self, session: &str) -> Result<LiveView, ControlError> {
        let session = session.to_string();
        self.call(|reply| Cmd::Live { session, reply }).await
    }

    pub async fn nudge(&self, session: &str, input: String) -> Result<(), ControlError> {
        let session = session.to_string();
        self.call(|reply| Cmd::Nudge { session, input, reply }).await
    }

    /// 实时状态 + 屏幕文本（屏幕在 actor 之外读，不卡 actor）。
    pub async fn inspect(&self, session: &str, max_lines: usize) -> Result<(LiveView, String), ControlError> {
        let (tx, rx) = oneshot::channel();
        let cmd = Cmd::Handle { session: session.to_string(), reply: tx };
        self.tx.send(Input::Cmd(cmd)).map_err(|_| stopped())?;
        let handle = rx.await.map_err(|_| stopped())?;
        let live = self.live(session).await?;
        let screen = match handle {
            Some(h) => h.screen_text(max_lines).await.unwrap_or_default(),
            None => String::new(),
        };
        Ok((live, screen))
    }

    pub async fn set_profile(&self, session: &str, model: Option<String>, effort: Option<String>) -> Result<(), ControlError> {
        let session = session.to_string();
        self.call(|reply| Cmd::SetProfile { session, model, effort, reply }).await
    }

    pub async fn set_captain(&self, crew: &str, session: &str) -> Result<(), ControlError> {
        let (crew, session) = (crew.to_string(), session.to_string());
        self.call(|reply| Cmd::SetCaptain { crew, session, reply }).await
    }

    pub async fn wakes(&self, session: Option<String>) -> Result<Vec<QueuedWake>, ControlError> {
        self.call(|reply| Cmd::Wakes { session, reply }).await
    }

    /// 人解熔断（只给客户端 RPC 用，agent 没有入口）。
    pub async fn recover(&self, scope: LatchScope, id: String) -> Result<(), ControlError> {
        self.call(|reply| Cmd::Recover { scope, id, reply }).await
    }

    pub async fn max_live(&self) -> Result<Option<usize>, ControlError> {
        self.call(|reply| Cmd::MaxLive { reply }).await
    }

    pub async fn set_max_live(&self, max: Option<usize>) -> Result<(), ControlError> {
        self.call(|reply| Cmd::SetMaxLive { max, reply }).await
    }

    /// 有了产出（测试用；生产路径是 M3a `sessions::note_progress` → `Change::Progress` → `forward_collab`，M3b / M4b 不调这里）。
    pub fn note_progress(&self, session: &str) {
        let _ = self.tx.send(Input::Progress { session: session.to_string() });
    }

    pub fn collab(&self, f: CollabFact) {
        let _ = self.tx.send(Input::Collab(f));
    }

    /// 叫停全部会话，最多等 `limit`。
    pub async fn shutdown(&self, limit: Duration) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Input::Shutdown(tx)).is_ok() {
            let _ = tokio::time::timeout(limit, rx).await;
        }
    }
}
```

Run: `cargo test -p crewd-server orchestra`
Expected: PASS（之前 46 条 + boot 10 条 + mod 1 条）。

- [ ] **Step 6: 变异自证（提交前做，改完手工改回）**

把 `boot` 里 `self.held = …` 那行删掉 → `boot_asks_the_human_and_starts_nothing_until_answered` 必须红（待恢复的被唤醒拉起了）。把 `next_deadline` 里的 `next_not_before_after` 换成「最早的任何 pending」（MemPort 里临时把 `w.not_before > now` 改成 `true`）→ `next_deadline_skips_due_but_blocked_wakes_and_finds_the_next_future_one` 必须红。改回后全绿。

- [ ] **Step 7: 门禁与提交**

Run: `./scripts/check.sh`
Expected: 全绿。

```bash
git add crates/server/src/orchestra/boot.rs crates/server/src/orchestra/core.rs crates/server/src/orchestra/deliver.rs crates/server/src/orchestra/mod.rs crates/server/src/orchestra/testkit.rs
git diff --cached --stat
git commit -m "crewd-server: restart asks the human before resuming, staggered resume, ask defaults on tick, shutdown, actor loop and Handle

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 10: 接上 M3a——`DbPort`、协作事件转发、`SessionControl`、M3c 的客户端 RPC 与契约

这是唯一碰 M3a 实际名字的任务。M3a 计划 `docs/superpowers/plans/2026-10-06-crewd-m3a-data-and-api.md` 的「冻结给 M3b / M3c 的接口」一节是对照表；**它与 spec §22.5 不同处以它为准**（`CollabEvent` = `crewd_collab::Change`、ask 唤醒由 M3a 入队且 source `ask:<crew>/H<n>`、队列 / 熔断台账 / session 生命周期 / 换机长 / 重启询问的写函数都在 collab、`SessionControl` 多 `resume` 与 `handoff_captain`）。

**Files:**
- Create: `crewd/crates/server/src/orchestra/db.rs`、`crewd/crates/server/src/orchestra/control.rs`、`crewd/crates/server/src/orchestra/rpc.rs`、`crewd/crates/proto/src/orchestra.rs`
- Modify: `crewd/crates/server/src/orchestra/mod.rs`（`pub mod db; pub mod control; pub mod rpc;`）、`crewd/crates/server/Cargo.toml` + `crewd/Cargo.lock`（`crewd-collab`、`chrono`）、`crewd/crates/server/src/dispatch.rs`（未知方法分支前一行）、`crewd/crates/proto/src/lib.rs`（`pub mod orchestra;` + `api_schema` 登记）、`crewd/schema/crewd-api.schema.json`（重新生成）。`AppState.orchestra` 与主题表里的 `wake` M3a 已有，不改 `lib.rs` / `conn.rs`

**Interfaces:**
- Consumes（M3a，冻结）：`crewd_store::{Db, Store, now_ms, ts_from_ms, channels::lobby_id}`；`crewd_collab::{Tx, Change, Sender as CollabSender, messages, channels, cursors, listens, crews, sessions, asks, restart, settings, wakes, wakes::latches, render, worldview}`；`crewd_server::{AppState::{collab, db, hub, tokens, sessions}, hub::CollabEvent, Hub::{subscribe_collab, publish}, control::{SessionControl, StartRequest, Inspection, QuotaReport, ControlError as M3aControlError}, ledger::StoreLedger}`；`crewd_proto::{Session, SessionLive, HealthView, HealthKindView, DecisionView, Role, Message, Mention, MentionKind, SenderKind, MemberRef, RpcError, Event}`。
- Produces：`orchestra::db::{DbPort, fact_of, forward_collab(rx, handle)}`、`orchestra::control::{OrchestraControl::new(Arc<OnceLock<Handle>>, Db), live_to_proto, map_error}`、`orchestra::rpc::route`、`crewd_proto::orchestra::{methods, events, SessionStartParams, SessionStopParams, SessionIdParams, CrewSetCaptainParams, WakeListParams, AdmissionRecoverParams, LatchScopeView, OrchestraSettings, WakeView, schema_entries}`；用 M3a 已建的 `AppState::orchestra: std::sync::OnceLock<orchestra::Handle>`。

- [ ] **Step 1: 对齐 M3a 的实际代码（只读，先做）**

M3a 合入 main 后，逐项打开实际签名（`rg -n 'pub fn <名字>' crates/collab/src crates/server/src crates/store/src`），把下表「本计划写成」一栏与实际对上；不一致的只改 `db.rs` / `control.rs` / `rpc.rs` 里那一处调用，**不改 crewd-wake、不改 Core、不改测试期望**；对不上又补不了的（缺函数）停下回报，不往 collab 里加东西。

| 本计划写成（= M3a 冻结块） | 用在 | 要核对的 |
|---|---|---|
| `crewd_collab::sessions::get(conn, id) -> Result<Session>`（不存在 = `CollabError::NotFound`，`db.rs::opt` 把它变成 `None`） | `session` | `Session` 的 v3 字段名：`desired`、`was_working`、`last_active_at`、`resume_failed_at`、`awaiting_question`、`last_progress_at`、`worktree`、`working_directory`、`role`、`executor`、`agent_session_id`、`display_name`、`model`、`effort`、`brief`（时间是 RFC 3339 TEXT） |
| `crewd_collab::crews::{get, captain_of, root_of, set_captain, list}` | 工作目录、机长、根、换机长、默认方案 | `set_captain(tx, crew, session) -> Result<()>`；`get` 返回的 `Crew.working_directory` |
| `crewd_collab::sessions::{register, NewSession, running_desired, set_desired, set_working, set_resume_failed, set_profile}` | `insert_session` / `sessions_desired_running` / `patch_session` | `NewSession { crew_id, role, executor, title, brief, display_name, working_directory, model, effort, worktree }`（`Default`） |
| `crewd_collab::wakes::{enqueue, due_for, by_ids, mark_inflight, mark_delivered, mark_failed(tx, id, error, retry_at_ms), requeue, mark_dropped, reset_inflight, sessions_with_due, next_due_after, list, prune}`、`WakeRequest`、`WakeRow`、`Enqueued` | 队列 | M3a 计划 Task 5 全文 |
| `crewd_collab::wakes::latches::{Scope, Latch, set(tx, &Scope, reason), clear(tx, &Scope), list}` | 熔断台账 | 库里 edge 的 id 是 `from→to`（M3a 拼）；M3c 自己的 `edge_id` 是 `from>to`，只在 `db.rs` 互转 |
| `crewd_collab::messages::{get, list(conn, channel, after: Option<i64>, before: Option<i64>, limit: u32), post}`、`NewPost`、`Message.{id, channel_id, seq, sender_kind, sender_session_id, mentions, created_at}` | 消息要素、系统消息 | after / before 是开区间 |
| `crewd_collab::channels::{get, members, owner_of, ensure_human_channel}` | 群成员与群主 | `members` = **有效**成员（大厅含本机组全部 session，人类群含根机组机长；M3a 冻结并有测试） |
| `crewd_collab::listens::active_for_channel(conn, channel, now_ms)` | 收听 | `Listen { session, channel, until_ms: Option<i64>, senders: Vec<String> }` |
| `crewd_collab::cursors::{cursor(conn, reader, channel) -> Option<i64>, unread_for, advance}` | 已注入判定、未读、推游标 | |
| `crewd_collab::render::{build_batch, wake_text}`、`settings::{locale, max_live_sessions, set_max_live_sessions}`、`worldview::{context_for, render}` | 渲染、设置、世界观 | `Locale` ↔ `crewd_wake::text::Lang` |
| `crewd_collab::restart::open(tx, ids) -> Result<Option<RestartPrompt>>`、`asks::apply_due_defaults(tx, crew)` | 重启询问、默认方案 | `RestartPrompt.id: i64` |
| `crewd_server::hub::{CollabEvent, Hub::subscribe_collab}`、`Hub::publish(Event)` | 订阅 / 发事件 | |
| `SessionControl`、`StartRequest`、`Inspection`、`QuotaReport`、`ControlError` | `control.rs` | M3a 计划 Task 12 全文 |
| proto `SessionLive / HealthView / HealthKindView / DecisionView / MemberRef::Session { id }`、`closed_vocab!` 宏、`api_schema()` 的登记方式 | `control.rs` / `proto/src/orchestra.rs` | `HealthKindView` 的变体名；主题表已含 `wake`（M3a） |

控制者裁定 C6 之后**没有任何只读 SQL**：原先缺的「晚于 now 的最早 not_before」「读者在某群的游标」「按 id 取 WakeRow」「按 id 原样退回 pending」「带退避时刻的失败」「旧行清理」都由 M3a 提供了函数（见上表）。

- [ ] **Step 2: 依赖**

```bash
cargo add --offline -p crewd-server --path crates/collab crewd-collab
cargo add --offline -p crewd-server chrono --no-default-features -F std
```

（`crewd-collab` M3a 若已加给 server 就跳过第一条。）

- [ ] **Step 3: 写 `proto/src/orchestra.rs`（契约类型）**

```rust
//! M3c 的客户端方法与事件（spec §22.4「客户端 RPC（M3c 新增）」）。`SessionLive` 等共享类型在 M3a 的 model。
//! 重启询问的 `restart.pending` / `restart.answer` 是 M3a 的方法（它记账、发 `Change::RestartAnswered`，M3c 订阅后恢复）。
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Role;

pub mod methods {
    pub const SESSION_START: &str = "session.start";
    pub const SESSION_STOP: &str = "session.stop";
    pub const SESSION_RESUME: &str = "session.resume";
    pub const SESSION_LIVE: &str = "session.live";
    pub const CREW_SET_CAPTAIN: &str = "crew.setCaptain";
    pub const WAKE_LIST: &str = "wake.list";
    /// 解环路熔断。只有客户端（人）能调，agent 没有入口。
    pub const ADMISSION_RECOVER: &str = "admission.recover";
    pub const SETTINGS_GET: &str = "orchestra.settings.get";
    pub const SETTINGS_SET: &str = "orchestra.settings.set";
    pub const ALL: &[&str] = &[
        SESSION_START, SESSION_STOP, SESSION_RESUME, SESSION_LIVE, CREW_SET_CAPTAIN, WAKE_LIST, ADMISSION_RECOVER,
        SETTINGS_GET, SETTINGS_SET,
    ];
}

pub mod events {
    /// 排障用：某个 session 的唤醒队列变了（数据 `{sessionId}`）。主题 = `wake`。
    pub const WAKE_CHANGED: &str = "wake.changed";
    pub const TOPIC_WAKE: &str = "wake";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionStartParams {
    pub crew_id: String,
    pub role: Role,
    /// `claude` / `codex` / `acp:<id>`（M3c 里只有 claude 起得来）。
    pub executor: String,
    pub brief: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// 必填（控制者裁定 C8）：true = 在机组工作目录里新建 git worktree 并在那里起；false = 机组共享目录。缺了 = 参数错。
    pub isolation: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionStopParams {
    pub session_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionIdParams {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CrewSetCaptainParams {
    pub crew_id: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WakeListParams {
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LatchScopeView {
    /// 「来源 → 目标」这一条（`from` 必填）。
    Edge,
    /// 目标的非 session 来源（系统、它自己的定时）。
    Session,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmissionRecoverParams {
    pub scope: LatchScopeView,
    pub target_session_id: String,
    /// `edge` 的来源（session id，或 `system`）。
    #[serde(default)]
    pub from: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrchestraSettings {
    /// 可选并发上限（§22.7 Q5：缺省 null = 不设；只挡自动拉起）。
    #[serde(default)]
    pub max_live_sessions: Option<u32>,
}

/// 排障用的一行唤醒（`wake.list`）。`kind` / `priority` / `state` 的词表同 schema v3 的 `wakes` 表。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WakeView {
    pub id: i64,
    pub session_id: String,
    pub kind: WakeKindView,
    pub source: String,
    pub origin: String,
    pub priority: WakePriorityView,
    pub not_before: i64,
    pub state: WakeStateView,
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WakeKindView {
    Message,
    Timer,
    AskAnswer,
    Restart,
    System,
    Checkpoint,
    StopCheck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WakePriorityView {
    Auto,
    System,
    Human,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WakeStateView {
    Pending,
    Inflight,
    Delivered,
    Dropped,
}

/// 给 `api_schema()` 登记的类型（M3a 的登记方式见 Step 1 对齐表）。
pub fn schema_entries() -> Vec<(&'static str, schemars::Schema)> {
    vec![
        ("SessionStartParams", schemars::schema_for!(SessionStartParams)),
        ("SessionStopParams", schemars::schema_for!(SessionStopParams)),
        ("SessionIdParams", schemars::schema_for!(SessionIdParams)),
        ("CrewSetCaptainParams", schemars::schema_for!(CrewSetCaptainParams)),
        ("WakeListParams", schemars::schema_for!(WakeListParams)),
        ("AdmissionRecoverParams", schemars::schema_for!(AdmissionRecoverParams)),
        ("OrchestraSettings", schemars::schema_for!(OrchestraSettings)),
        ("WakeView", schemars::schema_for!(WakeView)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn params_reject_unknown_fields_and_vocab_is_the_table_words() {
        assert!(serde_json::from_value::<SessionStopParams>(json!({"sessionId":"s","reason":"r","x":1})).is_err());
        assert!(serde_json::from_value::<WakeListParams>(json!({})).is_ok());
        assert_eq!(serde_json::to_value(WakeKindView::AskAnswer).unwrap(), json!("ask_answer"));
        assert_eq!(serde_json::to_value(LatchScopeView::Edge).unwrap(), json!("edge"));
        let p: AdmissionRecoverParams =
            serde_json::from_value(json!({"scope":"edge","targetSessionId":"w1","from":"cap"})).unwrap();
        assert_eq!(p.from.as_deref(), Some("cap"));
    }

    #[test]
    fn every_method_is_listed_once() {
        let mut v = methods::ALL.to_vec();
        v.sort();
        v.dedup();
        assert_eq!(v.len(), methods::ALL.len());
    }
}
```

`crates/proto/src/lib.rs` 加 `pub mod orchestra;`；在 M3a 的 `api_schema()` 里按它的登记方式把 `orchestra::schema_entries()` 与 `orchestra::methods::ALL` 登记进去（一处，几行）。

- [ ] **Step 4: 写 `db.rs` 的测试（真库）**

`db.rs` 底部（实现见 Step 5）。种子数据经 M3a 的 collab 写：一个机组 `c1`（工作目录是临时目录）、机长 `cap`、worker `w1`。种子帮手 `seed()` 的具体调用按 Step 1 对齐表写（`crews::create`、`sessions::register`、`crews::set_captain`）。

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crewd_wake::source;

    /// 内存库 + Db + AppState + DbPort；机组 c1、机长 cap、worker w1、大厅 c1/lobby。
    async fn seeded() -> (Arc<AppState>, DbPort, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = crewd_store::Store::open(&dir.path().join("crewd.db")).unwrap();
        let state = Arc::new(AppState::new(crewd_store::Db::spawn(store).unwrap()));
        let wd = dir.path().to_path_buf();
        state
            .collab(move |tx| seed(tx, &wd))
            .await
            .unwrap();
        let port = DbPort::new(state.clone(), dir.path().join("crewd.db"));
        (state, port, dir)
    }

    #[test]
    fn source_formats_are_byte_identical_to_collab() {
        assert_eq!(source::message("m-1"), cw::source::message("m-1"));
        assert_eq!(source::quota(5), cw::source::quota(5));
        assert_eq!(source::restart("7"), cw::source::restart(7));
        assert_eq!(source::system("x"), cw::source::system("x"));
        assert_eq!(source::timer("t"), cw::source::timer("t"));
    }

    #[tokio::test]
    async fn enqueue_dedupes_and_pending_resolves_the_message_position() {
        let (state, port, _d) = seeded().await;
        let posted = state.collab(|tx| post_as(tx, "cap", "c1/lobby", "合成派活甲", &["w1"])).await.unwrap();
        let w = NewWake {
            session: "w1".into(),
            kind: WakeKind::Message,
            source: source::message(&posted.id),
            priority: Priority::Auto,
            not_before: Ms(0),
            note: None,
            origin: "cap".into(),
        };
        assert!(port.enqueue(w.clone()).await.unwrap());
        assert!(!port.enqueue(w).await.unwrap());
        let p = port.pending("w1").await.unwrap();
        assert_eq!(p[0].msg, Some(MessageRef { channel: posted.channel_id.clone(), seq: posted.seq }));
        assert_eq!(p[0].origin, "cap");
    }

    #[tokio::test]
    async fn settle_follows_the_queue_rules_and_human_or_system_origins_round_trip() {
        let (_s, port, _d) = seeded().await;
        let mk = |src: &str, origin: &str, priority| NewWake {
            session: "w1".into(),
            kind: WakeKind::System,
            source: source::system(src),
            priority,
            not_before: Ms(0),
            note: Some("x".into()),
            origin: origin.into(),
        };
        port.enqueue(mk("a", "system", Priority::System)).await.unwrap();
        port.enqueue(mk("b", "human", Priority::Human)).await.unwrap();
        let p = port.pending("w1").await.unwrap();
        let origins: Vec<&str> = p.iter().map(|w| w.origin.as_str()).collect();
        assert_eq!(origins, ["human", "system"], "人优先；None 的 origin 按优先级还原");
        let ids: Vec<i64> = p.iter().map(|w| w.id).collect();
        port.settle(&ids, Settle::Inflight, Ms(0)).await.unwrap();
        port.settle(&ids[..1], Settle::Retry { attempts: 1, error: "e".into(), retry_at: Ms(0) }, Ms(0)).await.unwrap();
        assert_eq!(port.pending("w1").await.unwrap()[0].attempts, 1);
        assert_eq!(port.requeue_inflight().await.unwrap(), 1);
        port.settle(&ids, Settle::Inflight, Ms(0)).await.unwrap();
        port.settle(&ids, Settle::Delivered, Ms(0)).await.unwrap();
        assert!(port.pending("w1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn next_not_before_after_ignores_ones_already_due() {
        let (_s, port, _d) = seeded().await;
        let at = |src: &str, ms: i64| NewWake {
            session: "w1".into(),
            kind: WakeKind::Timer,
            source: source::timer(src),
            priority: Priority::Auto,
            not_before: Ms(ms),
            note: Some("x".into()),
            origin: "w1".into(),
        };
        port.enqueue(at("past", 10)).await.unwrap();
        port.enqueue(at("future", 500)).await.unwrap();
        assert_eq!(port.next_not_before_after(Ms(100)).await.unwrap(), Some(Ms(500)));
        assert_eq!(port.next_not_before_after(Ms(600)).await.unwrap(), None);
    }

    #[tokio::test]
    async fn latches_round_trip_both_scopes() {
        let (_s, port, _d) = seeded().await;
        let edge = Latch { scope: LatchScope::Edge, id: breaker::edge_id("cap", "w1"), reason: "no-progress:9/10min".into(), at: Ms(0) };
        let target = Latch { scope: LatchScope::Session, id: "w1".into(), reason: "no-progress:9/10min".into(), at: Ms(0) };
        port.put_latch(&edge).await.unwrap();
        port.put_latch(&target).await.unwrap();
        let mut got: Vec<(LatchScope, String)> = port.latches().await.unwrap().into_iter().map(|l| (l.scope, l.id)).collect();
        got.sort();
        assert_eq!(got, [(LatchScope::Session, "w1".into()), (LatchScope::Edge, "cap>w1".into())]);
        port.clear_latch(LatchScope::Edge, "cap>w1").await.unwrap();
        assert_eq!(port.latches().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn patching_runtime_columns_never_erases_the_agent_session_id() {
        let (_s, port, _d) = seeded().await;
        port.ledger("w1").record("agent-1").unwrap();
        let patch = SessionPatch { desired: Some(Desired::Running), was_working: Some(true), resume_failed_at: Some(Some(Ms(1))), ..Default::default() };
        port.patch_session("w1", patch).await.unwrap();
        let row = port.session("w1").await.unwrap().unwrap();
        assert_eq!(row.agent_session_id.as_deref(), Some("agent-1"), "§18 遗留");
        assert_eq!((row.desired, row.was_working), (Desired::Running, true));
        assert!(row.resume_failed_at.is_some());
    }

    #[tokio::test]
    async fn lobby_context_has_every_crew_session_and_the_captain_as_owner() {
        let (_s, port, _d) = seeded().await;
        let ctx = port.channel_ctx("c1/lobby").await.unwrap().unwrap();
        let mut m = ctx.session_members.clone();
        m.sort();
        assert_eq!(m, ["cap", "w1"]);
        assert_eq!(ctx.owner.as_deref(), Some("cap"));
        assert_eq!(port.captain("c1").await.unwrap().as_deref(), Some("cap"));
    }

    #[tokio::test]
    async fn collab_changes_become_orchestra_facts() {
        let ch = Change::RestartAnswered { id: 3, answer: crewd_proto::RestartAnswer::Resume, session_ids: vec!["w1".into()] };
        assert_eq!(fact_of(&ch), Some(CollabFact::RestartAnswered { prompt: 3, resume: true, sessions: vec!["w1".into()] }));
        assert_eq!(
            fact_of(&Change::WakeEnqueued { wake_id: 1, session_id: "w1".into() }),
            Some(CollabFact::WakeEnqueued { session: "w1".into() })
        );
        assert_eq!(
            fact_of(&Change::Progress { session_id: "w1".into(), kind: crewd_collab::ProgressKind::ToolCall }),
            Some(CollabFact::Progress { session: "w1".into() }),
            "C3：产出信号进熔断"
        );
        assert_eq!(fact_of(&Change::CrewChanged { crew_id: "c1".into() }), None);
    }
}
```

（`seed(tx, wd)` 与 `post_as(tx, sender_session, channel, text, mention_sessions) -> Message` 两个测试帮手写在测试模块里，按 Step 1 对齐表调 `crews::create` / `sessions::register` / `crews::set_captain` / `messages::post`。）

Run: `cargo test -p crewd-server orchestra::db`
Expected: 编译失败（`DbPort` 未定义）。

- [ ] **Step 5: 实现 `db.rs`**

```rust
//! `Port` 的生产实现。写一律经 M3a 冻结的 `crewd_collab` 函数（`AppState::collab` 在库线程上提交、按序发布变更），
//! 读经 `Db::call`；M3c 不建表、不写表、不写 SQL。M3a 的名字只在这个文件里出现（Task 10 Step 1 对齐）。
use std::path::PathBuf;
use std::sync::Arc;

use crewd_collab as collab;
use crewd_collab::Change;
use crewd_collab::wakes::{self as cw, Enqueued, WakePriority, WakeRequest, WakeRow};
use crewd_exec::resume::AgentIdLedger;
use crewd_proto::{MemberRef, MentionKind, Message, RestartAnswer, SenderKind};
use crewd_wake::breaker::{self, HUMAN, SYSTEM};
use crewd_wake::plan::{ChannelCtx, Listen, Mention, MsgFacts, Sender};
use crewd_wake::text::Lang;
use crewd_wake::{Latch, LatchScope, MessageRef, Ms, NewWake, Priority, QueuedWake, WakeKind, WakeState};
use tokio::sync::broadcast::error::RecvError;

use super::Handle;
use super::core::CollabFact;
use super::port::*;
use crate::AppState;
use crate::ledger::StoreLedger;

pub struct DbPort {
    state: Arc<AppState>,
    db_path: PathBuf,
}

impl DbPort {
    pub fn new(state: Arc<AppState>, db_path: PathBuf) -> Self {
        Self { state, db_path }
    }

    async fn read<T: Send + 'static>(
        &self,
        f: impl FnOnce(&rusqlite::Connection) -> collab::Result<T> + Send + 'static,
    ) -> Result<T> {
        Ok(self.state.db.call(move |s| f(s.conn())).await??)
    }

    async fn write<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut collab::Tx<'_>) -> collab::Result<T> + Send + 'static,
    ) -> Result<T> {
        Ok(self.state.collab(f).await?)
    }
}

/// M3a 的读函数「不存在 = NotFound」，Port 要的是 `Option`。
fn opt<T>(r: collab::Result<T>) -> collab::Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(collab::CollabError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

fn ms_of(ts: &str) -> Option<Ms> {
    chrono::DateTime::parse_from_rfc3339(ts).ok().map(|t| Ms(t.timestamp_millis()))
}

fn kind_to(k: WakeKind) -> cw::WakeKind {
    match k {
        WakeKind::Message => cw::WakeKind::Message,
        WakeKind::Timer => cw::WakeKind::Timer,
        WakeKind::AskAnswer => cw::WakeKind::AskAnswer,
        WakeKind::Restart => cw::WakeKind::Restart,
        WakeKind::System => cw::WakeKind::System,
        WakeKind::Checkpoint => cw::WakeKind::Checkpoint,
        WakeKind::StopCheck => cw::WakeKind::StopCheck,
    }
}

fn kind_from(k: cw::WakeKind) -> WakeKind {
    match k {
        cw::WakeKind::Message => WakeKind::Message,
        cw::WakeKind::Timer => WakeKind::Timer,
        cw::WakeKind::AskAnswer => WakeKind::AskAnswer,
        cw::WakeKind::Restart => WakeKind::Restart,
        cw::WakeKind::System => WakeKind::System,
        cw::WakeKind::Checkpoint => WakeKind::Checkpoint,
        cw::WakeKind::StopCheck => WakeKind::StopCheck,
    }
}

fn priority_to(p: Priority) -> WakePriority {
    match p {
        Priority::Auto => WakePriority::Auto,
        Priority::System => WakePriority::System,
        Priority::Human => WakePriority::Human,
    }
}

fn priority_from(p: WakePriority) -> Priority {
    match p {
        WakePriority::Auto => Priority::Auto,
        WakePriority::System => Priority::System,
        WakePriority::Human => Priority::Human,
    }
}

fn state_from(s: cw::WakeState) -> WakeState {
    match s {
        cw::WakeState::Pending => WakeState::Pending,
        cw::WakeState::Inflight => WakeState::Inflight,
        cw::WakeState::Delivered => WakeState::Delivered,
        cw::WakeState::Dropped => WakeState::Dropped,
    }
}

/// 熔断来源键 → `wakes.origin`（M3a：触发它的 session，人 / 系统 = NULL）。
fn origin_to(origin: &str) -> Option<String> {
    (origin != HUMAN && origin != SYSTEM).then(|| origin.to_string())
}

/// `wakes.origin` → 熔断来源键：NULL 按优先级还原成 `human` / `system`。
fn origin_from(row: &WakeRow) -> String {
    match &row.origin {
        Some(s) => s.clone(),
        None if row.priority == WakePriority::Human => HUMAN.into(),
        None => SYSTEM.into(),
    }
}

fn queued(conn: &rusqlite::Connection, row: &WakeRow) -> collab::Result<QueuedWake> {
    let msg = match row.source.strip_prefix("msg:") {
        Some(id) if row.kind == cw::WakeKind::Message => {
            opt(collab::messages::get(conn, id))?.map(|m| MessageRef { channel: m.channel_id, seq: m.seq })
        }
        _ => None,
    };
    Ok(QueuedWake {
        id: row.id,
        session: row.session.clone(),
        kind: kind_from(row.kind),
        source: row.source.clone(),
        priority: priority_from(row.priority),
        not_before: Ms(row.not_before_ms),
        note: row.note.clone(),
        state: state_from(row.state),
        attempts: row.attempts,
        created_at: ms_of(&row.created_at).unwrap_or_default(),
        origin: origin_from(row),
        msg,
    })
}

fn facts_of(m: &Message) -> MsgFacts {
    let sender = match (m.sender_kind, &m.sender_session_id) {
        (SenderKind::Human, _) => Sender::Human,
        (SenderKind::Session | SenderKind::Captain, Some(id)) => Sender::Session(id.clone()),
        _ => Sender::System,
    };
    let mentions = m
        .mentions
        .iter()
        .map(|x| match (x.kind, &x.target_id) {
            (MentionKind::Session | MentionKind::Captain, Some(t)) => Mention::Session(t.clone()),
            (MentionKind::Human, _) => Mention::Human,
            _ => Mention::Broadcast,
        })
        .collect();
    MsgFacts {
        id: m.id.clone(),
        at: MessageRef { channel: m.channel_id.clone(), seq: m.seq },
        sender,
        mentions,
        created_at: ms_of(&m.created_at).unwrap_or_default(),
    }
}

fn row_of(conn: &rusqlite::Connection, s: crewd_proto::Session) -> collab::Result<SessionRow> {
    let crew = collab::crews::get(conn, &s.crew_id)?;
    let captain = collab::crews::captain_of(conn, &s.crew_id)?;
    let workdir = s
        .worktree
        .clone()
        .or(s.working_directory.clone())
        .or(crew.working_directory.clone())
        .map(PathBuf::from);
    Ok(SessionRow {
        captain: captain.as_deref() == Some(s.id.as_str()),
        crew_id: s.crew_id,
        executor: s.executor,
        display_name: s.display_name,
        agent_session_id: s.agent_session_id,
        workdir,
        model: s.model,
        effort: s.effort,
        brief: s.brief,
        desired: if s.desired == crewd_proto::Desired::Running { Desired::Running } else { Desired::Stopped },
        was_working: s.was_working,
        last_active_at: s.last_active_at.as_deref().and_then(ms_of),
        resume_failed_at: s.resume_failed_at.as_deref().and_then(ms_of),
        awaiting_question: s.awaiting_question,
        last_progress_at: s.last_progress_at.as_deref().and_then(ms_of),
        id: s.id,
    })
}

fn latch_scope_to(l: &Latch) -> cw::latches::Scope {
    match l.scope {
        LatchScope::Session => cw::latches::Scope::Session(l.id.clone()),
        LatchScope::Edge => {
            let (from, to) = l.id.split_once('>').unwrap_or((l.id.as_str(), ""));
            cw::latches::Scope::Edge { from: from.into(), to: to.into() }
        }
    }
}

impl Port for DbPort {
    async fn session(&self, id: &str) -> Result<Option<SessionRow>> {
        let id = id.to_string();
        self.read(move |c| opt(collab::sessions::get(c, &id))?.map(|s| row_of(c, s)).transpose()).await
    }

    async fn sessions_desired_running(&self) -> Result<Vec<SessionRow>> {
        self.read(|c| collab::sessions::running_desired(c)?.into_iter().map(|s| row_of(c, s)).collect()).await
    }

    async fn all_session_ids(&self) -> Result<Vec<String>> {
        self.read(|c| Ok(collab::sessions::list(c, None)?.into_iter().map(|s| s.id).collect())).await
    }

    async fn crew_workdir(&self, crew: &str) -> Result<Option<PathBuf>> {
        let crew = crew.to_string();
        self.read(move |c| Ok(collab::crews::get(c, &crew)?.working_directory.filter(|w| !w.is_empty()).map(PathBuf::from))).await
    }

    async fn insert_session(&self, s: NewSession) -> Result<SessionRow> {
        let session = self
            .write(move |tx| {
                collab::sessions::register(
                    tx,
                    collab::sessions::NewSession {
                        crew_id: s.crew_id,
                        role: if s.captain { crewd_proto::Role::Captain } else { crewd_proto::Role::Worker },
                        executor: s.executor,
                        brief: Some(s.brief),
                        title: s.title,
                        model: s.model,
                        effort: s.effort,
                        worktree: s.worktree.map(|p| p.display().to_string()),
                        ..Default::default()
                    },
                )
            })
            .await?;
        self.read(move |c| row_of(c, session)).await
    }

    async fn patch_session(&self, id: &str, p: SessionPatch) -> Result<()> {
        let id = id.to_string();
        self.write(move |tx| {
            if let Some(d) = p.desired {
                let d = if d == Desired::Running { crewd_proto::Desired::Running } else { crewd_proto::Desired::Stopped };
                collab::sessions::set_desired(tx, &id, d)?;
            }
            // `set_working` 同时写 last_active_at；只给 last_active_at 的（退出时）没有对应的 M3a 写函数，略过。
            if let Some(w) = p.was_working {
                collab::sessions::set_working(tx, &id, w)?;
            }
            if let Some(f) = p.resume_failed_at {
                collab::sessions::set_resume_failed(tx, &id, f.is_some())?;
            }
            if p.model.is_some() || p.effort.is_some() {
                collab::sessions::set_profile(tx, &id, p.model.flatten(), p.effort.flatten())?;
            }
            Ok(())
        })
        .await
    }

    async fn set_captain(&self, crew: &str, session: &str) -> Result<Option<String>> {
        let (crew, session) = (crew.to_string(), session.to_string());
        self.write(move |tx| {
            let prev = collab::crews::captain_of(tx.conn(), &crew)?;
            collab::crews::set_captain(tx, &crew, &session)?;
            Ok(prev.filter(|p| *p != session))
        })
        .await
    }

    async fn captain(&self, crew: &str) -> Result<Option<String>> {
        let crew = crew.to_string();
        self.read(move |c| collab::crews::captain_of(c, &crew)).await
    }

    fn ledger(&self, session: &str) -> Arc<dyn AgentIdLedger> {
        Arc::new(StoreLedger::new(self.db_path.clone(), session))
    }

    async fn worldview(&self, session: &str) -> Result<String> {
        let session = session.to_string();
        self.read(move |c| {
            let locale = collab::settings::locale(c)?;
            Ok(collab::worldview::render(&collab::worldview::context_for(c, &session, locale)?))
        })
        .await
    }

    fn mint_token(&self, session: &str) -> String {
        self.state.tokens.mint(session)
    }

    async fn enqueue(&self, w: NewWake) -> Result<bool> {
        let r = self
            .write(move |tx| {
                cw::enqueue(
                    tx,
                    WakeRequest {
                        session: w.session,
                        kind: kind_to(w.kind),
                        source: w.source,
                        priority: priority_to(w.priority),
                        not_before_ms: Some(w.not_before.0),
                        note: w.note,
                        origin: origin_to(&w.origin),
                    },
                )
            })
            .await?;
        Ok(matches!(r, Enqueued::New(_)))
    }

    async fn pending(&self, session: &str) -> Result<Vec<QueuedWake>> {
        let session = session.to_string();
        // `due_for(…, i64::MAX)` = 全部 pending（不论到没到点），按优先级、入队先后。
        self.read(move |c| cw::due_for(c, &session, i64::MAX)?.iter().map(|r| queued(c, r)).collect()).await
    }

    async fn settle(&self, ids: &[i64], s: Settle, _now: Ms) -> Result<()> {
        let ids = ids.to_vec();
        self.write(move |tx| {
            match s {
                Settle::Inflight => cw::mark_inflight(tx, &ids)?,
                Settle::Delivered => cw::mark_delivered(tx, &ids)?,
                Settle::Retry { error, retry_at, .. } => {
                    for id in &ids {
                        cw::mark_failed(tx, *id, &error, Some(retry_at.0))?;
                    }
                }
                Settle::Dropped { error } => {
                    for id in &ids {
                        cw::mark_dropped(tx, *id, &error)?;
                    }
                }
                // inflight → pending，次数不变（M3a `requeue`）。
                Settle::Requeue => cw::requeue(tx, &ids)?,
            }
            Ok(())
        })
        .await
    }

    async fn requeue_inflight(&self) -> Result<usize> {
        self.write(cw::reset_inflight).await
    }

    async fn due_sessions(&self, now: Ms) -> Result<Vec<String>> {
        self.read(move |c| cw::sessions_with_due(c, now.0)).await
    }

    async fn next_not_before_after(&self, now: Ms) -> Result<Option<Ms>> {
        self.read(move |c| Ok(cw::next_due_after(c, now.0)?.map(Ms))).await
    }

    async fn list_wakes(&self, session: Option<&str>) -> Result<Vec<QueuedWake>> {
        let session = session.map(str::to_string);
        self.read(move |c| cw::list(c, session.as_deref(), 200)?.iter().map(|r| queued(c, r)).collect()).await
    }

    async fn latches(&self) -> Result<Vec<Latch>> {
        self.read(|c| {
            Ok(cw::latches::list(c)?
                .into_iter()
                .map(|l| {
                    let (scope, id) = match &l.scope {
                        cw::latches::Scope::Edge { from, to } => (LatchScope::Edge, breaker::edge_id(from, to)),
                        cw::latches::Scope::Session(s) => (LatchScope::Session, s.clone()),
                    };
                    Latch { scope, id, reason: l.reason.clone(), at: ms_of(&l.at).unwrap_or_default() }
                })
                .collect())
        })
        .await
    }

    async fn put_latch(&self, l: &Latch) -> Result<()> {
        let (scope, reason) = (latch_scope_to(l), l.reason.clone());
        self.write(move |tx| cw::latches::set(tx, &scope, &reason)).await
    }

    async fn clear_latch(&self, scope: LatchScope, id: &str) -> Result<()> {
        let l = Latch { scope, id: id.to_string(), reason: String::new(), at: Ms(0) };
        let scope = latch_scope_to(&l);
        self.write(move |tx| cw::latches::clear(tx, &scope).map(|_| ())).await
    }

    async fn max_live(&self) -> Result<Option<usize>> {
        self.read(|c| Ok(collab::settings::max_live_sessions(c)?.map(|n| n as usize))).await
    }

    async fn set_max_live(&self, max: Option<usize>) -> Result<()> {
        self.write(move |tx| collab::settings::set_max_live_sessions(tx, max.map(|n| n as u32))).await
    }

    async fn locale(&self) -> Result<Lang> {
        self.read(|c| {
            Ok(match collab::settings::locale(c)? {
                collab::Locale::Zh => Lang::Zh,
                collab::Locale::En => Lang::En,
            })
        })
        .await
    }

    async fn open_restart(&self, session_ids: Vec<String>) -> Result<Option<i64>> {
        self.write(move |tx| Ok(collab::restart::open(tx, session_ids)?.map(|p| p.id))).await
    }

    async fn prune_wakes(&self, before: Ms) -> Result<usize> {
        self.write(move |tx| cw::prune(tx, before.0)).await
    }

    async fn apply_due_defaults(&self) -> Result<()> {
        let crews: Vec<String> = self.read(|c| Ok(collab::crews::list(c)?.into_iter().map(|c| c.id).collect())).await?;
        for crew in crews {
            // 每个机组一个事务：一本账出错不连累别的机组。
            if let Err(e) = self.write(move |tx| collab::asks::apply_due_defaults(tx, &crew).map(|_| ())).await {
                eprintln!("crewd: 编排：默认方案到点处理失败：{e:#}");
            }
        }
        Ok(())
    }

    async fn message(&self, r: &MessageRef) -> Result<Option<(MsgFacts, String)>> {
        let r = r.clone();
        self.read(move |c| {
            let page = collab::messages::list(c, &r.channel, Some(r.seq - 1), Some(r.seq + 1), 1)?;
            Ok(page.into_iter().find(|m| m.seq == r.seq).map(|m| (facts_of(&m), m.text.clone())))
        })
        .await
    }

    async fn channel_ctx(&self, channel: &str) -> Result<Option<ChannelCtx>> {
        let channel = channel.to_string();
        self.read(move |c| {
            if collab::channels::get(c, &channel).is_err() {
                return Ok(None);
            }
            // `members` = 有效成员（M3a 冻结：大厅含本机组全部 session，人类群含根机组机长）。
            let session_members = collab::channels::members(c, &channel)?
                .into_iter()
                .filter_map(|m| match m {
                    MemberRef::Session { id } => Some(id),
                    MemberRef::Human => None,
                })
                .collect();
            let owner = match collab::channels::owner_of(c, &channel)? {
                Some(MemberRef::Session { id }) => Some(id),
                _ => None,
            };
            let listens = collab::listens::active_for_channel(c, &channel, 0)?
                .into_iter()
                .map(|l| Listen {
                    session: l.session,
                    until: l.until_ms.map_or(Ms(i64::MAX), Ms),
                    senders: (!l.senders.is_empty()).then_some(l.senders),
                })
                .collect();
            Ok(Some(ChannelCtx { session_members, owner, listens }))
        })
        .await
    }

    async fn unread(&self, session: &str) -> Result<Vec<MsgFacts>> {
        let session = session.to_string();
        let blocks = self.write(move |tx| collab::cursors::unread_for(tx, &session)).await?;
        Ok(blocks.iter().flat_map(|(_, msgs)| msgs.iter().map(facts_of)).collect())
    }

    async fn cursor(&self, session: &str, channel: &str) -> Result<i64> {
        let (session, channel) = (session.to_string(), channel.to_string());
        self.read(move |c| Ok(collab::cursors::cursor(c, &session, &channel)?.unwrap_or(0))).await
    }

    async fn advance(&self, session: &str, channel: &str, seq: i64) -> Result<()> {
        let (session, channel) = (session.to_string(), channel.to_string());
        self.write(move |tx| collab::cursors::advance(tx, &session, &channel, seq)).await
    }

    async fn post_system(&self, to: Audience, text: String) -> Result<()> {
        self.write(move |tx| {
            let (channel, mention_human, mention_owner) = match to {
                Audience::HumanChannel { crew } => {
                    let root = collab::crews::root_of(tx.conn(), &crew)?;
                    (collab::channels::ensure_human_channel(tx, &root)?.id, true, false)
                }
                Audience::Lobby { crew, mention_owner } => (crewd_store::channels::lobby_id(&crew), false, mention_owner),
                Audience::Channel { channel, mention_owner } => (channel, false, mention_owner),
            };
            let mut mentions = Vec::new();
            if mention_human {
                mentions.push(crewd_proto::Mention { kind: MentionKind::Human, target_id: None });
            }
            if mention_owner && let Some(MemberRef::Session { id }) = collab::channels::owner_of(tx.conn(), &channel)? {
                mentions.push(crewd_proto::Mention { kind: MentionKind::Session, target_id: Some(id) });
            }
            let post = collab::messages::NewPost { channel_id: channel, text, mentions, ..Default::default() };
            collab::messages::post(tx, collab::Sender::System, post).map(|_| ())
        })
        .await
    }

    async fn render_wake(&self, session: &str, batch: &[QueuedWake]) -> Result<Rendered> {
        let (session, ids): (String, Vec<i64>) = (session.to_string(), batch.iter().map(|w| w.id).collect());
        let (text, dropped) = self
            .read(move |c| {
                let rows: Vec<WakeRow> = cw::by_ids(c, &ids)?;
                let b = collab::render::build_batch(c, &session, &rows)?;
                let text = collab::render::wake_text(collab::settings::locale(c)?, &b);
                Ok((text, b.unusable))
            })
            .await?;
        let dropped_ids: Vec<i64> = dropped.iter().map(|(id, _)| *id).collect();
        if !dropped.is_empty() {
            self.write(move |tx| {
                for (id, why) in &dropped {
                    cw::mark_dropped(tx, *id, why)?;
                }
                Ok(())
            })
            .await?;
        }
        Ok(Rendered { text, dropped: dropped_ids })
    }

    fn publish(&self, e: OrchestraEvent) {
        let event = match e {
            OrchestraEvent::SessionChanged(v) => crewd_proto::Event {
                event: crewd_proto::events::SESSION_CHANGED.into(),
                data: serde_json::to_value(super::control::live_to_proto(&v)).unwrap_or_default(),
            },
            OrchestraEvent::WakeChanged { session } => crewd_proto::Event {
                event: crewd_proto::orchestra::events::WAKE_CHANGED.into(),
                data: serde_json::json!({ "sessionId": session }),
            },
        };
        self.state.hub.publish(event);
    }
}

/// M3a 的协作变更 → 编排关心的事实；其余的不管。
pub fn fact_of(c: &Change) -> Option<CollabFact> {
    match c {
        Change::MessageAppended { channel_id, seq, .. } => Some(CollabFact::MessageAppended { channel: channel_id.clone(), seq: *seq }),
        Change::WakeEnqueued { session_id, .. } => Some(CollabFact::WakeEnqueued { session: session_id.clone() }),
        Change::AskAnswered { crew_id, ask, cleared, .. } => {
            Some(CollabFact::AskAnswered { crew: crew_id.clone(), ask: ask.clone(), cleared: cleared.clone() })
        }
        Change::RestartAnswered { id, answer, session_ids } => Some(CollabFact::RestartAnswered {
            prompt: *id,
            resume: *answer == RestartAnswer::Resume,
            sessions: session_ids.clone(),
        }),
        // 控制者裁定 C3：工具调用 / 工作项变化（M3b 的 post-tool-use、M4b 的工作项命令经 M3a `note_progress` 发）。
        Change::Progress { session_id, .. } => Some(CollabFact::Progress { session: session_id.clone() }),
        _ => None,
    }
}

/// 把协作变更转给编排 actor；订阅落后 → `Lagged`（积压补救，裁定 16）。
/// `rx` 要在 `orchestra::spawn`（里面跑 `boot`）**之前**订阅：boot 期间产生的变更不能漏。
pub fn forward_collab(mut rx: tokio::sync::broadcast::Receiver<crate::hub::CollabEvent>, handle: Handle) {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(c) => {
                    if let Some(f) = fact_of(&c) {
                        handle.collab(f);
                    }
                }
                Err(RecvError::Lagged(_)) => handle.collab(CollabFact::Lagged),
                Err(RecvError::Closed) => return,
            }
        }
    });
}
```

（`db.rs` 不写 SQL，只用到 `rusqlite::Connection` 这个类型；`rusqlite` 若 server 没有直接依赖，经 `cargo add --offline -p crewd-server rusqlite`（Cargo.lock 已有同版本）取。`collab::Result` 里的错误转换以 M3a 的 `CollabError` 为准。）

Run: `cargo test -p crewd-server orchestra::db`
Expected: PASS（8 条）。

- [ ] **Step 6: `control.rs`（`SessionControl` 的实现）**

```rust
//! M3a 的 `SessionControl`（M3b 的工具经它起停会话）由编排实现。agent 起会话受可选上限约束（`Origin::Agent`）。
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use crewd_exec::health::HealthKind;
use crewd_proto::{DecisionView, HealthKindView, HealthView, Role, Session, SessionLive};

use super::Handle;
use super::core::{ControlError, Origin, StartSession};
use super::port::LiveView;
// `BoxFuture` 用 M3a `control.rs` 里 trait 签名用的那一个（Step 1 对齐表）。
use crate::control::{BoxFuture, ControlError as M3aError, Inspection, QuotaReport, SessionControl, StartRequest};

/// `AppState` 先建（编排的 Port 要持有它），编排后起：句柄晚到，放在 `OnceLock` 里。
pub struct OrchestraControl {
    cell: Arc<OnceLock<Handle>>,
    db: crewd_store::Db,
}

impl OrchestraControl {
    pub fn new(cell: Arc<OnceLock<Handle>>, db: crewd_store::Db) -> Self {
        Self { cell, db }
    }

    fn h(&self) -> Result<&Handle, M3aError> {
        self.cell.get().ok_or_else(|| M3aError::Failed("编排还没起来".into()))
    }

    async fn session(&self, id: String) -> Result<Session, M3aError> {
        self.db
            .call(move |s| crewd_collab::sessions::get(s.conn(), &id))
            .await
            .map_err(|e| M3aError::Failed(e.to_string()))?
            .map_err(|e| match e {
                // M3a：不存在 = CollabError::NotFound（冻结块）。
                crewd_collab::CollabError::NotFound(m) => M3aError::NotFound(m),
                e => M3aError::Failed(e.to_string()),
            })
    }
}

pub fn map_error(e: ControlError) -> M3aError {
    match e {
        ControlError::NotFound(s) => M3aError::NotFound(s),
        ControlError::AlreadyRunning(s) => M3aError::AlreadyRunning(s),
        ControlError::Unsupported(s) => M3aError::Unsupported(s),
        e @ (ControlError::NotRunning(_) | ControlError::NoWorkdir(_)) => M3aError::Invalid(e.to_string()),
        // 建 worktree 失败（C8）带原因原样给调用方（M3b 的 `start_session` 回执 = `r.failed` + 这段原因）。
        e @ (ControlError::AtCap { .. } | ControlError::Internal(_) | ControlError::Worktree(_)) => M3aError::Failed(e.to_string()),
    }
}

pub fn live_to_proto(v: &LiveView) -> SessionLive {
    SessionLive {
        session_id: v.session_id.clone(),
        running: v.running,
        working: v.working,
        health: v.health.as_ref().map(|h| HealthView {
            kind: match h.kind {
                HealthKind::AuthRequired => HealthKindView::AuthRequired,
                HealthKind::UsageLimit => HealthKindView::UsageLimit,
                HealthKind::RateLimited => HealthKindView::RateLimited,
                HealthKind::LaunchFailed => HealthKindView::LaunchFailed,
                HealthKind::BriefUndelivered => HealthKindView::BriefUndelivered,
                HealthKind::PromptUndelivered => HealthKindView::PromptUndelivered,
            },
            detail: h.detail.clone(),
        }),
        pending_decision: v.pending_decision.as_ref().map(|d| DecisionView {
            summary: d.prompt.clone(),
            options: d.options.clone(),
            numbered: d.numbered,
        }),
        awaiting_question: v.awaiting_question.clone(),
    }
}

impl SessionControl for OrchestraControl {
    fn start(&self, req: StartRequest) -> BoxFuture<'_, Result<Session, M3aError>> {
        Box::pin(async move {
            let start = StartSession {
                crew_id: req.crew_id,
                captain: req.role == Role::Captain,
                executor: req.executor,
                brief: req.brief,
                title: req.title,
                model: req.model,
                effort: req.effort,
                isolation: req.isolation,
            };
            let row = self.h()?.start(start, Origin::Agent).await.map_err(map_error)?;
            self.session(row.id).await
        })
    }

    fn resume(&self, session_id: String) -> BoxFuture<'_, Result<(), M3aError>> {
        Box::pin(async move { self.h()?.resume(&session_id, Origin::Agent).await.map_err(map_error) })
    }

    fn stop(&self, session_id: String, reason: String) -> BoxFuture<'_, Result<(), M3aError>> {
        Box::pin(async move {
            eprintln!("crewd: 停止 session {session_id}：{reason}");
            self.h()?.stop(&session_id).await.map_err(map_error)
        })
    }

    fn nudge(&self, session_id: String, text: String) -> BoxFuture<'_, Result<(), M3aError>> {
        Box::pin(async move { self.h()?.nudge(&session_id, text).await.map_err(map_error) })
    }

    fn inspect(&self, session_id: String, max_lines: usize) -> BoxFuture<'_, Result<Inspection, M3aError>> {
        Box::pin(async move {
            let (live, screen) = self.h()?.inspect(&session_id, max_lines).await.map_err(map_error)?;
            Ok(Inspection { live: live_to_proto(&live), screen })
        })
    }

    /// 只落库、下次启动生效（控制者裁定 C5；中途 `/model` 切换不在 M3c）。
    fn set_profile(&self, session_id: String, model: Option<String>, effort: Option<String>) -> BoxFuture<'_, Result<(), M3aError>> {
        Box::pin(async move { self.h()?.set_profile(&session_id, model, effort).await.map_err(map_error) })
    }

    fn handoff_captain(&self, crew_id: String, to_session_id: String) -> BoxFuture<'_, Result<(), M3aError>> {
        Box::pin(async move { self.h()?.set_captain(&crew_id, &to_session_id).await.map_err(map_error) })
    }

    /// crewd 还读不到额度快照（spec 问题：`Health` 不带重置时刻、额度来源未定），如实返回空。
    fn quota(&self) -> BoxFuture<'_, Result<QuotaReport, M3aError>> {
        Box::pin(async { Ok(QuotaReport { lines: vec![] }) })
    }

    fn live(&self, session_id: String) -> BoxFuture<'_, Result<SessionLive, M3aError>> {
        Box::pin(async move { self.h()?.live(&session_id).await.map(|v| live_to_proto(&v)).map_err(map_error) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crewd_exec::health::Health;
    use crewd_term::PendingDecision;

    #[test]
    fn errors_map_onto_the_m3a_vocabulary() {
        assert_eq!(map_error(ControlError::AlreadyRunning("s".into())), M3aError::AlreadyRunning("s".into()));
        assert!(matches!(map_error(ControlError::AtCap { max: 2 }), M3aError::Failed(_)));
        assert!(matches!(map_error(ControlError::NotRunning("s".into())), M3aError::Invalid(_)));
    }

    #[test]
    fn live_view_keeps_the_numbered_fact_and_health_kind() {
        let v = LiveView {
            session_id: "s".into(),
            running: true,
            working: false,
            health: Some(Health::new(HealthKind::UsageLimit, "x")),
            pending_decision: Some(PendingDecision { prompt: "Trust?".into(), options: vec!["Yes".into()], numbered: false }),
            awaiting_question: None,
        };
        let p = live_to_proto(&v);
        assert_eq!(p.health.unwrap().kind, HealthKindView::UsageLimit);
        assert!(!p.pending_decision.unwrap().numbered, "绝不编造编号");
    }
}
```

（M2c 合入后 `HealthKind` 多两个变体，`live_to_proto` 的 match 会编不过——在 M3a 的 `HealthKindView` 里有对应项就映射，没有就报回。）

- [ ] **Step 7: `rpc.rs`（M3c 的客户端 RPC）+ dispatch / AppState 接线**

```rust
//! M3c 的客户端方法（spec §22.4「客户端 RPC（M3c 新增）」）。客户端都是人：起 / 恢复不受可选上限、不错开。
use crewd_proto::orchestra::{
    AdmissionRecoverParams, CrewSetCaptainParams, LatchScopeView, OrchestraSettings, SessionIdParams, SessionStartParams,
    SessionStopParams, WakeKindView, WakeListParams, WakePriorityView, WakeStateView, WakeView, methods,
};
use crewd_proto::{Role, RpcError, parse_params};
use crewd_wake::breaker::edge_id;
use crewd_wake::{LatchScope, Priority, QueuedWake, WakeKind, WakeState};
use serde_json::{Value, json};

use super::Handle;
use super::control::live_to_proto;
use super::core::{ControlError, Origin, StartSession};

fn rpc_err(e: ControlError) -> RpcError {
    let code = match e {
        ControlError::NotFound(_) => -32004,
        ControlError::AlreadyRunning(_) => -32009,
        _ => RpcError::INVALID_PARAMS,
    };
    RpcError::new(code, e.to_string())
}

fn view(w: &QueuedWake) -> WakeView {
    WakeView {
        id: w.id,
        session_id: w.session.clone(),
        kind: match w.kind {
            WakeKind::Message => WakeKindView::Message,
            WakeKind::Timer => WakeKindView::Timer,
            WakeKind::AskAnswer => WakeKindView::AskAnswer,
            WakeKind::Restart => WakeKindView::Restart,
            WakeKind::System => WakeKindView::System,
            WakeKind::Checkpoint => WakeKindView::Checkpoint,
            WakeKind::StopCheck => WakeKindView::StopCheck,
        },
        source: w.source.clone(),
        origin: w.origin.clone(),
        priority: match w.priority {
            Priority::Auto => WakePriorityView::Auto,
            Priority::System => WakePriorityView::System,
            Priority::Human => WakePriorityView::Human,
        },
        not_before: w.not_before.0,
        state: match w.state {
            WakeState::Pending => WakeStateView::Pending,
            WakeState::Inflight => WakeStateView::Inflight,
            WakeState::Delivered => WakeStateView::Delivered,
            WakeState::Dropped => WakeStateView::Dropped,
        },
        attempts: w.attempts,
        note: w.note.clone(),
    }
}

/// 认得的方法返回 `Some`，其余 `None`（交回 M3a 的分发去报「未知方法」）。
pub async fn route(h: &Handle, db: &crewd_store::Db, method: &str, params: Value) -> Option<Result<Value, RpcError>> {
    if !methods::ALL.contains(&method) {
        return None;
    }
    Some(call(h, db, method, params).await)
}

async fn call(h: &Handle, db: &crewd_store::Db, method: &str, params: Value) -> Result<Value, RpcError> {
    let internal = |e: String| RpcError::new(RpcError::INTERNAL, e);
    match method {
        methods::SESSION_START => {
            let p: SessionStartParams = parse_params(params)?;
            let req = StartSession {
                crew_id: p.crew_id,
                captain: p.role == Role::Captain,
                executor: p.executor,
                brief: p.brief,
                title: p.title,
                model: p.model,
                effort: p.effort,
                isolation: p.isolation,
            };
            let row = h.start(req, Origin::Human).await.map_err(rpc_err)?;
            let s = db
                .call(move |s| crewd_collab::sessions::get(s.conn(), &row.id))
                .await
                .map_err(|e| internal(e.to_string()))?
                .map_err(|e| internal(e.to_string()))?;
            Ok(serde_json::to_value(s).map_err(|e| internal(e.to_string()))?)
        }
        methods::SESSION_STOP => {
            let p: SessionStopParams = parse_params(params)?;
            eprintln!("crewd: 客户端停止 session {}：{}", p.session_id, p.reason);
            h.stop(&p.session_id).await.map_err(rpc_err)?;
            Ok(json!({}))
        }
        methods::SESSION_RESUME => {
            let p: SessionIdParams = parse_params(params)?;
            h.resume(&p.session_id, Origin::Human).await.map_err(rpc_err)?;
            Ok(json!({}))
        }
        methods::SESSION_LIVE => {
            let p: SessionIdParams = parse_params(params)?;
            let v = h.live(&p.session_id).await.map_err(rpc_err)?;
            Ok(serde_json::to_value(live_to_proto(&v)).map_err(|e| internal(e.to_string()))?)
        }
        methods::CREW_SET_CAPTAIN => {
            let p: CrewSetCaptainParams = parse_params(params)?;
            h.set_captain(&p.crew_id, &p.session_id).await.map_err(rpc_err)?;
            Ok(json!({}))
        }
        methods::WAKE_LIST => {
            let p: WakeListParams = parse_params(params)?;
            let v: Vec<WakeView> = h.wakes(p.session_id).await.map_err(rpc_err)?.iter().map(view).collect();
            Ok(serde_json::to_value(v).map_err(|e| internal(e.to_string()))?)
        }
        methods::ADMISSION_RECOVER => {
            let p: AdmissionRecoverParams = parse_params(params)?;
            let (scope, id) = match (p.scope, p.from) {
                (LatchScopeView::Session, _) => (LatchScope::Session, p.target_session_id),
                (LatchScopeView::Edge, Some(from)) => (LatchScope::Edge, edge_id(&from, &p.target_session_id)),
                (LatchScopeView::Edge, None) => {
                    return Err(RpcError::new(RpcError::INVALID_PARAMS, "scope=edge 要给 from"));
                }
            };
            h.recover(scope, id).await.map_err(rpc_err)?;
            Ok(json!({}))
        }
        methods::SETTINGS_GET => {
            let max = h.max_live().await.map_err(rpc_err)?;
            Ok(serde_json::to_value(OrchestraSettings { max_live_sessions: max.map(|n| n as u32) }).expect("纯数据"))
        }
        methods::SETTINGS_SET => {
            let p: OrchestraSettings = parse_params(params)?;
            h.set_max_live(p.max_live_sessions.map(|n| n as usize)).await.map_err(rpc_err)?;
            Ok(serde_json::to_value(p).expect("纯数据"))
        }
        _ => unreachable!("route 已按 methods::ALL 过滤"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestra::clock::ManualClock;
    use crate::orchestra::exec::Executors;
    use crate::orchestra::hooks::Hooks;
    use crate::orchestra::testkit::{MemPort, T0};
    use crate::orchestra::{OrchestraConfig, spawn};
    use std::sync::Arc;

    async fn handle() -> Handle {
        spawn(Arc::new(MemPort::new()), Executors::new(), Hooks::default(), Arc::new(ManualClock::new(T0)), OrchestraConfig::for_tests())
            .await
            .unwrap()
    }

    fn db() -> crewd_store::Db {
        crewd_store::Db::spawn(crewd_store::Store::open_in_memory().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn unknown_methods_fall_through_to_the_m3a_dispatcher() {
        assert!(route(&handle().await, &db(), "message.post", json!({})).await.is_none());
    }

    #[tokio::test]
    async fn params_are_strict_and_edge_recovery_needs_a_source() {
        let (h, d) = (handle().await, db());
        let e = route(&h, &d, methods::SESSION_STOP, json!({"sessionId":"s","reason":"r","x":1})).await.unwrap().unwrap_err();
        assert_eq!(e.code, RpcError::INVALID_PARAMS);
        let e = route(&h, &d, methods::ADMISSION_RECOVER, json!({"scope":"edge","targetSessionId":"w1"})).await.unwrap().unwrap_err();
        assert_eq!(e.code, RpcError::INVALID_PARAMS);
        let e = route(&h, &d, methods::SESSION_LIVE, json!({"sessionId":"nope"})).await.unwrap().unwrap_err();
        assert_eq!(e.code, -32004);
    }

    #[tokio::test]
    async fn settings_round_trip() {
        let (h, d) = (handle().await, db());
        let v = route(&h, &d, methods::SETTINGS_SET, json!({"maxLiveSessions": 4})).await.unwrap().unwrap();
        assert_eq!(v["maxLiveSessions"], 4);
        let v = route(&h, &d, methods::SETTINGS_GET, json!({})).await.unwrap().unwrap();
        assert_eq!(v["maxLiveSessions"], 4);
    }
}
```

接线（只这一行；spec §22.5 的文件归属已列，`AppState.orchestra: OnceLock<orchestra::Handle>` M3a 已建——AppState 先建、编排后起，编排的 Port 要持有 AppState）：
1. `crates/server/src/dispatch.rs` 在「未知方法」那一支之前加：

```rust
        other if let Some(h) = state.orchestra.get()
            && let Some(r) = crate::orchestra::rpc::route(h, &state.db, other, req.params.clone()).await =>
        {
            r
        }
```

（M3a 的 `route` 若不是这个 match 形状，就在它返回「未知方法」之前调同一个函数——效果相同、只一处。）
2. 事件主题 `wake` 已在 M3a 的 `events::TOPICS` 里（`conn.rs` 不改）；`crewd_proto::orchestra::events::TOPIC_WAKE` 与它逐字相同（加一条断言）。

Run: `cargo test -p crewd-server && cargo run -p crewd -- schema > schema/crewd-api.schema.json && cargo test -p crewd --test e2e`
Expected: 全绿；`git diff --stat schema/crewd-api.schema.json` 只多出 M3c 的类型与方法。

- [ ] **Step 8: 门禁与提交**

Run: `./scripts/check.sh`
Expected: 全绿。

```bash
git add crates/server/src/orchestra/db.rs crates/server/src/orchestra/control.rs crates/server/src/orchestra/rpc.rs crates/server/src/orchestra/mod.rs crates/server/Cargo.toml Cargo.lock crates/server/src/dispatch.rs crates/proto/src/orchestra.rs crates/proto/src/lib.rs schema/crewd-api.schema.json
git diff --cached --stat
git commit -m "crewd-server: wire orchestra to M3a — DbPort over collab, change forwarding, SessionControl, M3c client RPCs

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 11: serve 拉起编排、`crewd serve --claude`、端到端（假 claude）

**Files:**
- Modify: `crewd/crates/server/src/lib.rs`（serve 启动那一段：`ServeOptions`、`serve_with`）、`crewd/crates/crewd/src/main.rs`（`serve --claude` / `--claude-env`）、`crewd/crates/crewd/tests/e2e.rs`（`crewd()` 帮手设 `CREWD_CLAUDE`）、`crewd/crates/crewd/Cargo.toml` + `crewd/Cargo.lock`（dev：`crewd-collab`、`crewd-wake`、`crewd-store` 已有）
- Create: `crewd/crates/server/tests/orchestra_fake_claude.rs`、`crewd/crates/crewd/tests/orchestra_restart.rs`

**Interfaces:**
- Consumes: Task 6 `ClaudeExecutor` / `ClaudeProgram`；Task 9 `spawn` / `Handle::shutdown`；Task 10 `DbPort`、`forward_collab`、`OrchestraControl`；M2b `crewd_server::reap_orphans`；M3a `AppState::{new, with_sessions}`、`Db::spawn`、`Hub::subscribe_collab`。
- Produces: `crewd_server::{ServeOptions, serve_with}`（`serve()` = `serve_with(…, ServeOptions::default(), …)`，**不起任何 agent**）；`crewd serve [--claude PATH | env CREWD_CLAUDE] [--claude-env K=V]…`。

- [ ] **Step 1: 先确认哪些测试会起 serve（安全闸，先做）**

```bash
rg -n '"serve"|serve\(|serve_with\(' crates/*/tests crates/*/src --glob '*.rs'
```

记下每一处。库里的 `serve()` 走 `ClaudeProgram::Disabled`（起不了任何 agent）；**起 `crewd serve` 二进制的每一个测试都必须设 `CREWD_CLAUDE`**（指向假 claude 或 `/usr/bin/false`），否则二进制会按 PATH 找到真 claude。本任务改 `crates/crewd/tests/e2e.rs` 的 `crewd()` 帮手；Step 1 列出的其余地方若也起二进制，同样加。

- [ ] **Step 2: `serve_with`（`crates/server/src/lib.rs` 的 serve 段）**

在 M3a 合入后的 `serve` 上改（保留它建 `Db` / `AppState` / socket / accept 循环的写法，只加编排）：

```rust
/// serve 的可选项。缺省 = 不起任何 agent（库里的 `serve()` 与测试用它）；`crewd serve` 二进制给 `Discover` 或路径。
#[derive(Default)]
pub struct ServeOptions {
    pub claude: orchestra::exec_claude::ClaudeProgram,
    pub claude_env: std::collections::BTreeMap<String, String>,
    /// MCP / hook helper；缺省 = 当前可执行文件（crewd 自己）。
    pub helper: Option<PathBuf>,
    pub claude_projects_dir: Option<PathBuf>,
    pub orchestra: orchestra::OrchestraConfig,
}

pub async fn serve(data_dir: PathBuf, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    serve_with(data_dir, ServeOptions::default(), shutdown).await
}

pub async fn serve_with(data_dir: PathBuf, opts: ServeOptions, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    ensure_data_dir(&data_dir)?;
    let lock = lock_data_dir(&data_dir)?;
    // 先回收孤儿（spec §22.4「serve 启动：回收孤儿 → …」），登记表交给会话管理。
    let (registry, reports) = reap_orphans(&data_dir)?;
    log_reap_reports(&reports);
    let sock = data_dir.join("crewd.sock");
    let _ = std::fs::remove_file(&sock);
    let db_path = data_dir.join("crewd.db");
    let db = crewd_store::Db::spawn(Store::open(&db_path)?)?;

    // 编排要持有 AppState（它的 Port 经 AppState::collab 写），AppState 又要持有编排（SessionControl）：句柄晚到，放 OnceLock。
    let cell = Arc::new(std::sync::OnceLock::new());
    let control = Arc::new(orchestra::control::OrchestraControl::new(cell.clone(), db.clone()));
    let state = Arc::new(AppState::new(db.clone()).with_sessions(control));
    let collab_rx = state.hub.subscribe_collab(); // 在 boot 之前订阅
    let helper = match opts.helper {
        Some(h) => h,
        None => std::env::current_exe()?,
    };
    let claude = orchestra::exec_claude::ClaudeExecutor::new(orchestra::exec_claude::ClaudeExecutorConfig {
        program: opts.claude,
        data_dir: data_dir.clone(),
        helper,
        registry: Some(registry),
        extra_env: opts.claude_env,
        projects_dir: opts.claude_projects_dir,
    });
    let execs = orchestra::exec::Executors::new().with("claude", Arc::new(claude));
    let port = Arc::new(orchestra::db::DbPort::new(state.clone(), db_path));
    let handle = orchestra::spawn(
        port,
        execs,
        orchestra::hooks::Hooks::default(),
        Arc::new(orchestra::clock::SystemClock),
        opts.orchestra,
    )
    .await?;
    let _ = cell.set(handle.clone());
    let _ = state.orchestra.set(handle.clone());
    orchestra::db::forward_collab(collab_rx, handle.clone());

    let listener = UnixListener::bind(&sock)?;
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                // ……M3a 的 accept 分支原样保留……
            }
        }
    }
    // 叫停全部会话（不改 desired / was_working：下次启动问人时要知道谁在跑、谁被打断）。
    handle.shutdown(std::time::Duration::from_secs(5)).await;
    let _ = std::fs::remove_file(&sock);
    drop(lock);
    Ok(())
}
```

（accept 分支按 M3a 合入后的原文保留，不重写；这里只是把它包进 `serve_with`。）

- [ ] **Step 3: `crewd serve --claude`（`crates/crewd/src/main.rs`）**

`Cmd::Serve` 从无字段改成：

```rust
    /// 常驻服务
    Serve {
        /// claude 可执行文件；不给就按登录 shell 的 PATH 找
        #[arg(long, env = "CREWD_CLAUDE")]
        claude: Option<PathBuf>,
        /// 给 agent 进程的额外环境变量（K=V，可重复；仍过执行器的禁用前缀）。测试器材用。
        #[arg(long = "claude-env", value_name = "K=V", hide = true)]
        claude_env: Vec<String>,
    },
```

分支：

```rust
        Cmd::Serve { claude, claude_env } => {
            let mut env = std::collections::BTreeMap::new();
            for kv in claude_env {
                let (k, v) = kv.split_once('=').with_context(|| format!("--claude-env 要写成 K=V：{kv}"))?;
                env.insert(k.to_string(), v.to_string());
            }
            let projects = std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .or_else(|| dirs::home_dir().map(|h| h.join(".claude")))
                .map(|d| d.join("projects"));
            let opts = crewd_server::ServeOptions {
                claude: match claude {
                    Some(p) => crewd_server::orchestra::exec_claude::ClaudeProgram::Path(p.clone()),
                    None => crewd_server::orchestra::exec_claude::ClaudeProgram::Discover,
                },
                claude_env: env,
                claude_projects_dir: projects,
                ..Default::default()
            };
            crewd_server::serve_with(dir, opts, shutdown_signal()).await?;
        }
```

`crates/crewd/tests/e2e.rs` 的 `crewd()` 帮手加一行 `.env("CREWD_CLAUDE", "/usr/bin/false")`（这些测试不起 agent；万一有唤醒拉起，起来的也只是 `false`）。

- [ ] **Step 4: 进程内端到端（`crates/server/tests/orchestra_fake_claude.rs`）**

```rust
//! 端到端：进程内 `serve_with` + crewd-exec 的假 claude（`target/<profile>/fake_claude`），经 socket 驱动 M3a 与 M3c 的 RPC。
//! 覆盖：起会话、人 @ worker 把它叫醒、停掉的 worker 被 @ 拉起（--resume）、并发 resume 只起一个、停机收掉全部。
//! session 之间互相 @、忙时 hook 注入不重投，要 M3b 的 helper 才能从 agent 侧发消息——单元测试覆盖，M3 集成验收再跑真链路。
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// 假 claude 与测试二进制在同一个 target 目录（`cargo test --workspace` 会先把 crewd-exec 的 bin 建好）。
fn fake_claude() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().and_then(Path::parent).unwrap();
    let fake = dir.join("fake_claude");
    assert!(fake.exists(), "找不到 {}：先跑 `cargo build -p crewd-exec --bin fake_claude`（check.sh 会自动建）", fake.display());
    fake
}

struct Client {
    lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    w: tokio::net::unix::OwnedWriteHalf,
    next: u64,
}

impl Client {
    async fn connect(sock: &Path) -> Self {
        for _ in 0..200 {
            if let Ok(s) = UnixStream::connect(sock).await {
                let (r, w) = s.into_split();
                return Self { lines: BufReader::new(r).lines(), w, next: 1 };
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("serve 5s 内没起来");
    }

    /// 返回 `Ok(result)` 或 `Err(code)`；途中的事件帧跳过。
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, i64> {
        let id = self.next;
        self.next += 1;
        let line = json!({ "id": id, "method": method, "params": params }).to_string() + "\n";
        self.w.write_all(line.as_bytes()).await.unwrap();
        loop {
            let l = tokio::time::timeout(Duration::from_secs(15), self.lines.next_line()).await.unwrap().unwrap().unwrap();
            let v: Value = serde_json::from_str(&l).unwrap();
            if v["id"] == id {
                return match v.get("error") {
                    Some(e) => Err(e["code"].as_i64().unwrap()),
                    None => Ok(v["result"].clone()),
                };
            }
        }
    }
}

async fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..300 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("15s 内没等到：{what}");
}

fn log_count(log: &Path, needle: &str) -> usize {
    std::fs::read_to_string(log).unwrap_or_default().matches(needle).count()
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_start_wake_resume_and_stop_through_the_real_driver() {
    let dir = tempfile::tempdir().unwrap();
    let (data, work, log) = (dir.path().join("data"), dir.path().join("work"), dir.path().join("submitted.log"));
    std::fs::create_dir_all(&work).unwrap();
    let mut env = std::collections::BTreeMap::new();
    env.insert("FAKE_CLAUDE_LOG".to_string(), log.display().to_string());
    let opts = crewd_server::ServeOptions {
        claude: crewd_server::orchestra::exec_claude::ClaudeProgram::Path(fake_claude()),
        claude_env: env,
        ..Default::default()
    };
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(crewd_server::serve_with(data.clone(), opts, async {
        let _ = stop_rx.await;
    }));
    let mut c = Client::connect(&data.join("crewd.sock")).await;

    let crew = c.call("crew.create", json!({"title":"合成机组","workingDirectory": work})).await.unwrap();
    let crew_id = crew["id"].as_str().unwrap().to_string();
    let cap = c
        .call("session.start", json!({"crewId": crew_id, "role":"captain", "executor":"claude", "brief":"合成开场甲", "isolation": false}))
        .await
        .unwrap();
    let w1 = c
        .call("session.start", json!({"crewId": crew_id, "role":"worker", "executor":"claude", "brief":"合成开场乙", "isolation": false}))
        .await
        .unwrap();
    let (cap_id, w1_id) = (cap["id"].as_str().unwrap().to_string(), w1["id"].as_str().unwrap().to_string());
    wait_for("两份开场任务都提交了", || log_count(&log, "合成开场甲") == 1 && log_count(&log, "合成开场乙") == 1).await;

    // 人 @ worker → 它被叫醒（正文由 M3a 的 render 组装，含原文）。
    let lobby = format!("{crew_id}/lobby");
    c.call("message.post", json!({"channelId": lobby, "text":"合成任务丙", "mentions":[{"kind":"session","targetId": w1_id}]}))
        .await
        .unwrap();
    wait_for("唤醒送到 worker", || log_count(&log, "合成任务丙") == 1).await;

    // 停掉 worker，再 @ 它 → 用 --resume 拉起，以唤醒正文开场。
    c.call("session.stop", json!({"sessionId": w1_id, "reason":"合成：测试"})).await.unwrap();
    for _ in 0..300 {
        if !c.call("session.live", json!({"sessionId": w1_id})).await.unwrap()["running"].as_bool().unwrap() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    c.call("message.post", json!({"channelId": lobby, "text":"合成任务丁", "mentions":[{"kind":"session","targetId": w1_id}]}))
        .await
        .unwrap();
    wait_for("被停的 worker 被拉起并收到唤醒", || log_count(&log, "合成任务丁") == 1).await;
    let wakes = c.call("wake.list", json!({"sessionId": w1_id})).await.unwrap();
    assert!(wakes.as_array().unwrap().iter().all(|w| w["state"] != "pending"), "{wakes}");

    // 同一个 session 并发 resume：只有一个进程。
    c.call("session.stop", json!({"sessionId": cap_id, "reason":"合成：测试"})).await.unwrap();
    for _ in 0..300 {
        if !c.call("session.live", json!({"sessionId": cap_id})).await.unwrap()["running"].as_bool().unwrap() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut c2 = Client::connect(&data.join("crewd.sock")).await;
    let (a, b) = tokio::join!(
        c.call("session.resume", json!({"sessionId": cap_id})),
        c2.call("session.resume", json!({"sessionId": cap_id}))
    );
    assert_eq!([a.is_ok(), b.is_ok()].iter().filter(|x| **x).count(), 1, "{a:?} {b:?}");
    assert!(a == Err(-32009) || b == Err(-32009));

    // 停机：会话全收掉，登记表清空。
    stop_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(15), server).await.unwrap().unwrap().unwrap();
    let reg = std::fs::read_to_string(data.join("exec-registry.json")).unwrap_or_default();
    assert!(!reg.contains(&cap_id) && !reg.contains(&w1_id), "停机后登记表里不该还有会话：{reg}");
}

#[tokio::test(flavor = "multi_thread")]
async fn isolated_start_creates_a_worktree_in_the_crew_repo_and_a_failed_one_leaves_nothing() {
    // 合成仓库（临时目录里 git init），绝不碰真仓库。
    let dir = tempfile::tempdir().unwrap();
    let (data, log) = (dir.path().join("data"), dir.path().join("submitted.log"));
    let repo = dir.path().join("repo");
    let plain = dir.path().join("plain");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&plain).unwrap();
    let git = |args: &[&str]| {
        let o = std::process::Command::new("git").args(args).current_dir(&repo).output().unwrap();
        assert!(o.status.success(), "git {args:?}：{}", String::from_utf8_lossy(&o.stderr));
    };
    git(&["init", "-b", "main"]);
    git(&["-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "--allow-empty", "-m", "init"]);
    let mut env = std::collections::BTreeMap::new();
    env.insert("FAKE_CLAUDE_LOG".to_string(), log.display().to_string());
    let opts = crewd_server::ServeOptions {
        claude: crewd_server::orchestra::exec_claude::ClaudeProgram::Path(fake_claude()),
        claude_env: env,
        ..Default::default()
    };
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(crewd_server::serve_with(data.clone(), opts, async {
        let _ = stop_rx.await;
    }));
    let mut c = Client::connect(&data.join("crewd.sock")).await;

    let crew = c.call("crew.create", json!({"title":"合成机组甲","workingDirectory": repo})).await.unwrap();
    let crew_id = crew["id"].as_str().unwrap().to_string();
    let shared = c
        .call("session.start", json!({"crewId": crew_id, "role":"worker", "executor":"claude", "brief":"合成共享任务", "isolation": false}))
        .await
        .unwrap();
    assert!(shared.get("worktree").map_or(true, |w| w.is_null()), "isolation=false 不记 worktree：{shared}");
    let iso = c
        .call("session.start", json!({"crewId": crew_id, "role":"worker", "executor":"claude", "brief":"merge-task 合成独立任务", "isolation": true}))
        .await
        .unwrap();
    let wt = std::path::PathBuf::from(iso["worktree"].as_str().expect("isolation=true 要记下 worktree 路径"));
    assert!(wt.is_dir() && wt.to_string_lossy().contains("/.pendingcrew/worktrees/merge-task-"), "{wt:?}");
    wait_for("独立 session 提交了开场任务", || log_count(&log, "合成独立任务") == 1).await;
    // 缺 isolation = 参数错（C8：必填，不替机长默认）。
    assert_eq!(
        c.call("session.start", json!({"crewId": crew_id, "role":"worker", "executor":"claude", "brief":"合成缺参"})).await,
        Err(-32602)
    );
    // 机组目录不是 git 仓库：isolation=true 起不来，不写行、不起进程、不退回共享目录。
    let crew2 = c.call("crew.create", json!({"title":"合成机组乙","workingDirectory": plain})).await.unwrap();
    let crew2_id = crew2["id"].as_str().unwrap().to_string();
    let before = c.call("session.list", json!({"crewId": crew2_id})).await.unwrap().as_array().unwrap().len();
    assert!(c
        .call("session.start", json!({"crewId": crew2_id, "role":"worker", "executor":"claude", "brief":"合成失败任务", "isolation": true}))
        .await
        .is_err());
    assert_eq!(c.call("session.list", json!({"crewId": crew2_id})).await.unwrap().as_array().unwrap().len(), before, "失败不留 session 行");
    assert_eq!(log_count(&log, "合成失败任务"), 0, "没有任何进程被起来");
    assert!(!plain.join(".pendingcrew").exists());

    stop_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(15), server).await.unwrap().unwrap().unwrap();
}
```

（`crew.create` / `message.post` 的参数名以 M3a 的契约为准，Task 10 Step 1 已核对。）

- [ ] **Step 5: kill -9 重启端到端（`crates/crewd/tests/orchestra_restart.rs`）**

```bash
cargo add --offline -p crewd --dev --path crates/collab crewd-collab
cargo add --offline -p crewd --dev --path crates/wake crewd-wake
```

```rust
//! kill -9 serve 之后再起：上次在跑的会话一个都不自动起、先问人（§22.7 Q1）；答「恢复」后错开接回，被打断的那个收到
//! 「被打断」附言；崩溃期间挂上的定时唤醒恰好投一次；上一个 serve 留下的假 claude 孤儿被回收。
//! 只按自己 spawn 的 pid 发信号，绝不按名字杀进程。
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn fake_claude() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let fake = exe.parent().and_then(Path::parent).unwrap().join("fake_claude");
    assert!(fake.exists(), "找不到 {}：先跑 `cargo build -p crewd-exec --bin fake_claude`", fake.display());
    fake
}

struct Serve(Child);

impl Serve {
    fn start(data: &Path, log: &Path) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_crewd"))
            .env("CREWD_DATA_DIR", data)
            .env("CREWD_CLAUDE", fake_claude())
            .env("SHELL", "/usr/bin/false")
            .args(["serve", "--claude-env"])
            .arg(format!("FAKE_CLAUDE_LOG={}", log.display()))
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let s = Serve(child);
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) {
            if rpc(data, "system.hello", "null").is_ok() {
                return s;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("serve 没起来");
    }

    fn term(&mut self) {
        let _ = Command::new("kill").args(["-TERM", &self.0.id().to_string()]).status();
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(15) {
            if self.0.try_wait().unwrap().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            self.term();
        }
    }
}

fn rpc(data: &Path, method: &str, params: &str) -> Result<serde_json::Value, String> {
    let out = Command::new(env!("CARGO_BIN_EXE_crewd"))
        .env("CREWD_DATA_DIR", data)
        .args(["rpc", method, params])
        .output()
        .unwrap();
    if out.status.success() {
        Ok(serde_json::from_slice(&out.stdout).unwrap())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(20) {
        if ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("20s 内没等到：{what}");
}

fn count(log: &Path, needle: &str) -> usize {
    std::fs::read_to_string(log).unwrap_or_default().matches(needle).count()
}

#[test]
fn kill_dash_nine_then_restart_asks_first_and_delivers_each_wake_once() {
    let dir = tempfile::tempdir().unwrap();
    let (data, work, log) = (dir.path().join("data"), dir.path().join("work"), dir.path().join("submitted.log"));
    std::fs::create_dir_all(&work).unwrap();
    let mut first = Serve::start(&data, &log);
    let crew = rpc(&data, "crew.create", &format!(r#"{{"title":"合成机组","workingDirectory":"{}"}}"#, work.display())).unwrap();
    let crew_id = crew["id"].as_str().unwrap().to_string();
    let start = |role: &str, brief: &str| {
        rpc(&data, "session.start", &format!(r#"{{"crewId":"{crew_id}","role":"{role}","executor":"claude","brief":"{brief}","isolation":false}}"#))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let cap = start("captain", "合成开场甲");
    let w1 = start("worker", "合成开场乙");
    wait_for("开场任务都提交了", || count(&log, "合成开场甲") == 1 && count(&log, "合成开场乙") == 1);

    first.0.kill().unwrap(); // SIGKILL：按我们自己 spawn 的 pid
    first.0.wait().unwrap();

    // 崩着的时候：机长停在干活中途（was_working=1），worker 名下挂一条 3 秒后到点的定时唤醒。
    {
        let mut store = crewd_store::Store::open(&data.join("crewd.db")).unwrap();
        let now = crewd_store::now_ms();
        let (cap2, w12) = (cap.clone(), w1.clone());
        crewd_collab::run(&mut store, now, move |tx| {
            crewd_collab::sessions::set_working(tx, &cap2, true)?;
            crewd_collab::wakes::enqueue(
                tx,
                crewd_collab::wakes::WakeRequest {
                    session: w12.clone(),
                    kind: crewd_collab::wakes::WakeKind::Timer,
                    source: crewd_collab::wakes::source::timer("restart-test"),
                    priority: crewd_collab::wakes::WakePriority::Auto,
                    not_before_ms: Some(now + 3_000),
                    note: Some("合成定时戊".into()),
                    origin: Some(w12),
                },
            )?;
            Ok(())
        })
        .unwrap();
    }

    let mut second = Serve::start(&data, &log);
    let prompt = rpc(&data, "restart.pending", "null").unwrap();
    let ids: Vec<&str> = prompt["sessionIds"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(ids.contains(&cap.as_str()) && ids.contains(&w1.as_str()), "{prompt}");
    std::thread::sleep(Duration::from_secs(4));
    for s in [&cap, &w1] {
        let live = rpc(&data, "session.live", &format!(r#"{{"sessionId":"{s}"}}"#)).unwrap();
        assert_eq!(live["running"], false, "没答之前一个都不起：{live}");
    }
    assert_eq!(count(&log, "合成定时戊"), 0, "待恢复的 session 不被唤醒拉起");

    rpc(&data, "restart.answer", &format!(r#"{{"id":{},"answer":"resume"}}"#, prompt["id"])).unwrap();
    let interrupted = crewd_wake::text::get(crewd_wake::text::Lang::Zh, "restart.interrupted");
    wait_for("机长收到被打断附言、worker 收到定时唤醒", || {
        count(&log, interrupted) == 1 && count(&log, "合成定时戊") == 1
    });
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(count(&log, interrupted), 1, "恰好一次");
    assert_eq!(count(&log, "合成定时戊"), 1, "恰好一次");

    second.term();
    let reg = std::fs::read_to_string(data.join("exec-registry.json")).unwrap_or_default();
    assert!(!reg.contains(&cap) && !reg.contains(&w1), "上一个 serve 的孤儿已回收、这一个的已停：{reg}");
}
```

（`restart.pending` 的结果字段名 `id` / `sessionIds` 以 M3a 的 `RestartPrompt` 为准；`count(&log, interrupted)`：假 claude 把正文里的换行记成 `\n`，附言本身没有换行。）

Run: `cargo build -p crewd-exec --bin fake_claude && cargo test -p crewd-server --test orchestra_fake_claude && cargo test -p crewd --test orchestra_restart && cargo test -p crewd --test e2e`
Expected: 全绿。各连跑 3 遍（`for i in 1 2 3; do cargo test -p crewd --test orchestra_restart || break; done`），不许偶发红。

- [ ] **Step 6: 门禁与提交**

Run: `./scripts/check.sh`
Expected: 全绿。

```bash
git add crates/server/src/lib.rs crates/crewd/src/main.rs crates/crewd/tests/e2e.rs crates/crewd/Cargo.toml Cargo.lock crates/server/tests/orchestra_fake_claude.rs crates/crewd/tests/orchestra_restart.rs
git diff --cached --stat
git commit -m "crewd: serve runs the orchestra; serve --claude; fake-claude end-to-end incl. kill -9 restart

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 12: 交付记录

**Files:**
- Modify: `crewd/README.md`（一段）、`docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`（在 §22 之后追加「M3c 交付记录与遗留」一节；不改 §22 正文）

- [ ] **Step 1: README**

`crewd/README.md` 加一段：`crewd serve` 起编排（会话管理 + 唤醒）；`--claude PATH` / `CREWD_CLAUDE`；重启后先问人（`restart.pending` / `restart.answer`）；环路熔断只能人解（`admission.recover`）；`orchestra.settings.*` 的可选并发上限缺省不设。

- [ ] **Step 2: spec 交付记录**

照 §18 / §19 / §20 的写法：已交付（按 crate / 模块一句）、与 spec / M3a 计划不同的决定（本计划「裁定」逐条一句）、给 M3b / M4b / M5b 的口（产出信号 = M3a `sessions::note_progress`，M4b 的工作项命令必须调；`IdleHook`、`AskAnsweredHook`、`Arbiter`、`ContextExtras`、`QuotaSource`）、遗留（本计划文末「spec 问题」里仍开着的 2 / 15、`set_profile` 只落库、worktree 只建不删（C8，删除归人）、非消息类唤醒至少一次）。读数写实测到的（测试条数、`check.sh` 连跑次数），推出来的标 [推]。

- [ ] **Step 3: 提交**

```bash
git add README.md ../docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md
git diff --cached --stat
git commit -m "docs: M3c delivery record in spec, README paragraph

Co-Authored-By: Claude <noreply@anthropic.com>"
```

（路径相对 `crewd/`；spec 在仓库根的 `docs/` 下。）

---

## spec 问题（写计划时发现的矛盾与缺口；2026-10-06 三份 M3 计划对齐时已逐条处理，状态写在每条末尾）

1. **§22.4 正文与 §22.7 拍板冲突**（准入窗口、max_live 默认 10、重启自动接回、`wake_latches` 的 scope）。→ **已解决**：spec §22.2 / §22.4 已按 §22.7 改写；本计划裁定 2、4、5 不变。
2. **§22.4「额度用尽：按 health 给的重置时刻入队」**：M2b 的 `Health` 没有重置时刻，`SessionControl::quota` 也没有数据来源。→ **仍开着（工程遗留）**：留 `QuotaSource` 口、缺省 45 分钟退避并在群里说明（裁定 13），`quota()` 如实返回空。
3. **「没有新产出」与 `last_output_at` 分辨不了**（发言也写它）。→ **已解决（控制者裁定 C3）**：M3a 另给 `sessions::note_progress` → `last_progress_at` + `Change::Progress`，发言不写；M3b 的 post-tool-use 对非 crewd 工具调；**M4b 的工作项命令必须调**。M3c 订阅 `Change::Progress`，`last_progress_at` 只作 `Lagged` 时的兜底；「回叫不算产出」（裁定 3 ③）保留。
4. **M3c 的文件归属不够用**。→ **已解决**：spec §22.5 的文件归属表已列；`AppState.orchestra` 与 `wake` 主题由 M3a 预先建好，M3c 只改 `dispatch.rs` 一行、`proto/src/lib.rs` 两行、`crates/crewd/src/main.rs` 的 serve 参数、`tests/e2e.rs`，与 M3b 共享的文件按「后合者重新生成、保留两边」。
5. **M3a 冻结接口缺的读 / 写**（晚于 now 的最早 not_before、游标值、`mark_failed` 不改 not_before、没有按 id 原样退回、`members` 是否含隐式成员、`note_output` 不发变更）。→ **已解决（控制者裁定 C6）**：M3a 加了 `wakes::{next_due_after, requeue, by_ids, prune}`、`mark_failed(.., retry_at_ms)`、`cursors::cursor`，冻结 `channels::members` = 有效成员（含隐式），产出走 `Change::Progress`。`db.rs` 不再有 SQL。
6. **问被回答的分工**（§22.4 / §22.5 / §22.6 互相矛盾）与 ask 唤醒的 source。→ **已解决**：spec 已统一为 M3a 的 `asks::answer` 入队、`Change::AskAnswered` 带 `cleared`、source = `ask:<crew>/H<n>`（裁定 15）。
7. **「对同一 session 并发 `session.start`」**只能是 `resume`。→ 照这个解释写测试（不变）。
8. **「kill -9 后各恰好投一次」**：非消息类只能至少一次。→ 照裁定 17（不变），验收只对崩溃前 pending 的定时唤醒断言恰好一次。
9. **待恢复名单与「@ 一个已停的 session 会拉起它」的交界**。→ **已解决（控制者裁定 C4）**：询问开出之后人 @ 名单里的某一个 = 只恢复它（算单个 resume），其余照旧等；启动前就排着的人类唤醒不算。
10. **人类唤醒与可选并发上限**。→ **已解决（控制者裁定 C5）**：人类唤醒绕过可选上限。
11. **`set_profile` 与 `session.start.worktree` 的归属**。→ **已解决（控制者裁定 C5、C8）**：`set_profile` 只落库、下次启动生效；**已被控制者裁定 C8 改写**：不再是「已存在目录」；`StartRequest` / `session.start` 带必填 `isolation: bool`，`true` 时会话管理器照 Swift 新建 git worktree（裁定 20）；M3b 的 `start_session` 保留 `isolation`。
12. **消息唤醒推进游标的前提**：Delivered 后把游标推到批里最大的 seq，前提是 `render::wake_text` 把该群这一 seq 之前的未读都带进了正文。→ **已解决**：M3a 的 `build_batch` 改为每个群带上「游标之后、到批里最大 seq 为止」它看得见的全部消息（M3a Task 11 测试钉住），所以投递成功后推游标不会越过没告知的消息。
13. **M2c 的事件顺序**。→ 不变（裁定 12）。
14. **`delivered` 行不清理**。→ **已解决（控制者裁定 C6）**：M3a `wakes::prune`；M3c 的定时拍每小时清一次 7 天前已结的行。
15. **默认方案到点的精度**：只能按心跳（30s）扫全部机组。→ **仍开着（可接受）**：精度 = 心跳；量大了再让 M3a 给「最早的默认方案到点时刻」。

## 未验证（写计划时没做过的）

- 本计划的代码一行都没编译过（M3a 未落地）；Task 1–5 只依赖 std，最可能一次编过；Task 6–11 的 `crewd-exec` 用法按 `crewd-m2b@b05a57e` 的实际代码写，M3a 的用法按其计划文本写。
- `fake_claude` 从 `current_exe()` 的上上级目录找：只在 `cargo test --workspace`（check.sh）下保证已建；单独 `cargo test -p crewd-server` 时可能没建或是旧的（测试会失败并提示怎么建）。
- RPITIT（trait 里 `-> impl Future + Send`）配 `async fn` 实现、`Port` 做泛型参数：没在本仓库的工具链上试过。
- `start_paused` 测试与 `Rig::settle` 的「连续 50 次让步」在 current_thread 运行时下的确定性是推的，没跑过。
- 积压补救在 1000+ session 的真实库上要多久没量（每个 session 一次 `unread_for` 事务）。
- 真 claude 一次没跑（M3 集成验收时，人点头后跑）。
