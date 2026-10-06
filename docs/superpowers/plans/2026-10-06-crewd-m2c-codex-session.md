# crewd M2c codex app-server 会话驱动 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 `crewd-exec` 里加 codex 执行器：起 `codex app-server`（stdio JSON-RPC），握手、开 / 接 thread（thread id 先落账）、开回合（白板走 `additionalContext` 不可信通道）、打断、换档位、压缩，**每一条服务端请求都回答**，健康只认协议字段，每个请求都有期限；进程自成会话、停止与退出都收掉整个进程组。对外用 **M2b 的同一个窄接口**（`driver::SessionHandle` + `session::SessionEvent`），只加几个两边都讲得通的最小口子。配一个假 app-server 测试器材做端到端、一条默认不跑的真机冒烟，和 `crewd codex` 子命令。

**Architecture:** 照 M2b 的三层。**纯计算层**：`codex::rpc`（线协议编解码、我方请求表与期限）、`codex::proto`（参数构造、服务端请求答复表、协议健康判定、压缩阈值）、`codex::turns`（回合跟踪、结构化记录）、`codex::session::CodexCore`（整个会话的 sans-IO 判断：喂一行 stdout / 定时拍 / 命令 / 账本结果 / 退出 → 吐出「写哪一行」「落账」「控制结果」「事件」，时间由调用方传入）。**进程层**：`codex::transport`（setsid + 白名单环境 + 剥计费 key 起进程、stdout 有上限按行读、stderr 一直排空、写 stdin 走写任务）。**驱动层**：`codex::driver::start_codex`（一个 tokio 任务拥有子进程与 `CodexCore`，命令走 M2b 的 `driver::Command`，事件走 M2b 的 `SessionEvent`）。claude 驱动只改窄接口那几处（Task 1）。

**Tech Stack:** Rust 1.94（edition 2024）、tokio 1.53（rt / macros / time / sync / process / io-util）、serde_json、rustix（setsid、killpg）、libc（测试器材与测试里的 `kill(pid, 0)`）——**全部是 crewd-exec 已有的依赖，M2c 不新增任何 crate，Cargo.toml / Cargo.lock 一行不动**。

**Spec:** `docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`（§13 Q12 窄接口、§16 执行器词表 `codex` 与「屏幕文本由结构化 transcript 代替」）；移植图 `docs/superpowers/specs/2026-10-05-crewd-m2-executor-porting-map.md`（§4 codex 全部、§5 进程生命周期、§6 状态来源）；M2b 计划 `docs/superpowers/plans/2026-10-06-crewd-m2b-claude-session.md`（窄接口、`env`、`registry`、`resume`）。Swift 对照：`Sources/Mac/LocalRunner/CodexAppServer/{CodexAppServerBackend,CodexAppServerConnection,CodexProtocol,CodexRPCDispatcher,CodexRPCMessage,CodexTranscript}.swift` 与 `Tests/PendingCrewTests/{CodexProtocolTests,CodexRPCMessageTests,CodexRPCDispatcherTests,CodexTranscriptTests}.swift`。协议核对：本机 `codex-cli 0.160.1` 的 `codex app-server generate-json-schema --experimental`（2026-10-06 生成，只读）。

> 写计划时已把本计划的全部代码在一个副本里按任务顺序重放：底座是 `crewd-m2b@1e35379`——M2b Task 1–10 都是**实际合入**的代码（delivery 的非终局 `Stalled`、`ResumeDecision`、带号校验的账本、`ApplyOutcome` / boot id 的登记表、`StartError::AlreadyRunning` 与 `LiveId`、`finish` 先收拾后发 `Exited`）；缺的 M2b Task 12（`crewd_server::reap_orphans`、`crewd claude`）按 M2b 计划原文补上。重放结果：每个任务结束 `fmt --check` + `clippy -D warnings` + 全量测试全绿；crewd-exec 单测**增量**依次 +1 / +8 / +15 / +10 / +24 / +9（Task 1–6，副本里 122 → 189），集成测试 `driver` +1（12 → 13），`fake_codex` 19、`live_codex` 2 ignored，crewd-server 单测 +1，`codex_cli` 2。`fake_codex` 与 `driver` 连跑 5 遍无抖动（`fake_codex` 每遍约 2.3s）。各变异步骤都真红过（红哪几条写在步骤里）。**数字以增量为准**：M2b 合入 main 时若又改了，绝对数会不同，执行时先量自己的基线（Task 1 Step 1）。**没做过的三件事**：没在 Linux 上编译；没跑过真 codex（Task 9 Step 3–4 要人点头）；官方网页文档（learn.chatgpt.com/docs/app-server）没取——协议以本机 0.160.1 生成的 schema 为准，schema 管不到的列在文末「未核实」表。


## Global Constraints

- 所有命令在 `crewd/` 目录下跑；Rust 工具链在 `/opt/homebrew/opt/rustup/bin`（找不到 `cargo` 先 `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`）。
- 在分支 `crewd-m2c` 上做：**M2b 合入 main 之后**从 main 切出（执行时用 worktree）。M2c 依赖的是 M2b **合入后的实际代码**，不是 M2b 计划原文；两者不同的地方按 Task 1 Step 1 的对齐检查处理。
- **不新增依赖**：不跑任何 `cargo add`；`Cargo.toml`、`Cargo.lock` 不出现在任何一次提交里。
- 依赖方向不变：`crewd-exec` 不依赖 store / server / proto（账本仍是 `resume::AgentIdLedger` trait）。
- 窄接口仍是 M2b 的 `driver::{SessionHandle, SessionStatus, StartError, Closed}` + `session::SessionEvent`；codex 只多一个启动入口 `codex::driver::{start_codex, CodexStart, CodexSpec, CodexConfig}`。外部（server / M3）不碰 `codex::{rpc, proto, turns, session, transport}`。公开类型里不出现 tokio 子进程类型以外的实现细节。
- 纯计算层（`codex::rpc`、`codex::proto`、`codex::turns`、`codex::session`）不起进程、不碰 tokio、不调 `Instant::now()` / `SystemTime::now()`——时间由调用方传入（`currentTime/read` 用的 Unix 秒由驱动层在 `Opening::unix_now` 里给）。
- **首条 prompt 与世界观绝不进 argv**：argv 只有 `app-server`；开场任务走 `turn/start`、世界观走 `thread/start.developerInstructions`，都经 stdin（`ps` 看不见）。
- 子进程环境 = M2b 的白名单（`env::build_env_from_process`）**再剥掉** `OPENAI_API_KEY` / `CODEX_API_KEY` / `CODEX_ACCESS_TOKEN`（带着它们 codex 改走按量计费）。
- **每一条服务端请求都必须回答**（答复表只在 `proto::answer_server_request` 一处）；**我们发出的每个请求都有期限**（`CodexCoreConfig`）。
- 测试**只用合成数据** + 假 app-server（`src/bin/fake_codex.rs`）+ shell 脚本；不读真实 `~/.codex`、不碰真实数据目录；会起登录 shell 的测试把 `SHELL` 设成 `/usr/bin/false`。
- 真 codex 只出现在 `tests/live_codex.rs`：`#[ignore]` + 环境变量 `CREWD_LIVE_CODEX` 两道闸；`check.sh` 不带 `--ignored`，**永远不跑它**；`=turn` 那条花额度，**跑之前要人点头**。
- codex 子进程由我们 setsid、自成进程组；假 app-server 起的 `sleep` 在它的组里。killpg 永远落不到跑 `cargo test` 的那一组。
- 本机 shell 是 zsh，不做词分割：手动跑变异步骤时照抄给出的命令，别把 `--test fake_codex` 这类参数塞进一个变量再展开。
- 每个任务结束跑 `./scripts/check.sh`（fmt + clippy -D warnings + test）全绿再提交。提交只 `git add` 该任务列出的文件（路径相对 `crewd/`），提交信息末尾加 `Co-Authored-By: Claude <noreply@anthropic.com>`（不带模型名）。变异步骤一律在提交**之后**做，改回用 `git checkout -- <那个文件>`（已提交，不会丢东西），做完 `git status` 必须干净。

## 范围

做：窄接口的最小扩展（`send_with_context`、`interrupt`、`control`、`snapshot` 可空、四种新事件、两种新 health、`HealthKind::rank` 共用、`StartError::Handshake`、`--agent codex` 的 MCP 命令行）；stdio JSON-RPC 编解码（无 `jsonrpc` 字段、整数与字符串 id、两套 id 分表、配不上的回应丢弃计数）；单请求期限（Swift 没有）；握手（`initialize` → `initialized` → `thread/resume{excludeTurns}` 失败退回 `thread/start`）；`thread/start` 参数（`cwd`、`approvalPolicy: on-request`、`sandbox: workspace-write`、`approvalsReviewer: auto_review`、`developerInstructions`、`model`、`serviceTier`、`config.model_reasoning_effort`、`config.mcp_servers.crew` 指向 helper）；thread id 落账在先、`Spawned` 在后；`turn/start`（单个 text 输入 + `additionalContext` 不可信白板）；排队；`turn/interrupt`；`thread/settings/update`（fast 等回显）；`thread/compact/start` 手动与自动（阈值照 Swift）；全部 11 种服务端请求的答复；回合跟踪、干活 / 空闲边沿、`TurnEnded`；协议健康（`account/rateLimits/updated`、`usageLimitExceeded`、`unauthorized`、版本太旧、失败回合、不再重试的错误）及其清除；进程（setsid、白名单 + 剥计费 key、stderr 排空与尾巴、stdout 行上限、写任务、停止 SIGTERM → 宽限 → killpg、退出后整组补刀、孤儿登记）；thread id 账本（含账里非法号）；假 app-server 器材 + 端到端；真机冒烟（opt-in）；`crewd codex` 子命令 + 账本按执行器把关。

不做：attach 协议与结构化历史回放（M2d——`SessionEvent::Item` 只负责把完成的条目发出来）；白板「未读」的取舍与去重（M3 决定每回合传什么；codex 自己按键去重）；`turn/steer`（运行中插话的语义归 M3，M2c 排队）；`model/list` / ephemeral thread 探测原生默认模型（Swift `resolveNativeDefaultModel`，唯一调用方是 M3 的显示）；推理增量的累积与 Swift transcript 的「输入回显行」（都是 UI，归 M2d / 客户端）；群通知（`Notice` 由 M3 转群）；codex 的 npm 托管回退（`AgentPaths` 只按 PATH 找）；awaitingReply（M3，挂在 `TurnEnded` 上）；serve 里的会话管理（M3）。

## 写计划时定下的裁定（执行时照做，有异议先回报）

1. **同一个窄接口，只加两边都讲得通的最小口子**（M3 不该为执行器写两套逻辑）：
   - `SessionHandle` 不改成 trait：句柄本来就只是「命令通道 + 投递编号」，两个驱动各自解释同一套 `Command`。为此 `Command` 改 `pub(crate)`、加 `SessionHandle::new`，`kill_group` / `terminate` / `decode` 改 `pub(crate)` 给 codex 驱动共用。
   - `send_with_context(text, UntrustedContext)`：白板是 codex 的一等参数（`additionalContext`，`kind: "untrusted"`）；claude 一侧由驱动包成 `<external_键>…</external_键>` 块放在正文前（与 codex 服务端渲染的形状一致，见「未核实」）。`send(text)` = 不带上下文。
   - `interrupt()`：codex 没有终端，Esc 字节没意义；claude = 写一个 Esc，codex = `turn/interrupt`。`raw()` 在 codex 上只认单独一个 Esc（= 打断），其余丢弃并发 `Notice`——不静默吞。
   - `control(Control) -> ControlOutcome`：换模型 / 推理强度 / fast、压缩。claude 回 `Unsupported`（它的 `/model`、`/compact` 是屏幕驱动，归 M3），不假装做了。
   - `snapshot()` 改返回 `Option<Snapshot>`：codex 没有终端，`None` 比「空屏快照」或「Closed」诚实。M2b 里没有调用方（M2b 计划的测试与 `crewd claude` 都没调）。`screen_text()` 在 codex 上返回结构化记录的文本渲染（spec §16「屏幕文本由结构化 transcript 代替」）。
   - `SessionEvent` 加四种：`Item`（完成的结构化条目原样，M2d 回放用）、`TurnEnded`（Stop hook 的 codex 对应物）、`Notice`（执行器替 agent 做了人该知道的决定）、`Profile`（服务端确认的档位）。claude 驱动不发它们。
   - `HealthKind` 加 `TurnFailed`、`VersionIncompatible`（Swift 的 `turnFailed` / `cliVersionIncompatible`）。M2b 合入的 claude 核心里有一张私有的档位表（`health_rank`：低档不盖高档），挪到 `health.rs` 成 `HealthKind::rank`，两个核心共用一张表。
   - `StartError` 加 `Handshake { health }`：进程起来了但握手 / 开 thread 没成（已杀进程）。
   - `HelperCommands::mcp_args_for(agent)`：codex 的 MCP 命令行带 `--agent codex`；`mcp_args()` 不变（= `mcp_args_for("claude")`）。
2. **`start_codex` 等握手与落账都完成才返回**。thread id 由服务端生成，`Spawned` 里的会话号必须是真的；第一条事件仍是 `Spawned`（就绪之前产生的事件先攒着）。**落账失败 = 启动失败**（杀进程、报 `StartError::Ledger`、一个回合都不开）——这是 M2b「账本写不进去就不起进程」在 codex 上能做到的最强形式：id 要起了进程才有。
3. **续跑被拒**：同一个进程里退回 `thread/start`，新 id 先落账；事件顺序是 **`Spawned`（新 id，`resumed: false`）→ `ResumeRefused`（旧 id + 服务端原话）**，与 claude 的「`ResumeRefused` → 再一次 `Spawned`」顺序不同；统一约定「当前会话号永远以最近一条 `Spawned` 为准」，写进 `SessionEvent` 文档。除了「进程没了」，任何 resume 失败（含超时）都退回。账里的号 `valid_agent_session_id` 不认 = 当作没记过。续上的就是账里那条时不再写账。
4. **「早到响应缓冲」在结构上不存在**：`Rpc::request` 先登记、后交出要写的那一行，同一个任务按顺序处理每一行输入，所以「回应比登记早」不会发生；配不上的回应只剩「超时之后迟到」与「不是我们的 id」，丢弃并计数。**服务端请求 id 支持字符串**（schema 的 `RequestId` 是 `string | int64`）——Swift 只认整数，字符串 id 的服务端请求会被分类成通知、永远不回，回合挂死。
5. **单请求期限**（Swift 没有）：`initialize` 30s、`thread/start|resume` 60s、其余 30s。到期 = 失败：握手中 → `Stalled` 启动失败；`turn/start` → `Undelivered` + 对应 health；设置 / 压缩 → `Rejected`；迟到的回应丢弃。（Swift 的 25s 启动探针被 6MB 的 resume 回包打穿过；有了 `excludeTurns`，60s 足够——推的，没实测。）
6. **服务端请求答复表**（0.160.1 `ServerRequest.json` 的 11 个方法全覆盖）：审批（`*/requestApproval`，加上旧式 `applyPatchApproval` / `execCommandApproval`——Swift 的 `hasSuffix("requestApproval")` 漏了这两个，落到「不支持」也回了错误，效果一样，这里显式归类）→ `-32000`；`mcpServer/elicitation/request` 带 `_meta.codex_approval_kind == "mcp_tool_call"` → `accept`，其余 → `decline` + `Notice`；`account/*` → `-32601` + `AuthRequired`；`currentTime/read`（0.160 新增）→ 照实回 Unix 秒；其余 → `-32601` + `Notice`。同一种方法的 `Notice` 只发一次。
7. **回合排队，不用 `turn/steer`**：有回合在跑、正在压缩、或上一个 `turn/start` 还没回，正文就排队，一条一个回合；连续的排队回合之间不报空闲边沿（M3 的「停下即检查」只在真停下时触发）。
8. **`TurnEnded` 只在回合 `completed` 且不是压缩回合时发，附本回合最后一条 agent 正文**（没有就是空串）。Swift 取的是整个 transcript 的最后一条——一个没说话的回合会把上一回合的话再报一遍（重复发群）。
9. **健康只认协议字段**；之后一个正常完成的回合清掉「额度 / 回合失败 / 版本」三类（第一手证据：它又能干活了）；低档不盖高档（同 claude，`HealthKind::rank`）。
10. **进程**：setsid（新会话、没有控制终端——codex 跑的工具命令碰 `/dev/tty` 立刻失败，而不是被 SIGTTIN 停住、把回合挂死；killpg 能连工具子命令一起收掉）。停止 = SIGTERM 给 codex → 宽限 2s → `killpg(SIGKILL)`；**codex 退出（任何原因）后总对整组补一刀 SIGKILL**。移植图 §5 的「codex 是否独立进程组未核实」在这里定掉：Swift 的 Foundation `Process` 不是，我们是。
11. **环境**：剥 `OPENAI_API_KEY`（Swift 同）+ `CODEX_API_KEY` + `CODEX_ACCESS_TOKEN`（0.160.1 二进制里与 `OPENAI_API_KEY` 同组的认证环境变量：`OPENAI_API_KEYCODEX_API_KEYCODEX_ACCESS_TOKENno Codex credentials were found`）。
12. **stderr 一直读**，只留最后 4KB 给握手失败的说明；**stdout 一行上限 64MiB**，超了整行丢弃、每次发 `Notice`（丢掉的可能正是某个请求的回应，那个请求随后按超时处理）。
13. **`serviceTier` 不选就不传**（Swift 总传 `default`，会盖掉用户 codex 配置里的档位）——人的原话「codex 本身的默认模型是什么逻辑就用什么逻辑」；实际档位从回包读、发 `Profile`。
14. **不订阅三种增量**：`item/agentMessage/delta`（Swift 同）+ `item/commandExecution/outputDelta` + `item/fileChange/outputDelta`（量大，`item/completed` 的条目里有全文）；推理增量留着（M2d 可能要）。
15. `initialized` 按 schema **不带 `params`**（Swift 带 `{}`）。
16. **fast 切换以回显为准**：`thread/settings/update` 的 `{}` 回包只是「收到」，以随后的 `thread/settings/updated` 为准；8s 没回显或回显了认不出的档位 = `NoConfirmation`（不冒充「标准」）；同一时刻只等一次。顺序处理让「回包之前到的回显」天然不算（Swift 要靠序号比较）。换模型 / effort 在回包时就算 `Applied`。
17. **自动压缩阈值照 Swift**：上下文 ≥70%，或 ≥45% 且本回合消耗 ≥ 窗口×2；距上次压缩不足 2 个回合不压；token 读数可能比 `turn/completed` 晚到，晚到时再判；压缩中排队，压完放行。
18. **`crewd codex --session` 只认执行器是 `codex` 的 session**（`StoreLedger::expecting("codex")`，不对就不读也不写）：拿 claude 的会话号去 `thread/resume` 必然失败，退回新 thread 时会把 claude 的号覆盖掉。
19. 正文不过 `sanitize_text`：codex 的正文是 JSON 字符串里的数据，控制字符没有按键语义（claude 才有）。
20. **同一个 thread 在本进程里只能跑一个**，与 claude 共用 M2b 的 `LiveId` 表（`LIVE_AGENT_IDS`）：账里记的号在起进程**之前**占住（撞了 = `StartError::AlreadyRunning`，不起进程），新开 thread 的号在 `Spawned` 时占住；`AlreadyRunning` 的说明去掉「claude」二字。
21. **收尾顺序照 M2b 的 `finish`**：进程收尸后先停附属任务、放开 thread id、清登记表，再把 `Exited` 发出去——调用方见到 `Exited` 就能马上用同一个 thread id 重起。登记表同样只收 `pgid == pid` 的进程。

## Review Focus

1. 服务端请求带**字符串 id**、或是我们不认识的方法 → 必须回答，否则 codex 在那儿等、回合永远不结束（Swift「转圈不结束」的病根）。（Task 7 `every_server_request_is_answered_and_the_turn_finishes`；Task 2 `string_request_ids_are_classified_and_echoed_verbatim`；Task 5 `every_server_request_is_answered_and_each_kind_is_announced_once`）
2. codex 往 stderr 狂写（长会话的 tracing）→ 不能卡住（没人读，64KB 管道写满它就停在写上）。（Task 6 `a_flood_on_stderr_cannot_stall_stdout_and_its_tail_is_kept`；Task 7 `a_stderr_flood_does_not_stall_the_handshake`）
3. thread 开出来了但 thread id 落不了账（session 行被删 / 库坏）→ 一个回合都不开、进程杀掉、报 `StartError::Ledger`（起了也接不回来）。（Task 7 `ledger_failure_kills_the_process_before_any_turn`）
4. codex 起的工具子命令（构建、`sleep`、dev server）→ 停止时、以及 codex 自己崩掉时，都不能留成孤儿。（Task 7 `stop_kills_the_whole_group_including_tool_children`、`tool_children_do_not_outlive_codex_exiting_on_its_own`）
5. codex 不读 stdin 了（卡死 / 正忙），我们却要写一大段（几十 KB 的白板 + 任务）→ 会话不能被卡死：状态查得到、停得掉，那条正文按超时报没送到。（Task 7 `a_server_that_stops_reading_cannot_wedge_the_session`）

## 文件结构

```
crewd/crates/exec/src/driver.rs        Command pub(crate)、SessionHandle 新口子、Handshake、helpers pub(crate)   (T1)
crewd/crates/exec/src/session.rs       SessionEvent 四种新事件、Profile；档位表改用 HealthKind::rank            (T1)
crewd/crates/exec/src/health.rs        HealthKind::{TurnFailed, VersionIncompatible}、rank()                    (T1)
crewd/crates/exec/src/launch.rs        HelperCommands::mcp_args_for                                            (T1)
crewd/crates/exec/tests/driver.rs      claude 一侧：打断 / 控制 / 快照                                          (T1)
crewd/crates/exec/src/lib.rs           pub mod codex;                                                          (T2)
crewd/crates/exec/src/codex/mod.rs     模块声明（每个任务追加一行）                                             (T2–T7)
crewd/crates/exec/src/codex/rpc.rs     线协议：RequestId、parse_line、编码、Rpc<T>（pending + 期限）            (T2)
crewd/crates/exec/src/codex/proto.rs   参数构造、服务端请求答复表、协议健康、上下文读数、压缩阈值              (T3)
crewd/crates/exec/src/codex/turns.rs   TurnTracker、Transcript、render_item                                     (T4)
crewd/crates/exec/src/codex/session.rs CodexCore（sans-IO 会话核心）、CodexAction、launch_health              (T5)
crewd/crates/exec/src/codex/transport.rs  spawn_codex（setsid / 环境 / 三根管子）、LineReader、StderrTail   (T6)
crewd/crates/exec/src/codex/driver.rs  start_codex、CodexStart / CodexSpec / CodexConfig、驱动任务             (T7)
crewd/crates/exec/src/bin/fake_codex.rs   测试器材：假 codex app-server                                       (T7)
crewd/crates/exec/tests/fake_codex.rs  端到端（19 条，含 Review Focus 1–5）                                   (T7)
crewd/crates/store/src/sessions.rs     sessions::executor                                                       (T8)
crewd/crates/server/src/ledger.rs      StoreLedger::expecting                                                   (T8)
crewd/crates/crewd/src/codex_cmd.rs    `crewd codex` 子命令                                                     (T8)
crewd/crates/crewd/src/main.rs         接上子命令                                                               (T8)
crewd/crates/crewd/tests/codex_cli.rs  库里落账 → 第二次 thread/resume；claude 的 session 被拒                 (T8)
crewd/crates/exec/tests/live_codex.rs  真 codex 冒烟（#[ignore] + CREWD_LIVE_CODEX）                            (T9)
crewd/README.md、docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md、docs/superpowers/specs/2026-10-05-crewd-m2-executor-porting-map.md  (T9)
```

## 窄接口一览（M3 的会话管理只用这些；粗体是 M2c 新增）

```rust
// claude：M2b
pub async fn start_claude(start: ClaudeStart, config: DriverConfig)
    -> Result<(SessionHandle, mpsc::UnboundedReceiver<SessionEvent>), StartError>;
// codex：M2c（返回时握手已完成、thread id 已落账；错误另有 Handshake，同号在跑 = AlreadyRunning）
pub async fn start_codex(start: CodexStart, config: CodexConfig)
    -> Result<(SessionHandle, mpsc::UnboundedReceiver<SessionEvent>), StartError>;

impl SessionHandle {            // 两个执行器同一个类型；Clone；全部 drop = 停掉会话
    pub fn send(&self, text) -> Result<u64, Closed>;
    pub fn send_with_context(&self, text, context: UntrustedContext) -> Result<u64, Closed>;   // M2c
    pub fn raw(&self, bytes) -> Result<(), Closed>;          // codex：只认单独一个 Esc
    pub fn interrupt(&self) -> Result<(), Closed>;           // M2c：claude 写 Esc / codex turn/interrupt
    pub async fn control(&self, op: Control) -> Result<ControlOutcome, Closed>;   // M2c：claude 回 Unsupported
    pub fn resize(&self, cols, lines) -> Result<(), Closed>; // codex：无操作
    pub fn stop(&self) -> Result<(), Closed>;
    pub fn clear_quota_health(&self) -> Result<(), Closed>;
    pub async fn screen_text(&self, max_lines) -> Result<String, Closed>;   // codex：结构化记录的文本
    pub async fn snapshot(&self) -> Result<Option<Snapshot>, Closed>;       // M2c 改：codex 为 None
    pub async fn status(&self) -> Result<SessionStatus, Closed>;            // codex：agent_session_id = thread id
}

pub enum SessionEvent {         // 第一条一定是 Spawned；当前会话号以最近一条 Spawned 为准
    Spawned { pid, agent_session_id, resumed },
    Working(bool),              // false = idle 边沿
    Decision(DecisionEvent),    // 只有 claude
    Delivery { id, subject, outcome },
    Health(Option<Health>),     // + TurnFailed / VersionIncompatible（M2c）
    ResumeRefused { id, said }, // codex：跟在新 thread 的 Spawned 之后
    Item { turn_id, item },             // M2c，只有 codex
    TurnEnded { last_agent_message },   // M2c，只有 codex
    Notice(String),                     // M2c
    Profile(Profile),                   // M2c，只有 codex
    Exited(ExitInfo),           // 最后一条
}
```

---

### Task 1: 窄接口的最小扩展（claude 一侧）

把 M3 要「两个执行器一个写法」的那几个口子加到 M2b 的驱动上，claude 一侧给出各自的含义（裁定 1）。codex 一侧在 Task 5 / 7 实现。这一步动的全是 M2b 已合入的文件：下面的「原文」取自 `crewd-m2b@1e35379` 的实际代码；M2b 合入 main 时若又改了字面（换了名、多了字段、多了分支），按同样的语义改，并在任务回报里写明差异。

**Files:**
- Modify: `crewd/crates/exec/src/driver.rs`、`crewd/crates/exec/src/session.rs`、`crewd/crates/exec/src/health.rs`、`crewd/crates/exec/src/launch.rs`、`crewd/crates/exec/tests/driver.rs`

**Interfaces:**
- Consumes: M2b 的 `driver`、`session`、`health`、`launch`（合入版）。
- Produces:
  - `crewd_exec::driver`：`pub type UntrustedContext = BTreeMap<String, String>`；`pub enum Control { Model(String), Effort(String), Fast(bool), Compact }`；`pub enum ControlOutcome { Applied(String), NoConfirmation(String), Rejected(String), Unsupported(String) }`；`pub fn with_untrusted_context(&UntrustedContext, &str) -> String`；`pub(crate) enum Command { Send { id: u64, text: String, context: UntrustedContext }, Raw(Vec<u8>), Interrupt, Resize { cols: u16, lines: u16 }, Stop, ClearQuotaHealth, ScreenText { max_lines: usize, reply: oneshot::Sender<String> }, Snapshot { reply: oneshot::Sender<Option<Snapshot>> }, Status { reply: oneshot::Sender<SessionStatus> }, Control { op: Control, reply: oneshot::Sender<ControlOutcome> } }`；`SessionHandle::new(tx: mpsc::UnboundedSender<Command>) -> Self`（`pub(crate)`）、`send_with_context(&self, impl Into<String>, UntrustedContext) -> Result<u64, Closed>`、`interrupt(&self) -> Result<(), Closed>`、`async control(&self, Control) -> Result<ControlOutcome, Closed>`、`async snapshot(&self) -> Result<Option<Snapshot>, Closed>`；`StartError::Handshake { health: Health }`；`pub(crate) fn kill_group(u32)`、`pub(crate) fn terminate(u32)`、`pub(crate) fn decode(io::Result<std::process::ExitStatus>) -> ExitStatusKind`
  - `crewd_exec::session`：`SessionEvent::{Item { turn_id: String, item: serde_json::Value }, TurnEnded { last_agent_message: String }, Notice(String), Profile(Profile)}`；`#[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct Profile { pub model: Option<String>, pub effort: Option<String>, pub fast: Option<bool> }`
  - `crewd_exec::health`：`HealthKind::{TurnFailed, VersionIncompatible}`、`HealthKind::rank(self) -> u8`
  - `crewd_exec::launch`：`HelperCommands::mcp_args_for(&self, agent: &str) -> Vec<String>`

- [ ] **Step 1: 对齐检查与基线**

```bash
grep -n "^enum Command\|^fn handle(\|^fn dispatch(\|^fn finish(\|^fn kill_group\|^fn terminate\|^fn decode\|pub async fn snapshot\|AlreadyRunning(String)\|^struct LiveId\|fn claim(" crates/exec/src/driver.rs
grep -n "pub enum SessionEvent\|fn health_rank\|pub fn preview\|pub const BRIEF_DELIVERY_ID\|pub enum DerivedState" crates/exec/src/session.rs
grep -n "pub enum HealthKind\|pub fn is_quota" crates/exec/src/health.rs
grep -n "pub fn mcp_args\|pub fn valid_agent_session_id" crates/exec/src/launch.rs
grep -n "pub fn check_agent_session_id\|pub struct MemoryLedger" crates/exec/src/resume.rs
grep -n "pub fn reap_orphans\|PROCESS_REGISTRY_FILE\|pub fn lock_data_dir\|pub fn ensure_data_dir" crates/server/src/lib.rs
grep -n "pub struct StoreLedger\|fn open(&self)\|fn to_io" crates/server/src/ledger.rs
ls crates/crewd/src/claude_cmd.rs
cargo test -p crewd-exec --lib 2>&1 | grep "test result"
cargo test -p crewd-exec --test driver 2>&1 | grep "test result"
```

Expected：每一行都有命中（`fn health_rank` 是合入版 claude 核心里的私有档位表）。记下两个 `passed` 数作为本计划的基线（写计划时 `crewd-m2b@1e35379` 是 122 / 12）。**任何一行没命中就停下回报**——M2c 的接线点在 M2b 里被改掉了。

- [ ] **Step 2: 写失败的测试**

`crates/exec/tests/driver.rs`：头部的 `use crewd_exec::driver::{…};` 换成（保留原有的名字，加上 `Control`、`ControlOutcome`）：

```rust
use crewd_exec::driver::{
    ClaudeStart, Control, ControlOutcome, DriverConfig, SessionHandle, StartError, start_claude,
};
```

文件末尾追加：

```rust
/// M2c 给窄接口加的三个口子在 claude 一侧：打断 = 写一个 Esc；控制一律 `Unsupported`
/// （claude 的 /model、/compact 归 M3 用屏幕驱动）；快照是 `Some`。
#[tokio::test]
async fn interrupt_is_a_lone_escape_and_control_is_unsupported_on_claude() {
    let env = Env::new();
    let program = env.script(
        "stty raw -echo; echo ready; b=$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d ' \\n'); \
         stty sane; echo \"byte:$b\"; sleep 30",
    );
    let (h, mut rx) = start_claude(
        env.start(program, Arc::new(MemoryLedger::default())),
        env.config(),
    )
    .await
    .unwrap();
    screen_contains(&h, "ready").await;
    h.interrupt().unwrap();
    screen_contains(&h, "byte:1b").await;
    assert!(matches!(
        h.control(Control::Model("opus".into())).await.unwrap(),
        ControlOutcome::Unsupported(_)
    ));
    assert!(h.snapshot().await.unwrap().is_some());
    h.stop().unwrap();
    until(&mut rx, |e| exited(e).is_some()).await;
}
```

`crates/exec/src/driver.rs` 文件末尾追加（与只在 Linux 编译的那个测试模块并列）：

```rust
#[cfg(test)]
mod context_tests {
    use super::*;

    #[test]
    fn untrusted_context_is_wrapped_in_key_order_before_the_text() {
        let mut ctx = UntrustedContext::new();
        ctx.insert("crew_whiteboard".into(), "A: 我改完了".into());
        ctx.insert("aa_first".into(), "x".into());
        ctx.insert("empty".into(), String::new());
        assert_eq!(
            with_untrusted_context(&ctx, "开工"),
            "<external_aa_first>\nx\n</external_aa_first>\n\n\
             <external_crew_whiteboard>\nA: 我改完了\n</external_crew_whiteboard>\n\n开工"
        );
        assert_eq!(
            with_untrusted_context(&UntrustedContext::new(), "开工"),
            "开工"
        );
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p crewd-exec --test driver interrupt`
Expected: 编译失败，报 `no method named \`interrupt\`` 与找不到 `Control` / `ControlOutcome`。

- [ ] **Step 4: 实现**

`crates/exec/src/health.rs`：`HealthKind` 末尾（`PromptUndelivered,` 之后）加两种：

```rust
    /// codex：回合失败（协议里的失败回合 / 不再重试的错误），不能当成空闲。
    TurnFailed,
    /// codex：本机 CLI 太旧，服务端要求新版本。
    VersionIncompatible,
```

`impl HealthKind {` 里、`is_quota` 之前加：

```rust
    /// health 那一格的档位：拉起失败 > 额度 / 限流 > 认证 / 版本不兼容 > 投递报告 / 回合失败。
    /// 低档不能盖掉高档（额度类只由它自己的恢复路径清掉：认证探测的来回既不能盖掉它，也不能借
    /// 「清掉认证」把它清掉）。claude 与 codex 两个核心共用这一张表。
    pub fn rank(self) -> u8 {
        match self {
            Self::LaunchFailed => 4,
            Self::UsageLimit | Self::RateLimited => 3,
            Self::AuthRequired | Self::VersionIncompatible => 2,
            Self::BriefUndelivered | Self::PromptUndelivered | Self::TurnFailed => 1,
        }
    }
```

`crates/exec/src/session.rs`：删掉私有的 `fn health_rank(kind: HealthKind) -> u8 { … }` 连同它的文档注释（内容已挪进上面的 `rank()`），唯一的调用处

```rust
                .is_some_and(|cur| health_rank(cur.kind) > health_rank(h.kind))
```

换成

```rust
                .is_some_and(|cur| cur.kind.rank() > h.kind.rank())
```

同一文件 `SessionEvent` 里，`ResumeRefused` 及其文档注释（「claude 拒绝 `--resume <id>`，原话 `said`。」）连同后面的 `Exited(ExitInfo),\n}` 换成下面这段（`Profile` 结构体紧跟在枚举后面）：

```rust
    /// 续跑被拒：`id` 接不回来了，原话 `said`。claude 随后换新号重起、再发一次 `Spawned`；
    /// codex 在同一个进程里新开一条 thread，这条事件跟在 `Spawned` **之后**发。
    /// 当前会话号永远以最近一条 `Spawned` 为准。
    ResumeRefused {
        id: String,
        said: String,
    },
    /// codex：一个完成了的结构化条目（`item/completed` 的 `item` 原样）。claude 不发——它的
    /// 记录就是屏幕。M2d 拿它给 attach 的客户端回放。
    Item {
        turn_id: String,
        item: serde_json::Value,
    },
    /// codex：一个回合正常结束，附本回合最后一条 agent 正文（没有就是空串）。
    /// claude 的对应物是 Stop hook（M3 经 helper 送进来），不走执行器。
    TurnEnded {
        last_agent_message: String,
    },
    /// 执行器替 agent 做了一个人应该知道的决定（代拒了一个表单、丢了一段原始按键……）。
    Notice(String),
    /// codex：服务端确认的实际模型 / 推理强度 / fast 档（握手回包、设置回显）。
    Profile(Profile),
    Exited(ExitInfo),
}

/// 服务端确认的会话档位。`None` = 服务端没说（不是「关」）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub fast: Option<bool>,
}
```

`crates/exec/src/launch.rs`：`HelperCommands::mcp_args` 开头的

```rust
    pub fn mcp_args(&self) -> Vec<String> {
        let c = self.0;
        let mut a = vec!["mcp".to_string()];
        a.extend(self.identity());
        a.extend(["--agent".to_string(), "claude".to_string()]);
```

换成（函数体其余部分原样留在 `mcp_args_for` 里）：

```rust
    pub fn mcp_args(&self) -> Vec<String> {
        self.mcp_args_for("claude")
    }

    /// 同 [`Self::mcp_args`]，`--agent` 由调用方给（codex 驱动传 `"codex"`）。
    pub fn mcp_args_for(&self, agent: &str) -> Vec<String> {
        let c = self.0;
        let mut a = vec!["mcp".to_string()];
        a.extend(self.identity());
        a.extend(["--agent".to_string(), agent.to_string()]);
```

`crates/exec/src/driver.rs`，八处：

(a) `use std::collections::BTreeSet;` 换成 `use std::collections::{BTreeMap, BTreeSet};`。

(b) `StartError` 的 `AlreadyRunning(String),` 之后加：

```rust
    /// 进程起来了但握手没完成（codex：`initialize` / `thread/start` 失败、超时或进程中途退出）。
    /// 已杀掉进程。`health` 是给人看的拉起失败。
    Handshake { health: Health },
```

`Display` 里 `AlreadyRunning` 那一臂连同之后改成（去掉「claude」——codex 的 thread id 也走这张表，裁定 20）：

```rust
            Self::AlreadyRunning(id) => {
                write!(f, "会话 {id} 已经在跑，不能同时再起一个")
            }
            Self::Handshake { health } => write!(f, "{}", health.detail),
```

(c) 从 `enum Command {` 起、到 `SessionHandle::raw` 方法结束为止的这一段（原文）：

```rust
enum Command {
    Send {
        id: u64,
        text: String,
    },
    Raw(Vec<u8>),
    Resize {
        cols: u16,
        lines: u16,
    },
    Stop,
    ClearQuotaHealth,
    ScreenText {
        max_lines: usize,
        reply: oneshot::Sender<String>,
    },
    Snapshot {
        reply: oneshot::Sender<Snapshot>,
    },
    Status {
        reply: oneshot::Sender<SessionStatus>,
    },
}

/// 窄接口：启动之外的一切（输入 / 停止 / 状态 / 屏幕文本 / 快照）。
/// 可克隆；**所有句柄都 drop 了 = 停掉会话**（没人能再控制它）。
#[derive(Clone)]
pub struct SessionHandle {
    tx: mpsc::UnboundedSender<Command>,
    next_id: Arc<AtomicU64>,
}

impl SessionHandle {
    /// 投一段正文（排队，回读屏幕确认提交）。返回投递编号，结果见 `SessionEvent::Delivery`。
    pub fn send(&self, text: impl Into<String>) -> Result<u64, Closed> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.cmd(Command::Send {
            id,
            text: text.into(),
        })?;
        Ok(id)
    }

    /// 原样写字节（菜单按键、Esc 打断）。
    pub fn raw(&self, bytes: impl Into<Vec<u8>>) -> Result<(), Closed> {
        self.cmd(Command::Raw(bytes.into()))
    }
```

整段换成：

```rust
/// 附在一条正文前面的「不可信上下文」：键 = 来源（如 `crew_whiteboard`），值 = 原文。
/// 是数据不是指令——codex 走 `turn/start.additionalContext`（`kind: "untrusted"`），
/// claude 由驱动层包成 `<external_键>…</external_键>` 放在正文前面（[`with_untrusted_context`]）。
pub type UntrustedContext = BTreeMap<String, String>;

/// 只有部分执行器支持的会话控制。不支持的一方回 [`ControlOutcome::Unsupported`]，不假装做了。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Control {
    /// 之后的回合换模型。
    Model(String),
    /// 之后的回合换推理强度。
    Effort(String),
    /// fast 档开 / 关。
    Fast(bool),
    /// 压缩上下文（只在空闲时接受）。
    Compact,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlOutcome {
    /// 已生效（附说明）。
    Applied(String),
    /// 请求被接受了，但没等到权威回显——实际状态未知。
    NoConfirmation(String),
    /// 被拒（附原因）。
    Rejected(String),
    /// 这个执行器不支持这项控制。
    Unsupported(String),
}

/// claude 一侧的「不可信上下文」：按键序包成 `<external_键>` 块放在正文前面，与 codex
/// 服务端渲染 `additionalContext` 的形状一致。没有上下文就原样返回正文。
pub fn with_untrusted_context(context: &UntrustedContext, text: &str) -> String {
    let mut out = String::new();
    for (key, value) in context {
        if value.is_empty() {
            continue;
        }
        out.push_str(&format!("<external_{key}>\n{value}\n</external_{key}>\n\n"));
    }
    out.push_str(text);
    out
}

/// 句柄发给会话任务的命令。两个驱动（claude / codex）各自解释同一套命令。
pub(crate) enum Command {
    Send {
        id: u64,
        text: String,
        context: UntrustedContext,
    },
    Raw(Vec<u8>),
    Interrupt,
    Resize {
        cols: u16,
        lines: u16,
    },
    Stop,
    ClearQuotaHealth,
    ScreenText {
        max_lines: usize,
        reply: oneshot::Sender<String>,
    },
    Snapshot {
        reply: oneshot::Sender<Option<Snapshot>>,
    },
    Status {
        reply: oneshot::Sender<SessionStatus>,
    },
    Control {
        op: Control,
        reply: oneshot::Sender<ControlOutcome>,
    },
}

/// 窄接口：启动之外的一切（输入 / 打断 / 停止 / 状态 / 屏幕文本 / 快照 / 控制）。
/// claude 与 codex 用同一个句柄类型。可克隆；**所有句柄都 drop 了 = 停掉会话**。
#[derive(Clone)]
pub struct SessionHandle {
    tx: mpsc::UnboundedSender<Command>,
    next_id: Arc<AtomicU64>,
}

impl SessionHandle {
    /// 驱动层建句柄（投递编号从 `BRIEF_DELIVERY_ID + 1` 起，0 留给开场任务）。
    pub(crate) fn new(tx: mpsc::UnboundedSender<Command>) -> Self {
        Self {
            tx,
            next_id: Arc::new(AtomicU64::new(BRIEF_DELIVERY_ID + 1)),
        }
    }

    /// 投一段正文（排队）。返回投递编号，结果见 `SessionEvent::Delivery`。
    pub fn send(&self, text: impl Into<String>) -> Result<u64, Closed> {
        self.send_with_context(text, UntrustedContext::new())
    }

    /// 投一段正文，附带不可信上下文（白板未读等）。见 [`UntrustedContext`]。
    pub fn send_with_context(
        &self,
        text: impl Into<String>,
        context: UntrustedContext,
    ) -> Result<u64, Closed> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.cmd(Command::Send {
            id,
            text: text.into(),
            context,
        })?;
        Ok(id)
    }

    /// 原样写字节（菜单按键）。codex 没有终端：只认单独一个 Esc（= [`Self::interrupt`]），
    /// 其余丢弃并发一条 `Notice`。
    pub fn raw(&self, bytes: impl Into<Vec<u8>>) -> Result<(), Closed> {
        self.cmd(Command::Raw(bytes.into()))
    }

    /// 打断当前回合：claude 写一个 Esc，codex 发 `turn/interrupt`（没有进行中的回合就什么都不做）。
    pub fn interrupt(&self) -> Result<(), Closed> {
        self.cmd(Command::Interrupt)
    }

    /// 会话控制（换模型 / 推理强度 / fast、压缩）。不支持的执行器回 `Unsupported`。
    pub async fn control(&self, op: Control) -> Result<ControlOutcome, Closed> {
        let (reply, rx) = oneshot::channel();
        self.cmd(Command::Control { op, reply })?;
        rx.await.map_err(|_| Closed)
    }
```

(d) `snapshot` 的签名

```rust
    pub async fn snapshot(&self) -> Result<Snapshot, Closed> {
```

换成

```rust
    /// 终端快照；`None` = 这个执行器没有终端（codex：用 `screen_text` 读结构化记录的文本）。
    pub async fn snapshot(&self) -> Result<Option<Snapshot>, Closed> {
```

(e) `start_claude` 末尾的

```rust
    Ok((
        SessionHandle {
            tx,
            next_id: Arc::new(AtomicU64::new(BRIEF_DELIVERY_ID + 1)),
        },
        ev_rx,
    ))
```

换成

```rust
    Ok((SessionHandle::new(tx), ev_rx))
```

(f) `fn kill_group`、`fn terminate`、`fn decode` 三个函数前加 `pub(crate) `（codex 驱动共用）。

(g) 会话号占用表给 codex 共用（裁定 20）：`LIVE_AGENT_IDS` 的文档注释与 `LiveId` 换成

```rust
/// 本进程里正在跑的 agent 会话号（claude 会话号与 codex thread id 共用一张表）。
static LIVE_AGENT_IDS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// 占着一个会话号；drop 时放开。
pub(crate) struct LiveId(String);
```

`impl LiveId` 里 `fn claim` 改成 `pub(crate) fn claim`，并在它之前加

```rust
    pub(crate) fn id(&self) -> &str {
        &self.0
    }

```

(h) `fn handle` 里：

```rust
        Command::Send { id, text } => t.core.send(id, &text, now),
        Command::Raw(bytes) => t.core.raw(&bytes),
```

换成

```rust
        Command::Send { id, text, context } => {
            t.core
                .send(id, &with_untrusted_context(&context, &text), now)
        }
        Command::Raw(bytes) => t.core.raw(&bytes),
        Command::Interrupt => t.core.raw(&[0x1b]),
```

```rust
        Command::Snapshot { reply } => {
            let _ = reply.send(t.core.snapshot());
        }
```

换成

```rust
            let _ = reply.send(Some(t.core.snapshot()));
        }
        Command::Control { op, reply } => {
            let _ = reply.send(ControlOutcome::Unsupported(format!(
                "claude 会话的 {op:?} 由屏幕驱动（M3 的 /model、/compact），执行器不直接支持"
            )));
        }
```

（上面这段的第一行之前保留 `Command::Snapshot { reply } => {`。）

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd-exec --test driver`（连跑三遍）
Expected: 比基线多 1 条，全过。`cargo test -p crewd-exec --lib` 比基线多 1 条（`untrusted_context_is_wrapped_in_key_order_before_the_text`）。

- [ ] **Step 6: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add crates/exec/src/driver.rs crates/exec/src/session.rs crates/exec/src/health.rs crates/exec/src/launch.rs crates/exec/tests/driver.rs
git commit -m "crewd-exec: narrow interface grows send_with_context, interrupt, control, optional snapshot and codex events

Co-Authored-By: Claude <noreply@anthropic.com>"
```

- [ ] **Step 7: 验这把尺子会红**

`fn handle` 里 `Command::Interrupt => t.core.raw(&[0x1b]),` 临时改成 `Command::Interrupt => t.core.raw(&[]),` → `cargo test -p crewd-exec --test driver` → 红 1 条：`interrupt_is_a_lone_escape_and_control_is_unsupported_on_claude`（约 5s，等屏上出现 `byte:1b` 超时）。`git checkout -- crates/exec/src/driver.rs` 改回，`git status` 干净。

---

### Task 2: 线协议（`codex::rpc`）

一行一个 JSON、没有 `jsonrpc` 字段；三种形状（回应 / 服务端请求 / 通知）；服务端请求 id 整数或字符串、原样回；我们的请求表先登记后写、每个请求有期限、配不上的回应丢弃计数（裁定 4、5）。Swift `CodexRPCMessageTests` / `CodexRPCDispatcherTests` 的五条场景都在（id 0 两边各用一次不串、错误回应、早到回应、通知转发），外加字符串 id 与期限。

**Files:**
- Create: `crewd/crates/exec/src/codex/mod.rs`、`crewd/crates/exec/src/codex/rpc.rs`
- Modify: `crewd/crates/exec/src/lib.rs`

**Interfaces:**
- Consumes: 无（serde_json）。
- Produces（`crewd_exec::codex::rpc`）:
  - `pub enum RequestId { Int(i64), Str(String) }`、`pub struct RpcError { pub code: i64, pub message: String }`、`pub enum RpcFailure { Server(RpcError), TimedOut(Duration), Disconnected }`（`Display`）
  - `pub enum Incoming { Response { id: RequestId, result: Result<Value, RpcError> }, Request { id: RequestId, method: String, params: Value }, Notification { method: String, params: Value } }`、`pub enum ParseError { NotJson(String), Unclassifiable(String) }`、`pub fn parse_line(&str) -> Result<Incoming, ParseError>`
  - `pub fn request_line(i64, &str, &Value) -> String`、`pub fn notification_line(&str, Option<&Value>) -> String`、`pub fn response_line(&RequestId, &Value) -> String`、`pub fn error_line(&RequestId, i64, &str) -> String`
  - `pub struct Rpc<T>`（`Default`）：`request(&mut self, tag: T, method: &str, params: &Value, now: Instant, timeout: Duration) -> String`、`resolve(&mut self, &RequestId, Result<Value, RpcError>) -> Option<(T, String, Result<Value, RpcFailure>)>`、`expire(&mut self, Instant) -> Vec<(T, String, RpcFailure)>`、`fail_all(&mut self) -> Vec<(T, String, RpcFailure)>`、`next_deadline(&self) -> Option<Instant>`、`pending_len(&self) -> usize`、`stray_responses(&self) -> u64`

- [ ] **Step 1: 写失败的测试**

`crates/exec/src/lib.rs`：模块声明里 `pub mod delivery;` 之前加一行 `pub mod codex;`。

`crates/exec/src/codex/mod.rs`（整个文件；之后每个任务追加一行模块声明，保持字母序）：

```rust
//! codex 执行器：`codex app-server`（stdio JSON-RPC）客户端。
//!
//! 与 claude 驱动共用窄接口（`driver::SessionHandle` + `session::SessionEvent`）、
//! 环境白名单、孤儿登记表与会话号账本；没有终端，「屏幕文本」由结构化记录代替。
pub mod rpc;
```

`crates/exec/src/codex/rpc.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_line_omits_jsonrpc_and_is_a_single_line() {
        let l = request_line(7, "turn/start", &json!({ "text": "a\nb" }));
        assert!(!l.contains("jsonrpc"), "{l}");
        assert!(!l.contains('\n'), "{l}");
        let v: Value = serde_json::from_str(&l).unwrap();
        assert_eq!(v["id"], 7);
        assert_eq!(v["method"], "turn/start");
        assert_eq!(v["params"]["text"], "a\nb");
    }

    #[test]
    fn initialized_has_no_params_key() {
        assert_eq!(
            notification_line("initialized", None),
            r#"{"method":"initialized"}"#
        );
    }

    #[test]
    fn classifies_the_three_shapes() {
        assert_eq!(
            parse_line(r#"{"id":3,"result":{"ok":true}}"#).unwrap(),
            Incoming::Response {
                id: RequestId::Int(3),
                result: Ok(json!({ "ok": true })),
            }
        );
        assert_eq!(
            parse_line(r#"{"id":4,"method":"item/tool/call","params":{"x":1}}"#).unwrap(),
            Incoming::Request {
                id: RequestId::Int(4),
                method: "item/tool/call".into(),
                params: json!({ "x": 1 }),
            }
        );
        assert_eq!(
            parse_line(r#"{"method":"turn/started","params":{"turn":{"id":"t"}}}"#).unwrap(),
            Incoming::Notification {
                method: "turn/started".into(),
                params: json!({ "turn": { "id": "t" } }),
            }
        );
    }

    #[test]
    fn string_request_ids_are_classified_and_echoed_verbatim() {
        let Incoming::Request { id, .. } =
            parse_line(r#"{"id":"srv-1","method":"mcpServer/elicitation/request","params":{}}"#)
                .unwrap()
        else {
            panic!("字符串 id 的服务端请求必须认成请求，不能当通知");
        };
        assert_eq!(id, RequestId::Str("srv-1".into()));
        let reply: Value = serde_json::from_str(&error_line(&id, -32601, "no")).unwrap();
        assert_eq!(reply["id"], "srv-1");
        assert_eq!(reply["error"]["code"], -32601);
        let ok: Value =
            serde_json::from_str(&response_line(&id, &json!({ "action": "accept" }))).unwrap();
        assert_eq!(
            ok,
            json!({ "id": "srv-1", "result": { "action": "accept" } })
        );
    }

    #[test]
    fn error_response_and_garbage() {
        assert_eq!(
            parse_line(r#"{"id":1,"error":{"code":-32600,"message":"bad"}}"#).unwrap(),
            Incoming::Response {
                id: RequestId::Int(1),
                result: Err(RpcError {
                    code: -32600,
                    message: "bad".into(),
                }),
            }
        );
        assert!(matches!(
            parse_line("not json"),
            Err(ParseError::NotJson(_))
        ));
        assert!(matches!(
            parse_line("[1,2]"),
            Err(ParseError::Unclassifiable(_))
        ));
        assert!(matches!(
            parse_line(r#"{"id":null,"method":"x"}"#),
            Err(ParseError::Unclassifiable(_))
        ));
        // 行尾的 \r 不影响。
        assert!(parse_line("{\"method\":\"x\"}\r").is_ok());
    }

    #[test]
    fn our_ids_and_server_ids_live_in_separate_tables() {
        let mut rpc: Rpc<&str> = Rpc::default();
        let now = Instant::now();
        let l = rpc.request(
            "init",
            "initialize",
            &json!({}),
            now,
            Duration::from_secs(5),
        );
        assert!(l.contains(r#""id":0"#), "{l}");
        // 服务端用同一个 0 发请求：分类成请求，不碰我们的表。
        let srv = parse_line(r#"{"id":0,"method":"item/tool/call","params":{}}"#).unwrap();
        assert!(matches!(srv, Incoming::Request { .. }));
        assert_eq!(rpc.pending_len(), 1);
        let (tag, method, result) = rpc.resolve(&RequestId::Int(0), Ok(json!({}))).unwrap();
        assert_eq!(
            (tag, method.as_str(), result),
            ("init", "initialize", Ok(json!({})))
        );
        assert_eq!(rpc.pending_len(), 0);
    }

    #[test]
    fn a_reply_written_back_before_anyone_awaits_is_still_matched() {
        // Swift 的早到缓冲守的场景：回应在「登记等待」之前就到了。这里登记发生在交出那一行之前，
        // 所以哪怕回应紧跟着在同一次 read 里到，也一定配得上。
        let mut rpc: Rpc<u8> = Rpc::default();
        let now = Instant::now();
        let _ = rpc.request(1, "thread/start", &json!({}), now, Duration::from_secs(5));
        let batch =
            "{\"id\":0,\"result\":{\"thread\":{\"id\":\"t\"}}}\n{\"method\":\"thread/started\"}\n";
        let mut matched = 0;
        for l in batch.lines() {
            if let Incoming::Response { id, result } = parse_line(l).unwrap() {
                matched += usize::from(rpc.resolve(&id, result).is_some());
            }
        }
        assert_eq!(matched, 1);
    }

    #[test]
    fn requests_expire_and_late_replies_are_dropped() {
        let mut rpc: Rpc<&str> = Rpc::default();
        let t0 = Instant::now();
        let _ = rpc.request("a", "turn/start", &json!({}), t0, Duration::from_secs(1));
        let _ = rpc.request(
            "b",
            "turn/interrupt",
            &json!({}),
            t0,
            Duration::from_secs(3),
        );
        assert_eq!(rpc.next_deadline(), Some(t0 + Duration::from_secs(1)));
        assert!(rpc.expire(t0 + Duration::from_millis(999)).is_empty());
        let expired = rpc.expire(t0 + Duration::from_secs(1));
        assert_eq!(
            expired,
            vec![(
                "a",
                "turn/start".to_string(),
                RpcFailure::TimedOut(Duration::from_secs(1))
            )]
        );
        assert_eq!(rpc.next_deadline(), Some(t0 + Duration::from_secs(3)));
        // 超时之后迟到的回应：丢弃、计数。
        assert!(rpc.resolve(&RequestId::Int(0), Ok(json!({}))).is_none());
        assert!(
            rpc.resolve(&RequestId::Str("x".into()), Ok(json!({})))
                .is_none()
        );
        assert_eq!(rpc.stray_responses(), 2);
        let failed = rpc.fail_all();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].2, RpcFailure::Disconnected);
        assert_eq!(rpc.next_deadline(), None);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-exec --lib codex::rpc`
Expected: 编译失败，报找不到 `parse_line` / `request_line` / `Rpc` 等。

- [ ] **Step 3: 实现**

写在测试模块之前：

```rust
//! codex app-server 的线协议（Swift `CodexRPCMessage` / `CodexRPCDispatcher` 的移植）。
//!
//! 一行一个 JSON 对象，**没有 `jsonrpc` 字段**（`codex app-server generate-json-schema`
//! 0.160.1 的 JSONRPCRequest / Response / Error / Notification 都不带它）。
//!
//! 两套 id 严格分开：我们发出的请求用我们自己的整数 id（[`Rpc`] 的 pending 表）；服务端发来的
//! 请求带它自己的 id——schema 的 `RequestId` 是 `string | int64`，**两种都要能原样回**
//! （Swift 只认整数：字符串 id 的服务端请求会被当成通知、永远不答，回合挂死）。
//!
//! Swift 的「早到响应缓冲」：它的请求方先写、后在 actor 上登记等待，响应可能先到。这里
//! [`Rpc::request`] **先登记、后交出要写的那一行**，同一个任务按顺序处理每一行输入，
//! 所以「响应比登记早」在结构上不会发生；配不上的响应只剩「超时之后迟到」与「根本不是
//! 我们的 id」两种，一律丢弃并计数（[`Rpc::stray_responses`]）。Swift 没有单请求超时，
//! 这里每个请求都有期限（[`Rpc::expire`]）。
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// 服务端请求的 id：整数或字符串，回的时候原样带回去。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RequestId {
    Int(i64),
    Str(String),
}

impl RequestId {
    fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Number(n) => n.as_i64().map(Self::Int),
            Value::String(s) => Some(Self::Str(s.clone())),
            _ => None,
        }
    }

    fn to_value(&self) -> Value {
        match self {
            Self::Int(i) => json!(i),
            Self::Str(s) => json!(s),
        }
    }
}

/// 服务端回的 error 对象。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// 一个请求没拿到结果的原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RpcFailure {
    /// 服务端回了 error。
    Server(RpcError),
    /// 到期没回（已从 pending 表删掉，之后迟到的响应会被丢弃）。
    TimedOut(Duration),
    /// 进程没了 / 管道断了。
    Disconnected,
}

impl std::fmt::Display for RpcFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Server(e) => write!(f, "{}（{}）", e.message, e.code),
            Self::TimedOut(d) => write!(f, "{} 秒内没有回应", d.as_secs_f32()),
            Self::Disconnected => write!(f, "codex app-server 已断开"),
        }
    }
}

/// 一行输入的分类。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Incoming {
    /// 对我们某个请求的回应（`id` 是我们发出去的那个）。
    Response {
        id: RequestId,
        result: Result<Value, RpcError>,
    },
    /// 服务端请求：**必须回**，否则它在等的那个回合永远挂着。
    Request {
        id: RequestId,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// 不是 JSON（原文截断后附上）。
    NotJson(String),
    /// 是 JSON 但不是对象，或者认不出是哪一种（有 method 但 id 既不是整数也不是字符串……）。
    Unclassifiable(String),
}

/// 分类一行。行尾的 `\r` 与空白先去掉。有 `method`+`id` = 服务端请求；只有 `method` = 通知；
/// 只有 `id` = 回应（带 `error` 就是失败）。
pub fn parse_line(line: &str) -> Result<Incoming, ParseError> {
    let line = line.trim_end();
    let value: Value =
        serde_json::from_str(line).map_err(|_| ParseError::NotJson(clip(line, 200)))?;
    let Value::Object(mut obj) = value else {
        return Err(ParseError::Unclassifiable(clip(line, 200)));
    };
    let params = obj.remove("params").unwrap_or(Value::Null);
    let id = obj.get("id");
    match obj.get("method").and_then(Value::as_str) {
        Some(method) => match id {
            None => Ok(Incoming::Notification {
                method: method.to_string(),
                params,
            }),
            Some(raw) => match RequestId::from_value(raw) {
                Some(id) => Ok(Incoming::Request {
                    id,
                    method: method.to_string(),
                    params,
                }),
                None => Err(ParseError::Unclassifiable(clip(line, 200))),
            },
        },
        None => {
            let Some(id) = id.and_then(RequestId::from_value) else {
                return Err(ParseError::Unclassifiable(clip(line, 200)));
            };
            let result = match obj.remove("error") {
                Some(err) => Err(RpcError {
                    code: err.get("code").and_then(Value::as_i64).unwrap_or(-32603),
                    message: err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("未知错误")
                        .to_string(),
                }),
                None => Ok(obj.remove("result").unwrap_or(Value::Null)),
            };
            Ok(Incoming::Response { id, result })
        }
    }
}

fn clip(s: &str, max_chars: usize) -> String {
    let mut out: String = s.chars().take(max_chars).collect();
    if s.chars().count() > max_chars {
        out.push('…');
    }
    out
}

fn line(value: Value) -> String {
    // serde_json 把字符串里的换行转义成 `\n`，所以序列化结果本身就是一行。
    serde_json::to_string(&value).expect("Value 一定能序列化")
}

/// 我们的请求：`{"id","method","params"}`。
pub fn request_line(id: i64, method: &str, params: &Value) -> String {
    line(json!({ "id": id, "method": method, "params": params }))
}

/// 我们的通知。`params` 为 `None` 时不带这个键（`initialized` 在 schema 里就没有 params）。
pub fn notification_line(method: &str, params: Option<&Value>) -> String {
    match params {
        Some(p) => line(json!({ "method": method, "params": p })),
        None => line(json!({ "method": method })),
    }
}

/// 回服务端请求：成功。
pub fn response_line(id: &RequestId, result: &Value) -> String {
    line(json!({ "id": id.to_value(), "result": result }))
}

/// 回服务端请求：失败。
pub fn error_line(id: &RequestId, code: i64, message: &str) -> String {
    line(json!({ "id": id.to_value(), "error": { "code": code, "message": message } }))
}

struct Pending<T> {
    tag: T,
    method: String,
    timeout: Duration,
    deadline: Instant,
}

/// 我们发出、还没回的请求。`T` 是调用方给每个请求贴的标签（回应回来时原样交还）。
pub struct Rpc<T> {
    next_id: i64,
    pending: BTreeMap<i64, Pending<T>>,
    stray: u64,
}

impl<T> Default for Rpc<T> {
    fn default() -> Self {
        Self {
            next_id: 0,
            pending: BTreeMap::new(),
            stray: 0,
        }
    }
}

impl<T> Rpc<T> {
    /// 登记一个请求，返回要写进 stdin 的那一行（不含换行）。**先登记、后写**。
    pub fn request(
        &mut self,
        tag: T,
        method: &str,
        params: &Value,
        now: Instant,
        timeout: Duration,
    ) -> String {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(
            id,
            Pending {
                tag,
                method: method.to_string(),
                timeout,
                deadline: now + timeout,
            },
        );
        request_line(id, method, params)
    }

    /// 收到回应：配上了就交出 `(标签, 方法, 结果)`；配不上（超时后迟到 / 不是我们的 id）
    /// 返回 `None` 并计一笔。
    pub fn resolve(
        &mut self,
        id: &RequestId,
        result: Result<Value, RpcError>,
    ) -> Option<(T, String, Result<Value, RpcFailure>)> {
        let found = match id {
            RequestId::Int(i) => self.pending.remove(i),
            RequestId::Str(_) => None,
        };
        match found {
            Some(p) => Some((p.tag, p.method, result.map_err(RpcFailure::Server))),
            None => {
                self.stray += 1;
                None
            }
        }
    }

    /// 到期的请求：从表里拿掉，交出 `(标签, 方法, TimedOut)`。
    pub fn expire(&mut self, now: Instant) -> Vec<(T, String, RpcFailure)> {
        let due: Vec<i64> = self
            .pending
            .iter()
            .filter(|(_, p)| p.deadline <= now)
            .map(|(id, _)| *id)
            .collect();
        due.into_iter()
            .filter_map(|id| self.pending.remove(&id))
            .map(|p| (p.tag, p.method, RpcFailure::TimedOut(p.timeout)))
            .collect()
    }

    /// 连接断了：所有等待中的请求一起失败。
    pub fn fail_all(&mut self) -> Vec<(T, String, RpcFailure)> {
        std::mem::take(&mut self.pending)
            .into_values()
            .map(|p| (p.tag, p.method, RpcFailure::Disconnected))
            .collect()
    }

    /// 最早的期限（驱动层按它定时）。
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.values().map(|p| p.deadline).min()
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// 配不上的回应累计条数。
    pub fn stray_responses(&self) -> u64 {
        self.stray
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-exec --lib codex::rpc`
Expected: `8 passed`。

- [ ] **Step 5: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add crates/exec/src/lib.rs crates/exec/src/codex/mod.rs crates/exec/src/codex/rpc.rs
git commit -m "crewd-exec: codex app-server wire codec with string request ids and per-request deadlines

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: 参数构造与协议判定（`codex::proto`）

纯函数：每个请求的参数、服务端请求答复表（裁定 6）、协议健康（裁定 9）、上下文读数与压缩阈值（裁定 17）。Swift `CodexProtocolTests` 里的协议单测全部移过来（启动 / 续跑 / 设置的档位、MCP 挂在 `config` 下、空值不传、白板走 `additionalContext`、打断参数、请求分类、额度 / 版本 / 未知失败 / 可重试错误、限流快照、审批路由、elicitation 接受与拒绝、上下文读数、压缩阈值）；分类表按 0.160.1 的 11 个方法重写。

**Files:**
- Create: `crewd/crates/exec/src/codex/proto.rs`
- Modify: `crewd/crates/exec/src/codex/mod.rs`

**Interfaces:**
- Consumes: Task 1 `driver::{Control, UntrustedContext}`、`session::Profile`、`health::{Health, HealthKind}`。
- Produces（`crewd_exec::codex::proto`）:
  - 常量 `CLIENT_NAME: &str`、`OPTED_OUT_NOTIFICATIONS: [&str; 3]`、`WHITEBOARD_CONTEXT_KEY: &str`（`"crew_whiteboard"`）、`COMPACT_AT: f64`、`COMPACT_AT_IF_EXPENSIVE: f64`、`COMPACT_SPEND_MULTIPLE: f64`、`MIN_TURNS_BETWEEN_COMPACTIONS: u32`
  - `pub fn initialize_params(version: &str) -> Value`
  - `#[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct ThreadOptions { pub model: Option<String>, pub effort: Option<String>, pub fast: Option<bool>, pub developer_instructions: Option<String>, pub mcp_servers: Option<Value> }`
  - `thread_start_params(&Path, &ThreadOptions) -> Value`、`thread_resume_params(thread_id: &str, &Path, &ThreadOptions) -> Value`、`fast_mode_for_tier(&str) -> Option<bool>`、`mcp_servers(helper: &Path, args: &[String]) -> Value`、`turn_start_params(thread_id: &str, text: &str, &UntrustedContext) -> Value`、`turn_interrupt_params(&str, &str) -> Value`、`settings_update_params(&str, &Control) -> Option<Value>`、`compact_params(&str) -> Value`、`thread_id_of(&Value) -> Option<String>`、`profile_of(&Value) -> Profile`
  - `pub enum ServerRequestKind { Approval, Elicitation, Account, CurrentTime, Unsupported }`、`server_request_kind(&str) -> ServerRequestKind`、`pub enum Reply { Result(Value), Error { code: i64, message: String } }`、`pub struct ServerRequestOutcome { pub reply: Reply, pub notice: Option<String>, pub auth_required: bool }`、`answer_server_request(method: &str, params: &Value, now_unix: i64) -> ServerRequestOutcome`、`unanswerable_notice(&str, &Value) -> String`、`auth_required_health() -> Health`
  - `health_from(method: &str, params: &Value) -> Option<Health>`
  - `pub struct ContextUsage { pub turn_id: String, pub context_tokens: i64, pub context_window: i64, pub cumulative_tokens: i64 }`、`context_usage(&Value, expected_thread: &str) -> Option<ContextUsage>`、`should_compact(&ContextUsage, turn_tokens: i64, turns_since_compaction: u32) -> bool`

- [ ] **Step 1: 写失败的测试**

`crates/exec/src/codex/mod.rs`：`pub mod rpc;` 之前加 `pub mod proto;`。

`crates/exec/src/codex/proto.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn opts() -> ThreadOptions {
        ThreadOptions {
            model: Some("gpt-5.5".into()),
            effort: Some("high".into()),
            fast: Some(true),
            developer_instructions: Some("你是 crew 的一员".into()),
            mcp_servers: Some(mcp_servers(
                &PathBuf::from("/opt/crewd"),
                &["mcp".into(), "--agent".into(), "codex".into()],
            )),
        }
    }

    #[test]
    fn initialize_carries_client_info_experimental_api_and_opt_outs() {
        let p = initialize_params("0.1.0");
        assert_eq!(p["clientInfo"]["name"], "crewd");
        assert_eq!(p["clientInfo"]["version"], "0.1.0");
        assert_eq!(p["capabilities"]["experimentalApi"], true);
        assert_eq!(p["capabilities"]["requestAttestation"], false);
        assert_eq!(
            p["capabilities"]["optOutNotificationMethods"][0],
            "item/agentMessage/delta"
        );
    }

    #[test]
    fn thread_start_pins_policy_and_puts_mcp_and_effort_under_config() {
        let p = thread_start_params(Path::new("/repo"), &opts());
        assert_eq!(p["cwd"], "/repo");
        assert_eq!(p["approvalPolicy"], "on-request");
        assert_eq!(p["sandbox"], "workspace-write");
        assert_eq!(p["approvalsReviewer"], "auto_review");
        assert_eq!(p["model"], "gpt-5.5");
        assert_eq!(p["serviceTier"], "fast");
        assert_eq!(p["developerInstructions"], "你是 crew 的一员");
        assert_eq!(p["config"]["model_reasoning_effort"], "high");
        assert_eq!(p["config"]["mcp_servers"]["crew"]["command"], "/opt/crewd");
        assert_eq!(p["config"]["mcp_servers"]["crew"]["args"][2], "codex");
        assert!(p.get("mcpServers").is_none(), "顶层没有 mcpServers");
        assert!(p.get("threadId").is_none());
    }

    #[test]
    fn thread_start_omits_what_was_not_chosen() {
        let p = thread_start_params(Path::new("/repo"), &ThreadOptions::default());
        for key in ["model", "serviceTier", "developerInstructions", "config"] {
            assert!(p.get(key).is_none(), "{key} 不该出现：{p}");
        }
        assert_eq!(
            thread_start_params(
                Path::new("/r"),
                &ThreadOptions {
                    fast: Some(false),
                    ..ThreadOptions::default()
                }
            )["serviceTier"],
            "default"
        );
    }

    #[test]
    fn resume_keeps_the_settings_and_excludes_turns() {
        let p = thread_resume_params("thr-old", Path::new("/repo"), &opts());
        assert_eq!(p["threadId"], "thr-old");
        assert_eq!(p["excludeTurns"], true);
        assert_eq!(p["approvalsReviewer"], "auto_review");
        assert_eq!(p["config"]["mcp_servers"]["crew"]["command"], "/opt/crewd");
    }

    #[test]
    fn turn_start_puts_untrusted_context_beside_a_single_text_input() {
        let mut ctx = UntrustedContext::new();
        ctx.insert(WHITEBOARD_CONTEXT_KEY.into(), "B: 别动 main".into());
        ctx.insert("empty".into(), String::new());
        let p = turn_start_params("thr", "开工", &ctx);
        assert_eq!(p["threadId"], "thr");
        assert_eq!(
            p["input"],
            json!([{ "type": "text", "text": "开工", "text_elements": [] }])
        );
        assert_eq!(
            p["additionalContext"],
            json!({ "crew_whiteboard": { "value": "B: 别动 main", "kind": "untrusted" } })
        );
        let bare = turn_start_params("thr", "开工", &UntrustedContext::new());
        assert!(bare.get("additionalContext").is_none());
    }

    #[test]
    fn interrupt_settings_and_compact_params() {
        assert_eq!(
            turn_interrupt_params("thr", "turn-1"),
            json!({ "threadId": "thr", "turnId": "turn-1" })
        );
        assert_eq!(
            settings_update_params("thr", &Control::Model("o5".into())).unwrap(),
            json!({ "threadId": "thr", "model": "o5" })
        );
        assert_eq!(
            settings_update_params("thr", &Control::Effort("low".into())).unwrap(),
            json!({ "threadId": "thr", "effort": "low" })
        );
        assert_eq!(
            settings_update_params("thr", &Control::Fast(false)).unwrap(),
            json!({ "threadId": "thr", "serviceTier": "default" })
        );
        assert_eq!(settings_update_params("thr", &Control::Compact), None);
        assert_eq!(compact_params("thr"), json!({ "threadId": "thr" }));
    }

    #[test]
    fn thread_id_and_resolved_profile_come_from_the_response() {
        let r = json!({
            "thread": { "id": "thr-9" },
            "model": "gpt-5.5",
            "reasoningEffort": "medium",
            "serviceTier": "priority",
        });
        assert_eq!(thread_id_of(&r).as_deref(), Some("thr-9"));
        assert_eq!(
            profile_of(&r),
            Profile {
                model: Some("gpt-5.5".into()),
                effort: Some("medium".into()),
                fast: Some(true),
            }
        );
        assert_eq!(thread_id_of(&json!({ "thread": { "id": "" } })), None);
        assert_eq!(fast_mode_for_tier("default"), Some(false));
        assert_eq!(
            fast_mode_for_tier("turbo-2027"),
            None,
            "认不出的新档位不冒充标准"
        );
    }

    #[test]
    fn every_real_server_request_method_is_classified() {
        use ServerRequestKind::*;
        // codex-cli 0.160.1 `ServerRequest.json` 的全部 11 个方法。
        for (m, k) in [
            ("item/commandExecution/requestApproval", Approval),
            ("item/fileChange/requestApproval", Approval),
            ("item/permissions/requestApproval", Approval),
            ("applyPatchApproval", Approval),
            ("execCommandApproval", Approval),
            ("mcpServer/elicitation/request", Elicitation),
            ("account/chatgptAuthTokens/refresh", Account),
            ("currentTime/read", CurrentTime),
            ("item/tool/requestUserInput", Unsupported),
            ("item/tool/call", Unsupported),
            ("attestation/generate", Unsupported),
        ] {
            assert_eq!(server_request_kind(m), k, "{m}");
        }
        assert_eq!(server_request_kind("future/brandNew"), Unsupported);
    }

    #[test]
    fn every_server_request_gets_a_reply() {
        let approval =
            answer_server_request("item/commandExecution/requestApproval", &json!({}), 0);
        assert!(matches!(approval.reply, Reply::Error { code: -32000, .. }));
        assert_eq!(approval.notice, None);

        let tool = answer_server_request(
            "mcpServer/elicitation/request",
            &json!({ "serverName": "crew", "message": "Allow post_to_crew?",
                     "_meta": { "codex_approval_kind": "mcp_tool_call" } }),
            0,
        );
        assert_eq!(tool.reply, Reply::Result(json!({ "action": "accept" })));
        assert_eq!(tool.notice, None);

        let form = answer_server_request(
            "mcpServer/elicitation/request",
            &json!({ "serverName": "crew", "mode": "form", "message": "What is your name?",
                     "requestedSchema": { "type": "object" } }),
            0,
        );
        assert_eq!(form.reply, Reply::Result(json!({ "action": "decline" })));
        assert!(
            form.notice
                .as_deref()
                .unwrap()
                .contains("What is your name?")
        );

        let account = answer_server_request(
            "account/chatgptAuthTokens/refresh",
            &json!({ "reason": "unauthorized" }),
            0,
        );
        assert!(matches!(account.reply, Reply::Error { code: -32601, .. }));
        assert!(account.auth_required);
        assert_eq!(account.notice, None);

        let time = answer_server_request(
            "currentTime/read",
            &json!({ "threadId": "t" }),
            1_790_000_000,
        );
        assert_eq!(
            time.reply,
            Reply::Result(json!({ "currentTimeAt": 1_790_000_000i64 }))
        );

        let other = answer_server_request("item/tool/requestUserInput", &json!({}), 0);
        assert!(matches!(other.reply, Reply::Error { code: -32601, .. }));
        assert!(
            other
                .notice
                .as_deref()
                .unwrap()
                .contains("item/tool/requestUserInput")
        );
    }

    #[test]
    fn usage_limit_comes_from_the_failed_turn_protocol_field() {
        let h = health_from(
            "turn/completed",
            &json!({ "turn": { "id": "t", "status": "failed", "error": {
                "message": "Weekly limit reached", "codexErrorInfo": "usageLimitExceeded" } } }),
        )
        .unwrap();
        assert_eq!(h.kind, HealthKind::UsageLimit);
        assert!(h.detail.contains("Weekly limit reached"));
    }

    #[test]
    fn outdated_cli_is_visible_even_with_object_error_info() {
        let m = "The 'gpt-6-astra' model requires a newer version of Codex. Please upgrade...";
        let h = health_from(
            "turn/completed",
            &json!({ "turn": { "status": "failed", "error": { "message": m,
                "codexErrorInfo": { "httpConnectionFailed": { "httpStatusCode": 400 } } } } }),
        )
        .unwrap();
        assert_eq!(h.kind, HealthKind::VersionIncompatible);
        assert!(h.detail.contains(m));
    }

    #[test]
    fn unknown_terminal_failure_is_visible_but_retrying_error_is_not() {
        let failed = health_from(
            "turn/completed",
            &json!({ "turn": { "status": "failed", "error": { "message": "new server error",
                "codexErrorInfo": { "futureVariant": 1 } } } }),
        )
        .unwrap();
        assert_eq!(failed.kind, HealthKind::TurnFailed);
        assert!(failed.detail.contains("new server error"));
        assert_eq!(
            health_from("turn/completed", &json!({ "turn": { "status": "failed" } }))
                .unwrap()
                .kind,
            HealthKind::TurnFailed
        );
        assert_eq!(
            health_from(
                "turn/completed",
                &json!({ "turn": { "status": "completed" } })
            ),
            None
        );
        assert_eq!(
            health_from(
                "error",
                &json!({ "error": { "message": "transient" }, "willRetry": true })
            ),
            None
        );
        assert_eq!(
            health_from(
                "error",
                &json!({ "error": { "message": "gone" }, "willRetry": false })
            )
            .unwrap()
            .kind,
            HealthKind::TurnFailed
        );
        assert_eq!(
            health_from(
                "error",
                &json!({ "error": { "message": "busy", "codexErrorInfo": "serverOverloaded" },
                         "willRetry": true })
            ),
            None,
            "无关的可重试错误不冒充额度"
        );
    }

    #[test]
    fn unauthorized_and_rate_limit_snapshots() {
        assert_eq!(
            health_from(
                "error",
                &json!({ "error": { "message": "401", "codexErrorInfo": "unauthorized" },
                         "willRetry": false })
            )
            .unwrap()
            .kind,
            HealthKind::AuthRequired
        );
        assert_eq!(
            health_from(
                "account/rateLimits/updated",
                &json!({ "rateLimits": { "rateLimitReachedType": "rate_limit_reached" } })
            )
            .unwrap()
            .kind,
            HealthKind::UsageLimit
        );
        assert_eq!(
            health_from(
                "account/rateLimits/updated",
                &json!({ "rateLimits": { "spendControlReached": true } })
            )
            .unwrap()
            .kind,
            HealthKind::UsageLimit
        );
        assert_eq!(
            health_from(
                "account/rateLimits/updated",
                &json!({ "rateLimits": { "rateLimitReachedType": null, "primary": {} } })
            ),
            None
        );
    }

    #[test]
    fn context_usage_reads_last_for_context_and_total_for_cost() {
        let u = context_usage(
            &json!({ "threadId": "a", "turnId": "t", "tokenUsage": {
                "last": { "totalTokens": 128_486 }, "total": { "totalTokens": 2_312_214 },
                "modelContextWindow": 258_400 } }),
            "a",
        )
        .unwrap();
        assert_eq!(
            u,
            ContextUsage {
                turn_id: "t".into(),
                context_tokens: 128_486,
                context_window: 258_400,
                cumulative_tokens: 2_312_214,
            }
        );
        assert_eq!(context_usage(&json!({ "threadId": "other" }), "a"), None);
        let unknown_window = context_usage(
            &json!({ "threadId": "a", "turnId": "t2", "tokenUsage": {
                "last": { "totalTokens": 35_000 }, "total": { "totalTokens": 70_000 },
                "modelContextWindow": null } }),
            "a",
        )
        .unwrap();
        assert_eq!(unknown_window.context_window, 0);
    }

    #[test]
    fn compaction_needs_pressure_and_skips_recent_compaction() {
        let u = ContextUsage {
            turn_id: "t".into(),
            context_tokens: 128_486,
            context_window: 258_400,
            cumulative_tokens: 2_312_214,
        };
        assert!(should_compact(&u, 2_300_000, 3));
        assert!(!should_compact(&u, 50_000, 3));
        assert!(!should_compact(&u, 2_300_000, 1));
        let nearly_full = ContextUsage {
            context_tokens: 190_000,
            ..u.clone()
        };
        assert!(should_compact(&nearly_full, 40_000, 3));
        let no_window = ContextUsage {
            context_window: 0,
            ..nearly_full
        };
        assert!(!should_compact(&no_window, 40_000, 3));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-exec --lib codex::proto`
Expected: 编译失败，报找不到 `initialize_params` / `ThreadOptions` 等。

- [ ] **Step 3: 实现**

写在测试模块之前：

```rust
//! codex app-server 的参数构造与协议判定（Swift `CodexProtocol` 的移植）。全是纯函数。
//!
//! 方法名与字段名逐个对过 `codex app-server generate-json-schema --experimental`
//! （codex-cli 0.160.1，2026-10-06）。schema 管不到的（`config` 里的键、`_meta` 里的值、
//! 服务端报错的原话）在计划的「未核实」表里，靠 Swift 在 0.137 上的真机记录与二进制里的字符串。
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::driver::{Control, UntrustedContext};
use crate::health::{Health, HealthKind};
use crate::session::Profile;

/// `initialize.clientInfo.name`。
pub const CLIENT_NAME: &str = "crewd";

/// 不订阅的通知：只渲染 `item/completed`，增量流对执行器没用。后两条量大（命令输出
/// 逐块推送），`item/completed` 的 `aggregatedOutput` 里有全文。推理增量保留（M2d 可能要）。
pub const OPTED_OUT_NOTIFICATIONS: [&str; 3] = [
    "item/agentMessage/delta",
    "item/commandExecution/outputDelta",
    "item/fileChange/outputDelta",
];

/// 每个回合都带的白板上下文键（codex 按键去重：值没变就不再注入）。
pub const WHITEBOARD_CONTEXT_KEY: &str = "crew_whiteboard";

/// `initialize` 的参数。`experimentalApi` 解锁 `turn/start.additionalContext`；
/// `requestAttestation` 不要（我们不生成上游证明）。
pub fn initialize_params(version: &str) -> Value {
    json!({
        "clientInfo": { "name": CLIENT_NAME, "title": CLIENT_NAME, "version": version },
        "capabilities": {
            "experimentalApi": true,
            "requestAttestation": false,
            "optOutNotificationMethods": OPTED_OUT_NOTIFICATIONS,
        },
    })
}

/// 开 / 接一条 thread 的选项。`None` = 不传，让 codex 按它自己的配置优先级定
/// （人的原话：「codex 本身的默认模型是什么逻辑就用什么逻辑」），实际值从回包里读。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadOptions {
    pub model: Option<String>,
    pub effort: Option<String>,
    /// `Some(true)` → `serviceTier: "fast"`，`Some(false)` → `"default"`，`None` 不传。
    pub fast: Option<bool>,
    /// 世界观（M3 渲染），走 `developerInstructions`——经 stdin，`ps` 看不见。
    pub developer_instructions: Option<String>,
    /// `config.mcp_servers` 整块（[`mcp_servers`] 生成）。
    pub mcp_servers: Option<Value>,
}

/// `thread/start`：审批走 codex 自己的 `auto_review`，沙箱 `workspace-write`。
/// MCP 只能挂在 `config.mcp_servers` 下（顶层没有 `mcpServers`，未知键会被静默丢掉）。
pub fn thread_start_params(cwd: &Path, o: &ThreadOptions) -> Value {
    let mut p = Map::new();
    p.insert("cwd".into(), json!(cwd.display().to_string()));
    p.insert("approvalPolicy".into(), json!("on-request"));
    p.insert("sandbox".into(), json!("workspace-write"));
    p.insert("approvalsReviewer".into(), json!("auto_review"));
    if let Some(m) = o.model.as_deref().filter(|m| !m.is_empty()) {
        p.insert("model".into(), json!(m));
    }
    if let Some(fast) = o.fast {
        p.insert("serviceTier".into(), json!(service_tier(fast)));
    }
    if let Some(d) = o
        .developer_instructions
        .as_deref()
        .filter(|d| !d.is_empty())
    {
        p.insert("developerInstructions".into(), json!(d));
    }
    let mut config = Map::new();
    if let Some(e) = o.effort.as_deref().filter(|e| !e.is_empty()) {
        config.insert("model_reasoning_effort".into(), json!(e));
    }
    if let Some(m) = &o.mcp_servers {
        config.insert("mcp_servers".into(), m.clone());
    }
    if !config.is_empty() {
        p.insert("config".into(), Value::Object(config));
    }
    Value::Object(p)
}

/// `thread/resume`：同 `thread/start` 的设置 + `threadId` + `excludeTurns: true`
/// （不带历史回合：长 thread 的整段历史是一行几 MB 的 JSON，Swift 吃过超时的亏）。
pub fn thread_resume_params(thread_id: &str, cwd: &Path, o: &ThreadOptions) -> Value {
    let mut p = thread_start_params(cwd, o);
    p["threadId"] = json!(thread_id);
    p["excludeTurns"] = json!(true);
    p
}

fn service_tier(fast: bool) -> &'static str {
    if fast { "fast" } else { "default" }
}

/// 服务端报的档位 → fast 开关。认不出的新档位返回 `None`（不冒充「标准」）。
pub fn fast_mode_for_tier(tier: &str) -> Option<bool> {
    match tier.to_ascii_lowercase().as_str() {
        "fast" | "priority" => Some(true),
        "default" => Some(false),
        _ => None,
    }
}

/// `config.mcp_servers`：`{"crew": {"command": helper, "args": [...]}}`。
pub fn mcp_servers(helper: &Path, args: &[String]) -> Value {
    json!({ "crew": { "command": helper.display().to_string(), "args": args } })
}

/// `turn/start`：正文是唯一的 `input` 条目；不可信上下文走 `additionalContext`
/// （`kind: "untrusted"`——队友的话是数据，不是给这个 agent 的指令）。空值的键不带。
pub fn turn_start_params(thread_id: &str, text: &str, context: &UntrustedContext) -> Value {
    let mut p = json!({
        "threadId": thread_id,
        "input": [{ "type": "text", "text": text, "text_elements": [] }],
    });
    let entries: Map<String, Value> = context
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k.clone(), json!({ "value": v, "kind": "untrusted" })))
        .collect();
    if !entries.is_empty() {
        p["additionalContext"] = Value::Object(entries);
    }
    p
}

pub fn turn_interrupt_params(thread_id: &str, turn_id: &str) -> Value {
    json!({ "threadId": thread_id, "turnId": turn_id })
}

/// `thread/settings/update`；`Compact` 不是设置，返回 `None`。
pub fn settings_update_params(thread_id: &str, op: &Control) -> Option<Value> {
    let mut p = json!({ "threadId": thread_id });
    match op {
        Control::Model(m) => p["model"] = json!(m),
        Control::Effort(e) => p["effort"] = json!(e),
        Control::Fast(f) => p["serviceTier"] = json!(service_tier(*f)),
        Control::Compact => return None,
    }
    Some(p)
}

pub fn compact_params(thread_id: &str) -> Value {
    json!({ "threadId": thread_id })
}

/// `thread/start` / `thread/resume` 回包里的 thread id。
pub fn thread_id_of(result: &Value) -> Option<String> {
    result
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 握手回包里 codex 自己解析出来的档位（以它为准，不管我们传没传）。
pub fn profile_of(result: &Value) -> Profile {
    let s = |k: &str| {
        result
            .get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Profile {
        model: s("model"),
        effort: s("reasoningEffort"),
        fast: s("serviceTier").and_then(|t| fast_mode_for_tier(&t)),
    }
}

/// 服务端请求的种类。**每一种都要回**——不回，codex 就在那儿等，回合永远不结束
/// （Swift「运行中…转圈不结束」的病根）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerRequestKind {
    /// `item/*/requestApproval`，以及旧式的 `applyPatchApproval` / `execCommandApproval`。
    Approval,
    /// `mcpServer/elicitation/request`。
    Elicitation,
    /// `account/*`（如 `account/chatgptAuthTokens/refresh`）：登录态失效的强信号。
    Account,
    /// `currentTime/read`：问现在几点，照实回。
    CurrentTime,
    /// 其它一切（`item/tool/requestUserInput`、`item/tool/call`、`attestation/generate`、将来的新方法）。
    Unsupported,
}

pub fn server_request_kind(method: &str) -> ServerRequestKind {
    if method.ends_with("/requestApproval")
        || method == "applyPatchApproval"
        || method == "execCommandApproval"
    {
        ServerRequestKind::Approval
    } else if method == "mcpServer/elicitation/request" {
        ServerRequestKind::Elicitation
    } else if method.starts_with("account/") {
        ServerRequestKind::Account
    } else if method == "currentTime/read" {
        ServerRequestKind::CurrentTime
    } else {
        ServerRequestKind::Unsupported
    }
}

/// 回给服务端的东西。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Result(Value),
    Error { code: i64, message: String },
}

/// 一条服务端请求的处置：回什么、要不要告诉人、是不是登录失效。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerRequestOutcome {
    pub reply: Reply,
    /// 执行器替 agent 拒了一个本该人来答的请求——要让人知道（Swift Todo #6）。
    pub notice: Option<String>,
    /// `account/*`：翻 `AuthRequired`。
    pub auth_required: bool,
}

/// 决定怎么回一条服务端请求。`now_unix` = 现在的 Unix 秒（`currentTime/read` 用）。
pub fn answer_server_request(method: &str, params: &Value, now_unix: i64) -> ServerRequestOutcome {
    let unsupported = |notice: bool| ServerRequestOutcome {
        reply: Reply::Error {
            code: -32601,
            message: format!("client does not handle {method}"),
        },
        notice: notice.then(|| unanswerable_notice(method, params)),
        auth_required: false,
    };
    match server_request_kind(method) {
        // auto_review 在 codex 里面把常规审批办了；漏到客户端的审批一律拒，不做审批卡片、
        // 也不悄悄放行。
        ServerRequestKind::Approval => ServerRequestOutcome {
            reply: Reply::Error {
                code: -32000,
                message: "approval request reached client while auto_review is enabled".into(),
            },
            notice: None,
            auth_required: false,
        },
        ServerRequestKind::Elicitation => {
            // codex 把「MCP 工具调用要不要批」做成一条 elicitation，`_meta.codex_approval_kind
            // == "mcp_tool_call"`。crew 只挂我们自己的 `crew` 一个 MCP server，它的工具是 session
            // 自己的通讯神经，必须放行（拒了它 post_to_crew 就发不出去）。真表单没有 UI 可填：拒。
            let tool_call = params
                .pointer("/_meta/codex_approval_kind")
                .and_then(Value::as_str)
                == Some("mcp_tool_call");
            if tool_call {
                ServerRequestOutcome {
                    reply: Reply::Result(json!({ "action": "accept" })),
                    notice: None,
                    auth_required: false,
                }
            } else {
                ServerRequestOutcome {
                    reply: Reply::Result(json!({ "action": "decline" })),
                    notice: Some(unanswerable_notice(method, params)),
                    auth_required: false,
                }
            }
        }
        ServerRequestKind::Account => ServerRequestOutcome {
            auth_required: true,
            ..unsupported(false)
        },
        ServerRequestKind::CurrentTime => ServerRequestOutcome {
            reply: Reply::Result(json!({ "currentTimeAt": now_unix })),
            notice: None,
            auth_required: false,
        },
        ServerRequestKind::Unsupported => unsupported(true),
    }
}

/// 「codex 要一个我们给不出的回答、已代它拒绝」的人话。
pub fn unanswerable_notice(method: &str, params: &Value) -> String {
    let summary = ["message", "prompt", "reason", "question"]
        .iter()
        .find_map(|k| {
            params
                .get(*k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        })
        .map(str::to_string)
        .unwrap_or_else(|| format!("codex 发来一个需要人回答的请求（{method}）"));
    format!("codex 想要一个回答，执行器给不了，已代它拒绝：{summary}")
}

/// 登录失效（`account/*` 请求）时的 health。
pub fn auth_required_health() -> Health {
    Health::new(
        HealthKind::AuthRequired,
        "Codex 登录态失效（它请求客户端刷新 ChatGPT 令牌，执行器做不到）——在终端跑 codex login 重新登录后重启这个 session。",
    )
}

/// 从协议字段认健康状态（不是从正文猜）：
/// - `account/rateLimits/updated`：`rateLimitReachedType` 非空 / `spendControlReached`
/// - `error`（`willRetry: false`）/ 失败的 `turn/completed`：`codexErrorInfo` =
///   `usageLimitExceeded` / `unauthorized`；「requires a newer version of Codex」；其余失败一律
///   `TurnFailed`（失败不能看起来像空闲）。会重试的 `error` 不算。
pub fn health_from(method: &str, params: &Value) -> Option<Health> {
    if method == "account/rateLimits/updated" {
        let limits = params.get("rateLimits")?;
        if let Some(reached) = limits
            .get("rateLimitReachedType")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            return Some(Health::new(
                HealthKind::UsageLimit,
                format!("Codex 额度已到上限（{reached}），等额度重置后继续。"),
            ));
        }
        if limits.get("spendControlReached").and_then(Value::as_bool) == Some(true) {
            return Some(Health::new(
                HealthKind::UsageLimit,
                "Codex 用量控制上限已触发，等额度重置后继续。",
            ));
        }
        return None;
    }
    let (error, terminal) = match method {
        "error" => (
            params.get("error"),
            params.get("willRetry").and_then(Value::as_bool) == Some(false),
        ),
        "turn/completed" => {
            let turn = params.get("turn")?;
            let failed = turn.get("status").and_then(Value::as_str) == Some("failed");
            (turn.get("error").filter(|e| !e.is_null()), failed)
        }
        _ => return None,
    };
    let message = error
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty());
    if let Some(m) = message
        && m.to_ascii_lowercase()
            .contains("requires a newer version of codex")
    {
        return Some(Health::new(
            HealthKind::VersionIncompatible,
            format!("{m}；本机 Codex 版本太旧，停掉所有 Codex session 后由人升级。"),
        ));
    }
    match error
        .and_then(|e| e.get("codexErrorInfo"))
        .and_then(Value::as_str)
    {
        Some("usageLimitExceeded") => Some(Health::new(
            HealthKind::UsageLimit,
            format!(
                "{}；等额度重置后继续。",
                message.unwrap_or("Codex 额度已到上限")
            ),
        )),
        Some("unauthorized") => Some(Health::new(
            HealthKind::AuthRequired,
            message
                .unwrap_or("Codex 登录态失效，请重新登录后重启这个 session。")
                .to_string(),
        )),
        _ if terminal => Some(Health::new(
            HealthKind::TurnFailed,
            message
                .map(|m| format!("Codex 回合失败：{m}"))
                .unwrap_or_else(|| "Codex 回合失败，服务端没有给错误详情。".to_string()),
        )),
        _ => None,
    }
}

/// 一次 `thread/tokenUsage/updated` 的读数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextUsage {
    pub turn_id: String,
    /// 上下文里现在有多少（`last.totalTokens`）。
    pub context_tokens: i64,
    /// 模型上下文窗口（服务端没给 = 0）。
    pub context_window: i64,
    /// 本 thread 累计消耗（`total.totalTokens`）。
    pub cumulative_tokens: i64,
}

pub fn context_usage(params: &Value, expected_thread: &str) -> Option<ContextUsage> {
    if params.get("threadId").and_then(Value::as_str) != Some(expected_thread) {
        return None;
    }
    let usage = params.get("tokenUsage")?;
    let context_tokens = usage.pointer("/last/totalTokens").and_then(Value::as_i64)?;
    let cumulative_tokens = usage
        .pointer("/total/totalTokens")
        .and_then(Value::as_i64)?;
    let context_window = match usage.get("modelContextWindow") {
        None | Some(Value::Null) => 0,
        Some(v) => v.as_i64()?,
    };
    if context_tokens < 0 || cumulative_tokens < 0 || context_window < 0 {
        return None;
    }
    Some(ContextUsage {
        turn_id: params.get("turnId").and_then(Value::as_str)?.to_string(),
        context_tokens,
        context_window,
        cumulative_tokens,
    })
}

/// 自动压缩阈值（Swift `CodexCompactionPolicy`）。
pub const COMPACT_AT: f64 = 0.70;
/// 上下文过了这条线、且这一回合烧掉的 token ≥ 窗口 × [`COMPACT_SPEND_MULTIPLE`] 时也压。
pub const COMPACT_AT_IF_EXPENSIVE: f64 = 0.45;
pub const COMPACT_SPEND_MULTIPLE: f64 = 2.0;
/// 刚压过（距上次压缩不足这么多回合）不再压，免得压缩本身的开销引发循环。
pub const MIN_TURNS_BETWEEN_COMPACTIONS: u32 = 2;

/// 回合结束时要不要触发 codex 自己的压缩器。只在回合边界做，不截断任何东西。
pub fn should_compact(usage: &ContextUsage, turn_tokens: i64, turns_since_compaction: u32) -> bool {
    if usage.context_window <= 0 || turns_since_compaction < MIN_TURNS_BETWEEN_COMPACTIONS {
        return false;
    }
    let window = usage.context_window as f64;
    let pressure = usage.context_tokens as f64 / window;
    pressure >= COMPACT_AT
        || (pressure >= COMPACT_AT_IF_EXPENSIVE
            && turn_tokens.max(0) as f64 >= window * COMPACT_SPEND_MULTIPLE)
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-exec --lib codex::proto`
Expected: `15 passed`。

- [ ] **Step 5: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add crates/exec/src/codex/mod.rs crates/exec/src/codex/proto.rs
git commit -m "crewd-exec: codex request builders, server-request answer table and protocol health

Co-Authored-By: Claude <noreply@anthropic.com>"
```

- [ ] **Step 6: 验这把尺子会红**

`answer_server_request` 的 `ServerRequestKind::CurrentTime => ServerRequestOutcome {` 那一臂里 `reply: Reply::Result(json!({ "currentTimeAt": now_unix })),` 临时改成 `reply: Reply::Result(json!({})),` → `cargo test -p crewd-exec --lib codex::proto` → 红 1 条：`every_server_request_gets_a_reply`。改回（`git checkout -- crates/exec/src/codex/proto.rs`），`git status` 干净。

---

### Task 4: 回合跟踪与结构化记录（`codex::turns`）

喂通知、吐结论：回合开始 / 结束（id 对得上才算）、条目活动补开始边沿、本回合最后一条 agent 正文（裁定 8）、自动压缩的判定时机（读数可能晚到）、压缩回合的结束与失败；`turn/start` 回包里的回合 id 记成当前回合，但完成通知先到了就不能再记回去（否则永远「在忙」）。结构化记录只收 `item/completed` 的条目原样（有上限），渲染成 `screen_text` 的文本。Swift `CodexTranscriptTests` 里与执行器有关的几条（条目按 id 追加、回合开关、旧回合迟到的完成不清新回合、条目活动恢复「在干活」、坏条目不崩）都在；输入回显行与推理增量是 UI，不移（见「不做」）。

**Files:**
- Create: `crewd/crates/exec/src/codex/turns.rs`
- Modify: `crewd/crates/exec/src/codex/mod.rs`

**Interfaces:**
- Consumes: Task 3 `proto::{ContextUsage, context_usage, should_compact}`。
- Produces（`crewd_exec::codex::turns`）:
  - `#[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct Signals { pub turn_ended: Option<String>, pub compact: bool, pub compaction_finished: bool, pub compaction_problem: Option<String>, pub completed_ok: bool, pub item: Option<(String, Value)> }`
  - `pub struct TurnTracker`：`new(thread_id: impl Into<String>) -> Self`、`is_working(&self) -> bool`、`is_busy(&self) -> bool`、`is_compacting(&self) -> bool`、`active_turn(&self) -> Option<&str>`、`usage(&self) -> Option<&ContextUsage>`、`on_turn_accepted(&mut self, &str)`、`begin_compaction(&mut self)`、`compaction_failed(&mut self)`、`reset(&mut self)`、`on_notification(&mut self, method: &str, params: &Value) -> Signals`
  - `pub struct Transcript`：`new(cap: usize) -> Self`、`push(&mut self, turn_id: String, item: Value)`、`len`、`is_empty`、`text(&self, max_lines: usize) -> String`；`pub fn render_item(&Value) -> Vec<String>`

- [ ] **Step 1: 写失败的测试**

`crates/exec/src/codex/mod.rs`：`pub mod rpc;` 之后加 `pub mod turns;`。

`crates/exec/src/codex/turns.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn started(id: &str) -> Value {
        json!({ "threadId": "thr", "turn": { "id": id, "status": "inProgress", "items": [] } })
    }

    fn completed(id: &str, status: &str) -> Value {
        json!({ "threadId": "thr", "turn": { "id": id, "status": status, "items": [] } })
    }

    fn agent(turn: &str, text: &str) -> Value {
        json!({ "threadId": "thr", "turnId": turn, "completedAtMs": 0,
                "item": { "id": format!("i-{text}"), "type": "agentMessage", "text": text } })
    }

    fn usage(turn: &str, context: i64, total: i64) -> Value {
        json!({ "threadId": "thr", "turnId": turn, "tokenUsage": {
            "last": { "totalTokens": context }, "total": { "totalTokens": total },
            "modelContextWindow": 258_400 } })
    }

    #[test]
    fn a_turn_starts_works_and_ends_with_its_own_last_answer() {
        let mut t = TurnTracker::new("thr");
        t.on_notification("turn/started", &started("t1"));
        assert!(t.is_busy() && t.is_working());
        t.on_notification("item/completed", &agent("t1", "第一句"));
        let s = t.on_notification("item/completed", &agent("t1", "最后一句"));
        assert_eq!(s.item.as_ref().unwrap().0, "t1");
        let s = t.on_notification("turn/completed", &completed("t1", "completed"));
        assert_eq!(s.turn_ended.as_deref(), Some("最后一句"));
        assert!(s.completed_ok);
        assert!(!t.is_busy() && !t.is_working());
        // 下一回合没有 agent 正文：报空串，不拿上一回合的冒充。
        t.on_notification("turn/started", &started("t2"));
        let s = t.on_notification("turn/completed", &completed("t2", "completed"));
        assert_eq!(s.turn_ended.as_deref(), Some(""));
    }

    #[test]
    fn a_late_completion_of_an_old_turn_cannot_end_the_new_one() {
        let mut t = TurnTracker::new("thr");
        t.on_notification("turn/started", &started("new"));
        let s = t.on_notification("turn/completed", &completed("old", "completed"));
        assert_eq!(s, Signals::default());
        assert_eq!(t.active_turn(), Some("new"));
    }

    #[test]
    fn item_activity_repairs_a_missing_start_edge() {
        let mut t = TurnTracker::new("thr");
        t.on_notification("item/started", &json!({ "item": { "type": "reasoning" } }));
        assert!(t.is_working());
        assert!(!t.is_busy(), "没有回合 id 就不算占着回合");
        t.on_notification("turn/completed", &completed("x", "completed"));
        assert!(!t.is_working());
    }

    #[test]
    fn failed_and_interrupted_turns_end_work_without_a_turn_ended_report() {
        let mut t = TurnTracker::new("thr");
        t.on_notification("turn/started", &started("t1"));
        let s = t.on_notification("turn/completed", &completed("t1", "interrupted"));
        assert_eq!(s.turn_ended, None);
        assert!(!s.completed_ok);
        assert!(!t.is_working());
    }

    #[test]
    fn accepted_turn_id_counts_as_busy_unless_it_already_completed() {
        let mut t = TurnTracker::new("thr");
        t.on_turn_accepted("t1");
        assert_eq!(t.active_turn(), Some("t1"));
        t.on_notification("turn/completed", &completed("t1", "completed"));
        // 回包晚于完成通知到：不能再把这个回合记成进行中。
        t.on_turn_accepted("t1");
        assert!(!t.is_busy());
    }

    #[test]
    fn high_context_turn_asks_for_one_compaction_and_its_completion_is_not_an_answer() {
        let mut t = TurnTracker::new("thr");
        t.on_notification("turn/started", &started("work"));
        t.on_notification("item/completed", &agent("work", "做完了"));
        let s = t.on_notification("turn/completed", &completed("work", "completed"));
        assert_eq!(s.turn_ended.as_deref(), Some("做完了"));
        assert!(!s.compact, "读数还没到");
        // 读数晚于完成到达：这时才判。
        let s = t.on_notification(
            "thread/tokenUsage/updated",
            &usage("work", 190_000, 500_000),
        );
        assert!(s.compact);
        t.begin_compaction();
        assert!(t.is_busy() && t.is_working());
        t.on_notification("turn/started", &started("compact"));
        t.on_notification(
            "item/completed",
            &json!({ "turnId": "compact", "item": { "id": "c", "type": "contextCompaction" } }),
        );
        let s = t.on_notification(
            "thread/tokenUsage/updated",
            &usage("compact", 28_000, 525_000),
        );
        assert!(!s.compact, "压缩中不再触发");
        let s = t.on_notification("turn/completed", &completed("compact", "completed"));
        assert!(s.compaction_finished);
        assert_eq!(s.compaction_problem, None);
        assert_eq!(s.turn_ended, None, "压缩完成不是第二次回答");
        assert!(!t.is_busy() && !t.is_working());
        assert_eq!(t.usage().unwrap().context_tokens, 28_000);
        // 刚压过：下一个高压回合不立刻再压。
        t.on_notification("turn/started", &started("w2"));
        t.on_notification("thread/tokenUsage/updated", &usage("w2", 200_000, 900_000));
        let s = t.on_notification("turn/completed", &completed("w2", "completed"));
        assert!(!s.compact);
    }

    #[test]
    fn usage_already_in_hand_decides_at_completion() {
        let mut t = TurnTracker::new("thr");
        t.on_notification("turn/started", &started("w"));
        t.on_notification("thread/tokenUsage/updated", &usage("w", 190_000, 500_000));
        let s = t.on_notification("turn/completed", &completed("w", "completed"));
        assert!(s.compact);
    }

    #[test]
    fn failed_compaction_turn_reports_the_problem() {
        let mut t = TurnTracker::new("thr");
        t.begin_compaction();
        t.on_notification("turn/started", &started("c"));
        let s = t.on_notification(
            "turn/completed",
            &json!({ "turn": { "id": "c", "status": "failed", "error": { "message": "boom" } } }),
        );
        assert!(s.compaction_finished);
        assert_eq!(
            s.compaction_problem.as_deref(),
            Some("Codex 压缩失败：boom")
        );
        assert!(!t.is_compacting());
    }

    #[test]
    fn compaction_rpc_failure_leaves_the_compacting_state() {
        let mut t = TurnTracker::new("thr");
        t.begin_compaction();
        t.compaction_failed();
        assert!(!t.is_busy() && !t.is_working());
    }

    #[test]
    fn transcript_keeps_the_last_items_and_renders_them() {
        let mut tr = Transcript::new(3);
        tr.push(
            "t".into(),
            json!({ "type": "userMessage", "content": [{ "type": "text", "text": "修 bug" }] }),
        );
        tr.push(
            "t".into(),
            json!({ "type": "commandExecution", "command": "cargo test", "exitCode": 0 }),
        );
        tr.push(
            "t".into(),
            json!({ "type": "fileChange", "changes": [{ "path": "src/a.rs" }] }),
        );
        tr.push(
            "t".into(),
            json!({ "type": "agentMessage", "text": "改好了\n测试全绿" }),
        );
        assert_eq!(tr.len(), 3, "超过上限丢最旧的");
        assert_eq!(
            tr.text(10),
            "$ cargo test（exit 0）\n✎ src/a.rs\n改好了\n测试全绿"
        );
        assert_eq!(tr.text(2), "改好了\n测试全绿");
        assert_eq!(
            render_item(&json!({ "type": "imageView" })),
            vec!["[imageView]"]
        );
        assert_eq!(
            render_item(
                &json!({ "type": "mcpToolCall", "server": "crew", "tool": "post_to_crew", "status": "completed" })
            ),
            vec!["⚙ crew.post_to_crew（completed）"]
        );
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-exec --lib codex::turns`
Expected: 编译失败，报找不到 `TurnTracker` / `Transcript` / `Signals` 等。

- [ ] **Step 3: 实现**

写在测试模块之前：

```rust
//! codex 回合跟踪与结构化记录（Swift `CodexAppServerBackend.trackTurn` / `CodexTranscript`
//! 的移植）。纯计算：喂通知，吐「发生了什么」，不碰 IO、不看钟。
//!
//! - 回合：`turn/started` 置当前回合并开始干活；任何 `item/*` 也算在干活（补丢了的开始边沿）；
//!   **id 对得上**的 `turn/completed` 才结束（旧回合迟到的完成不能清掉新回合）。
//! - 回合正常结束且不是压缩回合 → 报「回合结束」，附**本回合**最后一条 agent 正文。
//! - 自动压缩：回合结束时上下文压力够（[`proto::should_compact`]）→ 报「该压缩了」；
//!   token 读数可能比 `turn/completed` 晚到，晚到时再判一次。
//! - 结构化记录：只收 `item/completed` 的条目原样（有上限），渲染成文本给 `screen_text` 用。
use std::collections::VecDeque;

use serde_json::Value;

use crate::codex::proto::{self, ContextUsage};

/// 喂一条通知之后的结论。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Signals {
    /// 一个普通回合正常结束：本回合最后一条 agent 正文（没有就是空串）。
    pub turn_ended: Option<String>,
    /// 该触发自动压缩了（调用方发 `thread/compact/start` 并调 [`TurnTracker::begin_compaction`]）。
    pub compact: bool,
    /// 压缩回合结束了（成功与否见 `compaction_problem`）——排队的正文可以放出去了。
    pub compaction_finished: bool,
    pub compaction_problem: Option<String>,
    /// 一个回合以 `completed` 结束（用来清掉过时的额度 / 失败 health）。
    pub completed_ok: bool,
    /// 一个完成了的条目：`(turn_id, item)`。
    pub item: Option<(String, Value)>,
}

#[derive(Debug)]
pub struct TurnTracker {
    thread_id: String,
    active_turn: Option<String>,
    last_completed: Option<String>,
    working: bool,
    compacting: bool,
    compacted_in_turn: bool,
    last_agent_message: Option<String>,
    turns_since_compaction: u32,
    usage: Option<ContextUsage>,
    turn_start_cumulative: Option<i64>,
    /// 回合结束时 token 读数还没到：(turn_id, 开始时的累计, 当时的「距上次压缩」)。
    usage_pending_for: Option<(String, Option<i64>, u32)>,
}

impl TurnTracker {
    pub fn new(thread_id: impl Into<String>) -> Self {
        Self {
            thread_id: thread_id.into(),
            active_turn: None,
            last_completed: None,
            working: false,
            compacting: false,
            compacted_in_turn: false,
            last_agent_message: None,
            // 新会话允许第一次就压（Swift 从 3 起）。
            turns_since_compaction: 3,
            usage: None,
            turn_start_cumulative: None,
            usage_pending_for: None,
        }
    }

    /// 在干活（UI 的「干活中」与 `Working` 边沿）。
    pub fn is_working(&self) -> bool {
        self.working
    }

    /// 不能开新回合：有进行中的回合或正在压缩。
    pub fn is_busy(&self) -> bool {
        self.active_turn.is_some() || self.compacting
    }

    pub fn is_compacting(&self) -> bool {
        self.compacting
    }

    pub fn active_turn(&self) -> Option<&str> {
        self.active_turn.as_deref()
    }

    pub fn usage(&self) -> Option<&ContextUsage> {
        self.usage.as_ref()
    }

    /// `turn/start` 的回包带着回合 id：没有进行中的回合、且它不是刚结束的那个，就记成当前回合
    /// （`turn/started` 可能晚到；完成通知若先到了，不能再把它记回去，否则永远「在忙」）。
    pub fn on_turn_accepted(&mut self, turn_id: &str) {
        if self.active_turn.is_none() && self.last_completed.as_deref() != Some(turn_id) {
            self.active_turn = Some(turn_id.to_string());
            self.working = true;
        }
    }

    /// 我们发了 `thread/compact/start`（自动或手动）：进入压缩态。
    pub fn begin_compaction(&mut self) {
        self.compacting = true;
        self.working = true;
        self.usage_pending_for = None;
    }

    /// `thread/compact/start` 被拒 / 超时：退出压缩态（不再自动重试，直到再过两个回合）。
    pub fn compaction_failed(&mut self) {
        self.compacting = false;
        self.turns_since_compaction = 0;
        self.working = self.active_turn.is_some();
    }

    /// 进程没了：一切归零。
    pub fn reset(&mut self) {
        self.active_turn = None;
        self.working = false;
        self.compacting = false;
        self.usage_pending_for = None;
    }

    pub fn on_notification(&mut self, method: &str, params: &Value) -> Signals {
        let mut s = Signals::default();
        match method {
            "thread/tokenUsage/updated" => self.on_usage(params, &mut s),
            "turn/started" => {
                self.active_turn = params
                    .pointer("/turn/id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.turn_start_cumulative = self.usage.as_ref().map(|u| u.cumulative_tokens);
                self.usage_pending_for = None;
                self.compacted_in_turn = false;
                self.last_agent_message = None;
                self.working = true;
            }
            "turn/completed" => self.on_completed(params, &mut s),
            "item/completed" => {
                self.working = true;
                if let Some(item) = params.get("item") {
                    match item.get("type").and_then(Value::as_str) {
                        Some("contextCompaction") => {
                            self.compacted_in_turn = true;
                            self.turns_since_compaction = 0;
                        }
                        Some("agentMessage") => {
                            self.last_agent_message = Some(
                                item.get("text")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                            );
                        }
                        _ => {}
                    }
                    let turn = params
                        .get("turnId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    s.item = Some((turn, item.clone()));
                }
            }
            m if m.starts_with("item/") => self.working = true,
            _ => {}
        }
        s
    }

    fn on_usage(&mut self, params: &Value, s: &mut Signals) {
        let Some(usage) = proto::context_usage(params, &self.thread_id) else {
            return;
        };
        if self.turn_start_cumulative.is_none() {
            self.turn_start_cumulative = Some(usage.cumulative_tokens);
        }
        self.usage = Some(usage.clone());
        if let Some((turn, baseline, since)) = self.usage_pending_for.clone()
            && turn == usage.turn_id
            && self.active_turn.is_none()
            && !self.compacting
        {
            self.usage_pending_for = None;
            s.compact = proto::should_compact(&usage, spent(&usage, baseline), since);
        }
    }

    fn on_completed(&mut self, params: &Value, s: &mut Signals) {
        let completed_id = params
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let (Some(active), Some(done)) = (&self.active_turn, &completed_id)
            && active != done
        {
            return;
        }
        let ok = params.pointer("/turn/status").and_then(Value::as_str) == Some("completed");
        let finishing_compaction = self.compacting;
        if finishing_compaction {
            s.compaction_finished = true;
            if !ok {
                let why = params
                    .pointer("/turn/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("回合未完成");
                s.compaction_problem = Some(format!("Codex 压缩失败：{why}"));
            }
        }
        if ok && !finishing_compaction {
            // 压缩回合没有新回答；回合里压过一次又没有新回答的，也不算一次回答（别重复发群）。
            if !self.compacted_in_turn || self.last_agent_message.is_some() {
                s.turn_ended = Some(self.last_agent_message.clone().unwrap_or_default());
            }
            if !self.compacted_in_turn
                && let Some(done) = &completed_id
            {
                match &self.usage {
                    Some(u) if &u.turn_id == done => {
                        s.compact = proto::should_compact(
                            u,
                            spent(u, self.turn_start_cumulative),
                            self.turns_since_compaction,
                        );
                    }
                    _ => {
                        self.usage_pending_for = Some((
                            done.clone(),
                            self.turn_start_cumulative,
                            self.turns_since_compaction,
                        ));
                    }
                }
            }
        }
        s.completed_ok = ok;
        self.last_completed = completed_id;
        self.active_turn = None;
        self.last_agent_message = None;
        if finishing_compaction || self.compacted_in_turn {
            self.turns_since_compaction = 0;
        } else {
            self.turns_since_compaction = (self.turns_since_compaction + 1).min(3);
        }
        if finishing_compaction {
            self.compacting = false;
        }
        self.working = false;
    }
}

fn spent(usage: &ContextUsage, baseline: Option<i64>) -> i64 {
    usage.cumulative_tokens - baseline.unwrap_or(usage.cumulative_tokens)
}

/// 最近完成的条目（`item/completed` 原样），有上限。
#[derive(Debug)]
pub struct Transcript {
    items: VecDeque<(String, Value)>,
    cap: usize,
}

impl Transcript {
    pub fn new(cap: usize) -> Self {
        Self {
            items: VecDeque::new(),
            cap: cap.max(1),
        }
    }

    pub fn push(&mut self, turn_id: String, item: Value) {
        if self.items.len() == self.cap {
            self.items.pop_front();
        }
        self.items.push_back((turn_id, item));
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 渲染成文本，取最后 `max_lines` 行（`screen_text` 的 codex 版）。
    pub fn text(&self, max_lines: usize) -> String {
        let lines: Vec<String> = self
            .items
            .iter()
            .flat_map(|(_, i)| render_item(i))
            .collect();
        let start = lines.len().saturating_sub(max_lines);
        lines[start..].join("\n")
    }
}

/// 一个条目的文本形状。认不出的类型给一行 `[类型]`（codex 加了新条目也不至于看不见）。
pub fn render_item(item: &Value) -> Vec<String> {
    let s = |k: &str| item.get(k).and_then(Value::as_str).unwrap_or_default();
    let block = |prefix: &str, text: &str| -> Vec<String> {
        text.lines().map(|l| format!("{prefix}{l}")).collect()
    };
    match s("type") {
        "userMessage" => {
            let text: Vec<&str> = item
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|c| c.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|c| c.get("text").and_then(Value::as_str))
                .collect();
            block("› ", &text.join("\n"))
        }
        "agentMessage" => block("", s("text")),
        "reasoning" => {
            let summary: Vec<&str> = item
                .get("summary")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            block("· ", &summary.join("\n"))
        }
        "commandExecution" => {
            let code = item
                .get("exitCode")
                .and_then(Value::as_i64)
                .map(|c| format!("（exit {c}）"))
                .unwrap_or_default();
            vec![format!("$ {}{code}", s("command"))]
        }
        "fileChange" => {
            let paths: Vec<&str> = item
                .get("changes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|c| c.get("path").and_then(Value::as_str))
                .collect();
            vec![format!("✎ {}", paths.join(", "))]
        }
        "mcpToolCall" => vec![format!(
            "⚙ {}.{}（{}）",
            s("server"),
            s("tool"),
            s("status")
        )],
        "contextCompaction" => vec!["（上下文已压缩）".to_string()],
        "" => vec!["[?]".to_string()],
        other => vec![format!("[{other}]")],
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-exec --lib codex::turns`
Expected: `10 passed`。

- [ ] **Step 5: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add crates/exec/src/codex/mod.rs crates/exec/src/codex/turns.rs
git commit -m "crewd-exec: codex turn tracking, compaction timing and transcript rendering

Co-Authored-By: Claude <noreply@anthropic.com>"
```

- [ ] **Step 6: 验这把尺子会红**

`on_turn_accepted` 里 `if self.active_turn.is_none() && self.last_completed.as_deref() != Some(turn_id) {` 临时改成 `if self.active_turn.is_none() {` → `cargo test -p crewd-exec --lib codex::turns` → 红 1 条：`accepted_turn_id_counts_as_busy_unless_it_already_completed`。改回（`git checkout -- crates/exec/src/codex/turns.rs`），`git status` 干净。

---

### Task 5: 会话核心（`codex::session::CodexCore`，sans-IO）

一个 codex 会话的全部判断，不碰 IO，形状与 M2b 的 `session::Core` 一样：`on_line` / `on_tick` / `on_recorded` / `send` / `interrupt` / `raw` / `control` / `clear_quota_health` / `stop` / `on_exit`，吐出按顺序执行的 `CodexAction`。所以握手、续跑退回、落账先于 `Spawned`、服务端请求、排队、期限、压缩、fast 回显、健康档位、退出收尾，全能用合成 JSON 和合成时间单测（裁定 2–9、16、17）。

测试器材 `T` 是一个合成的 app-server：核心发出的每一行都解析回 JSON 记下来，回应按方法名配上 id。`T::ready` 走完「initialize → thread/start（回 `thr-1`）→ 落账成功」。

**Files:**
- Create: `crewd/crates/exec/src/codex/session.rs`
- Modify: `crewd/crates/exec/src/codex/mod.rs`

**Interfaces:**
- Consumes: Task 1 `driver::{Control, ControlOutcome, UntrustedContext}`、`session::{DeliveryOutcome, DerivedState, ExitInfo, ExitStatusKind, Profile, SessionEvent, preview}`、`health::{Health, HealthKind, LaunchVerdict}`、`delivery::Subject`、`resume::valid_agent_session_id`；Task 2 `rpc::{Incoming, ParseError, Rpc, RpcFailure, error_line, notification_line, parse_line, response_line}`；Task 3 `proto::{self, Reply, ThreadOptions}`；Task 4 `turns::{Signals, Transcript, TurnTracker}`。
- Produces（`crewd_exec::codex::session`）:
  - `#[derive(Clone, Copy, Debug)] pub struct CodexCoreConfig { pub initialize_timeout: Duration, pub thread_timeout: Duration, pub request_timeout: Duration, pub settings_echo_timeout: Duration, pub hit_limit_window: Duration, pub transcript_items: usize }`（`Default` = 30s / 60s / 30s / 8s / 600s / 200）
  - `pub struct PendingTurn { pub id: u64, pub subject: Subject, pub text: String, pub context: UntrustedContext }`
  - `pub struct Opening { pub pid: u32, pub cwd: PathBuf, pub options: ThreadOptions, pub recorded: Option<String>, pub client_version: String, pub brief: Option<PendingTurn>, pub unix_now: i64 }`
  - `pub enum StartFailure { Launch(Health), Ledger(String) }`
  - `pub enum CodexAction { Write(String), Record(String), ControlDone { ticket: u64, outcome: ControlOutcome }, Failed(StartFailure), Event(SessionEvent) }`
  - `pub struct CodexCore`：`new(Opening, CodexCoreConfig, now: Instant) -> Self`、`take_actions(&mut self) -> Vec<CodexAction>`、`next_wakeup(&self) -> Option<Instant>`、`is_ready`、`is_running`、`thread_id(&self) -> Option<&str>`、`health(&self) -> Option<&Health>`、`is_working`、`state(&self) -> DerivedState`、`transcript_text(&self, usize) -> String`、`on_line(&mut self, &str, Instant)`、`on_dropped_line(&mut self, bytes: usize, Instant)`、`on_recorded(&mut self, Result<(), String>, Instant)`、`on_tick(&mut self, Instant)`、`send(&mut self, id: u64, text: &str, UntrustedContext, Instant)`、`interrupt(&mut self, Instant)`、`raw(&mut self, &[u8], Instant)`、`control(&mut self, ticket: u64, Control, Instant)`、`clear_quota_health(&mut self, Instant)`、`stop(&mut self, Instant)`、`on_exit(&mut self, ExitStatusKind, stderr_tail: &str, Instant)`
  - `pub fn launch_health(LaunchVerdict, underlying: &str) -> Health`

- [ ] **Step 1: 写失败的测试**

`crates/exec/src/codex/mod.rs`：`pub mod rpc;` 之后加 `pub mod session;`。

`crates/exec/src/codex/session.rs`：先只放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PID: u32 = 4242;

    /// 合成的 app-server：核心发出的每一行都解析回 JSON 记下来，回应按方法名配。
    struct T {
        core: CodexCore,
        t0: Instant,
        sent: Vec<Value>,
        events: Vec<SessionEvent>,
        other: Vec<CodexAction>,
    }

    impl T {
        fn new(recorded: Option<&str>, brief: Option<&str>) -> Self {
            Self::with(recorded, brief, CodexCoreConfig::default())
        }

        fn with(recorded: Option<&str>, brief: Option<&str>, config: CodexCoreConfig) -> Self {
            let t0 = Instant::now();
            let core = CodexCore::new(
                Opening {
                    pid: PID,
                    cwd: PathBuf::from("/repo"),
                    options: ThreadOptions {
                        developer_instructions: Some("世界观".into()),
                        ..ThreadOptions::default()
                    },
                    recorded: recorded.map(str::to_string),
                    client_version: "0.1.0".into(),
                    brief: brief.map(|b| PendingTurn {
                        id: 0,
                        subject: Subject::Brief,
                        text: b.into(),
                        context: [("crew_whiteboard".to_string(), "A: 你好".to_string())]
                            .into_iter()
                            .collect(),
                    }),
                    unix_now: 1_790_000_000,
                },
                config,
                t0,
            );
            let mut t = Self {
                core,
                t0,
                sent: vec![],
                events: vec![],
                other: vec![],
            };
            t.pump();
            t
        }

        fn at(&self, secs: f64) -> Instant {
            self.t0 + Duration::from_secs_f64(secs)
        }

        fn pump(&mut self) {
            for a in self.core.take_actions() {
                match a {
                    CodexAction::Write(l) => self.sent.push(serde_json::from_str(&l).unwrap()),
                    CodexAction::Event(e) => self.events.push(e),
                    other => self.other.push(other),
                }
            }
        }

        fn methods(&self) -> Vec<String> {
            self.sent
                .iter()
                .filter_map(|v| v.get("method").and_then(Value::as_str).map(str::to_string))
                .collect()
        }

        fn last(&self, method: &str) -> Value {
            self.sent
                .iter()
                .rev()
                .find(|v| v["method"] == method)
                .unwrap_or_else(|| panic!("没发过 {method}：{:?}", self.methods()))
                .clone()
        }

        fn count(&self, method: &str) -> usize {
            self.sent.iter().filter(|v| v["method"] == method).count()
        }

        fn line(&mut self, v: Value, secs: f64) {
            let now = self.at(secs);
            self.core.on_line(&v.to_string(), now);
            self.pump();
        }

        fn reply(&mut self, method: &str, result: Value) {
            let id = self.last(method)["id"].clone();
            self.line(json!({ "id": id, "result": result }), 0.1);
        }

        fn reply_err(&mut self, method: &str, message: &str) {
            let id = self.last(method)["id"].clone();
            self.line(
                json!({ "id": id, "error": { "code": -32600, "message": message } }),
                0.1,
            );
        }

        fn notify(&mut self, method: &str, params: Value) {
            self.line(json!({ "method": method, "params": params }), 0.2);
        }

        fn take_events(&mut self) -> Vec<SessionEvent> {
            std::mem::take(&mut self.events)
        }

        fn take_other(&mut self) -> Vec<CodexAction> {
            std::mem::take(&mut self.other)
        }

        /// 新开：initialize → thread/start（回 `thr-1`）→ 落账成功。
        fn ready(brief: Option<&str>) -> Self {
            let mut t = Self::new(None, brief);
            t.reply("initialize", json!({ "userAgent": "codex" }));
            t.reply(
                "thread/start",
                json!({ "thread": { "id": "thr-1" }, "model": "gpt-5.5",
                        "reasoningEffort": "high", "serviceTier": "default" }),
            );
            assert_eq!(t.take_other(), vec![CodexAction::Record("thr-1".into())]);
            t.core.on_recorded(Ok(()), t.at(0.2));
            t.pump();
            t
        }

        fn turn(&mut self, id: &str, answer: &str) {
            self.notify(
                "turn/started",
                json!({ "threadId": "thr-1", "turn": { "id": id } }),
            );
            self.notify(
                "item/completed",
                json!({ "threadId": "thr-1", "turnId": id, "completedAtMs": 0,
                        "item": { "id": format!("{id}-a"), "type": "agentMessage", "text": answer } }),
            );
            self.notify(
                "turn/completed",
                json!({ "threadId": "thr-1", "turn": { "id": id, "status": "completed" } }),
            );
        }
    }

    fn deliveries(events: &[SessionEvent]) -> Vec<(u64, DeliveryOutcome)> {
        events
            .iter()
            .filter_map(|e| match e {
                SessionEvent::Delivery { id, outcome, .. } => Some((*id, outcome.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn fresh_start_records_the_thread_before_spawned_and_before_the_first_turn() {
        let mut t = T::new(None, Some("开场任务"));
        assert_eq!(t.methods(), vec!["initialize"]);
        assert_eq!(
            t.last("initialize")["params"]["capabilities"]["experimentalApi"],
            true
        );
        t.reply("initialize", json!({}));
        assert_eq!(
            t.methods(),
            vec!["initialize", "initialized", "thread/start"]
        );
        assert!(t.sent[1].get("id").is_none(), "initialized 是通知");
        let start = t.last("thread/start");
        assert_eq!(start["params"]["developerInstructions"], "世界观");
        assert_eq!(start["params"]["approvalsReviewer"], "auto_review");
        t.reply(
            "thread/start",
            json!({ "thread": { "id": "thr-1" }, "model": "gpt-5.5", "serviceTier": "fast" }),
        );
        assert_eq!(t.take_other(), vec![CodexAction::Record("thr-1".into())]);
        assert!(t.events.is_empty(), "落账之前不发任何事件");
        assert_eq!(t.count("turn/start"), 0, "落账之前不开回合");
        t.core.on_recorded(Ok(()), t.at(0.3));
        t.pump();
        let ev = t.take_events();
        assert_eq!(
            ev[0],
            SessionEvent::Spawned {
                pid: PID,
                agent_session_id: "thr-1".into(),
                resumed: false,
            }
        );
        assert_eq!(
            ev[1],
            SessionEvent::Profile(Profile {
                model: Some("gpt-5.5".into()),
                effort: None,
                fast: Some(true),
            })
        );
        assert_eq!(ev[2], SessionEvent::Working(true));
        let turn = t.last("turn/start");
        assert_eq!(turn["params"]["threadId"], "thr-1");
        assert_eq!(turn["params"]["input"][0]["text"], "开场任务");
        assert_eq!(
            turn["params"]["additionalContext"]["crew_whiteboard"],
            json!({ "value": "A: 你好", "kind": "untrusted" })
        );
        t.reply(
            "turn/start",
            json!({ "turn": { "id": "turn-1", "status": "inProgress" } }),
        );
        assert_eq!(
            deliveries(&t.take_events()),
            vec![(0, DeliveryOutcome::Delivered)]
        );
        assert!(t.core.is_working());
    }

    #[test]
    fn recorded_thread_is_resumed_without_turns_and_not_rerecorded() {
        let mut t = T::new(Some("thr-old"), None);
        t.reply("initialize", json!({}));
        let resume = t.last("thread/resume");
        assert_eq!(resume["params"]["threadId"], "thr-old");
        assert_eq!(resume["params"]["excludeTurns"], true);
        assert_eq!(resume["params"]["developerInstructions"], "世界观");
        t.reply("thread/resume", json!({ "thread": { "id": "thr-old" } }));
        assert!(t.take_other().is_empty(), "续上的就是账本里那条，不再写账");
        assert_eq!(
            t.take_events(),
            vec![SessionEvent::Spawned {
                pid: PID,
                agent_session_id: "thr-old".into(),
                resumed: true,
            }]
        );
        assert_eq!(t.count("thread/start"), 0);
    }

    #[test]
    fn refused_resume_falls_back_to_a_fresh_recorded_thread_and_says_so_after_spawned() {
        let mut t = T::new(Some("thr-gone"), Some("接着干"));
        t.reply("initialize", json!({}));
        t.reply_err("thread/resume", "no rollout found for thread id thr-gone");
        assert_eq!(t.count("thread/start"), 1);
        t.reply("thread/start", json!({ "thread": { "id": "thr-new" } }));
        assert_eq!(t.take_other(), vec![CodexAction::Record("thr-new".into())]);
        t.core.on_recorded(Ok(()), t.at(0.3));
        t.pump();
        let ev = t.take_events();
        assert_eq!(
            ev[0],
            SessionEvent::Spawned {
                pid: PID,
                agent_session_id: "thr-new".into(),
                resumed: false,
            }
        );
        let SessionEvent::ResumeRefused { id, said } = &ev[1] else {
            panic!("第二条应是 ResumeRefused：{ev:?}");
        };
        assert_eq!(id, "thr-gone");
        assert!(said.contains("no rollout found"), "{said}");
        assert_eq!(t.last("turn/start")["params"]["threadId"], "thr-new");
    }

    #[test]
    fn ledger_failure_fails_the_start_and_no_turn_is_opened() {
        let mut t = T::new(None, Some("开场任务"));
        t.reply("initialize", json!({}));
        t.reply("thread/start", json!({ "thread": { "id": "thr-1" } }));
        t.take_other();
        t.core.on_recorded(Err("库坏了".into()), t.at(0.3));
        t.pump();
        assert_eq!(
            t.take_other(),
            vec![CodexAction::Failed(StartFailure::Ledger("库坏了".into()))]
        );
        assert!(t.events.is_empty());
        assert_eq!(t.count("turn/start"), 0);
        assert!(!t.core.is_running());
    }

    #[test]
    fn a_silent_initialize_is_a_stalled_launch_at_its_deadline() {
        let mut t = T::new(None, None);
        t.core.on_tick(t.at(29.9));
        t.pump();
        assert!(t.other.is_empty());
        assert_eq!(t.core.next_wakeup(), Some(t.at(30.0)));
        t.core.on_tick(t.at(30.0));
        t.pump();
        let other = t.take_other();
        let [CodexAction::Failed(StartFailure::Launch(h))] = other.as_slice() else {
            panic!("应当启动失败");
        };
        assert_eq!(h.kind, HealthKind::LaunchFailed);
        assert_eq!(h.launch_verdict, Some(LaunchVerdict::Stalled));
        assert!(h.detail.contains("initialize"), "{}", h.detail);
    }

    #[test]
    fn dying_during_the_handshake_carries_the_stderr_tail() {
        let mut t = T::new(None, None);
        t.reply("initialize", json!({}));
        t.core
            .on_exit(ExitStatusKind::Code(1), "Error: not logged in", t.at(1.0));
        t.pump();
        let other = t.take_other();
        let [CodexAction::Failed(StartFailure::Launch(h))] = other.as_slice() else {
            panic!("应当启动失败");
        };
        assert_eq!(h.launch_verdict, Some(LaunchVerdict::DiedSilently));
        assert!(
            h.detail.contains("exit 1") && h.detail.contains("not logged in"),
            "{}",
            h.detail
        );
        assert!(t.events.is_empty(), "没就绪过就不发 Exited");
    }

    #[test]
    fn server_requests_during_the_handshake_are_answered_now_and_reported_after_spawned() {
        let mut t = T::new(None, None);
        t.reply("initialize", json!({}));
        t.line(
            json!({ "id": "srv-a", "method": "account/chatgptAuthTokens/refresh",
                    "params": { "reason": "unauthorized" } }),
            0.2,
        );
        let reply = t.sent.last().unwrap().clone();
        assert_eq!(reply["id"], "srv-a");
        assert_eq!(reply["error"]["code"], -32601);
        assert!(t.events.is_empty());
        t.reply("thread/start", json!({ "thread": { "id": "thr-1" } }));
        t.core.on_recorded(Ok(()), t.at(0.3));
        t.pump();
        let ev = t.take_events();
        assert!(matches!(ev[0], SessionEvent::Spawned { .. }), "{ev:?}");
        assert!(
            ev.iter().any(
                |e| matches!(e, SessionEvent::Health(Some(h)) if h.kind == HealthKind::AuthRequired)
            ),
            "{ev:?}"
        );
        assert_eq!(t.core.state(), DerivedState::Error);
    }

    #[test]
    fn every_server_request_is_answered_and_each_kind_is_announced_once() {
        let mut t = T::ready(None);
        t.take_events();
        let before = t.sent.len();
        for (i, (id, method, params)) in [
            (json!(7), "item/commandExecution/requestApproval", json!({})),
            (
                json!("e-1"),
                "mcpServer/elicitation/request",
                json!({ "_meta": { "codex_approval_kind": "mcp_tool_call" } }),
            ),
            (
                json!("e-2"),
                "mcpServer/elicitation/request",
                json!({ "mode": "form", "message": "你叫什么？" }),
            ),
            (
                json!(8),
                "item/tool/requestUserInput",
                json!({ "prompt": "选一个" }),
            ),
            (
                json!(9),
                "item/tool/requestUserInput",
                json!({ "prompt": "再选一个" }),
            ),
            (
                json!(10),
                "currentTime/read",
                json!({ "threadId": "thr-1" }),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            t.line(
                json!({ "id": id, "method": method, "params": params }),
                1.0 + i as f64,
            );
        }
        let replies = &t.sent[before..];
        assert_eq!(replies.len(), 6, "每一条都回");
        assert_eq!(replies[0]["error"]["code"], -32000);
        assert_eq!(
            replies[1],
            json!({ "id": "e-1", "result": { "action": "accept" } })
        );
        assert_eq!(
            replies[2],
            json!({ "id": "e-2", "result": { "action": "decline" } })
        );
        assert_eq!(replies[3]["error"]["code"], -32601);
        assert_eq!(replies[4]["id"], 9);
        // 构造时 unix 1_790_000_000，第 6 条在 6s 后到。
        assert_eq!(
            replies[5],
            json!({ "id": 10, "result": { "currentTimeAt": 1_790_000_006i64 } })
        );
        let notices: Vec<&SessionEvent> = t
            .events
            .iter()
            .filter(|e| matches!(e, SessionEvent::Notice(_)))
            .collect();
        assert_eq!(
            notices.len(),
            2,
            "表单一次、requestUserInput 一次（同种只喊一次）：{notices:?}"
        );
    }

    #[test]
    fn sends_queue_behind_a_running_turn_and_each_gets_its_own_turn() {
        let mut t = T::ready(Some("开场"));
        t.reply("turn/start", json!({ "turn": { "id": "t1" } }));
        t.core.send(1, "第二条", UntrustedContext::new(), t.at(1.0));
        t.core.send(2, "第三条", UntrustedContext::new(), t.at(1.0));
        t.pump();
        assert_eq!(t.count("turn/start"), 1, "回合在跑：排队");
        t.take_events();
        t.turn("t1", "开场做完了");
        let ev = t.take_events();
        assert!(ev.contains(&SessionEvent::TurnEnded {
            last_agent_message: "开场做完了".into()
        }));
        assert!(
            !ev.contains(&SessionEvent::Working(false)),
            "后面还有排队的：不报空闲边沿"
        );
        assert_eq!(t.count("turn/start"), 2);
        assert_eq!(t.last("turn/start")["params"]["input"][0]["text"], "第二条");
        assert!(
            t.last("turn/start")["params"]
                .get("additionalContext")
                .is_none()
        );
        t.reply("turn/start", json!({ "turn": { "id": "t2" } }));
        t.turn("t2", "第二条做完了");
        t.reply("turn/start", json!({ "turn": { "id": "t3" } }));
        t.turn("t3", "全做完了");
        let ev = t.take_events();
        assert_eq!(
            deliveries(&ev),
            vec![
                (1, DeliveryOutcome::Delivered),
                (2, DeliveryOutcome::Delivered)
            ]
        );
        assert_eq!(ev.last(), Some(&SessionEvent::Working(false)), "{ev:?}");
        assert!(!t.core.is_working());
        assert_eq!(t.core.state(), DerivedState::Idle);
        assert!(t.core.transcript_text(10).contains("全做完了"));
    }

    #[test]
    fn a_rejected_turn_start_is_undelivered_and_the_queue_moves_on() {
        let mut t = T::ready(Some("开场"));
        t.core.send(1, "第二条", UntrustedContext::new(), t.at(1.0));
        t.pump();
        t.reply_err("turn/start", "thread busy");
        let ev = t.take_events();
        let d = deliveries(&ev);
        assert!(
            matches!(&d[0], (0, DeliveryOutcome::Undelivered(m)) if m.contains("thread busy")),
            "{d:?}"
        );
        assert!(ev.iter().any(
            |e| matches!(e, SessionEvent::Health(Some(h)) if h.kind == HealthKind::BriefUndelivered)
        ));
        assert_eq!(t.last("turn/start")["params"]["input"][0]["text"], "第二条");
        t.reply("turn/start", json!({ "turn": { "id": "t2" } }));
        assert_eq!(
            t.core.health().map(|h| h.kind),
            Some(HealthKind::BriefUndelivered),
            "别的指令送达，不清掉「开场任务没送到」"
        );
    }

    #[test]
    fn a_turn_start_that_never_answers_times_out_and_the_session_stays_usable() {
        let mut t = T::ready(Some("开场"));
        t.take_events();
        t.core.on_tick(t.at(30.3));
        t.pump();
        let ev = t.take_events();
        assert!(
            matches!(&deliveries(&ev)[0], (0, DeliveryOutcome::Undelivered(m)) if m.contains("没有回应")),
            "{ev:?}"
        );
        assert!(ev.contains(&SessionEvent::Working(false)));
        // 迟到的回应被丢弃；之后的正文照常开回合。
        t.reply("turn/start", json!({ "turn": { "id": "late" } }));
        t.core.send(1, "再来", UntrustedContext::new(), t.at(31.0));
        t.pump();
        assert_eq!(t.last("turn/start")["params"]["input"][0]["text"], "再来");
    }

    #[test]
    fn interrupt_targets_the_active_turn_and_a_lone_escape_means_interrupt() {
        let mut t = T::ready(None);
        t.core.interrupt(t.at(1.0));
        t.pump();
        assert_eq!(t.count("turn/interrupt"), 0, "没有进行中的回合");
        t.notify(
            "turn/started",
            json!({ "threadId": "thr-1", "turn": { "id": "t9" } }),
        );
        t.core.raw(&[0x1b], t.at(1.5));
        t.pump();
        assert_eq!(
            t.last("turn/interrupt")["params"],
            json!({ "threadId": "thr-1", "turnId": "t9" })
        );
        t.take_events();
        t.core.raw(b"\x1b[A", t.at(1.6));
        t.pump();
        assert!(matches!(&t.take_events()[..], [SessionEvent::Notice(n)] if n.contains("3 字节")));
        assert_eq!(t.count("turn/interrupt"), 1);
    }

    #[test]
    fn usage_limit_health_is_cleared_by_a_later_completed_turn_and_counts_as_hit_limit() {
        let mut t = T::ready(None);
        t.notify(
            "account/rateLimits/updated",
            json!({ "rateLimits": { "rateLimitReachedType": "rate_limit_reached" } }),
        );
        assert_eq!(t.core.state(), DerivedState::RateLimited);
        t.turn("t1", "又能干了");
        assert_eq!(t.core.health(), None);
        t.notify(
            "turn/completed",
            json!({ "threadId": "thr-1", "turn": { "id": "t2", "status": "failed",
                "error": { "message": "Weekly limit reached", "codexErrorInfo": "usageLimitExceeded" } } }),
        );
        assert_eq!(t.core.health().unwrap().kind, HealthKind::UsageLimit);
        t.core.on_exit(ExitStatusKind::Code(0), "", t.at(100.0));
        t.pump();
        assert_eq!(
            t.events.last(),
            Some(&SessionEvent::Exited(ExitInfo {
                status: ExitStatusKind::Code(0),
                stopped_by_user: false,
                hit_limit: true,
            }))
        );
    }

    #[test]
    fn high_context_triggers_one_compaction_and_sends_wait_for_it() {
        let mut t = T::ready(Some("干活"));
        t.reply("turn/start", json!({ "turn": { "id": "work" } }));
        t.turn("work", "干完了");
        t.notify(
            "thread/tokenUsage/updated",
            json!({ "threadId": "thr-1", "turnId": "work", "tokenUsage": {
                "last": { "totalTokens": 190_000 }, "total": { "totalTokens": 500_000 },
                "modelContextWindow": 258_400 } }),
        );
        assert_eq!(t.count("thread/compact/start"), 1);
        assert_eq!(
            t.last("thread/compact/start")["params"],
            json!({ "threadId": "thr-1" })
        );
        assert!(t.core.is_working());
        t.core
            .send(1, "压缩时来的", UntrustedContext::new(), t.at(2.0));
        t.pump();
        assert_eq!(t.count("turn/start"), 1, "压缩中排队");
        t.reply("thread/compact/start", json!({}));
        t.notify(
            "turn/started",
            json!({ "threadId": "thr-1", "turn": { "id": "c" } }),
        );
        t.notify(
            "item/completed",
            json!({ "threadId": "thr-1", "turnId": "c", "item": { "id": "ci", "type": "contextCompaction" } }),
        );
        t.take_events();
        t.notify(
            "turn/completed",
            json!({ "threadId": "thr-1", "turn": { "id": "c", "status": "completed" } }),
        );
        let ev = t.take_events();
        assert!(
            !ev.iter()
                .any(|e| matches!(e, SessionEvent::TurnEnded { .. })),
            "压缩完成不是第二次回答：{ev:?}"
        );
        assert_eq!(t.count("turn/start"), 2);
        assert_eq!(
            t.last("turn/start")["params"]["input"][0]["text"],
            "压缩时来的"
        );
        assert_eq!(t.count("thread/compact/start"), 1);
    }

    #[test]
    fn a_compaction_that_cannot_start_releases_the_queue_and_says_why() {
        let mut t = T::ready(None);
        t.core.control(5, Control::Compact, t.at(1.0));
        t.pump();
        t.core.send(1, "排着", UntrustedContext::new(), t.at(1.1));
        t.pump();
        assert_eq!(t.count("turn/start"), 0);
        t.reply_err("thread/compact/start", "not supported");
        let other = t.take_other();
        assert!(
            matches!(&other[..], [CodexAction::ControlDone { ticket: 5, outcome: ControlOutcome::Rejected(m) }] if m.contains("not supported")),
            "{other:?}"
        );
        assert!(
            t.events
                .iter()
                .any(|e| matches!(e, SessionEvent::Notice(n) if n.contains("压缩没启动")))
        );
        assert_eq!(t.last("turn/start")["params"]["input"][0]["text"], "排着");
    }

    #[test]
    fn manual_compaction_is_refused_while_busy_and_accepted_when_idle() {
        let mut t = T::ready(Some("干活"));
        t.core.control(1, Control::Compact, t.at(1.0));
        t.pump();
        assert!(matches!(
            &t.take_other()[..],
            [CodexAction::ControlDone {
                ticket: 1,
                outcome: ControlOutcome::Rejected(_)
            }]
        ));
        t.reply("turn/start", json!({ "turn": { "id": "t1" } }));
        t.turn("t1", "好了");
        t.core.control(2, Control::Compact, t.at(2.0));
        t.pump();
        t.reply("thread/compact/start", json!({}));
        assert_eq!(
            t.take_other(),
            vec![CodexAction::ControlDone {
                ticket: 2,
                outcome: ControlOutcome::Applied("已开始压缩".into()),
            }]
        );
    }

    #[test]
    fn model_change_is_applied_on_ack_and_fast_waits_for_the_echo() {
        let mut t = T::ready(None);
        t.core.control(1, Control::Model("o5".into()), t.at(1.0));
        t.pump();
        assert_eq!(
            t.last("thread/settings/update")["params"],
            json!({ "threadId": "thr-1", "model": "o5" })
        );
        t.reply("thread/settings/update", json!({}));
        assert!(matches!(
            &t.take_other()[..],
            [CodexAction::ControlDone {
                ticket: 1,
                outcome: ControlOutcome::Applied(_)
            }]
        ));

        t.core.control(2, Control::Fast(true), t.at(2.0));
        t.pump();
        // 回包之前到的回显不算（那是旧状态）。
        t.notify(
            "thread/settings/updated",
            json!({ "threadId": "thr-1", "threadSettings": { "serviceTier": "default" } }),
        );
        t.core.control(3, Control::Fast(false), t.at(2.1));
        t.pump();
        assert!(
            matches!(
                &t.take_other()[..],
                [CodexAction::ControlDone {
                    ticket: 3,
                    outcome: ControlOutcome::Rejected(_)
                }]
            ),
            "同时只等一次回显"
        );
        t.reply("thread/settings/update", json!({}));
        assert!(t.other.is_empty(), "空 {{}} 回包只是收到，不是生效");
        // 别的 thread 的回显不算。
        t.notify(
            "thread/settings/updated",
            json!({ "threadId": "thr-x", "threadSettings": { "serviceTier": "fast" } }),
        );
        assert!(t.other.is_empty());
        t.take_events();
        t.notify(
            "thread/settings/updated",
            json!({ "threadId": "thr-1", "threadSettings": { "serviceTier": "fast", "model": "o5" } }),
        );
        assert!(matches!(
            &t.take_other()[..],
            [CodexAction::ControlDone {
                ticket: 2,
                outcome: ControlOutcome::Applied(_)
            }]
        ));
        assert!(t.events.contains(&SessionEvent::Profile(Profile {
            model: Some("o5".into()),
            effort: None,
            fast: Some(true),
        })));
    }

    #[test]
    fn fast_switch_without_an_echo_or_with_an_unknown_tier_is_not_confirmed() {
        let mut t = T::ready(None);
        t.core.control(1, Control::Fast(true), t.at(1.0));
        t.pump();
        t.reply("thread/settings/update", json!({}));
        t.core.on_tick(t.at(9.2));
        t.pump();
        assert!(matches!(
            &t.take_other()[..],
            [CodexAction::ControlDone {
                ticket: 1,
                outcome: ControlOutcome::NoConfirmation(_)
            }]
        ));
        t.core.control(2, Control::Fast(true), t.at(10.0));
        t.pump();
        t.reply("thread/settings/update", json!({}));
        t.notify(
            "thread/settings/updated",
            json!({ "threadId": "thr-1", "threadSettings": { "serviceTier": "turbo-2027" } }),
        );
        assert!(matches!(
            &t.take_other()[..],
            [CodexAction::ControlDone {
                ticket: 2,
                outcome: ControlOutcome::NoConfirmation(_)
            }]
        ));
    }

    #[test]
    fn exit_abandons_what_was_queued_and_reports_it_unless_we_stopped_it() {
        let mut t = T::ready(Some("开场"));
        t.core.send(1, "排着的", UntrustedContext::new(), t.at(1.0));
        t.pump();
        t.take_events();
        t.core.on_exit(ExitStatusKind::Signal(9), "", t.at(2.0));
        t.pump();
        let ev = t.take_events();
        let d = deliveries(&ev);
        assert_eq!(d.len(), 2, "在途的开场 + 排队的一条：{d:?}");
        assert!(
            d.iter()
                .all(|(_, o)| matches!(o, DeliveryOutcome::Abandoned(_)))
        );
        assert!(ev.iter().any(|e| matches!(e, SessionEvent::Health(Some(h)) if h.kind == HealthKind::PromptUndelivered)));
        assert!(ev.contains(&SessionEvent::Working(false)));
        assert!(matches!(
            ev.last(),
            Some(SessionEvent::Exited(ExitInfo {
                stopped_by_user: false,
                ..
            }))
        ));
        // 退出之后再投：当场报没送到。
        t.core.send(2, "晚了", UntrustedContext::new(), t.at(3.0));
        t.pump();
        assert!(matches!(
            &deliveries(&t.take_events())[..],
            [(2, DeliveryOutcome::Abandoned(_))]
        ));

        let mut s = T::ready(None);
        s.core.send(1, "排着的", UntrustedContext::new(), s.at(1.0));
        s.core.stop(s.at(1.1));
        s.core.on_exit(ExitStatusKind::Signal(15), "", s.at(1.2));
        s.pump();
        assert!(
            !s.events
                .iter()
                .any(|e| matches!(e, SessionEvent::Health(Some(_)))),
            "人叫停的不报 health"
        );
        assert!(matches!(
            s.events.last(),
            Some(SessionEvent::Exited(ExitInfo {
                stopped_by_user: true,
                ..
            }))
        ));
    }

    #[test]
    fn a_lower_ranked_health_does_not_hide_a_quota_problem() {
        let mut t = T::ready(None);
        t.notify(
            "account/rateLimits/updated",
            json!({ "rateLimits": { "rateLimitReachedType": "rate_limit_reached" } }),
        );
        t.notify(
            "error",
            json!({ "threadId": "thr-1", "turnId": "x", "willRetry": false,
                    "error": { "message": "stream closed" } }),
        );
        assert_eq!(t.core.health().unwrap().kind, HealthKind::UsageLimit);
        assert_eq!(t.core.state(), DerivedState::RateLimited);
    }

    #[test]
    fn an_invalid_recorded_id_is_not_resumed() {
        let mut t = T::new(Some("../../etc"), None);
        t.reply("initialize", json!({}));
        assert_eq!(t.count("thread/resume"), 0);
        assert_eq!(t.count("thread/start"), 1);
    }

    #[test]
    fn every_dropped_oversized_line_is_reported() {
        let mut t = T::ready(None);
        t.take_events();
        t.core.on_dropped_line(70_000_000, t.at(1.0));
        t.core.on_dropped_line(80_000_000, t.at(2.0));
        t.pump();
        let ev = t.take_events();
        assert_eq!(ev.len(), 2, "每次都说：丢掉的可能是某个回应：{ev:?}");
        assert!(matches!(&ev[1], SessionEvent::Notice(n) if n.contains("80000000")));
    }

    #[test]
    fn a_malformed_line_is_announced_once_and_skipped() {
        let mut t = T::ready(None);
        t.take_events();
        let now = t.at(1.0);
        t.core.on_line("this is not json", now);
        t.core.on_line("{\"also\": \"unclassifiable\"}", now);
        t.core.on_line("", now);
        t.pump();
        let ev = t.take_events();
        assert_eq!(ev.len(), 1, "{ev:?}");
        assert!(matches!(&ev[0], SessionEvent::Notice(n) if n.contains("this is not json")));
    }

    #[test]
    fn completed_items_become_events_and_transcript_text() {
        let mut t = T::ready(None);
        t.take_events();
        t.notify(
            "item/completed",
            json!({ "threadId": "thr-1", "turnId": "t1", "completedAtMs": 1,
                    "item": { "id": "c1", "type": "commandExecution", "command": "ls", "exitCode": 0 } }),
        );
        let ev = t.take_events();
        assert!(
            ev.contains(&SessionEvent::Working(true)),
            "条目活动也算在干活：{ev:?}"
        );
        assert!(ev.iter().any(|e| matches!(e, SessionEvent::Item { turn_id, item } if turn_id == "t1" && item["command"] == "ls")));
        assert_eq!(t.core.transcript_text(5), "$ ls（exit 0）");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-exec --lib codex::session`
Expected: 编译失败，报找不到 `CodexCore` / `Opening` / `CodexAction` 等。

- [ ] **Step 3: 实现**

写在测试模块之前：

```rust
//! 一个 codex 会话的全部判断，不碰 IO（与 claude 的 `session::Core` 同一个形状）：
//! 喂一行 stdout / 一次定时拍 / 一条命令 / 账本结果 / 进程退出，吐出按顺序执行的
//! [`CodexAction`]。时间由调用方传入。
//!
//! - **握手**：`initialize` → `initialized` → 记过 thread 就 `thread/resume`（`excludeTurns`），
//!   失败退回 `thread/start` 并在 `Spawned` 之后补一条 `ResumeRefused`；拿到 thread id 先落账
//!   （[`CodexAction::Record`]），落账成功才发 `Spawned`、才放第一个回合。落账失败 = 启动失败。
//!   就绪之前产生的事件先攒着，保证第一条事件是 `Spawned`。
//! - **每一条服务端请求都回**（[`proto::answer_server_request`]），代拒的发 `Notice`（每种方法一次）。
//! - **回合**：一次只开一个；有回合在跑、正在压缩、或上一个 `turn/start` 还没回，正文就排队。
//! - **健康**：只认协议字段（[`proto::health_from`]）；正常完成的回合清掉过时的额度 / 失败 / 版本 health。
//! - **每个请求都有期限**（[`Rpc::expire`]），到期按失败处理，会话不挂。
use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::codex::proto::{self, Reply, ThreadOptions};
use crate::codex::rpc::{
    Incoming, ParseError, Rpc, RpcFailure, error_line, notification_line, parse_line, response_line,
};
use crate::codex::turns::{Signals, Transcript, TurnTracker};
use crate::delivery::Subject;
use crate::driver::{Control, ControlOutcome, UntrustedContext};
use crate::health::{Health, HealthKind, LaunchVerdict};
use crate::resume::valid_agent_session_id;
use crate::session::{
    DeliveryOutcome, DerivedState, ExitInfo, ExitStatusKind, Profile, SessionEvent, preview,
};

/// 期限与上限。
#[derive(Clone, Copy, Debug)]
pub struct CodexCoreConfig {
    /// `initialize` 的期限。
    pub initialize_timeout: Duration,
    /// `thread/start` / `thread/resume` 的期限（MCP server 起得慢时它会久一点）。
    pub thread_timeout: Duration,
    /// 其余请求（`turn/start`、`turn/interrupt`、设置、压缩）的期限。
    pub request_timeout: Duration,
    /// fast 切换：`thread/settings/update` 回了之后，等 `thread/settings/updated` 回显多久。
    pub settings_echo_timeout: Duration,
    /// 退出前这么久之内撞过额度 → `ExitInfo::hit_limit`。
    pub hit_limit_window: Duration,
    /// 结构化记录保留多少条。
    pub transcript_items: usize,
}

impl Default for CodexCoreConfig {
    fn default() -> Self {
        Self {
            initialize_timeout: Duration::from_secs(30),
            thread_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(30),
            settings_echo_timeout: Duration::from_secs(8),
            hit_limit_window: Duration::from_secs(600),
            transcript_items: 200,
        }
    }
}

/// 一条等着开回合的正文。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingTurn {
    pub id: u64,
    pub subject: Subject,
    pub text: String,
    pub context: UntrustedContext,
}

/// 起一个会话要的东西（驱动层给）。
#[derive(Clone, Debug)]
pub struct Opening {
    pub pid: u32,
    pub cwd: PathBuf,
    pub options: ThreadOptions,
    /// 账本里记过的 thread id（有就先试 `thread/resume`）。
    pub recorded: Option<String>,
    /// `initialize.clientInfo.version`。
    pub client_version: String,
    /// 开场任务（就绪后第一个回合）。
    pub brief: Option<PendingTurn>,
    /// 构造时刻的 Unix 秒（`currentTime/read` 按它推算，核心不看墙钟）。
    pub unix_now: i64,
}

/// 启动没成。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartFailure {
    /// 握手失败 / 超时 / 进程中途退出。
    Launch(Health),
    /// thread id 落账失败（起了也接不回来）。
    Ledger(String),
}

/// 交给驱动层按顺序执行的动作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodexAction {
    /// 往 stdin 写一行（不含换行）。
    Write(String),
    /// 把 thread id 落账，结果用 [`CodexCore::on_recorded`] 交回。
    Record(String),
    /// 一次 [`Control`] 的结果（`ticket` 是驱动层给的编号）。
    ControlDone {
        ticket: u64,
        outcome: ControlOutcome,
    },
    /// 启动失败：驱动层杀掉进程、把它变成 `StartError`。
    Failed(StartFailure),
    Event(SessionEvent),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    Initializing,
    Opening,
    Recording,
    Ready,
    Failed,
    Exited,
}

#[derive(Clone, Debug)]
enum Tag {
    Initialize,
    Resume(String),
    Start,
    TurnStart {
        id: u64,
        subject: Subject,
        text: String,
    },
    Interrupt,
    Settings {
        ticket: u64,
        op: Control,
    },
    Compact {
        ticket: Option<u64>,
    },
}

struct EchoWait {
    ticket: u64,
    want_fast: bool,
    deadline: Instant,
}

pub struct CodexCore {
    config: CodexCoreConfig,
    phase: Phase,
    pid: u32,
    cwd: PathBuf,
    options: ThreadOptions,
    recorded: Option<String>,
    created: Instant,
    unix_at_created: i64,
    rpc: Rpc<Tag>,
    thread_id: Option<String>,
    resumed: bool,
    fallback: Option<(String, String)>,
    opened_profile: Profile,
    tracker: TurnTracker,
    transcript: Transcript,
    queue: VecDeque<PendingTurn>,
    in_flight: bool,
    echo: Option<EchoWait>,
    /// 一次 fast 切换的 `thread/settings/update` 已发、还没回。
    fast_requested: bool,
    health: Option<Health>,
    quota_at: Option<Instant>,
    working: bool,
    stopped_by_user: bool,
    notified: BTreeSet<String>,
    early: Vec<SessionEvent>,
    actions: Vec<CodexAction>,
}

impl CodexCore {
    /// 建核心并发出 `initialize`。
    pub fn new(opening: Opening, config: CodexCoreConfig, now: Instant) -> Self {
        let mut queue = VecDeque::new();
        if let Some(brief) = opening.brief.filter(|b| !b.text.is_empty()) {
            queue.push_back(brief);
        }
        let mut core = Self {
            config,
            phase: Phase::Initializing,
            pid: opening.pid,
            cwd: opening.cwd,
            options: opening.options,
            // 账里的号非法（`resume::valid_agent_session_id` 不认）= 当作没记过，开新 thread。
            recorded: opening
                .recorded
                .map(|r| r.trim().to_string())
                .filter(|r| valid_agent_session_id(r)),
            created: now,
            unix_at_created: opening.unix_now,
            rpc: Rpc::default(),
            thread_id: None,
            resumed: false,
            fallback: None,
            opened_profile: Profile::default(),
            tracker: TurnTracker::new(""),
            transcript: Transcript::new(config.transcript_items),
            queue,
            in_flight: false,
            echo: None,
            fast_requested: false,
            health: None,
            quota_at: None,
            working: false,
            stopped_by_user: false,
            notified: BTreeSet::new(),
            early: Vec::new(),
            actions: Vec::new(),
        };
        let params = proto::initialize_params(&opening.client_version);
        core.request(
            Tag::Initialize,
            "initialize",
            &params,
            now,
            config.initialize_timeout,
        );
        core
    }

    pub fn take_actions(&mut self) -> Vec<CodexAction> {
        std::mem::take(&mut self.actions)
    }

    /// 下一个要醒来的时刻（最早的请求期限 / fast 回显期限）。
    pub fn next_wakeup(&self) -> Option<Instant> {
        let echo = self.echo.as_ref().map(|e| e.deadline);
        match (self.rpc.next_deadline(), echo) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.phase == Phase::Ready
    }

    pub fn is_running(&self) -> bool {
        !matches!(self.phase, Phase::Exited | Phase::Failed)
    }

    pub fn thread_id(&self) -> Option<&str> {
        self.thread_id.as_deref()
    }

    pub fn health(&self) -> Option<&Health> {
        self.health.as_ref()
    }

    /// 在干活：有回合 / 在压缩 / `turn/start` 还没回 / 刚见到条目活动。
    pub fn is_working(&self) -> bool {
        self.working
    }

    pub fn state(&self) -> DerivedState {
        if self
            .health
            .as_ref()
            .is_some_and(|h| h.kind == HealthKind::LaunchFailed)
        {
            return DerivedState::LaunchFailed;
        }
        if !self.is_running() {
            return DerivedState::Exited;
        }
        if let Some(h) = &self.health {
            return if h.kind.is_quota() {
                DerivedState::RateLimited
            } else {
                DerivedState::Error
            };
        }
        if self.working {
            DerivedState::Working
        } else {
            DerivedState::Idle
        }
    }

    /// 结构化记录的文本（`screen_text` 的 codex 版）。
    pub fn transcript_text(&self, max_lines: usize) -> String {
        self.transcript.text(max_lines)
    }

    /// 喂一行 stdout（不含换行）。
    pub fn on_line(&mut self, line: &str, now: Instant) {
        if line.trim().is_empty() {
            return;
        }
        match parse_line(line) {
            Ok(Incoming::Response { id, result }) => {
                if let Some((tag, _method, result)) = self.rpc.resolve(&id, result) {
                    self.on_result(tag, result, now);
                }
            }
            Ok(Incoming::Request { id, method, params }) => {
                let unix = self.unix_at_created
                    + now.saturating_duration_since(self.created).as_secs() as i64;
                let outcome = proto::answer_server_request(&method, &params, unix);
                let reply = match &outcome.reply {
                    Reply::Result(v) => response_line(&id, v),
                    Reply::Error { code, message } => error_line(&id, *code, message),
                };
                self.actions.push(CodexAction::Write(reply));
                if outcome.auth_required {
                    self.set_health(Some(proto::auth_required_health()), now);
                }
                if let Some(notice) = outcome.notice
                    && self.notified.insert(method)
                {
                    self.event(SessionEvent::Notice(notice));
                }
            }
            Ok(Incoming::Notification { method, params }) => {
                self.on_notification(&method, &params, now)
            }
            Err(e) => {
                if self.notified.insert("<malformed>".to_string()) {
                    let what = match e {
                        ParseError::NotJson(l) | ParseError::Unclassifiable(l) => l,
                    };
                    self.event(SessionEvent::Notice(format!(
                        "codex app-server 输出了一行认不出的内容（已跳过，之后同类不再提示）：{what}"
                    )));
                }
            }
        }
        self.after_input(now);
    }

    /// 一行超过上限被整行丢掉了（`transport::MAX_LINE_BYTES`）。每次都说：丢掉的可能正是
    /// 某个请求的回应（那个请求随后按超时处理）。
    pub fn on_dropped_line(&mut self, bytes: usize, now: Instant) {
        self.event(SessionEvent::Notice(format!(
            "codex app-server 的一行输出有 {bytes} 字节，超过上限，已丢弃；如果它是某个请求的回应，那个请求会按超时处理。"
        )));
        self.after_input(now);
    }

    /// 账本结果（[`CodexAction::Record`] 的回应）。
    pub fn on_recorded(&mut self, result: Result<(), String>, now: Instant) {
        if self.phase != Phase::Recording {
            return;
        }
        match result {
            Ok(()) => self.become_ready(now),
            Err(e) => self.fail(StartFailure::Ledger(e)),
        }
    }

    /// 定时拍：请求到期、fast 回显到期。
    pub fn on_tick(&mut self, now: Instant) {
        for (tag, method, failure) in self.rpc.expire(now) {
            let _ = method;
            self.on_result(tag, Err(failure), now);
        }
        if let Some(e) = &self.echo
            && e.deadline <= now
        {
            let ticket = e.ticket;
            self.echo = None;
            self.actions.push(CodexAction::ControlDone {
                ticket,
                outcome: ControlOutcome::NoConfirmation(
                    "thread/settings/update 已接受，但没等到 thread/settings/updated 回显，实际档位未知"
                        .into(),
                ),
            });
        }
        self.after_input(now);
    }

    /// 投一段正文：排队，空闲时开一个回合。会话已经不在了就当场报没送到。
    pub fn send(&mut self, id: u64, text: &str, context: UntrustedContext, now: Instant) {
        if text.is_empty() {
            return;
        }
        let pending = PendingTurn {
            id,
            subject: Subject::Steer,
            text: text.to_string(),
            context,
        };
        if !self.is_running() {
            let detail = format!(
                "一条指令没送出去：session 已经不在跑了。开头是「{}」。需要重起或改派。",
                preview(text)
            );
            self.delivery(&pending, DeliveryOutcome::Abandoned(detail.clone()));
            self.set_health(
                Some(Health::new(HealthKind::PromptUndelivered, detail)),
                now,
            );
            return;
        }
        self.queue.push_back(pending);
        self.after_input(now);
    }

    /// 打断进行中的回合（没有就什么都不做）。
    pub fn interrupt(&mut self, now: Instant) {
        if self.phase != Phase::Ready {
            return;
        }
        let (Some(thread), Some(turn)) = (self.thread_id.clone(), self.tracker.active_turn())
        else {
            return;
        };
        let params = proto::turn_interrupt_params(&thread, turn);
        let timeout = self.config.request_timeout;
        self.request(Tag::Interrupt, "turn/interrupt", &params, now, timeout);
    }

    /// 原始按键：codex 没有终端。单独一个 Esc = 打断，其余丢弃并告知。
    pub fn raw(&mut self, bytes: &[u8], now: Instant) {
        if bytes == [0x1b] {
            self.interrupt(now);
        } else if !bytes.is_empty() {
            self.event(SessionEvent::Notice(format!(
                "codex 会话没有终端，丢弃了 {} 字节原始输入（只认单独一个 Esc = 打断）",
                bytes.len()
            )));
        }
        self.after_input(now);
    }

    /// 会话控制。结果经 [`CodexAction::ControlDone`] 交回（`ticket` 原样）。
    pub fn control(&mut self, ticket: u64, op: Control, now: Instant) {
        let reject = |why: &str| CodexAction::ControlDone {
            ticket,
            outcome: ControlOutcome::Rejected(why.to_string()),
        };
        let Some(thread) = self
            .thread_id
            .clone()
            .filter(|_| self.phase == Phase::Ready)
        else {
            self.actions.push(reject("codex 会话还没就绪或已经结束"));
            return;
        };
        let timeout = self.config.request_timeout;
        match op {
            Control::Compact => {
                if self.tracker.is_busy() || self.in_flight {
                    self.actions
                        .push(reject("Codex 正在执行回合或正在压缩，空闲时再压"));
                    return;
                }
                let params = proto::compact_params(&thread);
                self.request(
                    Tag::Compact {
                        ticket: Some(ticket),
                    },
                    "thread/compact/start",
                    &params,
                    now,
                    timeout,
                );
                self.tracker.begin_compaction();
            }
            op => {
                let fast = matches!(op, Control::Fast(_));
                if fast && (self.fast_requested || self.echo.is_some()) {
                    self.actions
                        .push(reject("上一次 fast 切换还在等服务端回显"));
                    return;
                }
                let params =
                    proto::settings_update_params(&thread, &op).expect("Compact 已在上面处理");
                self.request(
                    Tag::Settings { ticket, op },
                    "thread/settings/update",
                    &params,
                    now,
                    timeout,
                );
                self.fast_requested |= fast;
            }
        }
        self.after_input(now);
    }

    /// 额度重置唤醒到点：清掉额度类 health（codex 的额度判断全来自协议，没有扫描器要重新武装）。
    pub fn clear_quota_health(&mut self, now: Instant) {
        if self.health.as_ref().is_some_and(|h| h.kind.is_quota()) {
            self.set_health(None, now);
        }
    }

    /// 我们要停它了（驱动层随后发信号）。没投出去的不报 health——是人叫停的。
    pub fn stop(&mut self, now: Instant) {
        if !self.is_running() {
            return;
        }
        self.stopped_by_user = true;
        self.abandon("session 被停掉", false, now);
        self.after_input(now);
    }

    /// 子进程退出。`stderr_tail` 是 stderr 最后一段（握手失败时带进说明）。
    pub fn on_exit(&mut self, status: ExitStatusKind, stderr_tail: &str, now: Instant) {
        if matches!(self.phase, Phase::Exited | Phase::Failed) {
            return;
        }
        let ready = self.phase == Phase::Ready;
        for (tag, _method, _) in self.rpc.fail_all() {
            match tag {
                Tag::TurnStart { id, subject, text } => {
                    self.in_flight = false;
                    self.queue.push_front(PendingTurn {
                        id,
                        subject,
                        text,
                        context: UntrustedContext::new(),
                    });
                }
                Tag::Settings { ticket, .. }
                | Tag::Compact {
                    ticket: Some(ticket),
                } => self.actions.push(CodexAction::ControlDone {
                    ticket,
                    outcome: ControlOutcome::Rejected("codex 进程已退出".into()),
                }),
                _ => {}
            }
        }
        self.fast_requested = false;
        if let Some(e) = self.echo.take() {
            self.actions.push(CodexAction::ControlDone {
                ticket: e.ticket,
                outcome: ControlOutcome::Rejected("codex 进程已退出".into()),
            });
        }
        if !ready {
            let underlying = format!("{}{}", exit_words(&status), tail_words(stderr_tail));
            self.fail(StartFailure::Launch(launch_health(
                LaunchVerdict::DiedSilently,
                &underlying,
            )));
            return;
        }
        let stopped = self.stopped_by_user;
        self.abandon("进程退出", !stopped, now);
        self.tracker.reset();
        self.sync_working();
        let hit_limit = self
            .quota_at
            .is_some_and(|at| now.saturating_duration_since(at) <= self.config.hit_limit_window);
        self.phase = Phase::Exited;
        self.event(SessionEvent::Exited(ExitInfo {
            status,
            stopped_by_user: stopped,
            hit_limit,
        }));
    }

    // ---- 内部 ----

    fn request(&mut self, tag: Tag, method: &str, params: &Value, now: Instant, timeout: Duration) {
        let line = self.rpc.request(tag, method, params, now, timeout);
        self.actions.push(CodexAction::Write(line));
    }

    fn event(&mut self, e: SessionEvent) {
        if self.phase == Phase::Ready || self.phase == Phase::Exited {
            self.actions.push(CodexAction::Event(e));
        } else {
            self.early.push(e);
        }
    }

    fn fail(&mut self, failure: StartFailure) {
        if let StartFailure::Launch(h) = &failure {
            self.health = Some(h.clone());
        }
        self.phase = Phase::Failed;
        self.early.clear();
        self.actions.push(CodexAction::Failed(failure));
    }

    fn open_thread(&mut self, now: Instant) {
        let timeout = self.config.thread_timeout;
        match self.recorded.clone() {
            Some(id) if self.fallback.is_none() => {
                let params = proto::thread_resume_params(&id, &self.cwd, &self.options);
                self.request(Tag::Resume(id), "thread/resume", &params, now, timeout);
            }
            _ => {
                let params = proto::thread_start_params(&self.cwd, &self.options);
                self.request(Tag::Start, "thread/start", &params, now, timeout);
            }
        }
        self.phase = Phase::Opening;
    }

    fn opened(&mut self, thread: String, resumed: bool, result: &Value, now: Instant) {
        self.opened_profile = proto::profile_of(result);
        self.tracker = TurnTracker::new(thread.clone());
        self.thread_id = Some(thread.clone());
        self.resumed = resumed;
        self.phase = Phase::Recording;
        if self.recorded.as_deref() == Some(thread.as_str()) {
            // 续上的就是账本里那条：不用再写。
            self.become_ready(now);
        } else {
            self.actions.push(CodexAction::Record(thread));
        }
    }

    fn become_ready(&mut self, now: Instant) {
        self.phase = Phase::Ready;
        let thread = self.thread_id.clone().unwrap_or_default();
        self.actions.push(CodexAction::Event(SessionEvent::Spawned {
            pid: self.pid,
            agent_session_id: thread,
            resumed: self.resumed,
        }));
        if let Some((id, said)) = self.fallback.clone() {
            self.actions
                .push(CodexAction::Event(SessionEvent::ResumeRefused { id, said }));
        }
        let profile = self.opened_profile.clone();
        if profile != Profile::default() {
            self.actions
                .push(CodexAction::Event(SessionEvent::Profile(profile)));
        }
        let early = std::mem::take(&mut self.early);
        self.actions
            .extend(early.into_iter().map(CodexAction::Event));
        self.after_input(now);
    }

    fn on_result(&mut self, tag: Tag, result: Result<Value, RpcFailure>, now: Instant) {
        match tag {
            Tag::Initialize => match result {
                Ok(_) => {
                    self.actions
                        .push(CodexAction::Write(notification_line("initialized", None)));
                    self.open_thread(now);
                }
                Err(f) => self.handshake_failed("initialize", &f),
            },
            Tag::Resume(id) => match result {
                Ok(v) => {
                    let thread = proto::thread_id_of(&v).unwrap_or(id);
                    self.opened(thread, true, &v, now);
                }
                Err(RpcFailure::Disconnected) => {}
                Err(f) => {
                    self.fallback = Some((id, f.to_string()));
                    self.open_thread(now);
                }
            },
            Tag::Start => match result {
                Ok(v) => match proto::thread_id_of(&v) {
                    Some(thread) => self.opened(thread, false, &v, now),
                    None => self.fail(StartFailure::Launch(launch_health(
                        LaunchVerdict::SpawnFailed,
                        "thread/start 的回包里没有 thread.id",
                    ))),
                },
                Err(RpcFailure::Disconnected) => {}
                Err(f) => self.handshake_failed("thread/start", &f),
            },
            Tag::TurnStart { id, subject, text } => {
                self.in_flight = false;
                let pending = PendingTurn {
                    id,
                    subject,
                    text,
                    context: UntrustedContext::new(),
                };
                match result {
                    Ok(v) => {
                        if let Some(turn) = v.pointer("/turn/id").and_then(Value::as_str) {
                            self.tracker.on_turn_accepted(turn);
                        }
                        if self.health.as_ref().map(|h| h.kind) == Some(undelivered_kind(subject)) {
                            self.set_health(None, now);
                        }
                        self.delivery(&pending, DeliveryOutcome::Delivered);
                    }
                    Err(f) => {
                        let detail = format!(
                            "{}没送进 codex：turn/start {f}。开头是「{}」。",
                            subject_noun(subject),
                            preview(&pending.text)
                        );
                        self.delivery(&pending, DeliveryOutcome::Undelivered(detail.clone()));
                        self.set_health(Some(Health::new(undelivered_kind(subject), detail)), now);
                    }
                }
            }
            Tag::Interrupt => {
                if let Err(f) = result {
                    self.event(SessionEvent::Notice(format!(
                        "打断没成功：turn/interrupt {f}"
                    )));
                }
            }
            Tag::Settings { ticket, op } => {
                if matches!(op, Control::Fast(_)) {
                    self.fast_requested = false;
                }
                self.on_settings_result(ticket, op, result, now);
            }
            Tag::Compact { ticket } => match result {
                Ok(_) => {
                    if let Some(ticket) = ticket {
                        self.actions.push(CodexAction::ControlDone {
                            ticket,
                            outcome: ControlOutcome::Applied("已开始压缩".into()),
                        });
                    }
                }
                Err(f) => {
                    self.tracker.compaction_failed();
                    let why = format!("Codex 压缩没启动：{f}");
                    if let Some(ticket) = ticket {
                        self.actions.push(CodexAction::ControlDone {
                            ticket,
                            outcome: ControlOutcome::Rejected(why.clone()),
                        });
                    }
                    self.event(SessionEvent::Notice(why));
                }
            },
        }
    }

    fn on_settings_result(
        &mut self,
        ticket: u64,
        op: Control,
        result: Result<Value, RpcFailure>,
        now: Instant,
    ) {
        match result {
            Ok(_) => match op {
                Control::Fast(want_fast) => {
                    self.echo = Some(EchoWait {
                        ticket,
                        want_fast,
                        deadline: now + self.config.settings_echo_timeout,
                    });
                }
                other => self.actions.push(CodexAction::ControlDone {
                    ticket,
                    outcome: ControlOutcome::Applied(format!(
                        "thread/settings/update 已接受：{other:?}"
                    )),
                }),
            },
            Err(f) => self.actions.push(CodexAction::ControlDone {
                ticket,
                outcome: ControlOutcome::Rejected(f.to_string()),
            }),
        }
    }

    fn handshake_failed(&mut self, step: &str, f: &RpcFailure) {
        let verdict = match f {
            RpcFailure::TimedOut(_) => LaunchVerdict::Stalled,
            RpcFailure::Disconnected => return,
            RpcFailure::Server(_) => LaunchVerdict::SpawnFailed,
        };
        self.fail(StartFailure::Launch(launch_health(
            verdict,
            &format!("{step}：{f}"),
        )));
    }

    fn on_notification(&mut self, method: &str, params: &Value, now: Instant) {
        if let Some(h) = proto::health_from(method, params)
            && self.health.as_ref() != Some(&h)
        {
            self.set_health(Some(h), now);
        }
        if method == "thread/settings/updated" {
            self.on_settings_echo(params);
        }
        if self.thread_id.is_none() {
            return;
        }
        let s: Signals = self.tracker.on_notification(method, params);
        if let Some((turn_id, item)) = s.item {
            self.transcript.push(turn_id.clone(), item.clone());
            self.event(SessionEvent::Item { turn_id, item });
        }
        if let Some(last) = s.turn_ended {
            self.event(SessionEvent::TurnEnded {
                last_agent_message: last,
            });
        }
        if s.completed_ok
            && self.health.as_ref().is_some_and(|h| {
                h.kind.is_quota()
                    || matches!(
                        h.kind,
                        HealthKind::TurnFailed | HealthKind::VersionIncompatible
                    )
            })
        {
            // 一个后来正常完成的回合，就是「旧的额度 / 失败标记已过时」的第一手证据。
            self.set_health(None, now);
        }
        if let Some(problem) = s.compaction_problem {
            self.event(SessionEvent::Notice(problem));
        }
        if s.compact
            && let Some(thread) = self.thread_id.clone()
        {
            let params = proto::compact_params(&thread);
            let timeout = self.config.request_timeout;
            self.request(
                Tag::Compact { ticket: None },
                "thread/compact/start",
                &params,
                now,
                timeout,
            );
            self.tracker.begin_compaction();
        }
    }

    fn on_settings_echo(&mut self, params: &Value) {
        if params.get("threadId").and_then(Value::as_str) != self.thread_id.as_deref() {
            return;
        }
        let Some(settings) = params.get("threadSettings") else {
            return;
        };
        let s = |k: &str| {
            settings
                .get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let tier = s("serviceTier");
        let fast = tier.as_deref().and_then(proto::fast_mode_for_tier);
        self.event(SessionEvent::Profile(Profile {
            model: s("model"),
            effort: s("effort"),
            fast,
        }));
        if let Some(wait) = self.echo.take() {
            let outcome = match (fast, &tier) {
                (Some(f), _) if f == wait.want_fast => {
                    ControlOutcome::Applied(format!("服务端确认档位 {}", tier.unwrap_or_default()))
                }
                (Some(_), Some(t)) => {
                    ControlOutcome::Rejected(format!("Codex 返回的实际档位是 {t}"))
                }
                _ => ControlOutcome::NoConfirmation(format!(
                    "服务端回显了认不出的档位 {:?}，实际状态未知",
                    tier
                )),
            };
            self.actions.push(CodexAction::ControlDone {
                ticket: wait.ticket,
                outcome,
            });
        }
    }

    /// 每次输入之后：能开回合就开，然后对齐干活边沿。
    fn after_input(&mut self, now: Instant) {
        self.start_next(now);
        self.sync_working();
    }

    fn start_next(&mut self, now: Instant) {
        if self.phase != Phase::Ready || self.in_flight || self.tracker.is_busy() {
            return;
        }
        let Some(next) = self.queue.pop_front() else {
            return;
        };
        let thread = self.thread_id.clone().unwrap_or_default();
        let params = proto::turn_start_params(&thread, &next.text, &next.context);
        let timeout = self.config.request_timeout;
        self.request(
            Tag::TurnStart {
                id: next.id,
                subject: next.subject,
                text: next.text,
            },
            "turn/start",
            &params,
            now,
            timeout,
        );
        self.in_flight = true;
    }

    fn sync_working(&mut self) {
        let working = self.phase == Phase::Ready
            && (self.in_flight || self.tracker.is_busy() || self.tracker.is_working());
        if working != self.working {
            self.working = working;
            self.event(SessionEvent::Working(working));
        }
    }

    /// 换 health。低档不盖高档（[`HealthKind::rank`]，与 claude 核心同一张表）；`None` 总能清。
    fn set_health(&mut self, health: Option<Health>, now: Instant) {
        match &health {
            Some(h) => {
                if h.kind.is_quota() {
                    self.quota_at = Some(now);
                }
                if self
                    .health
                    .as_ref()
                    .is_some_and(|cur| cur.kind.rank() > h.kind.rank())
                {
                    return;
                }
            }
            None => self.quota_at = None,
        }
        if self.health == health {
            return;
        }
        self.health = health.clone();
        self.event(SessionEvent::Health(health));
    }

    fn delivery(&mut self, p: &PendingTurn, outcome: DeliveryOutcome) {
        self.event(SessionEvent::Delivery {
            id: p.id,
            subject: p.subject,
            outcome,
        });
    }

    fn abandon(&mut self, reason: &str, report: bool, now: Instant) {
        let lost: Vec<PendingTurn> = self.queue.drain(..).collect();
        for p in &lost {
            self.delivery(p, DeliveryOutcome::Abandoned(reason.to_string()));
        }
        let steers: Vec<&PendingTurn> = lost
            .iter()
            .filter(|p| p.subject == Subject::Steer)
            .collect();
        if report && let Some(first) = steers.first() {
            let detail = format!(
                "{} 条指令没送到就{reason}了，第一条开头是「{}」。这些指令没有被任何人接收，需要重发。",
                steers.len(),
                preview(&first.text)
            );
            self.set_health(
                Some(Health::new(HealthKind::PromptUndelivered, detail)),
                now,
            );
        }
    }
}

fn undelivered_kind(subject: Subject) -> HealthKind {
    match subject {
        Subject::Brief => HealthKind::BriefUndelivered,
        Subject::Steer => HealthKind::PromptUndelivered,
    }
}

fn subject_noun(subject: Subject) -> &'static str {
    match subject {
        Subject::Brief => "开场任务",
        Subject::Steer => "这条指令",
    }
}

fn exit_words(status: &ExitStatusKind) -> String {
    match status {
        ExitStatusKind::Code(c) => format!("exit {c}"),
        ExitStatusKind::Signal(s) => format!("signal {s}"),
    }
}

fn tail_words(stderr_tail: &str) -> String {
    let t = stderr_tail.trim();
    if t.is_empty() {
        String::new()
    } else {
        format!("；stderr 最后几行：{t}")
    }
}

/// codex 拉起失败的 health（措辞是 codex 的，`health::launch_failure` 是 claude 的）。
pub fn launch_health(verdict: LaunchVerdict, underlying: &str) -> Health {
    let detail = match verdict {
        LaunchVerdict::SpawnFailed => format!(
            "Codex app-server 没能启动（{underlying}）——这个 session 从头到尾没跑起来，活等于没派出去，请改派或排查后重起。"
        ),
        LaunchVerdict::DiedSilently => format!(
            "Codex app-server 握手完成前就退出了（{underlying}）——常见于未登录 / 额度耗尽 / 版本不兼容 / 工作目录不可用，请核实后重起。"
        ),
        _ => format!(
            "Codex app-server 起来了但握手没完成（{underlying}）——半死状态，已杀掉，请改派或重起。"
        ),
    };
    Health {
        kind: HealthKind::LaunchFailed,
        detail,
        launch_verdict: Some(verdict),
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-exec --lib codex::session`
Expected: `24 passed`。

- [ ] **Step 5: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add crates/exec/src/codex/mod.rs crates/exec/src/codex/session.rs
git commit -m "crewd-exec: sans-IO codex session core (handshake, ledger-first ready, turns, deadlines, health)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

- [ ] **Step 6: 验这几把尺子会红**

每刀单独做、做完 `git checkout -- crates/exec/src/codex/session.rs` 改回：
1. `set_health` 里 `.is_some_and(|cur| cur.kind.rank() > h.kind.rank())` 临时改成 `.is_some_and(|cur| cur.kind.rank() > 99)` → `cargo test -p crewd-exec --lib codex::session` → 红 1 条：`a_lower_ranked_health_does_not_hide_a_quota_problem`。
2. `opened` 里 `self.actions.push(CodexAction::Record(thread));` 之后加一行 `self.become_ready(now);` → 红 2 条：`fresh_start_records_the_thread_before_spawned_and_before_the_first_turn`、`ledger_failure_fails_the_start_and_no_turn_is_opened`（落账之前就就绪）。

`git status` 干净。

---

### Task 6: 起进程与管道（`codex::transport`）

setsid、白名单环境再剥计费 key、工作目录不存在即失败、三根管子：stdout 有上限按行读（取消安全，能放进 `select!`）、stderr 一直排空只留尾巴、stdin 走写任务（裁定 10–12）。

**Files:**
- Create: `crewd/crates/exec/src/codex/transport.rs`
- Modify: `crewd/crates/exec/src/codex/mod.rs`

**Interfaces:**
- Consumes: M2b `env::build_env`（只在测试里）；rustix `setsid`。
- Produces（`crewd_exec::codex::transport`）:
  - 常量 `BILLING_KEYS: [&str; 3]`、`MAX_LINE_BYTES: usize`（64MiB）、`STDERR_TAIL_BYTES: usize`（4096）；`pub fn strip_billing_keys(&mut BTreeMap<String, String>)`
  - `#[derive(Clone, Debug)] pub struct CodexProcessSpec { pub program: PathBuf, pub args: Vec<String>, pub env: BTreeMap<String, String>, pub cwd: PathBuf }`
  - `pub struct CodexProcess { pub child: tokio::process::Child, pub pid: u32, pub stdout: LineReader<tokio::io::BufReader<ChildStdout>>, pub writer: mpsc::UnboundedSender<String>, pub writer_task: JoinHandle<()>, pub stderr: StderrTail, pub stderr_task: JoinHandle<()> }`；`pub fn spawn_codex(&CodexProcessSpec) -> io::Result<CodexProcess>`
  - `#[derive(Clone, Debug, Default)] pub struct StderrTail`：`push(&self, &[u8])`、`text(&self) -> String`
  - `pub enum Line { Text(String), TooLong { bytes: usize } }`；`pub struct LineReader<R>`：`new(R, max: usize) -> Self`、`async next_line(&mut self) -> io::Result<Option<Line>>`（`R: AsyncBufRead + Unpin`）

- [ ] **Step 1: 写失败的测试**

`crates/exec/src/codex/mod.rs`：`pub mod session;` 之后加 `pub mod transport;`。

`crates/exec/src/codex/transport.rs`：先只放测试模块（Review Focus 2 的单元一半在这里）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    fn reader(bytes: &'static [u8], max: usize) -> LineReader<&'static [u8]> {
        LineReader::new(bytes, max)
    }

    #[tokio::test]
    async fn lines_are_split_on_newline_and_a_trailing_half_line_is_kept() {
        let mut r = reader(b"{\"a\":1}\r\n\n{\"b\":2}\npartial", 1024);
        assert_eq!(
            r.next_line().await.unwrap(),
            Some(Line::Text("{\"a\":1}\r".into()))
        );
        assert_eq!(
            r.next_line().await.unwrap(),
            Some(Line::Text(String::new()))
        );
        assert_eq!(
            r.next_line().await.unwrap(),
            Some(Line::Text("{\"b\":2}".into()))
        );
        assert_eq!(
            r.next_line().await.unwrap(),
            Some(Line::Text("partial".into()))
        );
        assert_eq!(r.next_line().await.unwrap(), None);
    }

    #[tokio::test]
    async fn an_overlong_line_is_dropped_whole_and_the_next_one_still_reads() {
        let mut r = reader(b"0123456789ABCDEF\nok\n0123456789", 8);
        assert_eq!(
            r.next_line().await.unwrap(),
            Some(Line::TooLong { bytes: 16 })
        );
        assert_eq!(r.next_line().await.unwrap(), Some(Line::Text("ok".into())));
        assert_eq!(
            r.next_line().await.unwrap(),
            Some(Line::TooLong { bytes: 10 })
        );
        assert_eq!(r.next_line().await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_line_split_across_many_small_writes_comes_out_whole() {
        let (mut w, r) = tokio::io::duplex(8);
        let mut r = LineReader::new(tokio::io::BufReader::with_capacity(3, r), 1024);
        let writer = tokio::spawn(async move {
            for chunk in ["{\"me", "thod\":\"tu", "rn/started\"}\n{\"x\"", ":1}\n"] {
                w.write_all(chunk.as_bytes()).await.unwrap();
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
        assert_eq!(
            r.next_line().await.unwrap(),
            Some(Line::Text("{\"method\":\"turn/started\"}".into()))
        );
        assert_eq!(
            r.next_line().await.unwrap(),
            Some(Line::Text("{\"x\":1}".into()))
        );
        writer.await.unwrap();
        assert_eq!(r.next_line().await.unwrap(), None);
    }

    #[tokio::test]
    async fn being_cancelled_mid_line_loses_no_bytes() {
        let (mut w, r) = tokio::io::duplex(64);
        let mut r = LineReader::new(tokio::io::BufReader::new(r), 1024);
        w.write_all(b"{\"half\":").await.unwrap();
        // 读到半行时被取消（select! 里别的分支先到）。
        let cancelled = tokio::time::timeout(Duration::from_millis(50), r.next_line()).await;
        assert!(cancelled.is_err());
        w.write_all(b"true}\n").await.unwrap();
        assert_eq!(
            r.next_line().await.unwrap(),
            Some(Line::Text("{\"half\":true}".into()))
        );
    }

    #[test]
    fn billing_keys_are_stripped_even_when_passed_explicitly() {
        let mut extra = BTreeMap::new();
        extra.insert("OPENAI_API_KEY".to_string(), "sk-live".to_string());
        extra.insert("CODEX_API_KEY".to_string(), "x".to_string());
        extra.insert("FAKE_CODEX_MODE".to_string(), "echo".to_string());
        let mut env = crate::env::build_env([("HOME", "/h")], "/usr/bin:/bin", &extra);
        strip_billing_keys(&mut env);
        assert!(!env.contains_key("OPENAI_API_KEY"));
        assert!(!env.contains_key("CODEX_API_KEY"));
        assert_eq!(env.get("FAKE_CODEX_MODE").map(String::as_str), Some("echo"));
        assert_eq!(env.get("HOME").map(String::as_str), Some("/h"));
    }

    fn sh(script: &str, cwd: PathBuf) -> CodexProcessSpec {
        let mut env = BTreeMap::new();
        env.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
        CodexProcessSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            env,
            cwd,
        }
    }

    #[tokio::test]
    async fn the_child_leads_its_own_session_without_a_terminal() {
        let mut p = spawn_codex(&sh(
            "echo pid=$$; echo pgid=$(ps -o pgid= -p $$ | tr -d ' '); \
             echo tty=$(ps -o tty= -p $$ | tr -d ' ')",
            std::env::temp_dir(),
        ))
        .unwrap();
        let mut got = Vec::new();
        while let Some(Line::Text(l)) = p.stdout.next_line().await.unwrap() {
            got.push(l);
        }
        assert_eq!(got[0], format!("pid={}", p.pid));
        assert_eq!(got[1], format!("pgid={}", p.pid), "自成进程组");
        assert!(
            got[2] == "tty=??" || got[2] == "tty=?",
            "没有控制终端：{}",
            got[2]
        );
        p.child.wait().await.unwrap();
    }

    #[tokio::test]
    async fn a_flood_on_stderr_cannot_stall_stdout_and_its_tail_is_kept() {
        // 1 MiB 写进 stderr：没人读的话子进程卡在第 64KB，stdout 那行永远不来。
        let mut p = spawn_codex(&sh(
            "head -c 1048576 /dev/zero | tr '\\0' 'x' >&2; echo 'last words' >&2; echo done",
            std::env::temp_dir(),
        ))
        .unwrap();
        let line = tokio::time::timeout(Duration::from_secs(10), p.stdout.next_line())
            .await
            .expect("stderr 没被排空，子进程卡住了")
            .unwrap();
        assert_eq!(line, Some(Line::Text("done".into())));
        p.child.wait().await.unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(2), &mut p.stderr_task).await;
        let tail = p.stderr.text();
        assert!(
            tail.ends_with("last words"),
            "{}",
            &tail[tail.len().saturating_sub(40)..]
        );
        assert!(tail.len() <= STDERR_TAIL_BYTES);
    }

    #[tokio::test]
    async fn lines_written_reach_stdin_in_order() {
        let mut p = spawn_codex(&sh("head -n 2", std::env::temp_dir())).unwrap();
        p.writer.send("{\"id\":0}".into()).unwrap();
        p.writer.send("{\"id\":1}".into()).unwrap();
        assert_eq!(
            p.stdout.next_line().await.unwrap(),
            Some(Line::Text("{\"id\":0}".into()))
        );
        assert_eq!(
            p.stdout.next_line().await.unwrap(),
            Some(Line::Text("{\"id\":1}".into()))
        );
        p.child.wait().await.unwrap();
        // 对端已经退出：继续写不会把我们打死（EPIPE 只是让写任务停下）。
        let _ = p.writer.send("{\"id\":2}".into());
        let _ = tokio::time::timeout(Duration::from_secs(2), &mut p.writer_task).await;
    }

    #[tokio::test]
    async fn a_missing_workdir_is_a_spawn_error() {
        let err = spawn_codex(&sh("true", "/nonexistent/crewd-m2c".into()))
            .err()
            .expect("必须报错");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(err.to_string().contains("工作目录"));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-exec --lib codex::transport`
Expected: 编译失败，报找不到 `LineReader` / `spawn_codex` / `strip_billing_keys` 等。

- [ ] **Step 3: 实现**

写在测试模块之前：

```rust
//! 起 `codex app-server` 子进程并接好三根管子（Swift `CodexAppServerConnection` 的移植）。
//!
//! - **新会话**（setsid）：自成进程组、没有控制终端——codex 跑的工具命令碰 `/dev/tty` 会立刻
//!   失败，而不是被 SIGTTIN 停住、把回合挂死；停止时 `killpg` 能连它起的子命令一起收掉。
//! - **环境**：白名单（`env::build_env_from_process`）再剥掉计费 key（[`BILLING_KEYS`]）——
//!   带着 `OPENAI_API_KEY` 起的 codex 会改走按量计费。
//! - **stderr 必须一直读**：codex 往 stderr 写日志，没人读的话 64KB 管道缓冲写满，它就卡在
//!   写上，整个 app-server 停摆（Swift 吃过：「回合永远不结束」只在长输出后出现）。这里一直读，
//!   只留最后一段（[`StderrTail`]）给握手失败的说明用。
//! - **stdout 按行读、有上限**（[`LineReader`]）：一行超过上限就丢掉这一行、报一次，读下一行，
//!   不会把内存吃光。读法是取消安全的（可以放进 `select!`）。
//! - 写 stdin 走单独的写任务（对端不读时写会阻塞，不能卡住驱动循环）。EPIPE 是普通错误：
//!   Rust 运行时启动时就忽略了 SIGPIPE（Swift 那次「写管道被信号打死」在这里不会发生），
//!   子进程那边由 std 在 exec 前把 SIGPIPE 复位成默认。
use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdout};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// 子进程环境里绝不能有的变量：有了 codex 就不走订阅额度、改走 API key 按量计费
/// （名字取自 codex-cli 0.160.1 二进制里「auth is provided by environment」那组）。
pub const BILLING_KEYS: [&str; 3] = ["OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_ACCESS_TOKEN"];

/// 一行最多这么多字节（`item/completed` 带全量命令输出，几 MB 是正常的）。
pub const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;

/// stderr 留最后这么多字节。
pub const STDERR_TAIL_BYTES: usize = 4096;

/// 剥掉计费 key。
pub fn strip_billing_keys(env: &mut BTreeMap<String, String>) {
    for key in BILLING_KEYS {
        env.remove(key);
    }
}

/// 怎么起 app-server。
#[derive(Clone, Debug)]
pub struct CodexProcessSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// 整份子进程环境（调用方已经过白名单、剥过计费 key）。
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
}

/// 起好的 app-server。
pub struct CodexProcess {
    pub child: Child,
    pub pid: u32,
    pub stdout: LineReader<tokio::io::BufReader<ChildStdout>>,
    /// 往 stdin 写一行（不含换行）。写任务按顺序写，写失败就停。
    pub writer: mpsc::UnboundedSender<String>,
    pub writer_task: JoinHandle<()>,
    pub stderr: StderrTail,
    pub stderr_task: JoinHandle<()>,
}

/// 起 app-server。工作目录不存在 = 失败（不让它静默落到别处）。
pub fn spawn_codex(spec: &CodexProcessSpec) -> io::Result<CodexProcess> {
    if !spec.cwd.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("工作目录 {} 不存在或不是目录", spec.cwd.display()),
        ));
    }
    let mut cmd = tokio::process::Command::new(&spec.program);
    cmd.args(&spec.args)
        .env_clear()
        .envs(&spec.env)
        .current_dir(&spec.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // SAFETY: pre_exec 在 fork 之后、exec 之前运行，只调 async-signal-safe 的 setsid(2)，
    // 不分配内存、不拿锁。
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::setsid()
                .map(|_| ())
                .map_err(io::Error::from)
        });
    }
    let mut child = cmd.spawn()?;
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("子进程刚起就没了"))?;
    let mut stdin = child.stdin.take().expect("stdin 是 piped");
    let stdout = child.stdout.take().expect("stdout 是 piped");
    let mut stderr = child.stderr.take().expect("stderr 是 piped");

    let (writer, mut lines) = mpsc::unbounded_channel::<String>();
    let writer_task = tokio::spawn(async move {
        while let Some(mut line) = lines.recv().await {
            line.push('\n');
            if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                break;
            }
        }
    });

    let tail = StderrTail::default();
    let sink = tail.clone();
    let stderr_task = tokio::spawn(async move {
        let mut buf = vec![0u8; 16 * 1024];
        while let Ok(n) = stderr.read(&mut buf).await {
            if n == 0 {
                break;
            }
            sink.push(&buf[..n]);
        }
    });

    Ok(CodexProcess {
        child,
        pid,
        stdout: LineReader::new(tokio::io::BufReader::new(stdout), MAX_LINE_BYTES),
        writer,
        writer_task,
        stderr: tail,
        stderr_task,
    })
}

/// stderr 的最后一段（共享、有上限）。
#[derive(Clone, Debug, Default)]
pub struct StderrTail(Arc<Mutex<Vec<u8>>>);

impl StderrTail {
    pub fn push(&self, bytes: &[u8]) {
        let mut b = self.0.lock().expect("stderr 锁中毒");
        b.extend_from_slice(bytes);
        if b.len() > STDERR_TAIL_BYTES {
            let cut = b.len() - STDERR_TAIL_BYTES;
            b.drain(..cut);
        }
    }

    /// 最后几行（UTF-8 有损解码，去掉首尾空白）。
    pub fn text(&self) -> String {
        let b = self.0.lock().expect("stderr 锁中毒");
        String::from_utf8_lossy(&b).trim().to_string()
    }
}

/// 读出来的一行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    Text(String),
    /// 超过上限的一行：已整行丢掉，只报长度（至少这么多字节）。
    TooLong {
        bytes: usize,
    },
}

/// 按 `\n` 切行、有上限的读取器。行尾的 `\r` 留给解析器去掉。
pub struct LineReader<R> {
    inner: R,
    buf: Vec<u8>,
    max: usize,
    /// 正在丢弃一行超长的：已丢了多少字节。
    discarding: Option<usize>,
}

impl<R: AsyncBufRead + Unpin> LineReader<R> {
    pub fn new(inner: R, max: usize) -> Self {
        Self {
            inner,
            buf: Vec::new(),
            max,
            discarding: None,
        }
    }

    /// 下一行；EOF 返回 `None`（EOF 前没有换行的半行也交出来）。
    ///
    /// 取消安全：唯一的 `.await` 是 `fill_buf`，拿到数据后同步地搬进 `self.buf` 并 `consume`，
    /// 中途被 `select!` 丢掉不会丢字节。
    pub async fn next_line(&mut self) -> io::Result<Option<Line>> {
        loop {
            let available = self.inner.fill_buf().await?;
            if available.is_empty() {
                if let Some(bytes) = self.discarding.take() {
                    return Ok(Some(Line::TooLong { bytes }));
                }
                if self.buf.is_empty() {
                    return Ok(None);
                }
                let bytes = std::mem::take(&mut self.buf);
                return Ok(Some(Line::Text(
                    String::from_utf8_lossy(&bytes).into_owned(),
                )));
            }
            let newline = available.iter().position(|b| *b == b'\n');
            let take = newline.unwrap_or(available.len());
            match &mut self.discarding {
                Some(dropped) => *dropped += take,
                None => {
                    if self.buf.len() + take > self.max {
                        self.discarding = Some(self.buf.len() + take);
                        self.buf.clear();
                    } else {
                        self.buf.extend_from_slice(&available[..take]);
                    }
                }
            }
            let consumed = newline.map_or(take, |i| i + 1);
            self.inner.consume(consumed);
            if newline.is_some() {
                if let Some(bytes) = self.discarding.take() {
                    return Ok(Some(Line::TooLong { bytes }));
                }
                let bytes = std::mem::take(&mut self.buf);
                return Ok(Some(Line::Text(
                    String::from_utf8_lossy(&bytes).into_owned(),
                )));
            }
        }
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-exec --lib codex::transport`（连跑三遍）
Expected: `9 passed`，每遍 0.2s 内。

- [ ] **Step 5: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add crates/exec/src/codex/mod.rs crates/exec/src/codex/transport.rs
git commit -m "crewd-exec: codex app-server process in its own session, bounded stdout lines, drained stderr

Co-Authored-By: Claude <noreply@anthropic.com>"
```

- [ ] **Step 6: 验这把尺子会红**

`spawn_codex` 的 stderr 任务里 `while let Ok(n) = stderr.read(&mut buf).await {` 之前临时插一行 `std::future::pending::<()>().await;`（攥着管道但不读）→ `cargo test -p crewd-exec --lib codex::transport` → 红 1 条：`a_flood_on_stderr_cannot_stall_stdout_and_its_tail_is_kept`（10s 超时）。改回（`git checkout -- crates/exec/src/codex/transport.rs`），`git status` 干净。

---

### Task 7: 驱动层 `start_codex` + 假 app-server + 端到端

一个会话 = 一个 tokio 任务。`start_codex`：读账本 → 账里记的号先占住（`LiveId::claim`，撞了 = `AlreadyRunning`，不起进程）→ 白名单环境剥计费 key → `spawn_codex`（失败即 `StartError::Spawn`）→ 登记表记一笔（只收 `pgid == pid`；写不进去就杀掉、报错）→ 建 `CodexCore`（它在构造时就发出 `initialize`）→ 起任务 → **等任务报「就绪」（`Spawned` 已进事件流）或「失败」**（`StartError::Handshake` / `Ledger`，进程已杀）。任务主循环 `select!`：stdout 一行、子进程退出（先把剩下的行读完，最多 500ms，**再对整组补一刀 SIGKILL**，然后照 M2b `finish` 的顺序先收拾——附属任务、thread id、登记表——再发 `Exited`）、命令、0.6s 节拍、`next_wakeup`（请求期限）、停止宽限到点（`killpg(SIGKILL)`）、落账结果（`spawn_blocking` 里调账本）。

假 app-server（`src/bin/fake_codex.rs`，cargo 给集成测试 `CARGO_BIN_EXE_fake_codex`）讲真 codex 的协议形状——一行一个 JSON、没有 `jsonrpc`、服务端请求的 id 有整数也有字符串、发出服务端请求后**阻塞等回应**（真 codex 就是这样挂死回合的）——行为由环境变量控制。执行器只放行调用方显式给的变量，所以测试经 `CodexSpec::additional_env` 传；测试同时塞一个 `OPENAI_API_KEY` 进去，验证它被剥掉。Review Focus 1–5 的端到端测试都在这里。

**Files:**
- Create: `crewd/crates/exec/src/codex/driver.rs`、`crewd/crates/exec/src/bin/fake_codex.rs`、`crewd/crates/exec/tests/fake_codex.rs`
- Modify: `crewd/crates/exec/src/codex/mod.rs`

**Interfaces:**
- Consumes: Task 1 `driver::{Command, ControlOutcome, LiveId, SessionHandle, SessionStatus, StartError, UntrustedContext, decode, kill_group, terminate}`、`resume::valid_agent_session_id`、`launch::{CommsSpec, HelperCommands}`；M2b `env::build_env_from_process`、`registry::{ProbeResult, ProcessRegistry, probe}`、`resume::AgentIdLedger`、`session::{BRIEF_DELIVERY_ID, SessionEvent}`、`delivery::Subject`、`health::LaunchVerdict`；Task 3 `proto::{self, ThreadOptions}`；Task 5 `session::{CodexAction, CodexCore, CodexCoreConfig, Opening, PendingTurn, StartFailure, launch_health}`；Task 6 `transport::{CodexProcess, CodexProcessSpec, Line, spawn_codex, strip_billing_keys}`。
- Produces（`crewd_exec::codex::driver`）:
  - `#[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct CodexSpec { pub workdir: PathBuf, pub model: Option<String>, pub effort: Option<String>, pub fast_mode: Option<bool>, pub developer_instructions: Option<String>, pub initial_prompt: Option<String>, pub initial_context: UntrustedContext, pub comms: Option<CommsSpec>, pub additional_env: BTreeMap<String, String> }`
  - `pub struct CodexStart { pub session_id: String, pub program: PathBuf, pub child_path: String, pub spec: CodexSpec, pub ledger: Arc<dyn AgentIdLedger> }`
  - `#[derive(Clone, Debug)] pub struct CodexConfig { pub core: CodexCoreConfig, pub tick: Duration, pub stop_grace: Duration, pub registry: Option<Arc<ProcessRegistry>>, pub client_version: String }`（`Default` = 0.6s 节拍、2s 宽限、不登记、`CARGO_PKG_VERSION`）
  - `pub fn codex_argv() -> Vec<String>`（`["app-server"]`）、`pub fn thread_options(&CodexSpec) -> ThreadOptions`
  - `pub async fn start_codex(CodexStart, CodexConfig) -> Result<(SessionHandle, mpsc::UnboundedReceiver<SessionEvent>), StartError>`（`Ledger` / `AlreadyRunning` / `Spawn` / `Registry` / `Handshake`）
  - 二进制 `fake_codex`（只给测试用；环境变量见文件头：`FAKE_CODEX_MODE`、`FAKE_CODEX_LOG`、`FAKE_CODEX_ENV_LOG`、`FAKE_CODEX_PID`、`FAKE_CODEX_CHILD_PID`）

- [ ] **Step 1: 写测试器材与失败的测试**

`crates/exec/src/bin/fake_codex.rs`（整个文件）：

```rust
//! 测试器材：假的 `codex app-server`（只给 `tests/fake_codex.rs` 用）。
//!
//! 讲真 codex 的 stdio 协议形状（一行一个 JSON、没有 `jsonrpc` 字段、服务端请求的 id 有整数也有
//! 字符串），行为由环境变量控制——执行器只放行调用方显式给的变量，所以测试经
//! `CodexSpec::additional_env` 传：
//!
//! - `FAKE_CODEX_LOG`：收到的每一行原样追加到这个文件。
//! - `FAKE_CODEX_ENV_LOG`：启动时把自己环境变量的**名字**（排序）写到这个文件。
//! - `FAKE_CODEX_PID`：启动时把自己的 pid 写到这个文件。
//! - `FAKE_CODEX_CHILD_PID`：`spawn_child` 模式下把起的 `sleep` 的 pid 写到这个文件。
//! - `FAKE_CODEX_MODE`：逗号分隔的开关：
//!   `die_on_init`（收到 initialize 就往 stderr 写一句、exit 3）、`init_silent`（不回 initialize）、
//!   `stderr_flood`（回 initialize 之前往 stderr 灌 1 MiB）、`resume_fails`、`turn_hangs`（不回
//!   turn/start）、`slow_turn`（回合一直跑到被 turn/interrupt）、`server_requests`（回合里依次发 6 种
//!   服务端请求，每条都等到回应才继续）、`usage_limit`、`big_context`（回合后上下文 190k/258k，
//!   等压缩）、`spawn_child`（回合里起一个 `sleep 600`）、`deaf`（thread/start 之后再也不读 stdin）、
//!   `huge_item`（回合里发一条 3 MB 的 agent 正文）、`exit_after_turn`（第一个回合结束就自己
//!   退出）、`ignore_term`（忽略 SIGTERM）。
//!
//! 缺省回合：回 `turn/start`、`turn/started`、userMessage 条目、`GOT: <正文>` 的 agentMessage
//! 条目、小额 token 读数、`turn/completed`。
use std::collections::VecDeque;
use std::io::{BufRead, Write};

use serde_json::{Value, json};

struct Fake {
    modes: Vec<String>,
    log: Option<std::fs::File>,
    lines: std::io::Lines<std::io::StdinLock<'static>>,
    backlog: VecDeque<Value>,
    turns: u32,
    next_server_id: i64,
    thread: String,
    active_turn: Option<String>,
}

impl Fake {
    fn has(&self, mode: &str) -> bool {
        self.modes.iter().any(|m| m == mode)
    }

    fn emit(&self, v: Value) {
        let mut out = std::io::stdout().lock();
        writeln!(out, "{v}").unwrap();
        out.flush().unwrap();
    }

    fn notify(&self, method: &str, params: Value) {
        self.emit(json!({ "method": method, "params": params }));
    }

    /// 下一条客户端消息（先取积压的）。EOF = 退出。
    fn next(&mut self) -> Value {
        match self.backlog.pop_front() {
            Some(v) => v,
            None => self.next_raw(),
        }
    }

    /// 发一条服务端请求并等它的回应（期间收到的别的消息放进积压）。
    fn ask(&mut self, id: Value, method: &str, params: Value) -> Value {
        self.emit(json!({ "id": id, "method": method, "params": params }));
        loop {
            let v = self.next_raw();
            if v.get("method").is_none() && v.get("id") == Some(&id) {
                return v;
            }
            self.backlog.push_back(v);
        }
    }

    fn next_raw(&mut self) -> Value {
        loop {
            let Some(Ok(line)) = self.lines.next() else {
                std::process::exit(0);
            };
            if let Some(f) = &mut self.log {
                writeln!(f, "{line}").unwrap();
            }
            if !line.trim().is_empty() {
                return serde_json::from_str(&line).expect("客户端发来的不是 JSON");
            }
        }
    }

    fn item(&self, turn: &str, item: Value) {
        self.notify(
            "item/completed",
            json!({ "threadId": self.thread, "turnId": turn, "completedAtMs": 0, "item": item }),
        );
    }

    fn usage(&self, turn: &str, context: i64, total: i64) {
        self.notify(
            "thread/tokenUsage/updated",
            json!({ "threadId": self.thread, "turnId": turn, "tokenUsage": {
                "last": { "totalTokens": context, "inputTokens": 0, "outputTokens": 0,
                          "cachedInputTokens": 0, "reasoningOutputTokens": 0 },
                "total": { "totalTokens": total, "inputTokens": 0, "outputTokens": 0,
                           "cachedInputTokens": 0, "reasoningOutputTokens": 0 },
                "modelContextWindow": 258_400 } }),
        );
    }

    fn complete(&mut self, turn: &str, status: &str, error: Value) {
        self.active_turn = None;
        self.notify(
            "turn/completed",
            json!({ "threadId": self.thread,
                    "turn": { "id": turn, "status": status, "items": [], "error": error } }),
        );
    }

    fn turn_start(&mut self, id: &Value, params: &Value) {
        if self.has("turn_hangs") {
            return;
        }
        self.turns += 1;
        let turn = format!("turn-{}", self.turns);
        let text = params
            .pointer("/input/0/text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.emit(json!({ "id": id, "result": { "turn": {
            "id": turn, "status": "inProgress", "items": [] } } }));
        self.notify(
            "turn/started",
            json!({ "threadId": self.thread, "turn": { "id": turn, "status": "inProgress", "items": [] } }),
        );
        self.active_turn = Some(turn.clone());
        self.item(
            &turn,
            json!({ "id": format!("{turn}-u"), "type": "userMessage",
                    "content": [{ "type": "text", "text": text, "text_elements": [] }] }),
        );
        if self.has("spawn_child") {
            let child = std::process::Command::new("/bin/sleep")
                .arg("600")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            if let Ok(path) = std::env::var("FAKE_CODEX_CHILD_PID") {
                std::fs::write(path, child.id().to_string()).unwrap();
            }
            std::mem::forget(child);
        }
        if self.has("server_requests") {
            self.server_requests(&turn);
        }
        if self.has("huge_item") {
            self.item(
                &turn,
                json!({ "id": format!("{turn}-big"), "type": "agentMessage",
                        "text": "x".repeat(3 * 1024 * 1024) }),
            );
        }
        if self.has("slow_turn") {
            return;
        }
        self.item(
            &turn,
            json!({ "id": format!("{turn}-a"), "type": "agentMessage", "text": format!("GOT: {text}") }),
        );
        if self.has("usage_limit") {
            self.notify(
                "account/rateLimits/updated",
                json!({ "rateLimits": { "rateLimitReachedType": "rate_limit_reached" } }),
            );
            self.complete(
                &turn,
                "failed",
                json!({ "message": "You've hit your usage limit.", "codexErrorInfo": "usageLimitExceeded" }),
            );
            return;
        }
        let big = self.has("big_context") && self.turns == 1;
        self.complete(&turn, "completed", Value::Null);
        if big {
            self.usage(&turn, 190_000, 500_000);
        } else {
            self.usage(&turn, 20_000, 40_000);
        }
        if self.has("exit_after_turn") {
            std::process::exit(0);
        }
    }

    fn server_requests(&mut self, turn: &str) {
        let thread = self.thread.clone();
        let asks = [
            (
                json!(self.next_server_id),
                "item/commandExecution/requestApproval",
                json!({ "threadId": thread, "turnId": turn, "itemId": "c1", "command": "rm -rf /" }),
            ),
            (
                json!("elicit-tool"),
                "mcpServer/elicitation/request",
                json!({ "threadId": thread, "serverName": "crew", "mode": "form", "message": "Allow post_to_crew?",
                     "requestedSchema": { "type": "object", "properties": {} },
                     "_meta": { "codex_approval_kind": "mcp_tool_call" } }),
            ),
            (
                json!("elicit-form"),
                "mcpServer/elicitation/request",
                json!({ "threadId": thread, "serverName": "crew", "mode": "form", "message": "你叫什么？",
                     "requestedSchema": { "type": "object", "properties": {} } }),
            ),
            (
                json!(self.next_server_id + 1),
                "account/chatgptAuthTokens/refresh",
                json!({ "reason": "unauthorized" }),
            ),
            (
                json!(self.next_server_id + 2),
                "item/tool/requestUserInput",
                json!({ "threadId": thread, "prompt": "选一个" }),
            ),
            (
                json!(self.next_server_id + 3),
                "currentTime/read",
                json!({ "threadId": thread }),
            ),
        ];
        self.next_server_id += 4;
        let mut answered = 0;
        for (id, method, params) in asks {
            let reply = self.ask(id, method, params);
            if reply.get("result").is_some() || reply.get("error").is_some() {
                answered += 1;
            }
        }
        self.item(
            turn,
            json!({ "id": format!("{turn}-sr"), "type": "agentMessage", "text": format!("answered {answered}") }),
        );
    }

    fn serve(&mut self) {
        loop {
            let msg = self.next();
            let Some(method) = msg
                .get("method")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            let id = msg.get("id").cloned();
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            let Some(id) = id else {
                continue; // 通知（initialized）
            };
            match method.as_str() {
                "initialize" => {
                    if self.has("die_on_init") {
                        eprintln!("fake codex: not logged in");
                        std::process::exit(3);
                    }
                    if self.has("init_silent") {
                        continue;
                    }
                    if self.has("stderr_flood") {
                        let chunk = "x".repeat(64 * 1024);
                        let mut err = std::io::stderr().lock();
                        for _ in 0..16 {
                            err.write_all(chunk.as_bytes()).unwrap();
                        }
                        err.flush().unwrap();
                    }
                    self.emit(json!({ "id": id, "result": {
                        "userAgent": "fake_codex/0.0.0", "codexHome": "/nonexistent",
                        "platformFamily": "unix", "platformOs": "macos" } }));
                }
                "thread/start" => {
                    self.thread = format!("thr-{}", std::process::id());
                    self.emit(json!({ "id": id, "result": {
                        "thread": { "id": self.thread }, "model": "fake-model",
                        "reasoningEffort": "medium",
                        "serviceTier": params.get("serviceTier").cloned().unwrap_or(Value::Null),
                    } }));
                    if self.has("deaf") {
                        loop {
                            std::thread::sleep(std::time::Duration::from_secs(60));
                        }
                    }
                }
                "thread/resume" => {
                    let wanted = params
                        .get("threadId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if self.has("resume_fails") {
                        self.emit(json!({ "id": id, "error": {
                            "code": -32600, "message": format!("no rollout found for thread id {wanted}") } }));
                    } else {
                        self.thread = wanted;
                        self.emit(json!({ "id": id, "result": {
                            "thread": { "id": self.thread }, "model": "fake-model" } }));
                    }
                }
                "turn/start" => self.turn_start(&id, &params),
                "turn/interrupt" => {
                    self.emit(json!({ "id": id, "result": {} }));
                    if let Some(turn) = self.active_turn.clone() {
                        self.complete(&turn, "interrupted", Value::Null);
                    }
                }
                "thread/settings/update" => {
                    self.emit(json!({ "id": id, "result": {} }));
                    let tier = params
                        .get("serviceTier")
                        .cloned()
                        .unwrap_or(json!("default"));
                    self.notify(
                        "thread/settings/updated",
                        json!({ "threadId": self.thread, "threadSettings": {
                            "serviceTier": tier, "model": params.get("model").cloned().unwrap_or(json!("fake-model")) } }),
                    );
                }
                "thread/compact/start" => {
                    self.emit(json!({ "id": id, "result": {} }));
                    self.notify(
                        "turn/started",
                        json!({ "threadId": self.thread, "turn": { "id": "compact", "status": "inProgress", "items": [] } }),
                    );
                    self.item(
                        "compact",
                        json!({ "id": "compact-item", "type": "contextCompaction" }),
                    );
                    self.usage("compact", 28_000, 525_000);
                    self.complete("compact", "completed", Value::Null);
                }
                other => {
                    self.emit(json!({ "id": id, "error": {
                        "code": -32601, "message": format!("fake codex does not implement {other}") } }));
                }
            }
        }
    }
}

fn main() {
    let modes: Vec<String> = std::env::var("FAKE_CODEX_MODE")
        .unwrap_or_default()
        .split(',')
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .collect();
    if modes.iter().any(|m| m == "ignore_term") {
        // SAFETY: 进程启动时、还没有别的线程；只把 SIGTERM 设成忽略。
        unsafe {
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
        }
    }
    if let Ok(path) = std::env::var("FAKE_CODEX_PID") {
        std::fs::write(path, std::process::id().to_string()).unwrap();
    }
    if let Ok(path) = std::env::var("FAKE_CODEX_ENV_LOG") {
        let mut names: Vec<String> = std::env::vars().map(|(k, _)| k).collect();
        names.sort();
        std::fs::write(path, names.join("\n")).unwrap();
    }
    let log = std::env::var("FAKE_CODEX_LOG").ok().map(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .unwrap()
    });
    let mut fake = Fake {
        modes,
        log,
        lines: std::io::stdin().lock().lines(),
        backlog: VecDeque::new(),
        turns: 0,
        next_server_id: 100,
        thread: String::new(),
        active_turn: None,
    };
    fake.serve();
}
```

`crates/exec/tests/fake_codex.rs`（整个文件）：

```rust
//! 端到端：真进程 + 假 codex app-server（`src/bin/fake_codex.rs`）。握手、落账、续跑与退回、
//! 回合、每一种服务端请求、额度、超时、压缩、打断、控制、停止与进程组、stderr 灌满、超长行。
//! 不碰真 codex（真机见 `live_codex.rs`）。
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crewd_exec::codex::driver::{CodexConfig, CodexSpec, CodexStart, start_codex};
use crewd_exec::driver::{Control, ControlOutcome, SessionHandle, StartError};
use crewd_exec::health::{HealthKind, LaunchVerdict};
use crewd_exec::launch::CommsSpec;
use crewd_exec::registry::ProcessRegistry;
use crewd_exec::resume::{AgentIdLedger, MemoryLedger};
use crewd_exec::session::{
    BRIEF_DELIVERY_ID, DeliveryOutcome, ExitInfo, ExitStatusKind, Profile, SessionEvent,
};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;

const FAKE: &str = env!("CARGO_BIN_EXE_fake_codex");

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn config(&self) -> CodexConfig {
        let mut c = CodexConfig {
            stop_grace: Duration::from_millis(500),
            ..CodexConfig::default()
        };
        c.core.initialize_timeout = Duration::from_secs(3);
        c.core.thread_timeout = Duration::from_secs(3);
        c.core.request_timeout = Duration::from_secs(2);
        c
    }

    fn start(
        &self,
        modes: &str,
        ledger: Arc<dyn AgentIdLedger>,
        brief: Option<&str>,
    ) -> CodexStart {
        let mut additional_env: BTreeMap<String, String> = [
            ("FAKE_CODEX_MODE", modes.to_string()),
            ("FAKE_CODEX_LOG", self.path("rpc.log").display().to_string()),
            (
                "FAKE_CODEX_ENV_LOG",
                self.path("env.log").display().to_string(),
            ),
            ("FAKE_CODEX_PID", self.path("pid").display().to_string()),
            (
                "FAKE_CODEX_CHILD_PID",
                self.path("child.pid").display().to_string(),
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        // 调用方显式给了也必须被剥掉（计费保护）。
        additional_env.insert("OPENAI_API_KEY".into(), "sk-should-never-arrive".into());
        CodexStart {
            session_id: "worker-1".into(),
            program: PathBuf::from(FAKE),
            child_path: "/usr/bin:/bin".into(),
            spec: CodexSpec {
                workdir: self.dir.path().to_path_buf(),
                developer_instructions: Some("你是 crew 的一员".into()),
                initial_prompt: brief.map(str::to_string),
                initial_context: [("crew_whiteboard".to_string(), "A: 别动 main".to_string())]
                    .into_iter()
                    .collect(),
                additional_env,
                ..CodexSpec::default()
            },
            ledger,
        }
    }

    /// 假 codex 收到的每一条消息。
    fn log(&self) -> Vec<Value> {
        std::fs::read_to_string(self.path("rpc.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn sent(&self, method: &str) -> Vec<Value> {
        self.log()
            .into_iter()
            .filter(|v| v["method"] == method)
            .collect()
    }

    /// 我们对服务端请求 `id` 的回应。
    fn reply_to(&self, id: Value) -> Value {
        self.log()
            .into_iter()
            .find(|v| v.get("method").is_none() && v["id"] == id)
            .unwrap_or_else(|| panic!("没回 {id}"))
    }

    fn pid_in(&self, name: &str) -> i32 {
        std::fs::read_to_string(self.path(name))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: 信号 0 只做存在性检查。
    unsafe { libc::kill(pid, 0) == 0 }
}

async fn gone(pid: i32) {
    for _ in 0..100 {
        if !alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("进程 {pid} 3s 后还活着");
}

async fn next(rx: &mut UnboundedReceiver<SessionEvent>) -> SessionEvent {
    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("10s 内没有事件")
        .expect("事件流提前结束")
}

/// 等到满足条件的事件，返回它之前（含）收到的全部事件。
async fn until(
    rx: &mut UnboundedReceiver<SessionEvent>,
    pred: impl Fn(&SessionEvent) -> bool,
) -> Vec<SessionEvent> {
    let mut seen = Vec::new();
    loop {
        let e = next(rx).await;
        let hit = pred(&e);
        seen.push(e);
        if hit {
            return seen;
        }
    }
}

fn exited(e: &SessionEvent) -> Option<&ExitInfo> {
    match e {
        SessionEvent::Exited(info) => Some(info),
        _ => None,
    }
}

fn is_idle_edge(e: &SessionEvent) -> bool {
    *e == SessionEvent::Working(false)
}

async fn stop(h: &SessionHandle, rx: &mut UnboundedReceiver<SessionEvent>) -> ExitInfo {
    h.stop().unwrap();
    let events = until(rx, |e| exited(e).is_some()).await;
    exited(events.last().unwrap()).unwrap().clone()
}

#[tokio::test]
async fn fresh_start_records_the_thread_and_runs_the_brief_with_its_context() {
    let env = Env::new();
    let ledger = Arc::new(MemoryLedger::default());
    let mut start = env.start("", ledger.clone(), Some("开场任务"));
    start.spec.comms = Some(CommsSpec {
        helper: PathBuf::from("/opt/crewd/bin/crewd"),
        data_dir: PathBuf::from("/data"),
        crew_id: "c1".into(),
        session_id: "worker-1".into(),
        captain: false,
        structured_post: false,
        label: None,
        hooks: false,
    });
    let (h, mut rx) = start_codex(start, env.config()).await.unwrap();
    let SessionEvent::Spawned {
        agent_session_id,
        resumed,
        pid,
    } = next(&mut rx).await
    else {
        panic!("第一条事件必须是 Spawned");
    };
    assert!(!resumed && pid > 0);
    assert_eq!(ledger.current().as_deref(), Some(agent_session_id.as_str()));
    let events = until(&mut rx, is_idle_edge).await;
    assert!(events.contains(&SessionEvent::Delivery {
        id: BRIEF_DELIVERY_ID,
        subject: crewd_exec::delivery::Subject::Brief,
        outcome: DeliveryOutcome::Delivered,
    }));
    assert!(events.contains(&SessionEvent::TurnEnded {
        last_agent_message: "GOT: 开场任务".into()
    }));
    assert!(events.contains(&SessionEvent::Profile(Profile {
        model: Some("fake-model".into()),
        effort: Some("medium".into()),
        fast: None,
    })));

    let methods: Vec<String> = env
        .log()
        .iter()
        .filter_map(|v| v["method"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        methods,
        vec!["initialize", "initialized", "thread/start", "turn/start"]
    );
    let start = &env.sent("thread/start")[0]["params"];
    assert_eq!(start["developerInstructions"], "你是 crew 的一员");
    assert_eq!(start["approvalPolicy"], "on-request");
    assert_eq!(start["sandbox"], "workspace-write");
    assert_eq!(start["approvalsReviewer"], "auto_review");
    let crew = &start["config"]["mcp_servers"]["crew"];
    assert_eq!(crew["command"], "/opt/crewd/bin/crewd");
    let args: Vec<&str> = crew["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(args[0], "mcp");
    assert!(
        args.windows(2).any(|w| w == ["--agent", "codex"]),
        "{args:?}"
    );
    let turn = &env.sent("turn/start")[0]["params"];
    assert_eq!(turn["threadId"], agent_session_id.as_str());
    assert_eq!(turn["input"][0]["text"], "开场任务");
    assert_eq!(
        turn["additionalContext"]["crew_whiteboard"]["kind"],
        "untrusted"
    );

    let names = std::fs::read_to_string(env.path("env.log")).unwrap();
    assert!(!names.lines().any(|n| n == "OPENAI_API_KEY"), "{names}");
    assert!(names.lines().any(|n| n == "FAKE_CODEX_MODE"));

    assert!(h.screen_text(20).await.unwrap().contains("GOT: 开场任务"));
    assert_eq!(h.snapshot().await.unwrap(), None, "codex 没有终端");
    let status = h.status().await.unwrap();
    assert_eq!(status.agent_session_id, agent_session_id);
    assert!(!status.working);
    let info = stop(&h, &mut rx).await;
    assert!(info.stopped_by_user);
}

#[tokio::test]
async fn a_recorded_thread_is_resumed_without_its_turns() {
    let env = Env::new();
    let ledger = Arc::new(MemoryLedger::with(Some("thr-old")));
    let (h, mut rx) = start_codex(env.start("", ledger.clone(), None), env.config())
        .await
        .unwrap();
    assert_eq!(
        next(&mut rx).await,
        SessionEvent::Spawned {
            pid: env.pid_in("pid") as u32,
            agent_session_id: "thr-old".into(),
            resumed: true,
        }
    );
    let resume = &env.sent("thread/resume")[0]["params"];
    assert_eq!(resume["threadId"], "thr-old");
    assert_eq!(resume["excludeTurns"], true);
    assert!(env.sent("thread/start").is_empty());
    assert_eq!(ledger.current().as_deref(), Some("thr-old"));
    stop(&h, &mut rx).await;
}

#[tokio::test]
async fn a_refused_resume_falls_back_to_a_fresh_thread_and_records_it() {
    let env = Env::new();
    let ledger = Arc::new(MemoryLedger::with(Some("thr-gone")));
    let (h, mut rx) = start_codex(
        env.start("resume_fails", ledger.clone(), Some("接着干")),
        env.config(),
    )
    .await
    .unwrap();
    let SessionEvent::Spawned {
        agent_session_id,
        resumed: false,
        ..
    } = next(&mut rx).await
    else {
        panic!("第一条事件必须是 Spawned（新 thread）");
    };
    assert_ne!(agent_session_id, "thr-gone");
    assert_eq!(ledger.current(), Some(agent_session_id.clone()));
    let SessionEvent::ResumeRefused { id, said } = next(&mut rx).await else {
        panic!("紧跟着说清楚原 thread 接不回来");
    };
    assert_eq!(id, "thr-gone");
    assert!(said.contains("no rollout found"), "{said}");
    until(&mut rx, is_idle_edge).await;
    assert_eq!(
        env.sent("turn/start")[0]["params"]["threadId"],
        agent_session_id.as_str()
    );
    stop(&h, &mut rx).await;
}

/// Review Focus 1：每一种服务端请求都必须被回答（整数和字符串 id 都原样回），否则回合挂死。
#[tokio::test]
async fn every_server_request_is_answered_and_the_turn_finishes() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start(
            "server_requests",
            Arc::new(MemoryLedger::default()),
            Some("干活"),
        ),
        env.config(),
    )
    .await
    .unwrap();
    let events = until(&mut rx, |e| matches!(e, SessionEvent::TurnEnded { .. })).await;
    assert!(h.screen_text(50).await.unwrap().contains("answered 6"));
    assert_eq!(env.reply_to(100.into())["error"]["code"], -32000);
    assert_eq!(
        env.reply_to("elicit-tool".into())["result"]["action"],
        "accept"
    );
    assert_eq!(
        env.reply_to("elicit-form".into())["result"]["action"],
        "decline"
    );
    assert_eq!(env.reply_to(101.into())["error"]["code"], -32601);
    assert_eq!(env.reply_to(102.into())["error"]["code"], -32601);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let told = env.reply_to(103.into())["result"]["currentTimeAt"]
        .as_i64()
        .unwrap();
    assert!((told - now).abs() < 60, "{told} vs {now}");
    assert!(
        events.iter().any(
            |e| matches!(e, SessionEvent::Health(Some(h)) if h.kind == HealthKind::AuthRequired)
        )
    );
    let notices = events
        .iter()
        .filter(|e| matches!(e, SessionEvent::Notice(_)))
        .count();
    assert_eq!(
        notices, 2,
        "代拒的表单 + 不支持的 requestUserInput：{events:?}"
    );
    stop(&h, &mut rx).await;
}

#[tokio::test]
async fn a_usage_limit_from_the_protocol_is_health_and_marks_the_exit() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start(
            "usage_limit",
            Arc::new(MemoryLedger::default()),
            Some("干活"),
        ),
        env.config(),
    )
    .await
    .unwrap();
    until(
        &mut rx,
        |e| matches!(e, SessionEvent::Health(Some(h)) if h.kind == HealthKind::UsageLimit),
    )
    .await;
    until(&mut rx, is_idle_edge).await;
    assert_eq!(
        h.status().await.unwrap().state,
        crewd_exec::session::DerivedState::RateLimited
    );
    let info = stop(&h, &mut rx).await;
    assert!(info.hit_limit && info.stopped_by_user);
}

#[tokio::test]
async fn a_turn_start_that_never_answers_times_out_and_the_session_stays_controllable() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start(
            "turn_hangs",
            Arc::new(MemoryLedger::default()),
            Some("开场"),
        ),
        env.config(),
    )
    .await
    .unwrap();
    let events = until(&mut rx, |e| matches!(e, SessionEvent::Delivery { .. })).await;
    let Some(SessionEvent::Delivery {
        outcome: DeliveryOutcome::Undelivered(why),
        ..
    }) = events.last()
    else {
        panic!("{events:?}");
    };
    assert!(why.contains("没有回应"), "{why}");
    assert!(!h.status().await.unwrap().working);
    stop(&h, &mut rx).await;
}

/// Review Focus 2：codex 往 stderr 狂写日志，没人读就卡死在第 64KB——握手必须照样完成。
#[tokio::test]
async fn a_stderr_flood_does_not_stall_the_handshake() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start("stderr_flood", Arc::new(MemoryLedger::default()), None),
        env.config(),
    )
    .await
    .expect("stderr 没被排空，握手卡住了");
    assert!(matches!(next(&mut rx).await, SessionEvent::Spawned { .. }));
    stop(&h, &mut rx).await;
}

#[tokio::test]
async fn handshake_failures_are_start_errors_and_leave_no_process() {
    let env = Env::new();
    let err = start_codex(
        env.start("die_on_init", Arc::new(MemoryLedger::default()), None),
        env.config(),
    )
    .await
    .err()
    .expect("必须报错");
    let StartError::Handshake { health } = err else {
        panic!("{err:?}");
    };
    assert_eq!(health.launch_verdict, Some(LaunchVerdict::DiedSilently));
    assert!(health.detail.contains("not logged in"), "{}", health.detail);

    let env = Env::new();
    let mut config = env.config();
    config.core.initialize_timeout = Duration::from_millis(500);
    let err = start_codex(
        env.start("init_silent", Arc::new(MemoryLedger::default()), None),
        config,
    )
    .await
    .err()
    .expect("必须报错");
    let StartError::Handshake { health } = err else {
        panic!("{err:?}");
    };
    assert_eq!(health.launch_verdict, Some(LaunchVerdict::Stalled));
    gone(env.pid_in("pid")).await;

    let env = Env::new();
    let mut start = env.start("", Arc::new(MemoryLedger::default()), None);
    start.spec.workdir = env.path("no-such-dir");
    let err = start_codex(start, env.config())
        .await
        .err()
        .expect("必须报错");
    assert!(matches!(err, StartError::Spawn { .. }), "{err:?}");
}

/// 账本写不进去。
struct BrokenLedger;

impl AgentIdLedger for BrokenLedger {
    fn recorded(&self) -> std::io::Result<Option<String>> {
        Ok(None)
    }

    fn record(&self, _: &str) -> std::io::Result<()> {
        Err(std::io::Error::other("库坏了"))
    }
}

/// Review Focus 3：thread id 落不了账就不能干活（起了也接不回来）——报错、杀掉进程、一个回合都不开。
#[tokio::test]
async fn ledger_failure_kills_the_process_before_any_turn() {
    let env = Env::new();
    let err = start_codex(
        env.start("", Arc::new(BrokenLedger), Some("开场")),
        env.config(),
    )
    .await
    .err()
    .expect("必须报错");
    assert!(matches!(err, StartError::Ledger(_)), "{err:?}");
    gone(env.pid_in("pid")).await;
    assert!(env.sent("turn/start").is_empty(), "落账失败后不该开回合");
}

/// Review Focus 4：codex 起的工具子命令（`sleep 600`）在停止时一起收掉；codex 不理 SIGTERM 时
/// 宽限后升级成整组 SIGKILL。
#[tokio::test]
async fn stop_kills_the_whole_group_including_tool_children() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start(
            "spawn_child,ignore_term",
            Arc::new(MemoryLedger::default()),
            Some("跑个命令"),
        ),
        env.config(),
    )
    .await
    .unwrap();
    until(&mut rx, is_idle_edge).await;
    let child = env.pid_in("child.pid");
    assert!(alive(child));
    let info = stop(&h, &mut rx).await;
    assert_eq!(
        info,
        ExitInfo {
            status: ExitStatusKind::Signal(9),
            stopped_by_user: true,
            hit_limit: false,
        }
    );
    gone(child).await;
}

/// codex 自己退出时（崩溃 / 被人在外面杀掉），它起的工具子命令也不能变成孤儿。
#[tokio::test]
async fn tool_children_do_not_outlive_codex_exiting_on_its_own() {
    let env = Env::new();
    let (_h, mut rx) = start_codex(
        env.start(
            "spawn_child,exit_after_turn",
            Arc::new(MemoryLedger::default()),
            Some("跑个命令就走"),
        ),
        env.config(),
    )
    .await
    .unwrap();
    let events = until(&mut rx, |e| exited(e).is_some()).await;
    let info = exited(events.last().unwrap()).unwrap();
    assert_eq!(info.status, ExitStatusKind::Code(0));
    assert!(!info.stopped_by_user);
    gone(env.pid_in("child.pid")).await;
}

#[tokio::test]
async fn high_context_triggers_one_compaction_and_a_send_waits_for_it() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start(
            "big_context",
            Arc::new(MemoryLedger::default()),
            Some("干活"),
        ),
        env.config(),
    )
    .await
    .unwrap();
    until(&mut rx, |e| matches!(e, SessionEvent::TurnEnded { .. })).await;
    let id = h.send("压缩之后").unwrap();
    until(
        &mut rx,
        |e| matches!(e, SessionEvent::Delivery { id: i, .. } if *i == id),
    )
    .await;
    until(&mut rx, is_idle_edge).await;
    let order: Vec<String> = env
        .log()
        .iter()
        .filter_map(|v| v["method"].as_str().map(str::to_string))
        .filter(|m| m == "turn/start" || m == "thread/compact/start")
        .collect();
    assert_eq!(
        order,
        vec!["turn/start", "thread/compact/start", "turn/start"]
    );
    stop(&h, &mut rx).await;
}

#[tokio::test]
async fn interrupt_ends_a_running_turn_and_raw_keys_are_refused_with_a_notice() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start(
            "slow_turn",
            Arc::new(MemoryLedger::default()),
            Some("慢慢来"),
        ),
        env.config(),
    )
    .await
    .unwrap();
    until(&mut rx, |e| matches!(e, SessionEvent::Delivery { .. })).await;
    assert!(h.status().await.unwrap().working);
    h.raw(b"y\r".to_vec()).unwrap();
    until(&mut rx, |e| matches!(e, SessionEvent::Notice(_))).await;
    h.interrupt().unwrap();
    let events = until(&mut rx, is_idle_edge).await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, SessionEvent::TurnEnded { .. })),
        "被打断的回合不算一次回答：{events:?}"
    );
    assert_eq!(env.sent("turn/interrupt")[0]["params"]["turnId"], "turn-1");
    stop(&h, &mut rx).await;
}

/// Review Focus 5：codex 不读 stdin 了（卡死 / 正忙），我们却要写一大段——会话不能被卡死。
#[tokio::test]
async fn a_server_that_stops_reading_cannot_wedge_the_session() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start("deaf", Arc::new(MemoryLedger::default()), None),
        env.config(),
    )
    .await
    .unwrap();
    next(&mut rx).await;
    let big = "很长的正文。".repeat(200_000);
    let id = h.send(big).unwrap();
    let status = tokio::time::timeout(Duration::from_secs(1), h.status())
        .await
        .expect("写阻塞把会话卡住了")
        .unwrap();
    assert!(status.working);
    let events = until(&mut rx, |e| matches!(e, SessionEvent::Delivery { .. })).await;
    assert!(matches!(
        events.last(),
        Some(SessionEvent::Delivery { id: i, outcome: DeliveryOutcome::Undelivered(_), .. }) if *i == id
    ));
    let info = tokio::time::timeout(Duration::from_secs(5), stop(&h, &mut rx))
        .await
        .expect("停不掉");
    assert!(info.stopped_by_user);
}

#[tokio::test]
async fn a_three_megabyte_item_arrives_whole() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start(
            "huge_item",
            Arc::new(MemoryLedger::default()),
            Some("写个大的"),
        ),
        env.config(),
    )
    .await
    .unwrap();
    let events = until(&mut rx, is_idle_edge).await;
    let big = events
        .iter()
        .find_map(|e| match e {
            SessionEvent::Item { item, .. } if item["id"] == "turn-1-big" => {
                item["text"].as_str().map(str::len)
            }
            _ => None,
        })
        .expect("3MB 的条目没到");
    assert_eq!(big, 3 * 1024 * 1024);
    stop(&h, &mut rx).await;
}

#[tokio::test]
async fn controls_round_trip_through_the_protocol() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start("", Arc::new(MemoryLedger::default()), None),
        env.config(),
    )
    .await
    .unwrap();
    next(&mut rx).await;
    assert!(matches!(
        h.control(Control::Fast(true)).await.unwrap(),
        ControlOutcome::Applied(_)
    ));
    assert!(matches!(
        h.control(Control::Model("o5".into())).await.unwrap(),
        ControlOutcome::Applied(_)
    ));
    assert!(matches!(
        h.control(Control::Compact).await.unwrap(),
        ControlOutcome::Applied(_)
    ));
    let updates = env.sent("thread/settings/update");
    assert_eq!(updates[0]["params"]["serviceTier"], "fast");
    assert_eq!(updates[1]["params"]["model"], "o5");
    assert_eq!(env.sent("thread/compact/start").len(), 1);
    h.resize(10, 10).unwrap();
    let events = until(
        &mut rx,
        |e| matches!(e, SessionEvent::Profile(p) if p.fast == Some(true)),
    )
    .await;
    assert!(!events.is_empty());
    stop(&h, &mut rx).await;
}

#[tokio::test]
async fn the_registry_holds_the_live_process_and_is_cleared_on_exit() {
    let env = Env::new();
    let registry = Arc::new(ProcessRegistry::load(&env.path("exec-registry.json")).unwrap());
    let mut config = env.config();
    config.registry = Some(registry.clone());
    let (h, mut rx) = start_codex(
        env.start("", Arc::new(MemoryLedger::default()), None),
        config,
    )
    .await
    .unwrap();
    next(&mut rx).await;
    let entries = registry.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, "worker-1");
    assert_eq!(entries[0].1.pid, env.pid_in("pid"));
    stop(&h, &mut rx).await;
    for _ in 0..50 {
        if registry.entries().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("退出后登记表没清");
}

#[tokio::test]
async fn dropping_every_handle_stops_the_session() {
    let env = Env::new();
    let (h, mut rx) = start_codex(
        env.start("", Arc::new(MemoryLedger::default()), None),
        env.config(),
    )
    .await
    .unwrap();
    let h2 = h.clone();
    drop(h);
    drop(h2);
    let events = until(&mut rx, |e| exited(e).is_some()).await;
    assert!(exited(events.last().unwrap()).unwrap().stopped_by_user);
    gone(env.pid_in("pid")).await;
}

/// 同一个 thread 在本进程里只能跑一个（与 claude 共用一张表）；前一个退出后立刻能再起。
#[tokio::test]
async fn a_running_thread_cannot_be_resumed_twice_but_is_free_once_exited() {
    let env = Env::new();
    let ledger = Arc::new(MemoryLedger::default());
    let (h, mut rx) = start_codex(env.start("", ledger.clone(), None), env.config())
        .await
        .unwrap();
    let SessionEvent::Spawned {
        agent_session_id, ..
    } = next(&mut rx).await
    else {
        panic!("第一条事件必须是 Spawned");
    };
    let twin = Env::new();
    let err = start_codex(
        twin.start(
            "",
            Arc::new(MemoryLedger::with(Some(&agent_session_id))),
            None,
        ),
        twin.config(),
    )
    .await
    .err()
    .expect("同一个 thread 不能同时再起");
    assert!(
        matches!(&err, StartError::AlreadyRunning(id) if *id == agent_session_id),
        "{err:?}"
    );
    assert!(!twin.path("pid").exists(), "占号失败时不该起进程");
    stop(&h, &mut rx).await;
    let (h2, mut rx2) = start_codex(
        twin.start(
            "",
            Arc::new(MemoryLedger::with(Some(&agent_session_id))),
            None,
        ),
        twin.config(),
    )
    .await
    .expect("前一个见到 Exited 之后就能再起");
    assert!(matches!(
        next(&mut rx2).await,
        SessionEvent::Spawned { resumed: true, .. }
    ));
    stop(&h2, &mut rx2).await;
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-exec --test fake_codex`
Expected: 编译失败，报 ``could not find `driver` in `codex` ``。

- [ ] **Step 3: 实现**

`crates/exec/src/codex/mod.rs`：`pub mod proto;` 之前加 `pub mod driver;`。

`crates/exec/src/codex/driver.rs`（整个文件）：

```rust
//! 一个 codex 会话的 tokio 任务：拥有 app-server 子进程、三根管子和 [`CodexCore`]，
//! 对外只交出与 claude 相同的 [`SessionHandle`] + `SessionEvent` 事件流。
//!
//! `start_codex` 等到握手完成、thread id 落账（第一条事件 `Spawned` 已排进事件流）才返回；
//! 握手失败 / 超时 / 落账失败都在这里变成 `StartError`，进程已杀掉。之后的节拍：每行 stdout、
//! 0.6s 定时、`CodexCore::next_wakeup`（请求期限）到点。进程退出后先把 stdout 剩下的行读完
//! （最多 500ms），再对整个进程组补一刀 SIGKILL——codex 起的工具子命令不留成孤儿；然后先收拾
//! （登记表、占着的会话号）再把 `Exited` 发出去，与 claude 驱动同一个顺序：调用方见到 `Exited`
//! 就能马上用同一个 thread id 重起。
//!
//! 同一个 thread id 在本进程里只能跑一个（与 claude 共用 `driver::LiveId` 那张表）：账里记的号
//! 在起进程之前就占住，新开 thread 的号在 `Spawned` 时占住。
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::{mpsc, oneshot};

use crate::codex::proto::{self, ThreadOptions};
use crate::codex::session::{
    CodexAction, CodexCore, CodexCoreConfig, Opening, PendingTurn, StartFailure, launch_health,
};
use crate::codex::transport::{
    CodexProcess, CodexProcessSpec, Line, spawn_codex, strip_billing_keys,
};
use crate::delivery::Subject;
use crate::driver::{
    Command, ControlOutcome, LiveId, SessionHandle, SessionStatus, StartError, UntrustedContext,
    decode, kill_group, terminate,
};
use crate::env::build_env_from_process;
use crate::health::LaunchVerdict;
use crate::launch::{CommsSpec, HelperCommands};
use crate::registry::{ProbeResult, ProcessRegistry, probe};
use crate::resume::{AgentIdLedger, valid_agent_session_id};
use crate::session::{BRIEF_DELIVERY_ID, SessionEvent};

/// 一次 codex 启动要的全部输入（调用方给的）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodexSpec {
    pub workdir: PathBuf,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// `Some(true)` → `serviceTier: "fast"`；`None` 不传（用 codex 自己的配置）。
    pub fast_mode: Option<bool>,
    /// 世界观（M3 渲染），走 `thread/start.developerInstructions`。
    pub developer_instructions: Option<String>,
    /// 开场任务（就绪后的第一个回合）。
    pub initial_prompt: Option<String>,
    /// 开场任务附带的不可信上下文（首轮白板）。
    pub initial_context: UntrustedContext,
    /// crew 通讯：挂到 `config.mcp_servers.crew`（`--agent codex`）。codex 没有 hook，`hooks` 字段不看。
    pub comms: Option<CommsSpec>,
    /// 额外环境变量。过白名单的禁用前缀，再剥掉 [`crate::codex::transport::BILLING_KEYS`]。
    pub additional_env: BTreeMap<String, String>,
}

/// 起一个 codex 会话要的东西。
pub struct CodexStart {
    /// crewd 的 session id（登记表的键）。
    pub session_id: String,
    /// codex 可执行文件（`AgentPaths::resolve("codex")`）。
    pub program: PathBuf,
    /// 子进程 PATH（`AgentPaths::child_path`；codex 的 npm 版要靠它找到 node）。
    pub child_path: String,
    pub spec: CodexSpec,
    pub ledger: Arc<dyn AgentIdLedger>,
}

/// 驱动层参数。
#[derive(Clone, Debug)]
pub struct CodexConfig {
    pub core: CodexCoreConfig,
    /// 定时节拍。
    pub tick: Duration,
    /// SIGTERM 之后多久升级成 `killpg(SIGKILL)`。
    pub stop_grace: Duration,
    /// 孤儿登记表；`None` = 不登记。
    pub registry: Option<Arc<ProcessRegistry>>,
    /// `initialize.clientInfo.version`。
    pub client_version: String,
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            core: CodexCoreConfig::default(),
            tick: Duration::from_millis(600),
            stop_grace: Duration::from_secs(2),
            registry: None,
            client_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// `codex app-server` 的参数（就这一个子命令；首条 prompt、世界观都走 stdin）。
pub fn codex_argv() -> Vec<String> {
    vec!["app-server".to_string()]
}

/// 起一个 codex 会话。返回时握手已完成、thread id 已落账，事件流的第一条是 `Spawned`。
pub async fn start_codex(
    start: CodexStart,
    config: CodexConfig,
) -> Result<(SessionHandle, mpsc::UnboundedReceiver<SessionEvent>), StartError> {
    let ledger = start.ledger.clone();
    let recorded = tokio::task::spawn_blocking(move || ledger.recorded())
        .await
        .map_err(|e| StartError::Ledger(io::Error::other(e)))?
        .map_err(StartError::Ledger)?;
    // 要续的那条先占住：同一个 thread 被两个 app-server 同时接着写，对话会乱。
    let live = match recorded
        .as_deref()
        .map(str::trim)
        .filter(|id| valid_agent_session_id(id))
    {
        Some(id) => Some(LiveId::claim(id)?),
        None => None,
    };
    let mut env = build_env_from_process(&start.child_path, &start.spec.additional_env);
    strip_billing_keys(&mut env);
    let spec = CodexProcessSpec {
        program: start.program.clone(),
        args: codex_argv(),
        env,
        cwd: start.spec.workdir.clone(),
    };
    let mut proc = spawn_codex(&spec).map_err(|source| StartError::Spawn {
        health: launch_health(
            LaunchVerdict::SpawnFailed,
            &format!("{}: {source}", start.program.display()),
        ),
        source,
    })?;
    if let Some(registry) = &config.registry {
        // 只登记自成进程组的（setsid 后 pgid == pid）：回收时按组杀，组号不对会杀错人。
        let recorded = match probe(proc.pid as i32) {
            ProbeResult::Found(identity) if identity.pgid == proc.pid as i32 => {
                registry.record(&start.session_id, identity)
            }
            ProbeResult::Found(identity) => Err(io::Error::other(format!(
                "刚起的进程 {} 不是自己进程组的组长（pgid {}），不登记",
                proc.pid, identity.pgid
            ))),
            other => Err(io::Error::other(format!("读不到刚起的进程身份：{other:?}"))),
        };
        if let Err(e) = recorded {
            kill_group(proc.pid);
            let _ = proc.child.wait().await;
            return Err(StartError::Registry(e));
        }
    }
    let unix_now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    let opening = Opening {
        pid: proc.pid,
        cwd: start.spec.workdir.clone(),
        options: thread_options(&start.spec),
        recorded,
        client_version: config.client_version.clone(),
        brief: start
            .spec
            .initial_prompt
            .clone()
            .filter(|p| !p.is_empty())
            .map(|text| PendingTurn {
                id: BRIEF_DELIVERY_ID,
                subject: Subject::Brief,
                text,
                context: start.spec.initial_context.clone(),
            }),
        unix_now,
    };
    let core = CodexCore::new(opening, config.core, Instant::now());
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let (tx, cmd_rx) = mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = oneshot::channel();
    tokio::spawn(run(Task {
        start,
        config,
        core,
        proc,
        cmd_rx,
        ev_tx,
        ready: Some(ready_tx),
        controls: HashMap::new(),
        next_ticket: 0,
        live,
    }));
    match ready_rx.await {
        Ok(Ok(())) => Ok((SessionHandle::new(tx), ev_rx)),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(StartError::Handshake {
            health: launch_health(LaunchVerdict::DiedSilently, "会话任务提前结束"),
        }),
    }
}

/// `CodexSpec` → `thread/start` 选项（MCP 指向 helper，`--agent codex`）。
pub fn thread_options(spec: &CodexSpec) -> ThreadOptions {
    ThreadOptions {
        model: spec.model.clone(),
        effort: spec.effort.clone(),
        fast: spec.fast_mode,
        developer_instructions: spec.developer_instructions.clone(),
        mcp_servers: spec
            .comms
            .as_ref()
            .map(|c| proto::mcp_servers(&c.helper, &HelperCommands(c).mcp_args_for("codex"))),
    }
}

struct Task {
    start: CodexStart,
    config: CodexConfig,
    core: CodexCore,
    proc: CodexProcess,
    cmd_rx: mpsc::UnboundedReceiver<Command>,
    ev_tx: mpsc::UnboundedSender<SessionEvent>,
    /// 还没告诉 `start_codex` 结果（`Some`）= 还在启动。
    ready: Option<oneshot::Sender<Result<(), StartError>>>,
    controls: HashMap<u64, oneshot::Sender<ControlOutcome>>,
    next_ticket: u64,
    /// 占着的 thread id（本进程里同一个 thread 只能跑一个）。
    live: Option<LiveId>,
}

type RecordMsg = Result<(), String>;

async fn run(mut t: Task) {
    let (record_tx, mut record_rx) = mpsc::unbounded_channel::<RecordMsg>();
    let mut reading = true;
    let mut commands_open = true;
    let mut stop_deadline: Option<tokio::time::Instant> = None;
    let mut tick = tokio::time::interval(t.config.tick);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let actions = t.core.take_actions();
        if execute(&mut t, actions, &record_tx) {
            // 启动失败：杀掉、收尸、收拾。
            kill_group(t.proc.pid);
            let _ = t.proc.child.wait().await;
            release(&mut t);
            return;
        }
        let wake = t.core.next_wakeup().map(tokio::time::Instant::from_std);
        tokio::select! {
            line = t.proc.stdout.next_line(), if reading => match line {
                Ok(Some(Line::Text(l))) => t.core.on_line(&l, Instant::now()),
                Ok(Some(Line::TooLong { bytes })) => t.core.on_dropped_line(bytes, Instant::now()),
                _ => reading = false,
            },
            status = t.proc.child.wait() => {
                let status = decode(status);
                drain(&mut t, Duration::from_millis(500)).await;
                kill_group(t.proc.pid);
                let tail = t.proc.stderr.text();
                t.core.on_exit(status, &tail, Instant::now());
                let actions = t.core.take_actions();
                release(&mut t);
                execute(&mut t, actions, &record_tx);
                if let Some(ready) = t.ready.take() {
                    let _ = ready.send(Err(StartError::Handshake {
                        health: launch_health(LaunchVerdict::DiedSilently, "会话任务提前结束"),
                    }));
                }
                return;
            }
            cmd = t.cmd_rx.recv(), if commands_open => match cmd {
                Some(c) => {
                    if handle(&mut t, c) {
                        stop_deadline = Some(tokio::time::Instant::now() + t.config.stop_grace);
                    }
                }
                None => {
                    commands_open = false;
                    t.core.stop(Instant::now());
                    terminate(t.proc.pid);
                    stop_deadline = Some(tokio::time::Instant::now() + t.config.stop_grace);
                }
            },
            _ = tick.tick() => t.core.on_tick(Instant::now()),
            _ = tokio::time::sleep_until(wake.unwrap_or_else(tokio::time::Instant::now)), if wake.is_some() => {
                t.core.on_tick(Instant::now());
            }
            _ = tokio::time::sleep_until(stop_deadline.unwrap_or_else(tokio::time::Instant::now)), if stop_deadline.is_some() => {
                stop_deadline = None;
                kill_group(t.proc.pid);
            }
            Some(result) = record_rx.recv() => t.core.on_recorded(result, Instant::now()),
        }
    }
}

/// 进程已收尸：停掉附属任务、放开 thread id、清登记表（都在发 `Exited` 之前）。
fn release(t: &mut Task) {
    t.proc.writer_task.abort();
    t.proc.stderr_task.abort();
    t.live = None;
    if let Some(registry) = &t.config.registry {
        let _ = registry.forget(&t.start.session_id);
    }
}

/// 进程退出后把 stdout 剩下的行读完（最多 `limit`）。
async fn drain(t: &mut Task, limit: Duration) {
    let deadline = tokio::time::Instant::now() + limit;
    while let Ok(Ok(Some(line))) =
        tokio::time::timeout_at(deadline, t.proc.stdout.next_line()).await
    {
        if let Line::Text(l) = line {
            t.core.on_line(&l, Instant::now());
        }
    }
}

/// 执行核心攒下的动作；返回 true = 启动失败（调用方杀进程收工）。
fn execute(
    t: &mut Task,
    actions: Vec<CodexAction>,
    record_tx: &mpsc::UnboundedSender<RecordMsg>,
) -> bool {
    for action in actions {
        match action {
            CodexAction::Write(line) => {
                let _ = t.proc.writer.send(line);
            }
            CodexAction::Record(thread) => {
                let (ledger, tx) = (t.start.ledger.clone(), record_tx.clone());
                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(move || ledger.record(&thread))
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r.map_err(|e| e.to_string()));
                    let _ = tx.send(result);
                });
            }
            CodexAction::ControlDone { ticket, outcome } => {
                if let Some(reply) = t.controls.remove(&ticket) {
                    let _ = reply.send(outcome);
                }
            }
            CodexAction::Failed(failure) => {
                let err = match failure {
                    StartFailure::Launch(health) => StartError::Handshake { health },
                    StartFailure::Ledger(e) => StartError::Ledger(io::Error::other(e)),
                };
                if let Some(ready) = t.ready.take() {
                    let _ = ready.send(Err(err));
                }
                return true;
            }
            CodexAction::Event(e) => {
                if let SessionEvent::Spawned {
                    agent_session_id, ..
                } = &e
                    && t.live.as_ref().map(LiveId::id) != Some(agent_session_id.as_str())
                {
                    // 新开的 thread（含续跑被拒后退回的那条）：占住它的号。新号不会撞，撞了也只是不占。
                    t.live = LiveId::claim(agent_session_id).ok();
                }
                if matches!(e, SessionEvent::Spawned { .. })
                    && let Some(ready) = t.ready.take()
                {
                    let _ = t.ev_tx.send(e);
                    let _ = ready.send(Ok(()));
                    continue;
                }
                let _ = t.ev_tx.send(e);
            }
        }
    }
    false
}

/// 处理一条命令；返回 true = 开始停止（要起 grace 计时）。
fn handle(t: &mut Task, cmd: Command) -> bool {
    let now = Instant::now();
    match cmd {
        Command::Send { id, text, context } => t.core.send(id, &text, context, now),
        Command::Raw(bytes) => t.core.raw(&bytes, now),
        Command::Interrupt => t.core.interrupt(now),
        // 没有终端，尺寸无意义。
        Command::Resize { .. } => {}
        Command::Stop => {
            if t.core.is_running() {
                t.core.stop(now);
                terminate(t.proc.pid);
                return true;
            }
        }
        Command::ClearQuotaHealth => t.core.clear_quota_health(now),
        Command::ScreenText { max_lines, reply } => {
            let _ = reply.send(t.core.transcript_text(max_lines));
        }
        Command::Snapshot { reply } => {
            let _ = reply.send(None);
        }
        Command::Status { reply } => {
            let _ = reply.send(SessionStatus {
                state: t.core.state(),
                working: t.core.is_working(),
                health: t.core.health().cloned(),
                pending_decision: None,
                pid: t.proc.pid,
                agent_session_id: t.core.thread_id().unwrap_or_default().to_string(),
            });
        }
        Command::Control { op, reply } => {
            let ticket = t.next_ticket;
            t.next_ticket += 1;
            t.controls.insert(ticket, reply);
            t.core.control(ticket, op, now);
        }
    }
    false
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p crewd-exec --test fake_codex`（连跑三遍）
Expected: `19 passed`，每遍 2–3s（最慢的是「停不掉要升级 SIGKILL」与两条超时）。

- [ ] **Step 5: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add crates/exec/src/codex/mod.rs crates/exec/src/codex/driver.rs crates/exec/src/bin/fake_codex.rs crates/exec/tests/fake_codex.rs
git commit -m "crewd-exec: start_codex drives codex app-server behind the shared SessionHandle; fake app-server end-to-end

Co-Authored-By: Claude <noreply@anthropic.com>"
```

- [ ] **Step 6: 验这几把尺子会红**

每刀单独做，跑 `cargo test -p crewd-exec --test fake_codex`，做完 `git checkout -- <那个文件>` 改回：
1. `codex/driver.rs` 子进程退出那一臂里删掉 `drain(…).await;` 之后的 `kill_group(t.proc.pid);` → 红 1 条：`tool_children_do_not_outlive_codex_exiting_on_its_own`。
2. `codex/transport.rs` stderr 任务的 `while` 之前插 `std::future::pending::<()>().await;` → 红 2 条：`a_stderr_flood_does_not_stall_the_handshake`、`handshake_failures_are_start_errors_and_leave_no_process`（stderr 尾巴没了，「not logged in」不在说明里）。
3. `codex/driver.rs` 删掉 `strip_billing_keys(&mut env);` → 红 1 条：`fresh_start_records_the_thread_and_runs_the_brief_with_its_context`。
4. `codex/session.rs` 的 `on_line` 里 `self.actions.push(CodexAction::Write(reply));` 改成只在 `outcome.notice.is_none()` 时写 → 红 1 条：`every_server_request_is_answered_and_the_turn_finishes`（回合挂死，10s 超时）。
5. `codex/rpc.rs` 的 `RequestId::from_value` 里 `Value::String(s) => Some(Self::Str(s.clone())),` 改成 `Value::String(_) => None,` → 红 1 条：`every_server_request_is_answered_and_the_turn_finishes`（字符串 id 的 elicitation 永远不回）。
6. `codex/session.rs` 的 `opened` 里 `Record` 之后加 `self.become_ready(now);` → 红 3 条：`fresh_start_records_the_thread_and_runs_the_brief_with_its_context`、`a_refused_resume_falls_back_to_a_fresh_thread_and_records_it`、`ledger_failure_kills_the_process_before_any_turn`。
7. `codex/driver.rs` 的 `execute` 里 `CodexAction::Write(line) => {` 那一臂在 `send` 之后加 `std::thread::sleep(Duration::from_secs(3));`（模拟写阻塞了驱动循环）→ `cargo test -p crewd-exec --test fake_codex wedge` → 红 1 条：`a_server_that_stops_reading_cannot_wedge_the_session`。
8. `codex/driver.rs` 的 `start_codex` 里 `Some(id) => Some(LiveId::claim(id)?),` 改成 `Some(_) => None,` → 红 1 条：`a_running_thread_cannot_be_resumed_twice_but_is_free_once_exited`。

`git status` 干净。

---

### Task 8: `crewd codex` 子命令与按执行器把关的账本

把 codex 执行器接到 crewd 上：`crewd codex` 前台起一个 codex 会话（拿数据目录锁、回收孤儿、`AgentPaths::discover` 找 codex、给了 `--session` 就用库里的账本，重跑 `thread/resume`），事件逐行打到 stdout（`Item` 只打类型与 id），stdin 每一行当一条指令（`/interrupt` 打断、`/compact` 压缩），Ctrl-C / SIGTERM 停掉，`--transcript` 时每次转入空闲把结构化记录打到 stderr。账本加 `expecting("codex")`（裁定 18）。这是 spec §3.3「每个 M 都以集成测试 + CLI 可驱动为完成标准」的 M2c 那一半。

测试里的「codex app-server」用 shell 脚本顶替：我们的请求 id 是确定的（`initialize` 0 号、`thread/start|resume` 1 号），脚本按顺序回包后退出。

**Files:**
- Create: `crewd/crates/crewd/src/codex_cmd.rs`、`crewd/crates/crewd/tests/codex_cli.rs`
- Modify: `crewd/crates/store/src/sessions.rs`、`crewd/crates/server/src/ledger.rs`、`crewd/crates/crewd/src/main.rs`

**Interfaces:**
- Consumes: Task 7 `codex::driver::{CodexConfig, CodexSpec, CodexStart, start_codex}`；Task 1 `driver::Control`；M2b `env::AgentPaths`、`resume::{AgentIdLedger, MemoryLedger}`、`crewd_server::{ensure_data_dir, lock_data_dir}`、M2b Task 12 的 `crewd_server::reap_orphans` 与 `crewd claude`（写计划时还没合入；Task 1 Step 1 的 grep 会提前暴露）、`crewd_server::ledger::StoreLedger`（合入版：`open()`、`to_io`）。
- Produces:
  - `crewd_store::sessions::executor(conn: &Connection, session_id: &str) -> Result<String>`（session 不存在 = `StoreError::NotFound`）
  - `crewd_server::ledger::StoreLedger::expecting(self, executor: impl Into<String>) -> Self`（行里的 `executor` 不符 → `recorded` / `record` 都报 `InvalidInput`）
  - 子命令 `crewd codex --workdir DIR [--session SID] [--prompt TEXT] [--model M] [--effort E] [--fast] [--codex PATH] [--transcript]`

- [ ] **Step 1: 写失败的测试**

`crates/crewd/tests/codex_cli.rs`（整个文件）：

```rust
//! `crewd codex` 的胶水：库里的 thread 账本 → 第一次新开、第二次 `thread/resume` 同一条。
//! 「codex app-server」用 shell 脚本顶替：按我们固定的请求顺序（initialize 是 0 号、
//! thread/start|resume 是 1 号）回包，然后退出。
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use crewd_proto::{Crew, Session};
use crewd_store::{Store, crews, sessions};

const FAKE_APP_SERVER: &str = r#"#!/bin/sh
read l
echo '{"id":0,"result":{}}'
read l
read l
case "$l" in
  *thread/resume*)
    tid=$(printf '%s' "$l" | sed 's/.*"threadId":"\([^"]*\)".*/\1/')
    echo "{\"id\":1,\"result\":{\"thread\":{\"id\":\"$tid\"}}}" ;;
  *)
    echo '{"id":1,"result":{"thread":{"id":"thr-cli-1"},"model":"fake-model"}}' ;;
esac
sleep 0.3
"#;

fn seed(data: &Path, executor: &str) {
    let s = Store::open(&data.join("crewd.db")).unwrap();
    crews::upsert(
        s.conn(),
        &Crew {
            id: "c".into(),
            title: "机组".into(),
            number: None,
            builtin: false,
            parent_ids: vec![],
            working_directory: None,
            captain_executor: None,
            hidden_at: None,
            created_at: "2026-10-01T00:00:00.000Z".into(),
            updated_at: "2026-10-01T00:00:00.000Z".into(),
        },
        None,
    )
    .unwrap();
    sessions::upsert(
        s.conn(),
        &Session {
            id: "w-1".into(),
            crew_id: "c".into(),
            role: "worker".into(),
            executor: executor.into(),
            agent_session_id: None,
            display_name: None,
            working_directory: None,
            model: None,
            effort: None,
            updated_at: "2026-10-01T00:00:00.000Z".into(),
        },
        None,
    )
    .unwrap();
}

fn run(data: &Path, fake: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_crewd"))
        .env("CREWD_DATA_DIR", data)
        // 登录 shell 抓 PATH 走失败分支：测试不依赖本机的 rc 文件。
        .env("SHELL", "/usr/bin/false")
        .args(["codex", "--session", "w-1", "--workdir"])
        .arg(data)
        .arg("--codex")
        .arg(fake)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap()
}

fn fake(data: &Path) -> std::path::PathBuf {
    let fake = data.join("fake-codex");
    std::fs::write(&fake, FAKE_APP_SERVER).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    fake
}

#[test]
fn second_run_resumes_the_thread_recorded_by_the_first() {
    let data = tempfile::tempdir().unwrap();
    seed(data.path(), "codex");
    let fake = fake(data.path());

    let first = run(data.path(), &fake);
    let out = String::from_utf8_lossy(&first.stdout);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        out.contains("agent_session_id: \"thr-cli-1\", resumed: false"),
        "{out}"
    );
    assert!(out.contains("Exited("), "{out}");
    let recorded = {
        let s = Store::open(&data.path().join("crewd.db")).unwrap();
        sessions::agent_session_id(s.conn(), "w-1").unwrap()
    };
    assert_eq!(recorded.as_deref(), Some("thr-cli-1"), "就绪之前就该落账");

    let second = run(data.path(), &fake);
    let out = String::from_utf8_lossy(&second.stdout);
    assert!(
        out.contains("agent_session_id: \"thr-cli-1\", resumed: true"),
        "{out}"
    );
    let registry = std::fs::read_to_string(data.path().join("exec-registry.json")).unwrap();
    assert_eq!(registry.trim(), "{}", "退出后登记表清空");
}

#[test]
fn a_claude_session_is_refused_and_its_resume_id_is_left_alone() {
    let data = tempfile::tempdir().unwrap();
    seed(data.path(), "claude");
    {
        let s = Store::open(&data.path().join("crewd.db")).unwrap();
        sessions::set_agent_session_id(s.conn(), "w-1", "claude-uuid").unwrap();
    }
    let out = run(data.path(), &fake(data.path()));
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("执行器是 claude"), "{err}");
    let s = Store::open(&data.path().join("crewd.db")).unwrap();
    assert_eq!(
        sessions::agent_session_id(s.conn(), "w-1")
            .unwrap()
            .as_deref(),
        Some("claude-uuid")
    );
}
```

`crates/server/src/ledger.rs` 测试模块末尾（最后一个 `}` 之前）加：

```rust
    #[test]
    fn a_ledger_expecting_codex_refuses_a_claude_session_without_touching_it() {
        let dir = tempfile::tempdir().unwrap();
        let db = seeded(dir.path());
        StoreLedger::new(db.clone(), "w-1")
            .record("claude-uuid")
            .unwrap();
        let codex = StoreLedger::new(db.clone(), "w-1").expecting("codex");
        let err = codex.recorded().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("claude"), "{err}");
        assert_eq!(
            codex.record("thr-1").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            StoreLedger::new(db.clone(), "w-1")
                .expecting("claude")
                .recorded()
                .unwrap()
                .as_deref(),
            Some("claude-uuid"),
            "claude 的会话号原样留着"
        );
    }
```

测试模块里的 `seeded()` 建的 session 执行器是 `claude`——合入版里不是的话，按合入版的写法造一个 `executor: "claude"` 的行。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p crewd-server ledger`
Expected: 编译失败，报 ``no method named `expecting` ``。

Run: `cargo test -p crewd --test codex_cli`
Expected: 运行时失败，断言里带 clap 的 ``unrecognized subcommand 'codex'``。

- [ ] **Step 3: 实现账本把关**

`crates/store/src/sessions.rs` 文件末尾追加：

```rust
/// 这个 session 的执行器类型（`claude` / `codex` / `acp:<id>` …）。session 不存在是错误。
pub fn executor(conn: &Connection, session_id: &str) -> Result<String> {
    conn.query_row(
        "SELECT executor FROM sessions WHERE id=?1",
        [session_id],
        |r| r.get(0),
    )
    .optional()?
    .ok_or_else(|| StoreError::NotFound(format!("没有 session {session_id}")))
}
```

`crates/server/src/ledger.rs`：`StoreLedger` 加一个字段，构造器里初始化，并加 `expecting`：

```rust
pub struct StoreLedger {
    db: PathBuf,
    session_id: String,
    expected_executor: Option<String>,
}
```

```rust
    pub fn new(db: PathBuf, session_id: impl Into<String>) -> Self {
        Self {
            db,
            session_id: session_id.into(),
            expected_executor: None,
        }
    }

    /// 只认这种执行器的 session：行里的 `executor` 不是它就报 `InvalidInput`，不读也不写。
    /// （拿 claude 的会话号去 `thread/resume` 必然失败，退回新 thread 时还会把 claude 的号覆盖掉。）
    pub fn expecting(mut self, executor: impl Into<String>) -> Self {
        self.expected_executor = Some(executor.into());
        self
    }
}
```

在 `fn open(&self)` 之后（同一个 `impl StoreLedger` 块里）加：

```rust
    fn check_executor(&self, store: &Store) -> io::Result<()> {
        let Some(want) = &self.expected_executor else {
            return Ok(());
        };
        let actual = sessions::executor(store.conn(), &self.session_id).map_err(to_io)?;
        if &actual != want {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "session {} 的执行器是 {actual}，不是 {want}",
                    self.session_id
                ),
            ));
        }
        Ok(())
    }
```

`AgentIdLedger for StoreLedger` 的 `recorded` 与 `record` 里，`let store = self.open()?;` 之后各加一行 `self.check_executor(&store)?;`。

Run: `cargo test -p crewd-server ledger`
Expected: 比原来多 1 条，全过。

- [ ] **Step 4: `crewd codex`**

`crates/crewd/src/codex_cmd.rs`（整个文件）：

```rust
//! `crewd codex`：前台起一个 codex 会话（M2c 的验收与调试入口；M3 起由 serve 拉起会话）。
//! 事件逐行打到 stdout；stdin 每一行当一条指令投进去（`/interrupt` 打断、`/compact` 压缩）；
//! Ctrl-C / SIGTERM 停掉会话。
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use crewd_exec::codex::driver::{CodexConfig, CodexSpec, CodexStart, start_codex};
use crewd_exec::driver::Control;
use crewd_exec::env::AgentPaths;
use crewd_exec::resume::{AgentIdLedger, MemoryLedger};
use crewd_exec::session::SessionEvent;
use crewd_server::ledger::StoreLedger;
use tokio::io::AsyncBufReadExt;

#[derive(clap::Args, Clone)]
pub struct CodexArgs {
    /// 工作目录
    #[arg(long)]
    workdir: PathBuf,
    /// crewd session id：给了就用库里的 thread 账本（行必须已存在且执行器是 codex，重跑会 thread/resume）；不给只在内存里记
    #[arg(long)]
    session: Option<String>,
    /// 开场任务（走 turn/start，经 stdin 发给 codex，绝不进 argv）
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    effort: Option<String>,
    /// 开 fast 档（不给 = 用 codex 自己的配置）
    #[arg(long)]
    fast: bool,
    /// codex 可执行文件；缺省按登录 shell 的 PATH 找
    #[arg(long)]
    codex: Option<PathBuf>,
    /// 每次转入空闲时把最近的结构化记录打到 stderr
    #[arg(long)]
    transcript: bool,
}

pub async fn run(data_dir: &Path, args: CodexArgs) -> anyhow::Result<()> {
    crewd_server::ensure_data_dir(data_dir)?;
    // 与 serve / import / claude 同一把锁：账本写库、回收孤儿都要求没有别的 crewd 活着。
    let _lock = crewd_server::lock_data_dir(data_dir)?;
    let (registry, reports) = crewd_server::reap_orphans(data_dir)?;
    for r in reports {
        eprintln!("crewd: {}", r.message);
    }
    let workdir = std::fs::canonicalize(&args.workdir)
        .with_context(|| format!("工作目录 {} 不可用", args.workdir.display()))?;
    let paths = AgentPaths::discover().await;
    let program = match &args.codex {
        Some(p) => std::fs::canonicalize(p).with_context(|| format!("找不到 {}", p.display()))?,
        None => paths
            .resolve("codex")
            .context("登录 shell 的 PATH 里找不到 codex")?,
    };
    let ledger: Arc<dyn AgentIdLedger> = match &args.session {
        Some(s) => {
            Arc::new(StoreLedger::new(data_dir.join("crewd.db"), s.clone()).expecting("codex"))
        }
        None => Arc::new(MemoryLedger::default()),
    };
    let config = CodexConfig {
        registry: Some(registry),
        ..CodexConfig::default()
    };
    let start = CodexStart {
        session_id: args.session.unwrap_or_else(|| "cli".to_string()),
        program,
        child_path: paths.child_path,
        spec: CodexSpec {
            workdir,
            model: args.model,
            effort: args.effort,
            fast_mode: args.fast.then_some(true),
            initial_prompt: args.prompt,
            ..CodexSpec::default()
        },
        ledger,
    };
    let (handle, mut events) = start_codex(start, config).await?;
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut stdin_open = true;
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        tokio::select! {
            ev = events.recv() => {
                let Some(ev) = ev else { break };
                match &ev {
                    // 条目原样太长，只打类型与 id。
                    SessionEvent::Item { turn_id, item } => println!(
                        "Item {{ turn_id: {turn_id:?}, type: {}, id: {} }}",
                        item["type"], item["id"]
                    ),
                    other => println!("{other:?}"),
                }
                if args.transcript
                    && ev == SessionEvent::Working(false)
                    && let Ok(text) = handle.screen_text(50).await
                {
                    eprintln!("----- transcript -----\n{text}\n----------------------");
                }
                if matches!(ev, SessionEvent::Exited(_)) {
                    break;
                }
            }
            line = lines.next_line(), if stdin_open => match line {
                Ok(Some(text)) if text == "/interrupt" => { let _ = handle.interrupt(); }
                Ok(Some(text)) if text == "/compact" => {
                    if let Ok(outcome) = handle.control(Control::Compact).await {
                        println!("{outcome:?}");
                    }
                }
                Ok(Some(text)) if !text.is_empty() => { let _ = handle.send(text); }
                Ok(Some(_)) => {}
                _ => stdin_open = false,
            },
            _ = tokio::signal::ctrl_c() => { let _ = handle.stop(); }
            _ = term.recv() => { let _ = handle.stop(); }
        }
    }
    Ok(())
}
```

`crates/crewd/src/main.rs` 三处：`mod claude_cmd;` 之后加 `mod codex_cmd;`；`enum Cmd` 里 `Claude(claude_cmd::ClaudeArgs),` 之后加

```rust
    /// 前台起一个 codex 会话（M2c 验收 / 调试；会话事件打到 stdout）
    Codex(codex_cmd::CodexArgs),
```

`run()` 的 `match &cli.cmd` 里 `Cmd::Claude(args) => …,` 之后加

```rust
        Cmd::Codex(args) => codex_cmd::run(&dir, args.clone()).await?,
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p crewd --test codex_cli`
Expected: `2 passed`（约 1.5s）。

Run: `cargo run -p crewd -- codex --help`
Expected: 列出 `--workdir`（必填）、`--session`、`--prompt`、`--model`、`--effort`、`--fast`、`--codex`、`--transcript`。

- [ ] **Step 6: 全量检查并提交**

Run: `./scripts/check.sh` → 0。

```bash
git add crates/store/src/sessions.rs crates/server/src/ledger.rs crates/crewd/src/codex_cmd.rs crates/crewd/src/main.rs crates/crewd/tests/codex_cli.rs
git commit -m "crewd: codex subcommand drives one codex session from the CLI; ledger refuses a session of another executor

Co-Authored-By: Claude <noreply@anthropic.com>"
```

- [ ] **Step 7: 验这把尺子会红**

`codex_cmd.rs` 里 `.expecting("codex")` 临时删掉 → `cargo test -p crewd --test codex_cli` → 红 1 条：`a_claude_session_is_refused_and_its_resume_id_is_left_alone`（claude 的号被拿去 resume）。`git checkout -- crates/crewd/src/codex_cmd.rs` 改回，`git status` 干净。

- [ ] **Step 8: 手动从命令行驱动一次（不用真 codex）**

```bash
cargo build -p crewd-exec --bin fake_codex
D=$(mktemp -d)
printf '第二条指令\n' | FAKE_CODEX_MODE= cargo run -q -p crewd -- --data-dir "$D/data" codex --workdir "$D" --codex target/debug/fake_codex --prompt "第一条" --transcript
```

Expected：stdout 依次出现 `Spawned { … resumed: false }`、`Profile(…)`、`Delivery { id: 0, … outcome: Delivered }`、`TurnEnded { last_agent_message: "GOT: 第一条" }`、`Delivery { id: 1, … }`、`TurnEnded { … "GOT: 第二条指令" }`；stderr 的 transcript 块里有 `› 第二条指令` 与 `GOT: 第二条指令`。stdin 读完后会话不退出——按 Ctrl-C，应看到 `Exited(ExitInfo { … stopped_by_user: true … })` 后进程结束。（`FAKE_CODEX_MODE` 传不到子进程——执行器只放行显式给的变量——这里写出来只是提醒：`crewd codex` 不转发环境，假 app-server 走缺省行为。）

---

### Task 9: 真机冒烟（opt-in）与交付记录

两条真 codex 冒烟，都是 `#[ignore]` + `CREWD_LIVE_CODEX`：`=1` 只握手开 thread（不开回合、不花额度），验 schema 核不到的那几件（不带 params 的 `initialized`、`optOutNotificationMethods`、`config.mcp_servers` 与 `developerInstructions` 被接受）；`=turn` 跑一个最小回合，验 Swift 一直没真机验过的「`additionalContext` 真的被模型看见」。然后写交付记录。

**Files:**
- Create: `crewd/crates/exec/tests/live_codex.rs`
- Modify: `crewd/README.md`、`docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`、`docs/superpowers/specs/2026-10-05-crewd-m2-executor-porting-map.md`

**Interfaces:**
- Consumes: Task 7 的启动入口；M2b `env::AgentPaths`、`launch::CommsSpec`、`resume::MemoryLedger`。
- Produces: 无（测试与文档）。

- [ ] **Step 1: 写冒烟测试**

`crates/exec/tests/live_codex.rs`（整个文件）：

```rust
//! 真 codex 冒烟。两道闸：`#[ignore]` + 环境变量；`check.sh` 不带 `--ignored`，永远不跑。
//!
//! - `CREWD_LIVE_CODEX=1`：只握手——initialize、thread/start、拿到 thread id，**不开回合**
//!   （不花额度）。验的是 schema 上核不到的那些：0.160 的 app-server 认不认我们的
//!   `initialize`（不带 params 的 `initialized`、`optOutNotificationMethods`）、`thread/start`
//!   带 `config.mcp_servers`（指向 `/usr/bin/true`：MCP 起不来，推测不挡握手，未核实）与
//!   `developerInstructions`。
//!   注意：thread/start 可能在 `~/.codex/sessions` 下留一条空 thread 的记录（未核实）。
//! - `CREWD_LIVE_CODEX=turn`：再跑一个最小回合（花几百个 token）：白板经
//!   `additionalContext`（`kind: "untrusted"`）送进去，问它暗号——验 Swift 一直没真机验过的
//!   「additionalContext 真的被模型看见」。**跑之前要人点头。**
//!
//! 工作目录取 `CREWD_LIVE_CODEX_WORKDIR`，缺省是一个临时目录。
//! 跑法：`CREWD_LIVE_CODEX=1 cargo test -p crewd-exec --test live_codex -- --ignored --nocapture`
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crewd_exec::codex::driver::{CodexConfig, CodexSpec, CodexStart, start_codex};
use crewd_exec::env::AgentPaths;
use crewd_exec::launch::CommsSpec;
use crewd_exec::resume::MemoryLedger;
use crewd_exec::session::{DerivedState, SessionEvent};

fn gate() -> Option<String> {
    match std::env::var("CREWD_LIVE_CODEX") {
        Ok(v) if v == "1" || v == "turn" => Some(v),
        _ => {
            eprintln!("跳过：没设 CREWD_LIVE_CODEX=1（或 =turn）");
            None
        }
    }
}

async fn start(
    brief: Option<&str>,
) -> (
    crewd_exec::driver::SessionHandle,
    tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
    tempfile::TempDir,
) {
    let tmp = tempfile::tempdir().unwrap();
    let workdir = std::env::var_os("CREWD_LIVE_CODEX_WORKDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| tmp.path().to_path_buf());
    let paths = AgentPaths::discover().await;
    let program = paths
        .resolve("codex")
        .expect("登录 shell 的 PATH 里找不到 codex");
    eprintln!("codex = {}", program.display());
    let start = CodexStart {
        session_id: "live".into(),
        program,
        child_path: paths.child_path,
        spec: CodexSpec {
            workdir,
            developer_instructions: Some("你在一次自动化冒烟测试里。回答尽量短。".into()),
            initial_prompt: brief.map(str::to_string),
            initial_context: [(
                "crew_whiteboard".to_string(),
                "测试白板：今天的暗号是「青蛙七号」。".to_string(),
            )]
            .into_iter()
            .collect(),
            comms: Some(CommsSpec {
                helper: PathBuf::from("/usr/bin/true"),
                data_dir: tmp.path().to_path_buf(),
                crew_id: "live".into(),
                session_id: "live".into(),
                captain: false,
                structured_post: false,
                label: None,
                hooks: false,
            }),
            ..CodexSpec::default()
        },
        ledger: Arc::new(MemoryLedger::default()),
    };
    let (h, rx) = start_codex(start, CodexConfig::default())
        .await
        .expect("真 codex 握手失败");
    (h, rx, tmp)
}

#[tokio::test]
#[ignore = "真 codex：CREWD_LIVE_CODEX=1 cargo test -p crewd-exec --test live_codex -- --ignored"]
async fn live_codex_handshakes_and_opens_a_thread_without_a_turn() {
    if gate().is_none() {
        return;
    }
    let (h, mut rx, _tmp) = start(None).await;
    let first = rx.recv().await.unwrap();
    eprintln!("{first:?}");
    let SessionEvent::Spawned {
        agent_session_id,
        resumed: false,
        ..
    } = first
    else {
        panic!("第一条应是 Spawned：{first:?}");
    };
    assert!(!agent_session_id.is_empty());
    tokio::time::sleep(Duration::from_secs(2)).await;
    let status = h.status().await.unwrap();
    eprintln!("{status:?}");
    assert_eq!(status.state, DerivedState::Idle);
    h.stop().unwrap();
    while let Some(e) = rx.recv().await {
        eprintln!("{e:?}");
        if let SessionEvent::Exited(info) = e {
            assert!(info.stopped_by_user);
            return;
        }
    }
    panic!("事件流结束前没有 Exited");
}

#[tokio::test]
#[ignore = "真 codex 回合（花额度）：CREWD_LIVE_CODEX=turn cargo test -p crewd-exec --test live_codex -- --ignored"]
async fn live_codex_turn_sees_the_untrusted_whiteboard_context() {
    if gate().as_deref() != Some("turn") {
        eprintln!("跳过：回合测试要 CREWD_LIVE_CODEX=turn");
        return;
    }
    let (h, mut rx, _tmp) = start(Some("白板里的暗号是什么？只回答暗号本身。")).await;
    let answer = tokio::time::timeout(Duration::from_secs(180), async {
        while let Some(e) = rx.recv().await {
            eprintln!("{e:?}");
            if let SessionEvent::TurnEnded { last_agent_message } = e {
                return last_agent_message;
            }
        }
        panic!("事件流结束前没有 TurnEnded");
    })
    .await
    .expect("180s 内回合没结束");
    assert!(
        answer.contains("青蛙七号"),
        "模型没看到 additionalContext：{answer}"
    );
    h.stop().unwrap();
}
```

- [ ] **Step 2: 确认默认不跑**

Run: `cargo test -p crewd-exec --test live_codex`
Expected: `0 passed; 0 failed; 2 ignored`。再跑一次 `./scripts/check.sh` → 0，并在输出里确认 `live_codex` 那一行是 `2 ignored`。

```bash
git add crates/exec/tests/live_codex.rs
git commit -m "crewd-exec: opt-in live codex smoke tests (handshake only, and one turn reading untrusted context)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

- [ ] **Step 3: 真机握手（要人点头才跑）**

先问人：「要在本机起一次真 codex app-server、开一个空 thread（不开回合、不花额度；可能在 `~/.codex/sessions` 下留一条空 thread 的记录），可以吗？」得到同意才跑：

```bash
codex --version
CREWD_LIVE_CODEX=1 cargo test -p crewd-exec --test live_codex live_codex_handshakes -- --ignored --nocapture
```

Expected：`1 passed`，输出里有 `Spawned { … resumed: false }`、`Profile(…)`（codex 自己解析出的模型）、`Idle`、`Exited(… stopped_by_user: true …)`。失败时把完整输出贴进交付记录，按「未核实」表逐条对：多半是某个字段 0.160 不认。

- [ ] **Step 4: 真机回合（花额度，要人单独点头）**

再问人：「要跑一个最小的真回合（几百 token），验证白板经 additionalContext 真的被模型看见，可以吗？」同意才跑：

```bash
CREWD_LIVE_CODEX=turn cargo test -p crewd-exec --test live_codex live_codex_turn -- --ignored --nocapture
```

Expected：`1 passed`，`TurnEnded` 的正文里有「青蛙七号」。不通过说明 `additionalContext` 没被渲染进上下文（Swift 的 tech-debt 一直挂着这条）——记进交付记录，**不要**改成把白板拼进正文来让它过（那会丢掉「不可信」包装，是 spec §16 不走 codex-acp 的理由之一）。

- [ ] **Step 5: 交付记录**

`crewd/README.md`：在 M2b 加的「前台起一个 claude 会话」那一行之后加两行：

```markdown
- codex 执行器：`crates/exec/src/codex`（`codex app-server` 客户端：握手、thread 落账与续跑、回合、服务端请求全答、协议健康、进程组；对外仍是 `driver::SessionHandle` + 事件流）
- 前台起一个 codex 会话：`cargo run -p crewd -- codex --workdir DIR [--session SID] [--prompt TEXT] [--transcript]`（给了 `--session` 则 thread id 记在库里那一行上，重跑自动 `thread/resume`；那一行的执行器必须是 `codex`；会拿数据目录锁，不能与 serve 同时跑）
```

移植图 `docs/superpowers/specs/2026-10-05-crewd-m2-executor-porting-map.md` §5 第一条里的

```markdown
codex 用 Foundation Process，是否独立进程组**未核实**（不是则 killpg 失败只杀父进程）。
```

换成

```markdown
codex 用 Foundation Process，不在独立进程组（Swift 的 killpg 只杀得到父进程）。crewd（M2c）里 codex 由 setsid 起、自成会话与进程组，停止与退出后都对整组 SIGKILL。
```

spec `docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md` 末尾追加一节，**编号 = 现有最大节号 + 1**（写计划时最大是 §17「机组内多群」，M2b 的交付记录也会追加一节，以执行时为准），并按 Step 3 / Step 4 的实际结果把两处「二选一」改成一句真话（版本号照实填；没跑就留「未跑」那句）：

```markdown
## N. M2c 交付记录与遗留（分支 crewd-m2c，计划 `docs/superpowers/plans/2026-10-06-crewd-m2c-codex-session.md`）

已交付：`crewd/crates/exec/src/codex`——线协议（无 `jsonrpc`、整数与字符串 id、两套 id 分表、单请求期限）、参数构造与服务端请求答复表（0.160.1 的 11 个方法全覆盖）、协议健康、回合跟踪与自动压缩、sans-IO 会话核心 `CodexCore`、进程（setsid、白名单环境剥计费 key、stderr 排空、stdout 行上限、写任务）、驱动 `start_codex`（握手 + thread id 落账完成才返回）。窄接口的最小扩展：`send_with_context`、`interrupt`、`control`、`snapshot` 可空、`SessionEvent::{Item, TurnEnded, Notice, Profile}`、`HealthKind::{TurnFailed, VersionIncompatible}` 与共用的 `HealthKind::rank`、`StartError::Handshake`。`crewd codex` 子命令；`StoreLedger::expecting`。验收：假 app-server 上的端到端 19 条（握手、落账先于就绪、续跑与退回、每一种服务端请求、额度、请求超时、stderr 灌满、落账失败、停止与工具子命令、codex 自己退出后的子命令、自动压缩与排队、打断、不读 stdin、3MB 条目、控制、登记表、句柄全丢、同一 thread 不能同时跑两个）；`crewd codex` 两次运行之间经库续跑、claude 的 session 被拒。

与 Swift 不同的决定（理由见计划「裁定」）：
- 服务端请求 id 支持字符串；`applyPatchApproval` / `execCommandApproval` 显式按审批拒；`currentTime/read` 照实回。
- 单请求期限（initialize 30s、thread 60s、其余 30s）；「早到响应缓冲」由「先登记后写 + 单任务顺序处理」在结构上消掉。
- 回合排队、不用 `turn/steer`；`TurnEnded` 只带本回合的最后一条 agent 正文。
- `serviceTier` 不选不传；`initialized` 不带 params；多订阅退两种输出增量。
- setsid + 退出后整组 SIGKILL；剥 `OPENAI_API_KEY` 之外再剥 `CODEX_API_KEY`、`CODEX_ACCESS_TOKEN`。
- thread id 落账失败 = 启动失败（Swift 只回调、不拦）。

真机核对：
- 【执行 Task 9 Step 3 后二选一，删掉另一条】已跑握手：codex-cli <版本>，`CREWD_LIVE_CODEX=1 … live_codex_handshakes … --ignored` 通过。/ 未跑：握手只在假 app-server 上验过；跑法见 `crewd/crates/exec/tests/live_codex.rs` 文件头。
- 【执行 Task 9 Step 4 后二选一，删掉另一条】已跑回合：模型答出了白板里的暗号，`additionalContext`（`kind: untrusted`）被看见。/ 未跑：`additionalContext` 是否真的进了模型上下文仍未真机验证（Swift 起就挂着）。

遗留：
- `turn/steer`（运行中插话）没接，M3 定语义后再说；现在一律排队。
- 白板「未读」的取舍与去重归 M3；codex 自己按 key 去重（值没变就不再注入），claude 一侧每次都包。
- `Item` 事件只发不存（结构化记录只在内存里留最近 200 条）；attach 回放、背压归 M2d。
- `Notice` 只发事件；转群（Swift Todo #6 的群通知）归 M3。
- 原生默认模型探测（ephemeral `thread/start`）、codex 的 npm 托管回退没做。
- `CODEX_HOME` 不在环境白名单里：用了自定义 `CODEX_HOME` 的机器上，crewd 起的 codex 读的是 `~/.codex`（见计划「待人拍板」）。
- 没在 Linux 上编译过。
```

- [ ] **Step 6: 提交文档**

```bash
git add README.md ../docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md ../docs/superpowers/specs/2026-10-05-crewd-m2-executor-porting-map.md
git commit -m "docs: M2c delivery record, codex process group in porting map, README

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## 覆盖对照（M2c 范围 → 任务）

| 范围项（来源） | 任务 |
|---|---|
| 同一个窄接口；codex 不适合的地方最小扩展并说明（spec §13 Q12、§16） | 1（裁定 1）、7 |
| stdio JSON-RPC：无 `jsonrpc`、两套 id 分表、早到回应、字符串 id（移植图 §4） | 2 |
| 单请求期限（Swift 没有） | 2、5、7 |
| stderr 排空（移植图 §4） | 6、7 |
| 握手 `initialize` / `initialized`（移植图 §4） | 3、5 |
| `thread/resume{excludeTurns}` 失败退回 `thread/start` 并公告（移植图 §4） | 3、5、7 |
| `thread/start` 参数：cwd、on-request、workspace-write、auto_review、developerInstructions、model、effort、serviceTier、MCP 指向 helper（移植图 §4、§1） | 3、7 |
| `turn/start` + `additionalContext` 不可信白板（移植图 §4、spec §2） | 3、5、7、9 |
| `turn/interrupt`、`thread/settings/update`（fast 等回显）、`thread/compact/start` + 自动压缩阈值（移植图 §4） | 3、4、5、7 |
| 每一条服务端请求都回（审批 -32000、elicitation、account、其余 -32601）（移植图 §4） | 3、5、7 |
| 回合跟踪、isBusy、idle 边沿、Stop hook 对应物（移植图 §4、§2） | 4、5、7 |
| 协议健康：限流快照、usageLimitExceeded、unauthorized、版本太旧、失败回合（移植图 §4、§6） | 3、5、7 |
| 退出原因 hitLimit（移植图 §6） | 5、7 |
| 剥 `OPENAI_API_KEY`（移植图 §1） | 6、7 |
| 进程组与停止语义（移植图 §5） | 6、7 |
| thread id 进会话号账本、重启续跑（spec §13 Q12、移植图 §5） | 5、7、8 |
| 孤儿登记（移植图 §5） | 7 |
| 假 app-server 器材 + 端到端 | 7 |
| 真机冒烟 opt-in、不进 check.sh | 9 |
| CLI 可驱动（spec §3.3） | 8 |

## 未核实（写计划时核不到的字段与行为）

依据：`codex app-server generate-json-schema --experimental`（codex-cli 0.160.1，本机，2026-10-06）——方法名、参数与回包字段、枚举值都对过；下表是 schema 管不到的。官方网页文档没取。

| 项 | 本计划的用法 | 证据 | 怎么核 |
|---|---|---|---|
| `thread/start.config` 里的 `model_reasoning_effort`、`mcp_servers.<名>.{command,args}` 被采纳 | effort 与 crew MCP 都挂在这里 | schema 里 `config` 是自由对象；两个键名在 0.160.1 二进制字符串里（47 / 28 处）；Swift 在 0.137 真机验过 | Task 9 Step 3 只验「被接受」；MCP 真的挂上要等 M3 的 `crewd mcp` |
| `_meta.codex_approval_kind == "mcp_tool_call"` | elicitation 接受 / 拒绝的分界 | schema 里 `_meta` 是任意值；`codex_approval_kind`、`mcp_tool_call` 在二进制字符串里；Swift 在 0.137 真机见过 | 真回合里调一次 crew 工具（M3） |
| 「requires a newer version of Codex」 | 版本太旧的判定 | 服务端原话，二进制里没有；Swift 观察到的 | 只能等它真出现 |
| `serviceTier` 取值 `fast` / `priority` / `default` | fast 档的判定 | schema 里是自由字符串；Swift 的映射 | Task 9 Step 3 的 `Profile` 读数 |
| `initialized` 不带 `params` 被接受 | 握手第二步 | schema 的 InitializedNotification 只有 `method`；Swift 发的是 `params: {}` | Task 9 Step 3 |
| `optOutNotificationMethods` 里两种输出增量真的被退订 | 少收大流量 | 方法名在 schema 的通知表里；退订效果没验 | 真回合跑个长命令，看有没有 outputDelta |
| `additionalContext`（`kind: untrusted`）被渲染成 `<external_键>` 并进入模型上下文 | 白板通道；claude 一侧的包装照这个形状 | schema 有字段与 `untrusted` / `application` 两个值；渲染形状来自 Swift 注释（读 codex 源码所得），Swift 自己标了「需真机确认」 | Task 9 Step 4 |
| 回合进行中再发 `turn/start` 的行为 | 不发（排队，裁定 7） | 未知 | 不需要 |
| `thread/start` 不开回合是否落盘、只开过 thread 的 id 能否 `thread/resume` | 握手后即落账 | 未知；接不上时退回新 thread（裁定 3） | 真机：握手后停、再用同号起 |
| app-server 收到 SIGTERM 是否干净退出、是否带走工具子进程 | 停止先 TERM 后整组 KILL | 未知；我们总补刀 | 不需要 |
| `currentTime/read` 回 `currentTimeAt`（Unix 秒） | 照实回 | schema（CurrentTimeReadResponse） | 只能等它真来 |

## 待人拍板（不影响执行；执行时按「当前做法」做）

1. **运行中插话**：现在一律排队到回合结束（裁定 7）。要不要在 M3 用 `turn/steer` 让人 / 机长的话插进正在跑的回合？
2. **`CODEX_HOME`**：环境白名单不含它（M2b 定的白名单），用了自定义 `CODEX_HOME` 的机器上 crewd 起的 codex 会读 `~/.codex`、登录态对不上。当前做法：不透传；要的话调用方经 `additional_env` 传。是否把它加进白名单？
3. **真机冒烟**：Task 9 Step 3（只握手）与 Step 4（花几百 token）各要一次点头。
