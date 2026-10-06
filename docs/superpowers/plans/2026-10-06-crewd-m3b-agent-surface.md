# crewd M3b agent 面（`crewd mcp` / `crewd hook` / 工具 / 世界观）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让每个 agent session 通过 crewd 说话：`crewd mcp`（rmcp 写的 stdio MCP server，一根哑管道）与 `crewd hook <post-tool-use|pre-tool-use|stop>` 连 `crewd.sock`、用 token 认身份；serve 里的 `server::agent` 按「此刻的角色」给工具表、执行工具、注入未读、拦审批、在停在问句上时代发；工具描述、回执、世界观、注入标注全部放在 `crates/collab/prompts/*.md`，代码里没有给 agent 看的字面量。

**Architecture:** 三层。**线**：`crewd-proto::agent`（`agent.attach / tools / call / hook` 四个方法 + 只推给 attach 连接的 `agent.tools_changed`）。**helper**（`crewd` 二进制里的 `agent_client` + `mcp_cmd` + `hook_cmd`）：不懂业务，只转发——MCP 侧用 rmcp 3.5 手写 `ServerHandler::list_tools / call_tool`（不用 `#[tool]` 宏，工具表来自 serve）。**serve**（`crewd-server::agent`）：`rpc`（attach 状态、角色变了推 tools_changed）、`catalog` + `text`（工具表与 md 文字）、`port`（**唯一**直接调 M3a 接口的文件）、`inject` / `approval` / `turn`（三个 hook）、`tools/*`（工具实现）。collab 里 M3b 拥有两个文件：`worldview`（世界观渲染 + tools md 的加载）与 `turn_trace`（停在问句上的判定与代发，记账在 `meta` 表）。可见性、「发给谁的」标注、注入渲染用 M3a 的 `cursors` / `render`。起进程、谁醒、准入都不在 M3b：起停经 M3a 定死的 `SessionControl`（M3c 实现），唤醒只入队（`WakeRequest`）。

**Tech Stack:** Rust 1.94（edition 2024）、tokio 1.53、serde / serde_json、schemars 1（已在 lock 里）、**rmcp 3.5.1**（features `server` + `transport-io`，关默认特性；这是 M3b 唯一新增的第三方依赖，会带进 futures / tokio-util / tracing / thiserror / indexmap / pin-project-lite / pastey 等，见 Task 8 Step 1）、crewd-exec（M2b）、crewd-work（M4a，只经 M3a 的 asks）、crewd-collab / crewd-store `Db`（M3a）。

**Spec:** `docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`——§9（工具总表）、§17（机组内多群）、§18（M2b 遗留：helper 命令行、hooks 等 M3）、§20（提示词放 md 的做法）、**§22 全部**（§22.3 是本计划的需求，§22.5 是能用的全部接口，§22.7 是人的拍板，2026-10-06 commit fdc884e）。Swift 对照（仓库根）：`Sources/Mcp/{McpServer,McpHelperMain,HookEmitter,McpPermissionHook,McpTurnHook,SessionTurnTrace}.swift`、`Sources/Stores/{CrewWhiteboardVisibility,LocalSessionWorldModel}.swift`、`Sources/Mac/LocalRunner/LocalSessionLaunch.swift:255-303`、`Resources/Prompts/session-world-model.{zh,en}.md`。

> **前提**：M3a 已合进 main（spec §22.1）。M3b 用的 M3a 名字全部取自 M3a 计划 `docs/superpowers/plans/2026-10-06-crewd-m3a-data-and-api.md` 的「冻结给 M3b / M3c 的接口」块（与 §22.5 不同处以那里为准），直接调用集中在 `crates/server/src/agent/port.rs`、`agent/testkit.rs` 与 collab 里 M3b 拥有的两个文件。**Task 1 Step 1 先对着落地的 M3a 代码逐条核对；对不上只改 `port.rs` / `testkit.rs`；缺的口子（下面 G 列表）按 §22.1「缺 → 改 M3a」回报控制者，不在 M3b 里自己补表、补 store 函数。**
>
> 计划里的代码没有在副本里重放过（M3a 还没落地，编不过）。各「确认失败」步骤写的是预期，执行时以真跑为准；数字对不上先查，别改期望凑数。

## Global Constraints

- 所有命令在 `crewd/` 目录下跑；Rust 工具链在 `/opt/homebrew/opt/rustup/bin`（找不到 `cargo` 先 `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`）。
- 每个任务结束跑 `./scripts/check.sh`（fmt + clippy -D warnings + test）全绿再提交。**绝不**带 `--ignored` 跑（真 claude 冒烟只在 Task 16 Step 9，人点头后单独跑那一条）。
- 提交：只 `git add` 该任务列出的具体路径（相对 `crewd/`，加文档时相对仓库根），**不用** `git add -A` / `git add .` / 目录通配；提交前 `git diff --cached --stat` 看行数对不对；提交信息末尾一行 `Co-Authored-By: Claude <noreply@anthropic.com>`（不带模型名）；**不 amend**；不 push。
- 依赖用 `cargo add` 加，不手写版本号；rmcp 是唯一的新第三方 crate（Task 8）。`Cargo.lock` 要提交。
- **测试绝不起真 claude**：helper 的端到端是测试自己照 `mcp.json` / `settings.json` 起 `crewd mcp`、用 `sh -c` 跑 hook 命令（Task 16）；真 claude 只在 `tests/live_agent.rs`（`#[ignore]` + `CREWD_LIVE_CLAUDE=1` 两道闸）。
- **绝不按名字 / 模式杀进程**（不用 `pkill` / `killall` / `pgrep | xargs kill`）：测试里起的子进程用 `Child` 句柄自己收，`kill_on_drop(true)`；排障时只按 pid 发信号。
- 仓库公开：测试只用合成数据（`crew-a`、`s-cap`、「示例任务」之类），不读真实数据目录、不读 `~/.claude`。
- 文件归属（spec §22.5）：M3b 只改 `crates/crewd/src/{mcp_cmd,hook_cmd,agent_client}.rs` 与 `main.rs` 的子命令接线、`crates/server/src/agent/**`、`crates/server/src/conn.rs` 的 agent 路由（Task 7，见「与 spec 的出入」第 4 条）、`crates/server/src/lib.rs` 的 `pub mod agent;` 与 `AppState` 一个字段、`crates/collab/src/{worldview,turn_trace}.rs`、`crates/collab/prompts/{world-model,tools}.{zh,en}.md`、`crates/exec/src/launch.rs`、`crates/proto/src/agent.rs`（新）与 `proto/src/lib.rs` 的 `pub mod agent;` + `api_schema` 一行。**不动** `crates/wake/`、`server/src/orchestra/`、serve 启动那一段、store 的 schema。契约 json（`crewd/schema/crewd-api.schema.json`）由 M3b / M3c 中后合者重新生成，不挑边合并。
- 给 agent 看的文字（工具描述、参数说明、回执、错误、注入标注、世界观、代发稿）**只**在 md 里；`server/src/agent/**` 与 `collab/src/{worldview,turn_trace}.rs` 的非测试代码里不得出现中日韩字符（Task 3 的扫描测试钉住）。给开发者看的（stderr、RPC 层错误）写英文短句。
- 时间：`collab` 两个文件不读时钟，`now` 由调用方传；`server::agent` 用 `crate::agent::port::now()` 一处取时钟。

## 查过的官方文档（2026-10-06）

| 来源 | 确认了什么 |
|---|---|
| crates.io API `https://crates.io/api/v1/crates/rmcp`（与 `/3.5.1`、`/3.5.1/dependencies`） | 最新 = 3.5.1（2026-10-05 发布，MSRV 1.88，仓库 modelcontextprotocol/rust-sdk）；`default = [base64, macros, server]`；`server = [transport-async-rw, schemars, dep:pastey, uuid]`；`transport-io = [transport-async-rw, tokio/io-std]` |
| `https://docs.rs/rmcp/3.5.1/rmcp/handler/server/trait.ServerHandler.html` 与源码页 `src/rmcp/handler/server.rs.html` | `fn get_info(&self) -> ServerConfig`；`fn list_tools(&self, Option<PaginatedRequestParams>, RequestContext<RoleServer>) -> impl Future<Output = Result<ListToolsResult, McpError>> + MaybeSendFuture + '_`；`fn call_tool(&self, CallToolRequestParams, RequestContext<RoleServer>) -> … Result<CallToolResponse, McpError>`；都有默认实现 |
| docs.rs `model::{InitializeResult, ServerCapabilities, Implementation, ListToolsResult, Tool, CallToolResult, CallToolResponse, ContentBlock, TextContent, ErrorData, CallToolRequestParams}` | `ServerConfig` = `InitializeResult` 的别名（`#[non_exhaustive]`，`InitializeResult::new(caps).with_server_info(..)`）；`ServerCapabilities::builder().enable_tools().enable_tool_list_changed().build()`；`Implementation::new(name, version)`（non_exhaustive）；`ListToolsResult::with_all_items(Vec<Tool>)`；`Tool::new(name, description, input_schema)`（non_exhaustive，`input_schema: Arc<JsonObject>`）；`CallToolResult::{success, error}(Vec<ContentBlock>)`、`impl From<CallToolResult> for CallToolResponse`；`ContentBlock::text(..)`、`as_text() -> Option<&TextContent>`、`TextContent.text`；`CallToolRequestParams { name: Cow<str>, arguments: Option<JsonObject>, .. }`；`ErrorData::internal_error(msg, Option<Value>)` |
| `https://docs.rs/rmcp/3.5.1/rmcp/transport/io/fn.stdio.html`、`service/struct.{RunningService,Peer}.html`、crate 首页 | `pub fn stdio() -> (Stdin, Stdout)`（要 `transport-io` + `server`）；`svc.serve(stdio()).await?` 得 `RunningService`，`.peer() -> &Peer<R>`、`.waiting().await`；`Peer` 是 Clone，`notify_tool_list_changed(&self) -> Result<(), ServiceError>` |
| `https://raw.githubusercontent.com/modelcontextprotocol/rust-sdk/main/README.md` | 手写 `ServerHandler`（不用宏）的写法；**注意** README 里 `call_tool` 还写着返回 `CallToolResult`，3.5.1 的 docs.rs 是 `CallToolResponse`——以 docs.rs 为准（Task 8 编译时会立刻暴露） |
| `https://code.claude.com/docs/en/hooks` | 配置 `{"hooks":{"<Event>":[{"matcher":..,"hooks":[{"type":"command","command":..,"timeout":秒}]}]}}`；**command hook 默认 timeout 600 秒**（UserPromptSubmit 等降到 30）；超时被取消、输出丢弃，**PreToolUse 超时不挡工具调用**（「don't count on a stalled hook to act as a gate」）；matcher 省略 / `""` / `"*"` = 全部，只含字母数字 `_ - , | 空格` = 精确匹配，含其它字符 = 不锚定的 JS 正则（MCP 工具名 `mcp__<server>__<tool>`）；没有 `args` 时 `command` 经 `sh -c` 跑（有 `args` 是 exec 形式）；所有匹配的 hook 并行跑；公共输入 `session_id / prompt_id（第一条用户输入前没有）/ transcript_path / cwd / permission_mode / hook_event_name / agent_id（只在子 agent 里）`；PreToolUse 输入 `tool_name / tool_input / tool_use_id`，输出 `hookSpecificOutput.{hookEventName:"PreToolUse", permissionDecision: allow|deny|ask|defer, permissionDecisionReason}`；PostToolUse 输出 `hookSpecificOutput.{hookEventName:"PostToolUse", additionalContext}`；Stop 输入含 `stop_hook_active / last_assistant_message`，不给 `decision` 就什么都不发生；exit 0 时 stdout 以 `{` 开头 `}` 结尾才按 JSON 解析，坏 JSON = 非阻塞错误；exit 2 = 阻塞；**`additionalContext` 每字段上限 10,000 字符**，超了存文件、只给 2,000 字符预览 |
| `https://code.claude.com/docs/en/mcp` | `--mcp-config` 文件 `{"mcpServers":{"<name>":{"type":"stdio","command":..,"args":[..],"env":{..}}}}`（缺 `type` 按 stdio 读，我们显式写）；支持 `list_changed` 通知动态刷新工具表；MCP 工具输出超 10,000 token 警告、默认上限 25,000 token |

没核到的（写进「未核实」）：rmcp 3.5.1 的 `Tool::new` 泛型约束原文、`ServerHandler` 是否要求 `Clone`（README 例子派生了 `Clone`，我们也派生）、`notify_tool_list_changed` 在 claude 2.1.x 上是否立刻刷新（文档说 interactive 模式自动刷新，没实测）。

## 人的拍板（spec §22.7，2026-10-06）与 M3b 的落点

| 题 | 人的决定 | M3b 怎么做 |
|---|---|---|
| Q1 重启后接回 | 问人，M3c 实现 | M3b 无改动 |
| Q2 群主与叫谁 | 每个群有群主（默认建群者，可转让：双方确认或机长强制；大厅 / 人类群的群主 = 机组机长）；人不 @ 发言由 Jev 判，缺席 / 低置信交群主；session 可 `channel_listen` 收听 | 工具 `channel_transfer(channel, to, force?)` / `channel_accept(channel)`、`channel_listen(channel, minutes?, senders?)` / `channel_unlisten(channel)`（取代 §22.3 表里的 `listen`）；归档权限 = 群主或机长；世界观写「每个群有群主、群主相当于这个群的机长；人不 @ 的话由调度判断叫谁，判不了交群主」。群主与转让由 M3a 提供（`Channel.owner`、`channels::{request,accept,withdraw}_owner_transfer`、`force_owner`） |
| Q3 私有群 | 不做 | 不提供私有选项；`read_channel` 可读本机组 `channel.list` 列出的任何群，注入 / `search` 只看自己在的群与收听的群 |
| Q4 旧工具 | `raise_attention` / `clear_attention` / `crew_ordering_signals` / `arrange_crews` 删 | 工具表里没有；`catalog` 测试断言它们不存在 |
| Q5 并发上限 | 默认不设上限，防异常唤醒循环（M3c） | `start_session` 回执不提排队；不在 M3b 里做任何限流 |
| 侧栏排序（2026-10-06 追加） | 手动拖拽（`crews.position`），没有任何自动排序 | 不提供排序工具；世界观不提排序 |
| 协作用群、不开子机组（2026-10-06 追加） | 话题 / 任务协作用机组里的群；子机组只表达组织结构（部门 / 团队） | 世界观「机组与群」一节、`channel_create` / `create_child_crew` / `create_parent_crew` 的描述都这么引导；Task 3 `child_crew_text_steers_collaboration_to_channels`、Task 4 `old_mechanisms_are_gone_and_new_rules_are_present` 钉住 |

## 待人拍板（按推荐默认写，每条都隔离在一处）

§22.7 的 Q1–Q5 人已拍板（上表）。写计划时新冒出来、按推荐默认写进去的：

- **P1 审批是否阻塞**：spec §22.3 写「等结果，最多 30min，到点 deny」，Swift #75 ② 起是「当场拒 + 开人类 Todo + 一次性放行票」。推荐照 spec（阻塞；审批台在客户端，人看得见在等什么）。隔离点：`AgentConfig.approval_wait`（`agent/runtime.rs`）+ `agent/approval.rs` 一个文件；改成不阻塞 = 等待设 0 并补放行票，只动这两处。
- **P2 hook 超时**：post-tool-use / stop 在 claude 侧 15s、helper 侧 5s 放弃。推荐值来自「一次注入是一次本机 socket 往返」；隔离点：`crewd-exec::launch::QUICK_HOOK_TIMEOUT_SECS` 与 `hook_cmd::QUICK`。
- **P3 审批闸缺省**：`["computer-use"]`（与 Swift `--gate computer-use` 相同）。隔离点：`AgentConfig::default()`。
- **P4 跨机组语法糖的发送者**：`contact("7")` / `report_to_parent` / `message_child_crew` 以系统身份 + 署名发（裁定 7），因为 §22.2 规定只有成员能发言。若人要「以本人身份出现在对方大厅」，需要 M3a 放开规则；隔离点：`tools/channels.rs::contact`、`tools/captain.rs` 两个函数里的 `PostSender::System`。

## M3a 对齐（取自 M3a 计划的冻结接口块；Task 1 Step 1 对着落地代码核）

| M3b 用到的 | M3a 冻结名 | 在哪用 |
|---|---|---|
| 库 | `crewd_store::Db::{spawn, call}`（`call` 返回 `crewd_store::Result<T>`）、`crewd_store::{now_ms, meta_get, meta_set}`、`crewd_store::channels::{lobby_id, human_id}` | port、testkit、turn_trace |
| 写事务 | `AppState::collab(\|tx: &mut Tx\| …) -> collab::Result<T>`（一个事务 + 把 `Change` 发到 Hub） | port 的全部写 |
| 事件 | `Hub::subscribe_collab() -> Receiver<Change>`；`Change::{SessionChanged, CrewChanged, ApprovalChanged { id }, …}` | 角色变化推送（Task 7）、审批等待（Task 10） |
| 状态 | `AppState { db, hub, tokens: TokenTable, sessions: Arc<dyn SessionControl>, … }`、`AppState::new(db).with_sessions(s)` | rpc、testkit |
| 令牌 | `TokenTable::{mint, verify, revoke}` | rpc（verify）、testkit（mint） |
| 会话控制 | `SessionControl::{start(StartRequest) -> Session, stop, nudge, inspect(id, max_lines) -> Inspection, set_profile, handoff_captain(crew, to), quota -> QuotaReport, live -> SessionLive, resume}`、`FakeSessionControl::calls() -> Vec<FakeCall>`、`ControlError` | 机长工具、`get_quota`、`set_session_profile` |
| 错误码 | `RpcError::{FORBIDDEN = -32003, NOT_FOUND = -32004, CONFLICT = -32009}`、`dispatch::collab_err` | agent RPC |
| 词表 | `crewd_proto::{Role, ChannelKind, MentionKind, SenderKind, MemberRef, Mention, Message(channel_id)}`、`crewd_collab::Locale`、`settings::locale(conn)` | 全部 |
| 群 | `channels::{create, ensure_dm, update, add_members, remove_member, ensure_human_channel, channels_of_session, list_for_crew, summary, request_owner_transfer, accept_owner_transfer, force_owner}`（**群权限由它们按 `by: &Sender` 判**） | 群工具 |
| 消息 | `messages::{post(tx, Sender, NewPost) -> Posted { message, unwatched_human_mention }, list, search, get}`；`message.post` 附件非空一律拒 | post / read / search / 代发 |
| 游标与可见性 | `cursors::{unread_for(tx, s) -> Vec<(String, Vec<Message>)>, advance}`（**已滤可见性与自己发的**，`visible_to` / `directed_targets` 是 M3a 的） | 注入 |
| 渲染 | `render::{blocks_for(conn, s, &unread) -> Vec<ChannelBlock>, injection(Locale, &[ChannelBlock]), wake_text, build_batch}`，文字在 `render.{zh,en}.md` | 注入 |
| 提示词 | `prompts::{parse, PromptFile, fill, placeholders}`，占位符 `{名字}` | text、worldview、turn_trace |
| 收听 | `listens::{set, clear, channels_of}` | 收听工具 |
| 机组 | `crews::{get, parents, children, root_of, captain_of, create(NewCrew), update(CrewPatch{policy: Option<Value>, status_line}), adopt, release, set_captain}` | 机长工具、testkit |
| session | `sessions::{get, list, role_of, display_name, register(NewSession), set_awaiting_question}` | port、turn_trace、testkit |
| 通讯录 | `directory::list(conn, query) -> Vec<DirectoryRow { number, kind, name, crew_id, session_id, org_path }>` | directory / contact、分机号 |
| 人类待确认 | `asks::{open(tx, asker, OpenAsk { question, default: Option<AskDefault { text, after_minutes }>, resume_note }) -> Ask, withdraw(tx, by: &Sender, crew, ask, reason)}` | ask / withdraw_ask |
| 唤醒 | `wakes::{enqueue, WakeRequest, WakeKind::Timer, WakePriority::System, source::timer}` | schedule_wakeup |
| 审批 | `approvals::{request(NewApproval) -> Approval, get, decide(tx, id, allow, by)}`（`request` 自己往根机组人类群发 `notice.approval`） | pre-tool-use |
| 由 M3b 填的 stub | `worldview::{WorldCtx, context_for, render}`、`turn_trace::{TurnEnd, on_turn_end(tx, session, prompt_id, last_text)}` | Task 4、5 |

## M3b 需要、M3a 冻结块里没有的（执行前由控制者确认；缺哪条就先改 M3a）

- **G3 Todo 回应**：Agent 那本 Todo 追加一条回应（`todo_responses` 表在，写函数不在冻结块里）。`respond_todo` 依赖它。
- **G-F `prompts::fill` 会再展开**：M3a 的 `fill` 是逐个 `replace`，前一个值里的 `{x}` 会被后一个变量再替换（agent / 人写的文字里有花括号就会串）。要改成一遍扫描（Task 3 `fill_does_not_expand_inside_values`、Task 4 `values_with_braces_are_not_expanded` 会先红）。
- **G-S `StartRequest.worktree: Option<String>` 的取值**：`start_session(isolation: true)` 该传什么（路径？名字？「auto」？）M3a 没定；M3b 先传 `Some("auto")`，由 M3c 定义。`executor` 是必填 `String`：M3b 在 runner 缺省时取机组的 `captain_executor`，再缺省 `"claude"`。
- **G-T `TurnEnd` 只有三个值**：「同轮重放」「这一轮叫过人」「不知道叫没叫过（重启后第一轮）」都只能报 `AlreadyCalled`。要分开就在 M3a 合并前改枚举。
- **G9 `AppState` 要留 `agent: AgentRuntime` 字段**（M3a 的 `AppState::new` 构造它的缺省值）。
- **G11 测试支撑**：`FakeSessionControl` 在非 `cfg(test)` 构建里也可用（`crewd` 的集成测试经 feature `testkit` 用 server 的夹具）；`conn::run` 至少 `pub(crate)`；`dispatch::handle` 可从 crate 内调。

## 写计划时定下的裁定（执行时照做，有异议先回报）

1. **helper 哑管道**：`crewd mcp` 每次 `tools/list` 都现问 `agent.tools`、每次 `tools/call` 都转 `agent.call`，不缓存工具表；serve 推 `agent.tools_changed` 时转成 `notifications/tools/list_changed`。serve 掉线后 helper 不重连（crewd 重启时子进程跟着死，token 也只在内存里）：之后每次调用回 `isError` + 一句英文说明。
2. **token 文件**：`CommsSpec.token` 由 `prepare_claude` 写成 `<launch dir>/agent-token`（0600，随 `LaunchFiles` 删）；helper 命令行只有 `--data-dir D --session S --token-file F`。
3. **PreToolUse 只给命中审批闸的工具注册**（偏离 §22.3「没命中 → 不输出」的写法，结果相同）：`CommsSpec.approval_gates` 拼成 matcher `(g1|g2)`（正则转义、外面一层括号保证走正则路径、不锚定 = 子串匹配，与 Swift `toolName.contains(gate)` 同义）；空表 = 不注册 PreToolUse。理由：官方文档写明 PreToolUse 超时**不挡**工具调用，所以「连不上 crewd → 命中的 deny」只有在 helper 自己知道「我是因为命中才被叫」时才做得到；顺带省掉每次工具调用一次进程 + socket 往返。serve 收到后再按自己的配置判一次（配置改了就放行不输出）。
4. **hook 超时写显式**：post-tool-use / stop `timeout: 15`（helper 自己 5s 连不上就放弃）——默认 600 秒会让一个卡住的 crewd 把 claude 每次工具调用都拖 10 分钟；pre-tool-use `timeout = approval_wait_secs + 60`，helper 自己的期限 = `approval_wait_secs + 30`，到期 **deny**（不能让 claude 的超时替我们「放行」）。
5. **hook 一律 shell 形式**（`sh -c` + 单引号拼词，M2b 已测过），不用文档里的 `args` exec 形式：`args` 是哪个 claude 版本加的没核到，旧版会忽略 `args` 只跑 `command`，静默坏掉。
6. **注入上限 9,000 字符**（`AgentConfig.inject_cap_chars`）：官方上限 10,000，超了只剩 2,000 预览。超限从最旧的消息开始省略，块头写明「较早 N 条已省略、用 read_channel 看」（Swift #105 ①：有上限就必须自报），被省略的照样推进游标（它们已「告知过」存在）。
7. **跨机组语法糖用系统发言**：`contact("7")`、`report_to_parent`、`message_child_crew` 发到别的机组大厅——按 §22.2「只有成员能发言」，session 不是对方大厅成员。这里以 `Sender::System` 发、正文第一行是 md 里的「来自 {机组名}（{号码}）{显示名}：」，@ 对方机长；同时在自己机组大厅留一行「已联系 …」（Swift 的跨线留痕）。`7-3` 走 dm（自己是成员，以自己身份发）。
8. **回合留痕的记账放 `meta` 表**（键 `agent.turn.<session>`，M1 起就有的键值表，不加迁移）：本 session 经 `post` / `ask` / `contact` 等发出的消息只要 @ 了机长或人类就记 `calledOut = yes`，Stop 时判完清成 `no`；从没记过 = `unknown`，按 Swift「since 为空时宁可判已经叫过人」不代发（`SessionTurnTrace.swift:42-43`），但 `awaiting_question` 照写。这样不需要「按发送者查消息」的新查询，也跨重启。
9. **代发的目标**：worker → 自己机组大厅 @机长；机长 → 根机组人类群 @人（`SessionTurnTrace.swift:21-26`，绝不 @ 自己）。代发以 `Sender::System` 发、正文点名是谁停住了（与 Swift 以 session 身份发不同：系统代发不该算作「它这一轮说过话」，也不该触发「自己 @ 机长」的自唤醒判定）。
10. **工具参数里引用消息**：`reply_to` / `before` 收消息的 `seq`（数字或 `#123`），也收消息 id；渲染出来的每条消息带 `#seq`。seq 全库唯一且短（agent 抄 UUID 容易抄错）。
11. **频道参数**：`channel` 收群 id，也收「自己能看见的群里唯一的群名」；对不上回执列出可选的群。缺省：`post` 有 `reply_to` 就发到那条所在的群，否则发本机组大厅；`read_channel` 缺省本机组大厅。
12. **角色每次现查**：`agent.tools` 与 `agent.call` 都从库里现取（`sessions.role` 与机组 builtin），不信 attach 时的结果；attach 返回值只给 helper 打日志用。
13. **`post` 的 `messages[≤6]`**：先整批校验，任何一条不合法整批不发、错误指名第几条；校验过了逐条发，中途落库失败如实报「前 k 条已发」（不假装原子）。
14. **`change_workdir` 只改库里的工作目录**（预览 / `confirm: true` 两步、目录必须存在、只能动自己这棵子树）；Swift 那套「搬 claude 项目记忆与权限」不在 M3b（遗留 L3）。
15. **语言**：serve 一个全局 `AgentConfig.locale`（缺省 zh），工具文字、回执、世界观都按它取；`zh` / `en` 两份 md 的节名集合必须一致（测试钉住）。
16. **内建总机组**（`crews.builtin`）不列 `ask` / `respond_todo` / `start_session`，调用时再拒一次（`McpServer.swift:565`、`:745-753`、`:985`、`:1129`、`:1617`；spec §22.3 只写了前两个，`start_session` 照 Swift 补上）。

## 与 spec 的出入（执行时照这里，交付时写进 spec 的交付记录）

1. `CommsSpec` 比 §22.5 多两个字段 `approval_gates`、`approval_wait_secs`（裁定 3、4）。
2. `listen(channel?, minutes?, senders?, off?)` 换成 `channel_listen` / `channel_unlisten`（§22.7 Q2）；新增 `channel_transfer` / `channel_accept`；归档权限 = 群主或机长。
3. 回合记账（这一轮叫没叫过人、上一轮的 prompt_id）存在 collab 的 `meta` 表（键 `agent.turn.<session>`），不加迁移；M3a 冻结的 `turn_trace::on_turn_end(tx, …)` 在 collab 里实现落库（代发 + `awaiting_question`），M3c 的 codex `TurnEnded` 直接调它；新增 `turn_trace::note_post(tx, session, mentions)` 由 M3b 的工具在发消息后调。
4. agent RPC 必须按连接记 attach 状态，所以 M3b 要改 `server/src/conn.rs`（§22.5 的文件归属没列它；M3c 不碰 conn.rs，不冲突）。
5. 可见性（`CrewWhiteboardVisibility.swift:48`）与「发给 X 的」标注用 M3a 的 `cursors::unread_for` / `render::blocks_for`（§22.3 写的「M3b 移植可见性」由 M3a 做掉了，M3b 不另写一份）；M3b 只加注入的总字数上限与「省略了几条」的自报。M3c 给 codex 组 `additionalContext` 时调 `server::agent::inject::prepare / commit`。
6. Swift 的权限 hook 自 #75 ② 起**不阻塞**（`McpPermissionHook.swift:69-76`：当场拒 + 开人类 Todo + 一次性放行票），§22.3 写的是**阻塞等 30 分钟**。本计划照 spec 做，等待上限放在 `AgentConfig.approval_wait`；这条要人知道（见文末「交付时要回报」）。
7. `post` 暂不收 `attachments`：M3a 的 `message.post` 附件非空一律拒（M3a 计划裁定 14，附件上传不在 M3）。Swift 一直有这个参数，等 M3a 之后支持附件再加回（遗留 L8）。
8. 机长注入面上的附加物（命名提醒、拆组信号、计划陈旧度、全机组织树、派活策略更新，`HookEmitter.swift:246-311`）不移植：计划板已删（§9），派活策略改由 `read_delegation_policy` 现读；组织树由世界观的 lineage 节 + `directory` 取代（遗留 L1）。
9. 世界观去掉 Swift 的 `humanRoster` / `sharesBlock` / `tiebreakerBlock` / `captainRunnerBlock` 与入门机组的 SKILL.md 提示（`LocalSessionWorldModel.swift:56-70`）；人类只有一个人，runner 偏好设置 crewd 里还没有（遗留 L2）。

## Review Focus

1. crewd 卡住（库锁死 / serve 不回话）时 claude 的每次工具调用 → 不能被 hook 拖住：post-tool-use / stop 在 5s 内放弃、什么都不输出、exit 0。（Task 9 `hook_gives_up_quickly_when_crewd_hangs`）
2. 一个 session 攒了几百条未读（离开一晚上）→ 注入块不超过 9,000 字符、块头说清省略了几条、游标照样推到末尾，下一次工具调用不重复注入。（Task 9 `huge_backlog_is_capped_and_announced`）
3. 人在审批框前一直不答 / crewd 在等待中途停掉 → 命中的工具一定被 deny，不能因为 claude 的 hook 超时被放行。（Task 10 `crewd_gone_during_wait_means_deny`、`unanswered_approval_times_out_to_deny`）
4. 换机长（M3c 的 `crew.setCaptain`）发生在一个 session 正连着的时候 → 不重启：下一次 `tools/list` 就是新工具表、旧机长调机长工具当场被拒（攻击面是「helper 缓存了角色」）。（Task 7 `role_change_pushes_tools_changed_and_takes_effect_without_restart`）
5. agent 在消息里贴了带 `{sessionId}`、`{name}` 的文本，或工具参数里有未知字段 → 世界观 / 回执的占位符替换不能被正文里的花括号触发，未知字段整条拒绝并说清是哪个字段（不静默忽略）。（Task 3 `fill_does_not_expand_inside_values`、Task 12 `unknown_argument_is_named`）

## 文件结构

```
crewd/crates/proto/src/agent.rs                新：agent RPC 线上类型、方法名、schema 片段          (T1)
crewd/crates/proto/src/lib.rs                  pub mod agent; api_schema 加 "agent" 一行             (T1)
crewd/crates/exec/src/launch.rs                CommsSpec / HelperCommands / token 文件 / matcher / timeout (T2)
crewd/crates/collab/prompts/tools.zh.md        工具描述、参数说明、回执、代发稿（中文）             (T3，T5、T7、T9–T15 追加节)
crewd/crates/collab/prompts/tools.en.md        同上（英文），节名集合与 zh 一致                       (T3，T9–T15)
crewd/crates/collab/prompts/world-model.zh.md  世界观（中文）                                        (T4)
crewd/crates/collab/prompts/world-model.en.md  世界观（英文）                                        (T4)
crewd/crates/collab/src/worldview.rs           tools_file（T3）；WorldCtx 追加字段、context_for、render (T4)
crewd/crates/collab/src/turn_trace.rs          trailing_question、note_post、on_turn_end（记账在 meta） (T5)
crewd/crates/server/src/agent/mod.rs           模块声明、Identity / Role                              (T3)
crewd/crates/server/src/agent/text.rs          取 tools.*.md 的节、填占位符                           (T3)
crewd/crates/server/src/agent/catalog.rs       ToolSpec 表、按角色给表、input schema                  (T3，T12–T15 填 handler 名)
crewd/crates/server/src/agent/literals.rs      源码字面量扫描测试（只有 #[cfg(test)]）                (T3)
crewd/crates/server/src/agent/port.rs          唯一调用 M3a 的文件                                    (T6，后续任务追加函数)
crewd/crates/server/src/agent/testkit.rs       测试夹具（#[doc(hidden)] pub）：建库、种机组 / session、起 socket (T6)
crewd/crates/server/src/agent/runtime.rs       AgentRuntime、AgentConfig                              (T6)
crewd/crates/server/src/agent/rpc.rs           AgentConn、attach / tools / call / hook 路由、tools_changed 推送 (T7)
crewd/crates/server/src/conn.rs                agent.* 交给 rpc                                       (T7)
crewd/crates/server/src/lib.rs                 pub mod agent; AppState.agent                          (T6)
crewd/crates/server/src/agent/inject.rs        prepare / commit / hook_output（post-tool-use）         (T9)
crewd/crates/server/src/agent/approval.rs      pre-tool-use 审批闸                                    (T10)
crewd/crates/server/src/agent/turn.rs          stop hook 薄壳、note_post                              (T11)
crewd/crates/server/src/agent/tools/mod.rs     Ctx、参数解析、分发                                    (T7 骨架，T12–T15)
crewd/crates/server/src/agent/tools/{messaging,channels,asks,captain}.rs                             (T12–T15)
（serve 一侧的测试都写在各模块的 `#[cfg(test)] mod tests` 里：集成测试目录拿不到 cfg(test) 的 testkit。）
crewd/crates/crewd/src/agent_client.rs         连 socket、attach、按 id 复用一条连接、收推送           (T8)
crewd/crates/crewd/src/mcp_cmd.rs              `crewd mcp`（rmcp ServerHandler）                      (T8)
crewd/crates/crewd/src/hook_cmd.rs             `crewd hook <event>`                                   (T8–T11)
crewd/crates/crewd/src/main.rs                 子命令接线                                             (T8)
crewd/crates/crewd/tests/helper_cli.rs         假 serve socket 上测 `crewd mcp` / `crewd hook`          (T8–T11)
crewd/crates/crewd/tests/agent_e2e.rs          真 serve 夹具 + 照 mcp.json / settings.json 起真 helper (T16)
crewd/crates/crewd/tests/live_agent.rs         真 claude 冒烟（#[ignore] + CREWD_LIVE_CLAUDE=1）       (T16)
crewd/README.md、docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md（交付记录）(T16)
```

## 工具表（Swift 源 `Sources/Mcp/McpServer.swift`，声明行 / 处理行；C = 机长专用）

| crewd 工具 | Swift 来源 | 变化 |
|---|---|---|
| `post(channel?, message \| messages[≤6], mentions?, reply_to?, category?)` | `post_to_crew` :190 / :811 | 加 `channel`；删 `plan` / `todo` / `todo_status` / `todo_reference` / `evidence*` / `headline` / `audience` / `crew_status`；未注视的群 @人类 时回执带 §17 那句 |
| `read_channel(channel?, limit?, before?)` | `read_whiteboard` :294 / :905 | 按群；读到最新一条推进游标（只对自己在的群） |
| `search(query, channel?, after?, before?, limit?)` | `search_whiteboard` :305 / :930 | 范围 = 自己在的群 + 收听的群 |
| `list_channels()`、`human_watching()` | 新 | 后者 = 人类注视的群 + 根机组人类群 |
| `channel_create(name, purpose?, members[], kind?)`、`channel_invite(channel, members[])`、`channel_leave(channel)`、`channel_archive(channel)` | 新 | 归档：群主或机长 |
| `channel_transfer(channel, to, force?)`、`channel_accept(channel)` | 新（§22.7 Q2） | 群主发起 + 对方接受；机长 `force` 直接转 |
| `channel_listen(channel, minutes?, senders?)`、`channel_unlisten(channel)` | `listen` :365 / :1400 | 按群（§22.7 Q2） |
| `directory(query?)` | `directory` :272 / :882 | 行里加上群 |
| `contact(to, message)` | `contact` :282 / :903 | `7` → 7 号大厅 @机长（系统代发 + 署名）；`7-3` → 与它的 dm（没有就建） |
| `ask(question, resume_note?, fallback_after_minutes?, fallback?)` | `ask` :319 / :984 + `add_human_todo` :394 / :1696 | `open_ask` 发 H 号，不阻塞 |
| `withdraw_ask(ask, reason)` | `withdraw_human_todo` :406 / :1786 | `supersedes` = 撤旧 + 开新 |
| `respond_todo(number, response)` | `respond_todo` :378 / :1616 | 去掉 `status` / `evidence*`（§9） |
| `get_quota()`、`schedule_wakeup(after_minutes \| at, note)`、`set_session_profile(model?, effort?)` | 同名 :333 / :1340、:338 / :1380、:438 / :1958 | 定时唤醒入 `wakes(kind=timer)` |
| C `rename_crew`、`change_workdir`、`set_crew_status`、`read_delegation_policy`、`update_delegation_policy` | :523 / :1084、:550 / :1312、post 的 `crew_status`、:653 / :1182、:658 / :1192 | `set_crew_status` 从 post 拆出（≤200 字） |
| C `start_session`、`stop_session`、`inspect_session`、`nudge_session`、`list_sessions`、`handoff_captain_to_session`、`create_and_handoff_captain` | :567 / :1127、:636 / :1293、:613 / :1264、:624 / :1278、:648 / :1994、:585 / :1203、:597 / :1227 | 经 `SessionControl` |
| C `report_to_parent`、`message_child_crew` | :667 / :2014、:676 / :2030 | 发到父 / 子机组大厅 @机长（系统代发 + 署名，裁定 7） |
| C `adopt_crew`、`release_crew`、`create_parent_crew`、`adopt_parent`、`create_child_crew` | :688 / :2048、:699 / :2059、:711 / :2073、:721 / :2085、:732 / :2096 | DAG 写经 port；新机组的机长经 `SessionControl::start` |
| 删 | `continue_work` :351 / :768、`confirm_todo_sweep` :459 / :1431、`plan_add` :488 / :1497、`plan_update` :500 / :1525、`plan_list` :518 / :1613 | M4 取代（§9） |
| 删（§22.7 Q4） | `raise_attention` :534 / :1102、`clear_attention` :545 / :1117、`crew_ordering_signals` :418 / :1834、`arrange_crews` :423 / :1865 | 侧栏排序另起一节 |

hook 对照：`McpHelperMain.swift:5-10`（四种 helper 模式）→ `crewd mcp` + `crewd hook <3 种>`；PostToolUse `HookEmitter.swift:213-262`；PreToolUse `McpPermissionHook.swift:61-129`；Stop `McpTurnHook.swift:31-92` + `SessionTurnTrace.swift:44-222`。

---
### Task 1: 核对 M3a 接口 + agent 线上类型（`crewd-proto::agent`）

**Files:**
- Create: `crewd/crates/proto/src/agent.rs`
- Modify: `crewd/crates/proto/src/lib.rs`（`pub mod agent;`、`api_schema` 加一行）、`crewd/schema/crewd-api.schema.json`（重新生成）

**Interfaces:**
- Consumes: M3a 的 proto（`Request` / `Response` / `RpcError` / `Event` / `ServerFrame`、`Role`）。
- Produces（后面所有任务用）：
  - `crewd_proto::agent::methods::{ATTACH, TOOLS, CALL, HOOK}`（`"agent.attach"` / `"agent.tools"` / `"agent.call"` / `"agent.hook"`）、`crewd_proto::agent::TOOLS_CHANGED = "agent.tools_changed"`
  - `AttachParams { session_id, token }`、`AttachResult { role, crew_id, label }`
  - `ToolDef { name, description, input_schema: Value }`
  - `CallParams { tool, args: Value }`、`CallResult { text, is_error }` + `CallResult::ok(String)` / `CallResult::err(String)`
  - `HookKind { PostToolUse, PreToolUse, Stop }`（线上 `"post-tool-use"` / `"pre-tool-use"` / `"stop"`，`HookKind::as_str()`、`HookKind::parse(&str) -> Option<Self>`）
  - `HookParams { event: HookKind, payload: Value }`、`HookResult { stdout: Option<String> }`
  - `crewd_proto::agent::schema() -> Value`

- [ ] **Step 1: 核对 M3a 接口（不写代码）**

对着落地的 M3a 代码逐条跑，把与「M3a 对齐」表不同的地方记进本任务的提交信息（「M3a 对照：…」）。G 列表里的缺口还在 → 停下回报控制者（§22.1：缺 → 改 M3a），不要在 M3b 里补。

```bash
grep -n "pub async fn call\|pub fn spawn" crates/store/src/db.rs
grep -n "pub struct AppState" -A12 crates/server/src/lib.rs
grep -n "pub async fn collab\|pub fn with_sessions" crates/server/src/lib.rs
grep -n "pub fn subscribe_collab" crates/server/src/hub.rs
grep -n "fn mint\|fn verify" crates/server/src/token.rs
grep -n "pub trait SessionControl" -A12 crates/server/src/control.rs
grep -n "pub struct FakeSessionControl\|pub fn calls" crates/server/src/control.rs
grep -n "FORBIDDEN\|NOT_FOUND\|CONFLICT" crates/proto/src/lib.rs
grep -n "pub fn unread_for\|pub fn advance" crates/collab/src/cursors.rs
grep -n "pub fn blocks_for\|pub fn injection" crates/collab/src/render.rs
grep -n "pub fn fill\|pub fn placeholders\|pub fn parse" crates/collab/src/prompts.rs
grep -n "pub struct WorldCtx\|pub fn context_for\|pub fn render" crates/collab/src/worldview.rs
grep -n "pub enum TurnEnd\|pub fn on_turn_end" crates/collab/src/turn_trace.rs
grep -n "pub fn post\|pub struct Posted\|unwatched_human_mention" crates/collab/src/messages.rs
grep -n "pub fn ensure_dm\|pub fn force_owner\|pub fn request_owner_transfer" crates/collab/src/channels.rs
grep -n "pub fn set\|pub fn clear\|pub fn channels_of" crates/collab/src/listens.rs
grep -n "pub fn open\|pub fn withdraw\|pub struct OpenAsk\|pub struct AskDefault" crates/collab/src/asks.rs crates/proto/src/*.rs
grep -n "pub fn request\|pub fn decide" crates/collab/src/approvals.rs
grep -n "pub fn enqueue\|pub fn timer" crates/collab/src/wakes.rs
grep -n "pub fn register\|pub fn set_awaiting_question\|pub fn display_name" crates/collab/src/sessions.rs
grep -rn "todo_responses" crates/collab/src/ | head -3          # G3：空 = 缺
grep -n "replace(&format" crates/collab/src/prompts.rs           # G-F：有输出 = fill 仍是逐个 replace
grep -n "pub agent" crates/server/src/lib.rs                     # G9
```

Expected：除 G3 / G-F 两行外每条都有输出；`port.rs`（Task 6）里的调用照这里查到的真实签名写。

- [ ] **Step 2: 写失败的测试**

`crates/proto/src/agent.rs`（先只放测试模块，`lib.rs` 加 `pub mod agent;`）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn attach_params_are_camel_case_and_strict() {
        let p: AttachParams =
            serde_json::from_value(json!({"sessionId": "s-1", "token": "t"})).unwrap();
        assert_eq!(p.session_id, "s-1");
        let e = serde_json::from_value::<AttachParams>(
            json!({"sessionId": "s-1", "token": "t", "crew": "x"}),
        )
        .unwrap_err();
        assert!(e.to_string().contains("unknown field"), "{e}");
    }

    #[test]
    fn hook_kind_wire_names() {
        for (k, s) in [
            (HookKind::PostToolUse, "post-tool-use"),
            (HookKind::PreToolUse, "pre-tool-use"),
            (HookKind::Stop, "stop"),
        ] {
            assert_eq!(serde_json::to_value(k).unwrap(), json!(s));
            assert_eq!(k.as_str(), s);
            assert_eq!(HookKind::parse(s), Some(k));
        }
        assert_eq!(HookKind::parse("user-prompt-submit"), None, "M4b 才加");
    }

    #[test]
    fn call_params_default_args_to_null_and_result_is_camel_case() {
        let p: CallParams = serde_json::from_value(json!({"tool": "post"})).unwrap();
        assert_eq!(p.args, serde_json::Value::Null);
        let r = serde_json::to_value(CallResult::err("x".into())).unwrap();
        assert_eq!(r, json!({"text": "x", "isError": true}));
        assert!(!CallResult::ok("y".into()).is_error);
    }

    #[test]
    fn hook_result_omits_absent_stdout() {
        assert_eq!(
            serde_json::to_value(HookResult { stdout: None }).unwrap(),
            json!({})
        );
    }

    #[test]
    fn schema_lists_the_four_methods() {
        let s = schema();
        for m in [methods::ATTACH, methods::TOOLS, methods::CALL, methods::HOOK] {
            assert!(s["methods"][m].is_object(), "{m}");
        }
        assert!(s["types"]["ToolDef"].is_object());
    }
}
```

- [ ] **Step 3: 跑一遍确认失败**

Run: `cargo test -p crewd-proto agent`
Expected: 编译失败，`cannot find type `AttachParams``。

- [ ] **Step 4: 写实现**（测试模块上面）

```rust
//! agent 面（spec §22.3）：`crewd mcp` / `crewd hook` 与 serve 之间的 RPC。
//! 只有 `agent.attach` 成功过的连接能调其余三个；`agent.tools_changed` 只推给 attach 过的那条连接，
//! 不是 `events.subscribe` 的主题。
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub mod methods {
    pub const ATTACH: &str = "agent.attach";
    pub const TOOLS: &str = "agent.tools";
    pub const CALL: &str = "agent.call";
    pub const HOOK: &str = "agent.hook";
}

/// serve → helper 的推送：角色变了，工具表要重取。`data = { "role": Role }`。
pub const TOOLS_CHANGED: &str = "agent.tools_changed";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttachParams {
    pub session_id: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AttachResult {
    /// M3a 的 `crewd_proto::Role`（线上 "captain" / "worker"）。
    pub role: crate::Role,
    pub crew_id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CallParams {
    pub tool: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CallResult {
    pub text: String,
    pub is_error: bool,
}

impl CallResult {
    pub fn ok(text: String) -> Self {
        Self { text, is_error: false }
    }
    pub fn err(text: String) -> Self {
        Self { text, is_error: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum HookKind {
    PostToolUse,
    PreToolUse,
    Stop,
}

impl HookKind {
    pub const ALL: [HookKind; 3] = [Self::PostToolUse, Self::PreToolUse, Self::Stop];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::PostToolUse => "post-tool-use",
            Self::PreToolUse => "pre-tool-use",
            Self::Stop => "stop",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HookParams {
    pub event: HookKind,
    /// claude 喂给 hook 的 stdin JSON，原样转来。
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HookResult {
    /// helper 原样打到 stdout 的内容；None = 什么都不打。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
}

/// 契约里 agent 那一段（`api_schema()["agent"]`）。
pub fn schema() -> Value {
    json!({
        "methods": {
            methods::ATTACH: { "params": "AttachParams", "result": "AttachResult" },
            methods::TOOLS: { "params": null, "result": "ToolDef[]" },
            methods::CALL: { "params": "CallParams", "result": "CallResult" },
            methods::HOOK: { "params": "HookParams", "result": "HookResult" },
        },
        "push": { TOOLS_CHANGED: "{ role: Role }" },
        "types": {
            "AttachParams": schema_for!(AttachParams),
            "AttachResult": schema_for!(AttachResult),
            "ToolDef": schema_for!(ToolDef),
            "CallParams": schema_for!(CallParams),
            "CallResult": schema_for!(CallResult),
            "HookKind": schema_for!(HookKind),
            "HookParams": schema_for!(HookParams),
            "HookResult": schema_for!(HookResult),
        }
    })
}
```

`crates/proto/src/lib.rs`：模块声明区加 `pub mod agent;`；`api_schema()` 返回的对象顶层加一项 `"agent": agent::schema(),`（M3a 把 `api_schema` 改成什么形状都行，只加这一个键）。

- [ ] **Step 5: 跑通 + 重新生成契约**

```bash
cargo test -p crewd-proto agent
cargo run -q -p crewd -- schema > schema/crewd-api.schema.json
cargo test -p crewd-proto
```
Expected：`5 passed`；契约漂移测试（M1 起就有）通过。

- [ ] **Step 6: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/proto/src/agent.rs crates/proto/src/lib.rs schema/crewd-api.schema.json
git diff --cached --stat
git commit -m "crewd-proto: agent RPC types (attach/tools/call/hook, tools_changed push)

M3a 对照：<Step 1 查到的实际名字，一行一条>

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: helper 命令行、token 文件、审批闸 matcher、hook 超时（`crewd-exec::launch`）

**Files:**
- Modify: `crewd/crates/exec/src/launch.rs`（`CommsSpec`、`HelperCommands`、`HookEvent`、`prepare_claude` 的 comms 段、测试模块）
- Modify（**仅当** M2c 已合进 main，Step 6）：codex 驱动里拼 MCP 命令行的地方与它测试里的 `CommsSpec { .. }` 字面量

**Interfaces:**
- Consumes: M2b 的 `prepare_claude`、`LaunchFiles`、`shell_quote`、`write_private`。
- Produces:
  - `pub struct CommsSpec { pub helper: PathBuf, pub data_dir: PathBuf, pub session_id: String, pub token: String, pub hooks: bool, pub approval_gates: Vec<String>, pub approval_wait_secs: u64 }`
  - `pub const TOKEN_FILE: &str = "agent-token"`、`pub const QUICK_HOOK_TIMEOUT_SECS: u64 = 15`、`pub const DEFAULT_APPROVAL_WAIT_SECS: u64 = 1800`
  - `HelperCommands::mcp_args(&self, token_file: &Path) -> Vec<String>`、`HelperCommands::hook_command(&self, event: HookEvent, token_file: &Path) -> String`（PreToolUse 多带 `--deadline-secs <approval_wait_secs + 30>`）
  - `HookEvent::timeout_secs(self, comms: &CommsSpec) -> u64`
  - `pub fn gate_matcher(gates: &[String]) -> Option<String>`
  - `pub fn write_token_file(dir: &Path, token: &str) -> std::io::Result<PathBuf>`（M3c / codex 驱动复用）

M2b 写的是 `--crew --captain --label --structured-post --agent --gate` 一串身份参数（§18 裁定 14）；§22.3 改成只带 `--data-dir --session --token-file`，角色每次现查（裁定 12）。

- [ ] **Step 1: 改测试**（`launch.rs` 测试模块：`comms()`、`mcp_config_is_exec_form_and_hooks_are_quoted_shell_strings`、`hooks_can_be_omitted_until_m3` 三段删掉，换成下面的；其余测试不动）

```rust
    fn comms() -> CommsSpec {
        CommsSpec {
            helper: PathBuf::from("/opt/crew d/crewd"),
            data_dir: PathBuf::from("/data"),
            session_id: "worker-1".to_string(),
            token: "tok-SECRET".to_string(),
            hooks: true,
            approval_gates: vec!["computer-use".to_string()],
            approval_wait_secs: 1800,
        }
    }

    #[test]
    fn mcp_config_is_exec_form_with_token_file_only() {
        let rt = tempfile::tempdir().unwrap();
        let spec = ClaudeSpec {
            comms: Some(comms()),
            ..ClaudeSpec::default()
        };
        let (args, files) =
            prepare_claude(&spec, AgentSessionArg::New("u-3".into()), rt.path()).unwrap();
        let token_file = files.dir().join(TOKEN_FILE);
        assert_eq!(std::fs::read_to_string(&token_file).unwrap(), "tok-SECRET");
        assert_eq!(mode(&token_file), 0o600);
        let mcp: Value =
            serde_json::from_slice(&std::fs::read(args.mcp_config_file.unwrap()).unwrap()).unwrap();
        let crew = &mcp["mcpServers"]["crew"];
        assert_eq!(crew["type"], "stdio");
        assert_eq!(crew["command"], "/opt/crew d/crewd");
        assert_eq!(
            crew["args"],
            json!([
                "mcp",
                "--data-dir",
                "/data",
                "--session",
                "worker-1",
                "--token-file",
                token_file.display().to_string()
            ])
        );
        let all = std::fs::read_to_string(files.dir().join("mcp.json")).unwrap()
            + &std::fs::read_to_string(files.dir().join("settings.json")).unwrap()
            + &args.argv().join(" ");
        assert!(!all.contains("tok-SECRET"), "token 只在 0600 文件里，不进配置与 argv");
    }

    #[test]
    fn hooks_are_quoted_shell_strings_with_explicit_timeouts() {
        let rt = tempfile::tempdir().unwrap();
        let spec = ClaudeSpec {
            comms: Some(comms()),
            ..ClaudeSpec::default()
        };
        let (args, files) =
            prepare_claude(&spec, AgentSessionArg::New("u-4".into()), rt.path()).unwrap();
        let tf = files.dir().join(TOKEN_FILE).display().to_string();
        let settings: Value =
            serde_json::from_slice(&std::fs::read(args.settings_file.unwrap()).unwrap()).unwrap();
        let post = &settings["hooks"]["PostToolUse"][0];
        assert!(post.get("matcher").is_none(), "PostToolUse 对所有工具");
        assert_eq!(
            post["hooks"][0]["command"],
            format!(
                "'/opt/crew d/crewd' 'hook' 'post-tool-use' '--data-dir' '/data' \
                 '--session' 'worker-1' '--token-file' '{tf}'"
            )
        );
        assert_eq!(post["hooks"][0]["type"], "command");
        assert_eq!(post["hooks"][0]["timeout"], 15);
        assert_eq!(settings["hooks"]["Stop"][0]["hooks"][0]["timeout"], 15);
        let pre = &settings["hooks"]["PreToolUse"][0];
        assert_eq!(pre["matcher"], "(computer-use)");
        assert_eq!(pre["hooks"][0]["timeout"], 1800 + 60);
        assert!(
            pre["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .ends_with("'--deadline-secs' '1830'"),
            "helper 的期限早于 claude 的超时"
        );
    }

    #[test]
    fn no_gates_means_no_pre_tool_use_hook() {
        let rt = tempfile::tempdir().unwrap();
        let mut c = comms();
        c.approval_gates.clear();
        let spec = ClaudeSpec {
            comms: Some(c),
            ..ClaudeSpec::default()
        };
        let (args, _f) =
            prepare_claude(&spec, AgentSessionArg::New("u-5".into()), rt.path()).unwrap();
        let settings: Value =
            serde_json::from_slice(&std::fs::read(args.settings_file.unwrap()).unwrap()).unwrap();
        assert!(settings["hooks"].get("PreToolUse").is_none());
        assert!(settings["hooks"].get("PostToolUse").is_some());
    }

    #[test]
    fn hooks_can_still_be_switched_off() {
        let rt = tempfile::tempdir().unwrap();
        let mut c = comms();
        c.hooks = false;
        let spec = ClaudeSpec {
            comms: Some(c),
            ..ClaudeSpec::default()
        };
        let (args, files) =
            prepare_claude(&spec, AgentSessionArg::New("u-6".into()), rt.path()).unwrap();
        assert!(args.mcp_config_file.is_some());
        assert_eq!(args.settings_file, None);
        assert!(files.dir().join(TOKEN_FILE).exists(), "MCP 也要 token");
    }

    #[test]
    fn gate_matcher_escapes_and_forces_the_regex_path() {
        assert_eq!(gate_matcher(&[]), None);
        assert_eq!(gate_matcher(&["".into(), "  ".into()]), None, "空项忽略");
        assert_eq!(gate_matcher(&["computer-use".into()]).unwrap(), "(computer-use)");
        assert_eq!(
            gate_matcher(&["a.b".into(), "x|y".into(), "(z)".into()]).unwrap(),
            r"(a\.b|x\|y|\(z\))"
        );
    }

    #[test]
    fn token_file_is_private_and_refuses_to_overwrite() {
        let rt = tempfile::tempdir().unwrap();
        let p = write_token_file(rt.path(), "t1").unwrap();
        assert_eq!(mode(&p), 0o600);
        assert!(write_token_file(rt.path(), "t2").is_err(), "已存在 = 有人在抢");
    }
```

`prompt_never_reaches_argv_and_files_are_private` 不用改（新字段由 `comms()` 给）。

- [ ] **Step 2: 跑一遍确认失败**

Run: `cargo test -p crewd-exec --lib launch`
Expected: 编译失败（`no field token on type CommsSpec`、`cannot find value TOKEN_FILE`…）。

- [ ] **Step 3: 写实现**

`CommsSpec` / `HelperCommands` / `HookEvent` 整段换成：

```rust
/// 文件名：`<launch dir>/agent-token`。helper 读它做 `agent.attach`。
pub const TOKEN_FILE: &str = "agent-token";
/// post-tool-use / stop 的 hook 超时（秒）。官方默认 600 秒：卡住的 crewd 会把每次工具调用拖 10 分钟。
pub const QUICK_HOOK_TIMEOUT_SECS: u64 = 15;
/// serve 等人审批的缺省上限（spec §22.3：最多 30min，到点 deny）。
pub const DEFAULT_APPROVAL_WAIT_SECS: u64 = 1800;

/// crew 通讯（MCP + hooks）要用的 helper 与身份（spec §22.3「身份」）。
/// 机组、角色、显示名不在这里：会话中途会变（换机长），由 serve 每次现查。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommsSpec {
    /// helper 可执行文件（crewd 自己）。
    pub helper: PathBuf,
    /// crewd 数据目录（helper 据此找 `crewd.sock`）。
    pub data_dir: PathBuf,
    /// crewd 的 session id（不是 claude 的会话号）。
    pub session_id: String,
    /// `TokenTable::mint` 给的；只写进 0600 的 token 文件，不进任何配置或 argv。
    pub token: String,
    /// false = 只挂 MCP，不注册任何 hook。
    pub hooks: bool,
    /// 要人审批的工具名子串（serve 配置）；空 = 不注册 PreToolUse。
    pub approval_gates: Vec<String>,
    /// serve 等审批的上限（秒）；PreToolUse 的 hook 超时 = 它 + 60。
    pub approval_wait_secs: u64,
}

/// helper 命令行。子命令形状只在这里拼。
pub struct HelperCommands<'a>(pub &'a CommsSpec);

impl HelperCommands<'_> {
    fn identity(&self, token_file: &Path) -> Vec<String> {
        let c = self.0;
        vec![
            "--data-dir".to_string(),
            c.data_dir.display().to_string(),
            "--session".to_string(),
            c.session_id.clone(),
            "--token-file".to_string(),
            token_file.display().to_string(),
        ]
    }

    /// MCP server：exec 形式（claude 直接 spawn，不过 shell），所以不加引号。
    pub fn mcp_args(&self, token_file: &Path) -> Vec<String> {
        let mut a = vec!["mcp".to_string()];
        a.extend(self.identity(token_file));
        a
    }

    /// hook：claude 经 `sh -c` 跑（没有 `args` 字段 = shell 形式），整条按词单引号拼。
    pub fn hook_command(&self, event: HookEvent, token_file: &Path) -> String {
        let mut words = vec![
            self.0.helper.display().to_string(),
            "hook".to_string(),
            event.subcommand().to_string(),
        ];
        words.extend(self.identity(token_file));
        if event == HookEvent::PreToolUse {
            // helper 自己的期限：serve 等审批的上限 + 30s，到点 deny（claude 的超时 = 上限 + 60s，超时会放行）。
            words.extend([
                "--deadline-secs".to_string(),
                (self.0.approval_wait_secs + 30).to_string(),
            ]);
        }
        words
            .iter()
            .map(|w| shell_quote(w))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// 我们注册的 claude hook 事件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookEvent {
    PostToolUse,
    PreToolUse,
    Stop,
}

impl HookEvent {
    pub const ALL: [HookEvent; 3] = [Self::PostToolUse, Self::PreToolUse, Self::Stop];

    /// claude settings.json 里的事件名。
    pub fn settings_key(self) -> &'static str {
        match self {
            Self::PostToolUse => "PostToolUse",
            Self::PreToolUse => "PreToolUse",
            Self::Stop => "Stop",
        }
    }

    /// `crewd hook <它>`；与 `crewd_proto::agent::HookKind` 的线上名一致。
    pub fn subcommand(self) -> &'static str {
        match self {
            Self::PostToolUse => "post-tool-use",
            Self::PreToolUse => "pre-tool-use",
            Self::Stop => "stop",
        }
    }

    /// 显式超时（秒）。PreToolUse 超时**不挡**工具调用（官方 hooks 文档），所以它必须比
    /// serve 的审批等待长；helper 自己在等待上限 + 30s 时 deny。
    pub fn timeout_secs(self, comms: &CommsSpec) -> u64 {
        match self {
            Self::PreToolUse => comms.approval_wait_secs + 60,
            Self::PostToolUse | Self::Stop => QUICK_HOOK_TIMEOUT_SECS,
        }
    }
}

/// 审批闸 → PreToolUse 的 matcher。外层括号让 claude 走正则路径（只含字母数字 `_ - , |`
/// 空格的 matcher 是精确匹配，`computer-use` 就匹配不到 `mcp__computer-use__x`）；
/// 不锚定 = 子串匹配，与 Swift `toolName.contains(gate)` 同义。空表 = None。
pub fn gate_matcher(gates: &[String]) -> Option<String> {
    let parts: Vec<String> = gates
        .iter()
        .map(|g| g.trim())
        .filter(|g| !g.is_empty())
        .map(|g| {
            let mut s = String::new();
            for ch in g.chars() {
                if r"\^$.|?*+()[]{}/".contains(ch) {
                    s.push('\\');
                }
                s.push(ch);
            }
            s
        })
        .collect();
    (!parts.is_empty()).then(|| format!("({})", parts.join("|")))
}

/// 写 token 文件（0600、`create_new`）。claude 的启动目录与 codex 驱动共用。
pub fn write_token_file(dir: &Path, token: &str) -> std::io::Result<PathBuf> {
    write_private(dir, TOKEN_FILE, token.as_bytes())
}
```

`prepare_claude` 里 `if let Some(comms) = &spec.comms { … }` 整段换成：

```rust
    if let Some(comms) = &spec.comms {
        let token_file = write_token_file(files.dir(), &comms.token)?;
        let helper = HelperCommands(comms);
        let mcp = json!({"mcpServers": {"crew": {
            "type": "stdio",
            "command": comms.helper.display().to_string(),
            "args": helper.mcp_args(&token_file),
        }}});
        args.mcp_config_file = Some(write_json(files.dir(), "mcp.json", &mcp)?);
        if comms.hooks {
            let mut hooks = serde_json::Map::new();
            for e in HookEvent::ALL {
                let mut entry = serde_json::Map::new();
                if e == HookEvent::PreToolUse {
                    match gate_matcher(&comms.approval_gates) {
                        Some(m) => {
                            entry.insert("matcher".to_string(), Value::String(m));
                        }
                        None => continue,
                    }
                }
                entry.insert(
                    "hooks".to_string(),
                    json!([{
                        "type": "command",
                        "command": helper.hook_command(e, &token_file),
                        "timeout": e.timeout_secs(comms),
                    }]),
                );
                hooks.insert(
                    e.settings_key().to_string(),
                    Value::Array(vec![Value::Object(entry)]),
                );
            }
            settings.insert("hooks".to_string(), Value::Object(hooks));
        }
    }
```

模块文档第 8–9 行改成：「hooks / MCP 的命令行指向 helper（`crewd mcp` / `crewd hook`，spec §22.3）。命令行的形状只在 [`HelperCommands`] 一处拼。」

- [ ] **Step 4: 跑通**

Run: `cargo test -p crewd-exec --lib launch`
Expected: 全过（M2b 原有 10 条，删 2 加 6：`launch` 下 14 条）。

- [ ] **Step 5: 变异核对**

把 `gate_matcher` 末尾的 `format!("({})", ..)` 改成 `parts.join("|")`：Expected `gate_matcher_escapes_and_forces_the_regex_path`、`hooks_are_quoted_shell_strings_with_explicit_timeouts` 红。用编辑器撤销（**不要** `git checkout -- <file>`，会丢掉未提交的实现），再跑回全绿。

- [ ] **Step 6（仅当 M2c 已在 main 上）: codex 调用点**

```bash
grep -rn "mcp_args_for\|crew_id: \|structured_post\|captain: " crates/exec/src crates/exec/tests crates/crewd/src | grep -v "^crates/exec/src/launch.rs"
```

每处 `HelperCommands(c).mcp_args_for("codex")` 改成：先 `let token_file = crate::launch::write_token_file(<该会话的启动目录>, &c.token)?;`，再 `HelperCommands(c).mcp_args(&token_file)`；codex 驱动没有现成的启动目录就照 `prepare_claude` 建 `runtime_dir.join(format!("codex-{key}"))`（0700，`LaunchFiles` 管删除），`key` 用 crewd session id。删掉 `mcp_args_for`（不留转发：无兼容垫片）。测试里的 `CommsSpec { crew_id, captain, structured_post, label, .. }` 字面量按新字段改（`token: "t".into(), approval_gates: vec![], approval_wait_secs: 0`）。grep 没输出 = M2c 还没合，跳过本步，并在提交信息里写「M2c 未合，codex 调用点留给后合者」。

- [ ] **Step 7: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/exec/src/launch.rs   # Step 6 改过的 codex 文件逐个列上
git diff --cached --stat
git commit -m "crewd-exec: helper argv carries only data dir/session/token file; gated PreToolUse matcher; explicit hook timeouts

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 3: 工具表与工具文字（`server::agent::{catalog, text}`、`tools.{zh,en}.md`、`worldview::tools_file`）+ 字面量扫描

**Files:**
- Create: `crewd/crates/collab/prompts/tools.zh.md`、`crewd/crates/collab/prompts/tools.en.md`
- Modify: `crewd/crates/collab/src/worldview.rs`（加 `tools_file`：tools md 在整个 workspace 里唯一的加载处，collab 的 `turn_trace` 与 server 的 `agent::text` 共用；`render` / `WorldCtx` 留到 Task 4）
- Create: `crewd/crates/server/src/agent/mod.rs`、`agent/text.rs`、`agent/catalog.rs`、`agent/literals.rs`
- Modify: `crewd/crates/server/src/lib.rs`（`pub mod agent;`）、`crewd/crates/server/Cargo.toml`（`crewd-collab` 依赖，M3a 已加就跳过）

**Interfaces:**
- Consumes: Task 1 `crewd_proto::agent::ToolDef`；M3a `crewd_proto::Role`、`crewd_collab::Locale`、`crewd_collab::prompts::{parse, PromptFile, fill, placeholders}`（占位符语法是 M3a 定的 `{名字}`）。
- Produces:
  - `crewd_collab::worldview::tools_file(l: Locale) -> &'static PromptFile`
  - `crate::agent::{Identity, Role}`（`Role` = `crewd_proto::Role` 的再导出）：`Identity { session_id: String, crew_id: String, role: Role, label: String, builtin_crew: bool }`
  - `crate::agent::text::{get(Locale, &str) -> &'static str, r(Locale, &str, &[(&str, &str)]) -> String, section_names(Locale) -> Vec<&'static str>}`
  - `crate::agent::catalog::{Scope, Ty, Param, ToolSpec, CATALOG, REMOVED, find(&str) -> Option<&'static ToolSpec>, visible(&ToolSpec, &Identity) -> bool, tools_for(&Identity, Locale) -> Vec<ToolDef>, input_schema(&ToolSpec, Locale) -> Value, CATEGORIES}`

- [ ] **Step 1: 写失败的测试**

`crates/server/src/agent/catalog.rs` 测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Identity, Role};
    use crewd_collab::worldview::Locale;

    fn who(role: Role, builtin: bool) -> Identity {
        Identity {
            session_id: "s-1".into(),
            crew_id: "crew-a".into(),
            role,
            label: "示例".into(),
            builtin_crew: builtin,
        }
    }

    fn names(id: &Identity) -> Vec<String> {
        tools_for(id, Locale::Zh).into_iter().map(|t| t.name).collect()
    }

    #[test]
    fn names_are_unique_and_removed_tools_are_absent() {
        let mut seen = std::collections::BTreeSet::new();
        for t in CATALOG {
            assert!(seen.insert(t.name), "重名 {}", t.name);
        }
        for gone in REMOVED {
            assert!(find(gone).is_none(), "{gone} 应已删除（§9 / §22.7 Q4）");
        }
        assert_eq!(CATALOG.len(), 40);
    }

    #[test]
    fn worker_and_captain_get_different_tables() {
        let worker = names(&who(Role::Worker, false));
        let captain = names(&who(Role::Captain, false));
        assert!(worker.contains(&"post".to_string()));
        assert!(!worker.contains(&"start_session".to_string()));
        assert!(captain.contains(&"start_session".to_string()));
        assert_eq!(worker.len(), 21);
        assert_eq!(captain.len(), 40);
    }

    #[test]
    fn builtin_crew_hides_ask_respond_and_start_session() {
        let chief = names(&who(Role::Captain, true));
        for hidden in ["ask", "respond_todo", "start_session"] {
            assert!(!chief.contains(&hidden.to_string()), "{hidden}");
        }
        assert!(chief.contains(&"report_to_parent".to_string()));
    }

    #[test]
    fn schema_is_strict_and_carries_md_descriptions() {
        let post = find("post").unwrap();
        let s = input_schema(post, Locale::Zh);
        assert_eq!(s["type"], "object");
        assert_eq!(s["additionalProperties"], false);
        assert_eq!(
            s["properties"]["channel"]["description"],
            crate::agent::text::get(Locale::Zh, "post.channel")
        );
        assert_eq!(s["properties"]["mentions"]["items"]["properties"]["kind"]["enum"][3], "broadcast");
        assert_eq!(s["properties"]["reply_to"]["type"], serde_json::json!(["string", "integer"]));
        let ask = input_schema(find("ask").unwrap(), Locale::Zh);
        assert_eq!(ask["required"], serde_json::json!(["question"]));
        let none = input_schema(find("list_channels").unwrap(), Locale::Zh);
        assert_eq!(none["properties"], serde_json::json!({}));
    }

    #[test]
    fn child_crew_text_steers_collaboration_to_channels() {
        // 人 2026-10-06：协作与话题用群，子机组只表达组织结构（部门 / 团队）。
        for loc in [Locale::Zh, Locale::En] {
            assert!(crate::agent::text::get(loc, "create_child_crew").contains("channel_create"), "{loc:?}");
            assert!(crate::agent::text::get(loc, "channel_create").len() > 40, "{loc:?}");
        }
    }

    #[test]
    fn every_tool_and_param_has_text_in_both_locales() {
        for loc in [Locale::Zh, Locale::En] {
            let have = crate::agent::text::section_names(loc);
            for t in CATALOG {
                assert!(have.contains(&t.name), "{loc:?} 缺 ## {}", t.name);
                for p in t.params {
                    let key = format!("{}.{}", t.name, p.name);
                    assert!(have.iter().any(|h| *h == key), "{loc:?} 缺 ## {key}");
                }
            }
        }
    }
}
```

`crates/server/src/agent/text.rs` 测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zh_and_en_have_the_same_sections() {
        assert_eq!(section_names(Locale::Zh), section_names(Locale::En));
    }

    #[test]
    fn r_fills_placeholders() {
        // r.unknown_tool 是 Task 3 就有的节：「没有 {name} 这个工具。」
        let s = r(Locale::Zh, "r.unknown_tool", &[("name", "nope")]);
        assert!(s.contains("nope") && !s.contains("{name}"), "{s}");
    }

    #[test]
    fn fill_does_not_expand_inside_values() {
        // Review Focus 5：agent 的原话里可能就有 `{名字}` 形状的文字，替换进去的值不能被再展开。
        // 这条测的是 M3a 的 `crewd_collab::prompts::fill`：M3a 计划里的实现是逐个 `replace`，会红——
        // 红了就停下回报控制者（G-F：改成一遍扫描），不要在 M3b 里另写一个 fill。
        let out = crewd_collab::prompts::fill("{a} / {b}", &[("a", "{b}"), ("b", "B")]);
        assert_eq!(out, "{b} / B");
    }

    #[test]
    #[should_panic(expected = "tools.zh.md")]
    fn missing_section_names_the_file() {
        get(Locale::Zh, "no.such.section");
    }
}
```

`crates/server/src/agent/literals.rs`（整个文件，只有测试）：

```rust
//! 尺子：给 agent 看的文字只在 md 里（spec §22.3 验收「源码里没有工具描述 / 世界观字面量」）。
//! 扫 agent 模块与 collab 两个文件的**非测试代码里的字符串字面量**（注释不算），出现 CJK 就红；
//! 另外把源码里出现的每个 `"r.…"` / `"trace.…"` / `"inject.…"` 键都核一遍 md 里有。
#![cfg(test)]

use std::path::{Path, PathBuf};

use crewd_collab::worldview::Locale;

fn sources() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    let mut stack = vec![root.join("src/agent")];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") && !p.ends_with("literals.rs") {
                out.push(p);
            }
        }
    }
    for f in ["worldview.rs", "turn_trace.rs"] {
        out.push(root.join("../collab/src").join(f));
    }
    assert!(out.len() >= 4, "扫描范围不对：{out:?}");
    out
}

/// 非测试部分：第一个 `#[cfg(test)]` 之前（各文件的测试模块都放在末尾）。
fn non_test(src: &str) -> &str {
    src.find("#[cfg(test)]").map_or(src, |i| &src[..i])
}

/// 取出全部字符串字面量的内容（普通 / 原始 / 字节串），跳过行注释、块注释与字符字面量。
fn string_literals(src: &str) -> Vec<String> {
    let b: Vec<char> = src.chars().collect();
    let (mut i, mut out) = (0, Vec::new());
    while i < b.len() {
        match b[i] {
            '/' if b.get(i + 1) == Some(&'/') => {
                while i < b.len() && b[i] != '\n' {
                    i += 1;
                }
            }
            '/' if b.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            'r' if matches!(b.get(i + 1), Some('"') | Some('#'))
                && (i == 0 || !b[i - 1].is_alphanumeric() && b[i - 1] != '_') =>
            {
                let mut j = i + 1;
                let mut hashes = 0;
                while b.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if b.get(j) != Some(&'"') {
                    i += 1;
                    continue;
                }
                j += 1;
                let start = j;
                loop {
                    if j >= b.len() {
                        break;
                    }
                    if b[j] == '"' && (0..hashes).all(|k| b.get(j + 1 + k) == Some(&'#')) {
                        break;
                    }
                    j += 1;
                }
                out.push(b[start..j.min(b.len())].iter().collect());
                i = j + 1 + hashes;
            }
            '"' => {
                let mut j = i + 1;
                let mut s = String::new();
                while j < b.len() && b[j] != '"' {
                    if b[j] == '\\' {
                        j += 1;
                    }
                    if j < b.len() {
                        s.push(b[j]);
                    }
                    j += 1;
                }
                out.push(s);
                i = j + 1;
            }
            '\'' => {
                // 字符字面量 'x' / '\n' / '\''；生命周期 'a 不闭合，原样跳过一个字符。
                if b.get(i + 1) == Some(&'\\') {
                    let mut j = i + 2;
                    while j < b.len() && b[j] != '\'' {
                        j += 1;
                    }
                    i = j + 1;
                } else if b.get(i + 2) == Some(&'\'') {
                    i += 3;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3000..=0x303F | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF)
}

#[test]
fn tokenizer_sees_strings_not_comments() {
    let src = "// 注释里的「中文」\nlet a = \"x\"; /* 块 */ let b = r#\"y\"#; let c = '\"'; let d: &'static str = \"z\";";
    assert_eq!(string_literals(src), ["x", "y", "z"]);
}

#[test]
fn no_agent_facing_literals_in_source() {
    for f in sources() {
        let src = std::fs::read_to_string(&f).unwrap();
        for lit in string_literals(non_test(&src)) {
            assert!(
                !lit.chars().any(is_cjk),
                "{} 的字符串字面量里有给人看的文字（应放进 prompts/*.md）：{lit:?}",
                f.display()
            );
        }
    }
}

#[test]
fn every_text_key_used_in_source_exists_in_both_locales() {
    let prefixes = ["r.", "trace.", "inject."];
    let mut used = std::collections::BTreeSet::new();
    for f in sources() {
        let src = std::fs::read_to_string(&f).unwrap();
        for lit in string_literals(&src) {
            if prefixes.iter().any(|p| lit.starts_with(p))
                && lit.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
            {
                used.insert(lit);
            }
        }
    }
    assert!(used.contains("r.unknown_tool"), "尺子没扫到已知的键：{used:?}");
    for loc in [Locale::Zh, Locale::En] {
        let have = crate::agent::text::section_names(loc);
        for k in &used {
            assert!(have.iter().any(|h| h == k), "{loc:?} 的 tools md 缺 ## {k}");
        }
    }
}
```

- [ ] **Step 2: 跑一遍确认失败**

Run: `cargo test -p crewd-server --lib agent`
Expected: 编译失败（`unresolved module agent`）。

- [ ] **Step 3: 写 `tools_file`**（`worldview.rs`，M3a stub 的 `WorldCtx` / `context_for` / `render` 不动，Task 4 再改）

```rust
use std::sync::LazyLock;

use crate::prompts::{PromptFile, parse};

static TOOLS_ZH: LazyLock<PromptFile> = LazyLock::new(|| {
    parse("tools.zh", include_str!("../prompts/tools.zh.md")).unwrap_or_else(|e| panic!("{e}"))
});
static TOOLS_EN: LazyLock<PromptFile> = LazyLock::new(|| {
    parse("tools.en", include_str!("../prompts/tools.en.md")).unwrap_or_else(|e| panic!("{e}"))
});

/// `prompts/tools.{zh,en}.md`：工具描述、参数说明、回执、代发稿。整个 workspace 只在这里加载一次
/// （collab 的 `turn_trace` 取 `trace.*`，server 的 `agent::text` 取其余）。
pub fn tools_file(l: crate::Locale) -> &'static PromptFile {
    match l {
        crate::Locale::Zh => &TOOLS_ZH,
        crate::Locale::En => &TOOLS_EN,
    }
}
```

- [ ] **Step 4: 写 `agent/mod.rs`、`text.rs`、`catalog.rs`**

`crates/server/src/lib.rs` 模块声明区加 `pub mod agent;`。

`crates/server/src/agent/mod.rs`：

```rust
//! agent 面（spec §22.3）：`crewd mcp` / `crewd hook` 在 serve 里的另一半。
//! 给 agent 看的文字全在 `crates/collab/prompts/{tools,world-model}.*.md`；本模块的非测试代码里
//! 不写中文字面量（`literals` 测试钉住），给开发者看的错误写英文短句。
pub mod catalog;
pub mod text;

mod literals;

pub use crewd_proto::Role;

/// 一个 attach 过的 session 此刻的身份。每次调用现查（角色会变：换机长）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub session_id: String,
    pub crew_id: String,
    pub role: Role,
    /// 群里的显示名（session 的 title / display_name，没有就是 id）。
    pub label: String,
    /// 所在机组是内建总机组（`crews.builtin`）。
    pub builtin_crew: bool,
}
```

`crates/server/src/agent/text.rs`：

```rust
//! `crates/collab/prompts/tools.{zh,en}.md` 的取用（加载在 `crewd_collab::worldview::tools_file`）。节：`## <工具>`、
//! `## <工具>.<参数>`、`## r.*`（回执）、`## trace.*`（代发稿）、`## inject.*`（注入块补充）。占位符是 M3a 的 `{名字}`。
pub use crewd_collab::Locale;
use crewd_collab::prompts::fill;
use crewd_collab::worldview::tools_file;

/// 缺节 = 程序员错误（`every_text_key_used_in_source_exists_in_both_locales` 钉住），panic 写明文件与节。
pub fn get(loc: Locale, key: &str) -> &'static str {
    tools_file(loc).get(key)
}

/// 取一节并填 `{占位符}`。
pub fn r(loc: Locale, key: &str, vars: &[(&str, &str)]) -> String {
    fill(get(loc, key), vars)
}

pub fn section_names(loc: Locale) -> Vec<&'static str> {
    tools_file(loc).section_names()
}
```

（`PromptFile::get` 的 panic 文案是 `prompts/{name}.md 缺少一节`，`name = "tools.zh"` 时恰好含 `tools.zh.md`；M3a 的解析器文案不同就按它改 `missing_section_names_the_file` 的 `expected`。）

`crates/server/src/agent/catalog.rs`：

```rust
//! 工具表（spec §22.3 表 + §22.7 Q2 / Q4）。只有名字、范围、参数形状；文字全在 tools.*.md。
use serde_json::{Map, Value, json};

use crate::agent::text::{self, Locale};
use crate::agent::{Identity, Role};
use crewd_proto::agent::ToolDef;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    Captain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    Str,
    Int,
    Num,
    Bool,
    StrList,
    /// 消息引用：序号（数字或 `#123`）或消息 id。
    Ref,
    /// `[{kind, target_id?}]`
    Mentions,
    /// `[{text, category?}]`
    Messages,
}

#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub name: &'static str,
    pub ty: Ty,
    pub required: bool,
    pub choices: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub struct ToolSpec {
    pub name: &'static str,
    pub scope: Scope,
    /// 内建总机组上不列（`McpServer.swift:565`、`:745-753`）。
    pub not_on_builtin: bool,
    pub params: &'static [Param],
}

pub const CATEGORIES: &[&str] = &[
    "progress", "done", "blocked", "question", "finding", "handoff", "ack", "note",
];
pub const MENTION_KINDS: &[&str] = &["session", "captain", "human", "broadcast"];
const RUNNERS: &[&str] = &["claude", "codex"];
const CHANNEL_KINDS: &[&str] = &["topic", "dm"];

const fn p(name: &'static str, ty: Ty, required: bool) -> Param {
    Param { name, ty, required, choices: &[] }
}
const fn c(name: &'static str, required: bool, choices: &'static [&'static str]) -> Param {
    Param { name, ty: Ty::Str, required, choices }
}
const fn all(name: &'static str, params: &'static [Param]) -> ToolSpec {
    ToolSpec { name, scope: Scope::All, not_on_builtin: false, params }
}
const fn cap(name: &'static str, params: &'static [Param]) -> ToolSpec {
    ToolSpec { name, scope: Scope::Captain, not_on_builtin: false, params }
}
const fn not_builtin(t: ToolSpec) -> ToolSpec {
    ToolSpec { not_on_builtin: true, ..t }
}

use Ty::*;

pub const CATALOG: &[ToolSpec] = &[
    all("post", &[
        p("channel", Str, false),
        p("message", Str, false),
        p("messages", Messages, false),
        p("mentions", Mentions, false),
        p("reply_to", Ref, false),
        c("category", false, CATEGORIES),
    ]),
    all("read_channel", &[p("channel", Str, false), p("limit", Int, false), p("before", Ref, false)]),
    all("search", &[
        p("query", Str, true),
        p("channel", Str, false),
        p("after", Str, false),
        p("before", Str, false),
        p("limit", Int, false),
    ]),
    all("list_channels", &[]),
    all("human_watching", &[]),
    all("channel_create", &[
        p("name", Str, true),
        p("purpose", Str, false),
        p("members", StrList, true),
        c("kind", false, CHANNEL_KINDS),
    ]),
    all("channel_invite", &[p("channel", Str, true), p("members", StrList, true)]),
    all("channel_leave", &[p("channel", Str, true)]),
    all("channel_archive", &[p("channel", Str, true)]),
    all("channel_transfer", &[p("channel", Str, true), p("to", Str, true), p("force", Bool, false)]),
    all("channel_accept", &[p("channel", Str, true)]),
    all("channel_listen", &[p("channel", Str, true), p("minutes", Num, false), p("senders", StrList, false)]),
    all("channel_unlisten", &[p("channel", Str, true)]),
    all("directory", &[p("query", Str, false)]),
    all("contact", &[p("to", Str, true), p("message", Str, true)]),
    not_builtin(all("ask", &[
        p("question", Str, true),
        p("resume_note", Str, false),
        p("fallback_after_minutes", Num, false),
        p("fallback", Str, false),
    ])),
    all("withdraw_ask", &[p("ask", Str, true), p("reason", Str, true)]),
    not_builtin(all("respond_todo", &[p("number", Int, true), p("response", Str, true)])),
    all("get_quota", &[]),
    all("schedule_wakeup", &[p("after_minutes", Num, false), p("at", Str, false), p("note", Str, true)]),
    all("set_session_profile", &[p("model", Str, false), p("effort", Str, false)]),
    cap("rename_crew", &[p("name", Str, true)]),
    cap("change_workdir", &[
        p("new_path", Str, true),
        p("crew", Str, false),
        p("include_children", Bool, false),
        p("confirm", Bool, false),
    ]),
    cap("set_crew_status", &[p("text", Str, true)]),
    cap("read_delegation_policy", &[]),
    cap("update_delegation_policy", &[p("text", Str, true)]),
    not_builtin(cap("start_session", &[
        p("brief", Str, true),
        p("title", Str, false),
        c("runner", false, RUNNERS),
        p("isolation", Bool, true),
        p("model", Str, false),
        p("effort", Str, false),
    ])),
    cap("stop_session", &[p("session_id", Str, true), p("reason", Str, true)]),
    cap("inspect_session", &[p("session_id", Str, true)]),
    cap("nudge_session", &[p("session_id", Str, true), p("input", Str, true)]),
    cap("list_sessions", &[]),
    cap("handoff_captain_to_session", &[p("session_id", Str, true), p("target_crew_id", Str, false)]),
    cap("create_and_handoff_captain", &[
        c("runner", true, RUNNERS),
        p("model", Str, false),
        p("effort", Str, false),
        p("title", Str, false),
        p("opening_brief", Str, false),
        p("target_crew_id", Str, false),
    ]),
    cap("report_to_parent", &[p("message", Str, true)]),
    cap("message_child_crew", &[p("crew", Str, true), p("message", Str, true)]),
    cap("adopt_crew", &[p("crew", Str, true)]),
    cap("release_crew", &[p("crew", Str, true), p("to", Str, false)]),
    cap("create_parent_crew", &[p("title", Str, false)]),
    cap("adopt_parent", &[p("crew", Str, true)]),
    cap("create_child_crew", &[p("brief", Str, true), p("title", Str, false), c("runner", false, RUNNERS)]),
];

/// 已删的旧工具名（§9、§22.3、§22.7 Q4）与改了名的旧名：测试断言它们不在表里。
pub const REMOVED: &[&str] = &[
    "continue_work", "confirm_todo_sweep", "plan_add", "plan_update", "plan_list",
    "raise_attention", "clear_attention", "crew_ordering_signals", "arrange_crews",
    "post_to_crew", "read_whiteboard", "search_whiteboard", "add_human_todo",
    "withdraw_human_todo", "listen", "answer_decision",
];

pub fn find(name: &str) -> Option<&'static ToolSpec> {
    CATALOG.iter().find(|t| t.name == name)
}

/// 这个身份此刻能不能看见 / 调这个工具。
pub fn visible(t: &ToolSpec, me: &Identity) -> bool {
    (t.scope == Scope::All || me.role == Role::Captain) && !(t.not_on_builtin && me.builtin_crew)
}

pub fn tools_for(me: &Identity, loc: Locale) -> Vec<ToolDef> {
    CATALOG
        .iter()
        .filter(|t| visible(t, me))
        .map(|t| ToolDef {
            name: t.name.to_string(),
            description: text::get(loc, t.name).to_string(),
            input_schema: input_schema(t, loc),
        })
        .collect()
}

pub fn input_schema(t: &ToolSpec, loc: Locale) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for prm in t.params {
        let mut s = match prm.ty {
            Str => json!({"type": "string"}),
            Int => json!({"type": "integer"}),
            Num => json!({"type": "number"}),
            Bool => json!({"type": "boolean"}),
            StrList => json!({"type": "array", "items": {"type": "string"}}),
            Ref => json!({"type": ["string", "integer"]}),
            Mentions => json!({"type": "array", "items": {
                "type": "object",
                "properties": {
                    "kind": {"type": "string", "enum": MENTION_KINDS},
                    "target_id": {"type": "string"},
                },
                "required": ["kind"],
                "additionalProperties": false,
            }}),
            Messages => json!({"type": "array", "maxItems": 6, "items": {
                "type": "object",
                "properties": {
                    "text": {"type": "string"},
                    "category": {"type": "string", "enum": CATEGORIES},
                },
                "required": ["text"],
                "additionalProperties": false,
            }}),
        };
        if !prm.choices.is_empty() {
            s["enum"] = json!(prm.choices);
        }
        let key = format!("{}.{}", t.name, prm.name);
        s["description"] = Value::String(text::get(loc, &key).to_string());
        props.insert(prm.name.to_string(), s);
        if prm.required {
            required.push(prm.name);
        }
    }
    let mut schema = json!({
        "type": "object",
        "properties": props,
        "additionalProperties": false,
    });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    schema
}
```

`mod literals;` 已在 `mod.rs` 里；`literals.rs` 是 Step 1 的整文件。

- [ ] **Step 5: 写 `tools.zh.md` 与 `tools.en.md`**

`crates/collab/prompts/tools.zh.md`：

```markdown
<!--
给 agent 看的工具文字（spec §22.3）。第一个节标题之前是注释（注释里任何一行都不许以「## 」开头，解析器会当成节）。
节名：<工具> = 工具描述；<工具>.<参数> = 参数说明；r.* = 回执；trace.* = 停在问句上时的代发稿；inject.* = 注入块的补充。
占位符 {名字} 由代码填（M3a 的 prompts::fill）。zh 与 en 的节名集合必须一致（测试钉住）。改工具行为时同一拍改这里。
-->

## post
把关键节点发到群里：开始、完成、卡住、交接、重要发现。终端里的过程（读了什么、跑了什么）别往群里倒。
不填 channel：给了 reply_to 就发到那条所在的群，否则发本机组大厅。
@ 的规则：@session / @captain 收窄可见范围——只有被点到的 agent 在注入里看得见、并被叫醒；同时加上 @broadcast 就是「全组看得见、只叫醒被点到的」；@human 不收窄，只是标给人看。人类没有注视的群里 @人类，标记会亮，但人可能很久才看到——急事发根机组人类群，或用 ask。

## post.channel
群 id，或你所在的群里唯一的群名。不填见工具说明。

## post.message
发一条时用它。与 messages 二选一。

## post.messages
一次发几条（最多 6 条），每条一个气泡、各带自己的 category。任何一条不合法就整批不发，错误会指名第几条。

## post.mentions
定向 @：{kind:"session", target_id:"<session id>"} / {kind:"captain"} / {kind:"human"} / {kind:"broadcast"}。不填 = 发给群里所有人看。被 @ 的 session 必须是这个群的成员。

## post.reply_to
你在回复哪一条：消息序号（如 #128 或 128）或消息 id。会自动 @ 原发送者，形状是 [broadcast, 原发送者]：全组照样看得见，只是把他叫醒。

## post.category
这条是什么：progress 推进 / done 完成 / blocked 卡住 / question 要问 / finding 发现 / handoff 交接 / ack 收到 / note 其它。


## read_channel
分页读一个群的消息（旧在前）。读到最新一条时，这个群对你就算已读，之后不会再注入这些消息。每条前面的 #序号 可以填进 reply_to。

## read_channel.channel
群 id 或群名；不填 = 本机组大厅。本机组的群都能读，不必是成员。

## read_channel.limit
条数，1–200，默认 50。

## read_channel.before
只看这一条之前的（消息序号或 id）。不填 = 最新一页。

## search
在你所在的群和你收听的群里搜消息。空白分隔的词全部命中才算；按时间倒序。

## search.query
要搜的词，空白分隔，全部命中。

## search.channel
只搜这一个群（群 id 或群名）。

## search.after
只要这个时刻之后的（ISO8601，含边界）。

## search.before
只要这个时刻之前的（ISO8601，含边界）。

## search.limit
条数上限，1–200，默认 50。

## list_channels
列出你所在的群（大厅、你加入的群、你收听的群）和本机组的其它群：id、名字、用途、群主、成员、人类是否在注视。

## human_watching
人类现在在哪儿：他标了「我在注视」的群，加上根机组的人类群。要人拍板、要人看见，发到这些群；不在其中的群里 @人类，他可能很久才看到。

## channel_create
建一个群（话题群或两人私聊）。要和谁就某件事协作、要单独讨论一个话题，就用它——不要为此开子机组。你是群主，自动在群里。群有名字和用途，例如「发版」「scroll bug 排查」。

## channel_create.name
群名，短。

## channel_create.purpose
这个群是干什么的，一句话。

## channel_create.members
要拉进来的成员：session id、通讯录号码（如 7-3），或 human（人类）。

## channel_create.kind
topic（话题群，默认）或 dm（恰好两个 session 的私聊）。

## channel_invite
把人拉进一个群。

## channel_invite.channel
群 id 或群名。

## channel_invite.members
session id、通讯录号码（如 7-3），或 human。

## channel_leave
退出一个群。大厅退不了。

## channel_leave.channel
群 id 或群名。

## channel_archive
归档一个群（消息保留，不能再发言；可以由人解档）。只有群主或机长能归档。

## channel_archive.channel
群 id 或群名。

## channel_transfer
把群主转给别人。群主发起后要对方用 channel_accept 接受才生效；机长可以 force 直接转。群主相当于这个群的机长：人类在群里说话没点名时，调度判断不了就叫群主。

## channel_transfer.channel
群 id 或群名。

## channel_transfer.to
新群主：session id 或通讯录号码。必须是群成员。

## channel_transfer.force
机长专用：true = 不等对方接受，直接转。

## channel_accept
接受别人转给你的群主。

## channel_accept.channel
群 id 或群名。

## channel_listen
收听一个群：这个群有新消息时也叫醒你（不用被 @）。在等某个群的动静时用；收听期间正常结束回合等着，别轮询 read_channel。

## channel_listen.channel
群 id 或群名。

## channel_listen.minutes
听多久（分钟，1–480）。不填 = 一直听到 channel_unlisten。

## channel_listen.senders
只听这些人：human、captain，或 session id。不填 = 所有人。

## channel_unlisten
停止收听一个群。

## channel_unlisten.channel
群 id 或群名。

## directory
查通讯录：本机每个机组、每个 session 都有短号码——机组是整数（7），成员是分机（7-3），-1 恒定是那个机组的机长。返回号码、名字、所属机组、在干什么、在不在线，以及各机组的群。人类不编号，找人用 ask。

## directory.query
按号码前缀、名字或关键词过滤；不填 = 全表。

## contact
按号码联系别的机组或 session。to 填机组号（如 7）= 发到它的大厅并 @它的机长，消息带上你的来源；填分机号（如 7-3）= 与那个 session 私聊（没有就建）。你自己的大厅会留一行「已联系 …」。本机组的事直接 post。

## contact.to
目标号码：7（机组）或 7-3（session）。

## contact.message
要说的话：写清你是谁、要什么。

## ask
向人类提一个要他拍板的问题。不阻塞：问题记成一条待确认（H 号），发进根机组人类群并 @人类；你立刻拿到回执，去干别的或收工。人回答时系统直接叫醒你，并把 resume_note 原样念回给你。

## ask.question
要人拍板的那件事。把选项和你的倾向写出来——「A / B，我倾向 A，因为 …」。

## ask.resume_note
答复回来后你要接着做什么（做到哪一步、下一步是什么）。半路上问的问题一定要写。

## ask.fallback_after_minutes
可选：等这么多分钟没人答，就叫醒你按 fallback 办。只用于可撤回的事；到点之后若要动生产、花钱、隐私或凭据，照样会被审批拦住。

## ask.fallback
到点没人答时你打算怎么办（「就按 A 做」）。和 fallback_after_minutes 一起给。

## withdraw_ask
撤回一条还没人答的问（那件事已经不成立了）。你自己提的能撤；机长能撤本机组的任何一条。还卡着工作项的问撤不了。

## withdraw_ask.ask
H 号，如 H3。

## withdraw_ask.reason
为什么不成立了，一句话说清变了什么。

## respond_todo
回应 Agent 那本 Todo（人类派给你们的活）的某一条：认领、进展、结果。追加式，不改状态。

## respond_todo.number
条目编号 #N 里的 N。

## respond_todo.response
回应内容，一两句。

## get_quota
查本机 claude / codex 的订阅额度：各窗口已用百分比与重置时刻。开长活前看一眼。没有剩余 token 的绝对量，别编。

## schedule_wakeup
定时叫醒自己：到点把 note 送回给你。额度将尽时的标准动作：收尾到可交接 → 约在重置后几分钟 → 停手。醒来时只有 note 和群消息可依靠，note 要自足。

## schedule_wakeup.after_minutes
多少分钟后（1–1440）。与 at 二选一。

## schedule_wakeup.at
ISO8601 时刻，必须在未来。与 after_minutes 二选一。

## schedule_wakeup.note
醒来该干什么、上下文在哪。

## set_session_profile
切你自己的模型或推理强度。不是当场生效：本回合结束后才切，结果回执到群里。最贵的档位只在普通档位确实不行时才用。

## set_session_profile.model
模型别名，如 opus / sonnet。

## set_session_profile.effort
推理强度档位。

## rename_crew
（机长）给机组起个短标签名，例如「鉴权重构」。搞清楚机组在做什么再起，起一次就够。

## rename_crew.name
几个字的短标签，不带标点和前后缀。

## change_workdir
（机长）改机组的工作目录。先不带 confirm 看预览，再 confirm: true 执行。只能改自己和自己名下的子机组。

## change_workdir.new_path
新目录的绝对路径，必须已存在。

## change_workdir.crew
改哪个机组（名字、id 或号码）；不填 = 本机组。

## change_workdir.include_children
连子机组一起改，默认 true。

## change_workdir.confirm
true = 真执行；不填 = 只看预览。

## set_crew_status
（机长）一句话说清整个机组现在怎么样，显示在侧栏这个机组那一行。超过 200 字不收。

## set_crew_status.text
一句话状态。

## read_delegation_policy
（机长）读本机组的派活原则。

## update_delegation_policy
（机长）改本机组的派活原则：提交完整的新正文。

## update_delegation_policy.text
完整的新正文。

## start_session
（机长）在本机组起一个 worker 去干一件明确的编码任务。

## start_session.brief
要它干的事，写清目标与完成标准。

## start_session.title
≤18 字、不带项目名的任务概括，作群里的显示名。

## start_session.runner
claude 或 codex；不填 = 机组默认。

## start_session.isolation
必填。false = 用机组共享目录；true = 新建独立 worktree。每次都要判断。

## start_session.model
可选模型。

## start_session.effort
可选推理强度。

## stop_session
（机长）停掉本机组某个 session 的进程。先在大厅写明原因再停；不可撤销。只想打断一轮用 nudge_session 发 Esc。

## stop_session.session_id
目标 session id。

## stop_session.reason
停的原因，会公开写进大厅。

## inspect_session
（机长）看某个 session 的现场：状态 + 终端最近的输出（codex 是记录尾部）。@ 不应时先看它卡在哪。

## inspect_session.session_id
目标 session id。

## nudge_session
（机长）给某个 session 的终端发按键或文本替它解围：Enter / Esc 发对应按键，其它文本当一条输入提交。先 inspect_session 看清再按。

## nudge_session.session_id
目标 session id。

## nudge_session.input
Enter、Esc，或要发的文本。

## list_sessions
（机长）列出本机组的 session：id、号码、显示名、角色、状态。

## handoff_captain_to_session
（机长）把机长位置交给一个现有 session。target_crew_id 不填 = 本机组；填了只能是你的直系子机组。

## handoff_captain_to_session.session_id
接任者的 session id，必须属于目标机组。

## handoff_captain_to_session.target_crew_id
可选：直系子机组的 id。

## create_and_handoff_captain
（机长）新起一个 session 并把机长位置交给它。target_crew_id 不填 = 本机组；填了只能是你的直系子机组。

## create_and_handoff_captain.runner
claude 或 codex，必须明确选。

## create_and_handoff_captain.model
可选模型。

## create_and_handoff_captain.effort
可选推理强度。

## create_and_handoff_captain.title
可选显示名；不填 = 机长。

## create_and_handoff_captain.opening_brief
可选：新机长的第一件事。

## create_and_handoff_captain.target_crew_id
可选：直系子机组的 id。

## report_to_parent
（机长）向上级汇报：发到每个直系父机组的大厅并 @父机长。阶段性结果、要上级拍板或协调资源、被卡住时用。结论先行。

## report_to_parent.message
汇报内容。

## message_child_crew
（机长）给一个直系子机组下达消息：发到它的大厅并 @子机长。

## message_child_crew.crew
子机组的名字、id 或号码。

## message_child_crew.message
要传达的内容。

## adopt_crew
（机长）收编：把一个与本机组没有上下级关系的机组挂到本机组名下。不能成环；内建总机组不能被收编。

## adopt_crew.crew
目标机组的名字、id 或号码。

## release_crew
（机长）调整自己直系子机组的挂靠：不填 to = 摘出到顶层；填 to = 转挂到你的另一个直系子机组名下。

## release_crew.crew
直系子机组的名字、id 或号码。

## release_crew.to
可选：转挂目的地（你的另一个直系子机组）。

## create_parent_crew
（机长）在本机组头上新建一个父机组：本机组成为它的子机组，父机组继承工作目录并起父机长。只在组织结构真要多一层（把几个部门归到一个上级下）时用。

## create_parent_crew.title
可选：父机组的短标签名。

## adopt_parent
（机长）认父：把一个现有机组认作本机组的父机组。不能成环。

## adopt_parent.crew
目标机组的名字、id 或号码。

## create_child_crew
（机长）以本机组为父建子机组：继承工作目录，自动起子机长，brief 是子机长的第一件事。子机组只表达组织结构——一个长期存在、有自己机长的部门或团队。围绕某个话题或某件事协作，用 channel_create 建群，不要开子机组。

## create_child_crew.brief
子机组要干的事。

## create_child_crew.title
可选：短标签名。

## create_child_crew.runner
可选：子机长用 claude 还是 codex。

## r.unknown_tool
没有 {name} 这个工具。

## r.captain_only
{name} 只有机长能用，你现在不是机长。

## r.not_on_builtin
{name} 在总机组里不能用。

## r.bad_args
参数不对：{detail}
```

`crates/collab/prompts/tools.en.md`（节名与上面逐一对应）：

```markdown
<!--
Agent-facing tool text (spec §22.3). Everything before the first section heading is a comment (no line in it may start with "## ").
Section names: <tool> = tool description; <tool>.<param> = parameter text; r.* = receipts; trace.* = relay posts; inject.* = injection extras.
Placeholders {name} are filled by code (M3a's prompts::fill). zh and en must have exactly the same section names (tested).
-->

## post
Post key moments to a channel: started, done, blocked, handing off, important findings. Keep terminal detail (what you read or ran) out of channels.
Without channel: if reply_to is given the post goes to that message's channel, otherwise to your crew's lobby.
Mentions: @session / @captain narrow visibility — only the named agents see it in their injected context and get woken; add @broadcast to make it "everyone sees, only the named wake"; @human does not narrow, it only flags the message for the human. @human in a channel the human is not watching lights the badge but may be seen much later — for anything urgent post in the root crew's human channel or use ask.

## post.channel
Channel id, or a channel name that is unique among channels you are in. See the tool text when omitted.

## post.message
Use for a single message. Mutually exclusive with messages.

## post.messages
Several messages at once (at most 6), one bubble each with its own category. If any item is invalid nothing is posted and the error names the item.

## post.mentions
Directed mentions: {kind:"session", target_id:"<session id>"} / {kind:"captain"} / {kind:"human"} / {kind:"broadcast"}. Omit for everyone in the channel. A mentioned session must be a member of the channel.

## post.reply_to
The message you are replying to: its number (#128 or 128) or its id. The original sender is mentioned automatically as [broadcast, sender]: everyone still sees it, the sender is woken.

## post.category
What this is: progress / done / blocked / question / finding / handoff / ack / note.


## read_channel
Read a channel page by page (oldest first). Reading up to the newest message marks the channel read for you; those messages will not be injected again. The #number before each message can go into reply_to.

## read_channel.channel
Channel id or name; omitted = your crew's lobby. Any channel of your crew is readable, membership not required.

## read_channel.limit
Number of messages, 1–200, default 50.

## read_channel.before
Only messages before this one (number or id). Omitted = newest page.

## search
Search messages in channels you are in or listening to. All whitespace-separated terms must match; newest first.

## search.query
Terms, whitespace-separated, all must match.

## search.channel
Search only this channel (id or name).

## search.after
Only at or after this time (ISO8601).

## search.before
Only at or before this time (ISO8601).

## search.limit
Maximum results, 1–200, default 50.

## list_channels
List channels you are in (lobby, joined channels, channels you listen to) and the other channels of your crew: id, name, purpose, owner, members, whether the human is watching.

## human_watching
Where the human is now: channels marked "watching", plus the root crew's human channel. Post there when you need the human to see or decide; @human elsewhere may be seen much later.

## channel_create
Create a channel (topic channel or two-person dm). Use it whenever you need to collaborate with someone on something or discuss a topic separately — do not create a child crew for that. You become the owner and a member. Give it a name and a purpose, e.g. "release", "scroll bug".

## channel_create.name
Short channel name.

## channel_create.purpose
One sentence: what the channel is for.

## channel_create.members
Members to add: session ids, directory numbers (like 7-3), or human.

## channel_create.kind
topic (default) or dm (exactly two sessions).

## channel_invite
Add members to a channel.

## channel_invite.channel
Channel id or name.

## channel_invite.members
Session ids, directory numbers (like 7-3), or human.

## channel_leave
Leave a channel. You cannot leave the lobby.

## channel_leave.channel
Channel id or name.

## channel_archive
Archive a channel (messages kept, no new posts; the human can unarchive). Only the owner or the captain can archive.

## channel_archive.channel
Channel id or name.

## channel_transfer
Hand channel ownership to someone else. The new owner must accept with channel_accept; the captain can force it. The owner acts as the channel's captain: when the human posts without naming anyone and the dispatcher cannot tell who should answer, the owner is woken.

## channel_transfer.channel
Channel id or name.

## channel_transfer.to
New owner: session id or directory number. Must be a member.

## channel_transfer.force
Captain only: true = transfer without waiting for acceptance.

## channel_accept
Accept ownership of a channel offered to you.

## channel_accept.channel
Channel id or name.

## channel_listen
Listen to a channel: new messages there wake you even without a mention. Use it while waiting for activity; end your turn normally while listening instead of polling read_channel.

## channel_listen.channel
Channel id or name.

## channel_listen.minutes
How long (minutes, 1–480). Omitted = until channel_unlisten.

## channel_listen.senders
Only these senders: human, captain, or session ids. Omitted = everyone.

## channel_unlisten
Stop listening to a channel.

## channel_unlisten.channel
Channel id or name.

## directory
Look up the directory: every crew and session on this machine has a short number — crews are integers (7), members are extensions (7-3), and -1 is always that crew's captain. Returns numbers, names, crews, current work, online state, and each crew's channels. The human has no number; use ask.

## directory.query
Filter by number prefix, name or keyword; omitted = everything.

## contact
Reach another crew or session by number. A crew number (7) posts to its lobby mentioning its captain, with your origin attached; an extension (7-3) opens a dm with that session (created if missing). Your own lobby gets a "contacted …" line. For your own crew just post.

## contact.to
Target number: 7 (crew) or 7-3 (session).

## contact.message
What you want to say: who you are and what you need.

## ask
Ask the human for a decision. Non-blocking: the question becomes a pending item (an H number), is posted to the root crew's human channel mentioning the human, and you get a receipt right away — go do something else or end your turn. When the human answers you are woken and resume_note is read back to you verbatim.

## ask.question
The decision you need. State the options and your preference — "A / B, I lean A because …".

## ask.resume_note
What you will do once answered (where you are, what is next). Always write it for questions asked mid-task.

## ask.fallback_after_minutes
Optional: if nobody answers within this many minutes, you are woken to act on fallback. Only for reversible things; production, money, privacy or credentials still hit the approval gate.

## ask.fallback
What you will do if nobody answers in time ("go with A"). Give together with fallback_after_minutes.

## withdraw_ask
Withdraw an unanswered question that no longer applies. You can withdraw your own; the captain can withdraw any in the crew. A question still blocking a work item cannot be withdrawn.

## withdraw_ask.ask
The H number, like H3.

## withdraw_ask.reason
Why it no longer applies: what changed.

## respond_todo
Respond to an item in the Agent Todo list (work the human assigned): claim, progress, result. Append-only; does not change status.

## respond_todo.number
The N in #N.

## respond_todo.response
One or two sentences.

## get_quota
Subscription usage for claude / codex on this machine: percent used per window and reset times. Check before long work. There is no absolute remaining token count; do not invent one.

## schedule_wakeup
Wake yourself later: at the time, note is delivered back to you. When quota runs low: wrap up to a hand-off state, schedule a few minutes after the reset, stop. When you wake you only have the note and channel messages, so make the note self-contained.

## schedule_wakeup.after_minutes
Minutes from now (1–1440). Either this or at.

## schedule_wakeup.at
ISO8601 time in the future. Either this or after_minutes.

## schedule_wakeup.note
What to do on waking and where the context is.

## set_session_profile
Switch your own model or reasoning effort. Not immediate: it applies after the current turn ends and the result is reported in the channel. Use the most expensive tier only when normal tiers clearly fail.

## set_session_profile.model
Model alias such as opus / sonnet.

## set_session_profile.effort
Reasoning effort level.

## rename_crew
(Captain) Give the crew a short label like "auth refactor". Do it once you know what the crew is doing.

## rename_crew.name
A few-word label, no punctuation.

## change_workdir
(Captain) Change the crew's working directory. Call without confirm for a preview, then with confirm: true. Only your crew and your child crews.

## change_workdir.new_path
Absolute path of an existing directory.

## change_workdir.crew
Which crew (name, id or number); omitted = your crew.

## change_workdir.include_children
Also change child crews; default true.

## change_workdir.confirm
true = apply; omitted = preview only.

## set_crew_status
(Captain) One sentence on how the whole crew is doing, shown on the crew's sidebar row. Over 200 characters is refused.

## set_crew_status.text
One-sentence status.

## read_delegation_policy
(Captain) Read the crew's delegation policy.

## update_delegation_policy
(Captain) Replace the crew's delegation policy with the full new text.

## update_delegation_policy.text
The full new text.

## start_session
(Captain) Start a worker in this crew for a clear coding task.

## start_session.brief
The task, with goal and done criteria.

## start_session.title
At most 18 characters, no project name; the worker's display name.

## start_session.runner
claude or codex; omitted = crew default.

## start_session.isolation
Required. false = the crew's shared directory; true = a new worktree. Decide every time.

## start_session.model
Optional model.

## start_session.effort
Optional reasoning effort.

## stop_session
(Captain) Stop a session's process in this crew. The reason is posted to the lobby first; irreversible. To interrupt one turn, nudge_session with Esc instead.

## stop_session.session_id
Target session id.

## stop_session.reason
Why; posted publicly to the lobby.

## inspect_session
(Captain) See a session's state and recent terminal output (codex: transcript tail). Use it when a mention gets no answer.

## inspect_session.session_id
Target session id.

## nudge_session
(Captain) Send keys or text to a session's terminal: Enter / Esc send that key, anything else is submitted as input. Inspect first.

## nudge_session.session_id
Target session id.

## nudge_session.input
Enter, Esc, or text to send.

## list_sessions
(Captain) List this crew's sessions: id, number, name, role, state.

## handoff_captain_to_session
(Captain) Hand the captain role to an existing session. target_crew_id omitted = this crew; otherwise only a direct child crew.

## handoff_captain_to_session.session_id
The successor's session id; must belong to the target crew.

## handoff_captain_to_session.target_crew_id
Optional: a direct child crew id.

## create_and_handoff_captain
(Captain) Start a new session and hand it the captain role. target_crew_id omitted = this crew; otherwise only a direct child crew.

## create_and_handoff_captain.runner
claude or codex; must be explicit.

## create_and_handoff_captain.model
Optional model.

## create_and_handoff_captain.effort
Optional reasoning effort.

## create_and_handoff_captain.title
Optional display name; default "Captain".

## create_and_handoff_captain.opening_brief
Optional first task for the new captain.

## create_and_handoff_captain.target_crew_id
Optional: a direct child crew id.

## report_to_parent
(Captain) Report upward: posts to each direct parent crew's lobby mentioning its captain. For milestones, decisions or resources you need from above, or being blocked. Lead with the conclusion.

## report_to_parent.message
The report.

## message_child_crew
(Captain) Send a message to a direct child crew: posted to its lobby mentioning its captain.

## message_child_crew.crew
Child crew name, id or number.

## message_child_crew.message
What to pass on.

## adopt_crew
(Captain) Adopt a crew that has no parent/child relation with yours, making it your child. No cycles; the built-in chief crew cannot be adopted.

## adopt_crew.crew
Target crew name, id or number.

## release_crew
(Captain) Re-home a direct child crew: without to it becomes top-level; with to it moves under another of your direct children.

## release_crew.crew
Direct child crew name, id or number.

## release_crew.to
Optional destination: another of your direct child crews.

## create_parent_crew
(Captain) Create a parent crew above yours: your crew becomes its child, it inherits the working directory and gets its own captain. Only when the organisation really needs another level (grouping departments under one parent).

## create_parent_crew.title
Optional short label.

## adopt_parent
(Captain) Make an existing crew your crew's parent. No cycles.

## adopt_parent.crew
Target crew name, id or number.

## create_child_crew
(Captain) Create a child crew under yours: it inherits the working directory, gets its own captain, and brief is that captain's first task. Child crews only express organisational structure — a lasting department or team with its own captain. To collaborate on a topic or a task, create a channel with channel_create instead.

## create_child_crew.brief
What the child crew should do.

## create_child_crew.title
Optional short label.

## create_child_crew.runner
Optional: claude or codex for the child captain.

## r.unknown_tool
There is no tool named {name}.

## r.captain_only
{name} is for the captain only, and you are not the captain now.

## r.not_on_builtin
{name} is not available in the chief crew.

## r.bad_args
Invalid arguments: {detail}
```

注意：`tools.zh.md` 的工具说明里有阿拉伯数字（1–200、6 条、≤18 字）——这与 jev 的「提示词不写数字」规则无关（那条只管 Jev 的状态描述），这里的数字是参数范围，必须写。

- [ ] **Step 6: 跑通**

Run: `cargo test -p crewd-server --lib agent`
Expected：catalog 6 + text 4 + literals 3 = `13 passed`。`fill_does_not_expand_inside_values` 红 = M3a 的 `fill` 还是逐个 `replace`（G-F），停下回报，不要绕。（此时源码里唯一的 `"r.…"` 键是 text.rs 测试里的 `r.unknown_tool`——键扫描用的是整份源码，含测试段；Task 7 起非测试代码里才有。）

- [ ] **Step 7: 变异核对**（两把尺子都要先证明会红）

1. 在 `catalog.rs` 的 `find` 函数体第一行前加 `let _ = "示例";`：Expected `no_agent_facing_literals_in_source` 红、报 `catalog.rs`。撤销。
2. 把 `tools.en.md` 的 `## r.bad_args` 改名 `## r.bad_arg`：Expected `zh_and_en_have_the_same_sections` 红。撤销。
3. 删掉 `CATALOG` 里的 `not_builtin(` 包装（`ask` 那一项）：Expected `builtin_crew_hides_ask_respond_and_start_session` 红。撤销。

撤销都用编辑器，不用 `git checkout`。

- [ ] **Step 8: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/collab/prompts/tools.zh.md crates/collab/prompts/tools.en.md crates/collab/src/worldview.rs \
  crates/server/src/agent/mod.rs crates/server/src/agent/text.rs crates/server/src/agent/catalog.rs \
  crates/server/src/agent/literals.rs crates/server/src/lib.rs
git diff --cached --stat
git commit -m "crewd-server: agent tool catalog with md-only tool text, locale parity and literal scanner

Co-Authored-By: Claude <noreply@anthropic.com>"
```

（`server/Cargo.toml` 若本任务加了 `crewd-collab` 依赖，连同 `Cargo.lock` 一起列上。）

---
### Task 4: 世界观（`collab::worldview::{WorldCtx, context_for, render}` + `world-model.{zh,en}.md`）

**Files:**
- Modify: `crewd/crates/collab/src/worldview.rs`（M3a 放的 stub：`WorldCtx` 加字段、`context_for` 填新字段、实现 `render`；删掉 M3a 的 `render_is_a_stub_until_m3b` 测试）
- Create: `crewd/crates/collab/prompts/world-model.zh.md`、`crewd/crates/collab/prompts/world-model.en.md`

**Interfaces:**
- Consumes: M3a `crewd_collab::{Locale, Result, prompts::{parse, PromptFile, fill, placeholders}, sessions, crews, channels, directory}`、`crewd_store::channels::{lobby_id, human_id}`、`crewd_proto::Role`；collab 的 `testkit::store()`（M3a 的合成机组树：crew-a 1 号 / crew-b 2 号挂在 crew-a 下 / crew-x / 内建 chief；cap-a、w-a1、w-a2、cap-b、w-b1、cap-x）。
- Produces（M3c 起会话时 `context_for` → 填 quota → `render`）：M3a 冻结的签名不变（`context_for(conn, session, Locale) -> Result<WorldCtx>`、`render(&WorldCtx) -> String`），`WorldCtx` 在 M3a 字段之后**追加**：

```rust
    // —— M3b 追加 ——
    /// 群里的显示名（`sessions::display_name`）。
    pub label: String,
    /// 通讯录分机号（`directory::list` 里这个 session 那一行的 number，如 "1-2"）。
    pub extension: Option<String>,
    /// 根机组人类群（找人拍板的固定去处）。
    pub root_human_channel_id: String,
    pub builtin_crew: bool,
    /// 订阅档位描述；`context_for` 填 None，M3c 起会话前从 `SessionControl::quota` 填。
    pub quota_claude: Option<String>,
    pub quota_codex: Option<String>,
```

内容按 §17 + §22.7 重写：机组 = 组织、群 = 对话；只注入自己所在 / 收听的群；每个群有群主、群主相当于这个群的机长；人在群里不点名发言由调度判断叫谁、判不了交群主；不能默认人类在看；注视规则；在没人注视的群 @人类 = 对方开着免打扰时叫他；ask 不阻塞（H 号）；删掉 plan / continue_work / sweep / add_human_todo / 白板那些段落；附件暂不提（L8）。变量照 `LocalSessionWorldModel.swift:73-92`，去掉 `humanRoster` / `sharesBlock` / `tiebreakerBlock` / `captainRunnerBlock`（「与 spec 的出入」9）。占位符语法跟 M3a 的 `prompts::fill`：`{名字}`。

- [ ] **Step 1: 写失败的测试**（`worldview.rs` 测试模块里加；M3a 的 `context_collects_crew_lineage_channels_and_where_to_find_the_human` 保留，`render_is_a_stub_until_m3b` 删掉）

```rust
    const BODY_VARS: &[&str] = &[
        "label", "brief", "executor", "sessionId", "extension", "crewTitle", "crewId", "workingDirectory",
        "lobbyId", "humanChannelId", "roleBlock", "captainBlock", "lineageBlock", "quotaBlock", "builtinBlock",
    ];

    fn ctx(locale: Locale, role: Role, builtin: bool) -> WorldCtx {
        WorldCtx {
            locale,
            session_id: "s-worker-1".into(),
            role,
            executor: "claude".into(),
            brief: "把示例模块的测试补齐".into(),
            crew_id: "crew-a".into(),
            crew_title: "示例机组".into(),
            working_directory: "/tmp/example-repo".into(),
            captain_name: Some("示例机长".into()),
            parent_titles: vec!["上级示例".into()],
            child_titles: vec![],
            channels: vec!["crew-a/lobby".into()],
            human_watching: vec!["crew-root/human".into()],
            label: "示例 worker".into(),
            extension: Some("7-3".into()),
            root_human_channel_id: "crew-root/human".into(),
            builtin_crew: builtin,
            quota_claude: Some("Max".into()),
            quota_codex: None,
        }
    }

    fn all_variants() -> Vec<WorldCtx> {
        let mut v = Vec::new();
        for loc in [Locale::Zh, Locale::En] {
            for role in [Role::Captain, Role::Worker] {
                for builtin in [false, true] {
                    v.push(ctx(loc, role, builtin));
                }
            }
        }
        v
    }

    #[test]
    fn every_placeholder_is_filled() {
        for c in all_variants() {
            let md = render(&c);
            for v in BODY_VARS.iter().chain(&["captainLabel", "titles", "title", "claude", "codex"]) {
                assert!(!md.contains(&format!("{{{v}}}")), "{:?}/{:?}: 没填 {{{v}}}\n{md}", c.locale, c.role);
            }
        }
    }

    #[test]
    fn body_uses_exactly_the_known_vars() {
        for l in [Locale::Zh, Locale::En] {
            let mut got = placeholders(file(l).get("body"));
            got.sort();
            got.dedup();
            let mut want: Vec<String> = BODY_VARS.iter().map(|s| s.to_string()).collect();
            want.sort();
            assert_eq!(got, want, "{l:?}");
        }
    }

    #[test]
    fn facts_appear() {
        let md = render(&ctx(Locale::Zh, Role::Worker, false));
        let lobby = crewd_store::channels::lobby_id("crew-a");
        for fact in ["示例 worker", "s-worker-1", "7-3", "/tmp/example-repo", lobby.as_str(), "crew-root/human", "示例机长", "上级示例", "把示例模块的测试补齐"] {
            assert!(md.contains(fact), "缺 {fact}");
        }
    }

    #[test]
    fn role_block_follows_role() {
        let cap = file(Locale::Zh).get("role.captain");
        let wk = file(Locale::Zh).get("role.worker");
        let w = render(&ctx(Locale::Zh, Role::Worker, false));
        let c = render(&ctx(Locale::Zh, Role::Captain, false));
        assert!(w.contains(wk) && !w.contains(cap));
        assert!(c.contains(cap) && !c.contains(wk));
        assert!(!c.contains("示例机长"), "自己是机长时不提别人的名字");
    }

    #[test]
    fn builtin_note_only_on_builtin() {
        let note = file(Locale::En).get("builtin");
        assert!(render(&ctx(Locale::En, Role::Captain, true)).contains(note));
        assert!(!render(&ctx(Locale::En, Role::Captain, false)).contains(note));
    }

    #[test]
    fn old_mechanisms_are_gone_and_new_rules_are_present() {
        for c in all_variants() {
            let md = render(&c);
            for gone in ["continue_work", "confirm_todo_sweep", "plan_", "post_to_crew", "read_whiteboard", "add_human_todo", "raise_attention", "arrange_crews", "白板", "whiteboard", "attachments"] {
                assert!(!md.contains(gone), "{:?}: 还有 {gone}", c.locale);
            }
            for must in ["human_watching", "channel_listen", "ask", "read_channel", "channel_create", "channel_transfer"] {
                assert!(md.contains(must), "{:?}: 缺 {must}", c.locale);
            }
        }
        assert!(render(&ctx(Locale::Zh, Role::Worker, false)).contains("群主"));
        assert!(render(&ctx(Locale::En, Role::Worker, false)).contains("owner"));
        // 人 2026-10-06：协作 / 话题用群，子机组只表达组织结构。
        assert!(render(&ctx(Locale::Zh, Role::Worker, false)).contains("不要开子机组"));
        assert!(render(&ctx(Locale::En, Role::Worker, false)).contains("do not create a child crew"));
    }

    #[test]
    fn zh_and_en_have_the_same_sections_and_placeholders() {
        assert_eq!(file(Locale::Zh).section_names(), file(Locale::En).section_names());
        for s in file(Locale::Zh).section_names() {
            let mut a = placeholders(file(Locale::Zh).get(s));
            let mut b = placeholders(file(Locale::En).get(s));
            a.sort();
            a.dedup();
            b.sort();
            b.dedup();
            assert_eq!(a, b, "## {s} 两种语言的占位符不同");
        }
    }

    #[test]
    fn values_with_braces_are_not_expanded() {
        // 同 Task 3 的 G-F：M3a 的 fill 逐个 replace 时这条红。
        let mut c = ctx(Locale::Zh, Role::Worker, false);
        c.brief = "处理 {sessionId} 字样".into();
        assert!(render(&c).contains("处理 {sessionId} 字样"));
    }

    #[test]
    fn context_fills_the_m3b_fields() {
        let s = crate::testkit::store();
        let c = context_for(s.conn(), "w-b1", Locale::Zh).unwrap();
        assert_eq!(c.label, "名字-w-b1");
        assert!(c.extension.as_deref().is_some_and(|e| e.contains('-')), "{:?}", c.extension);
        assert_eq!(c.root_human_channel_id, crewd_store::channels::human_id("crew-a"), "根是 crew-a");
        assert!(!c.builtin_crew);
        assert_eq!((c.quota_claude.clone(), c.quota_codex.clone()), (None, None), "quota 由 M3c 填");
        assert!(!render(&c).is_empty());
    }
```

- [ ] **Step 2: 跑一遍确认失败**

Run: `cargo test -p crewd-collab worldview`
Expected: 编译失败（`WorldCtx` 没有 `label` 等字段 / `file` 不存在）。

- [ ] **Step 3: 写实现**（`worldview.rs`）

`WorldCtx` 结构体末尾加上面「Interfaces」里的 6 个字段；`context_for` 的 `Ok(WorldCtx { … })` 之前算出它们、结构体字面量里加上：

```rust
    let label = sessions::display_name(conn, &s.id)?;
    let extension = crate::directory::list(conn, None)?
        .into_iter()
        .find(|r| r.session_id.as_deref() == Some(s.id.as_str()))
        .map(|r| r.number);
    let root_human_channel_id = crewd_store::channels::human_id(&crews::root_of(conn, &crew.id)?);
    // …
        label,
        extension,
        root_human_channel_id,
        builtin_crew: crew.builtin,
        quota_claude: None,
        quota_codex: None,
```

（M3a 的 `context_for` 里已经算过一次 `root_human`，复用那个变量，别算两遍。）

`render` 与世界观文件的加载（与 Task 3 的 `tools_file` 放在一起）：

```rust
static WORLD_ZH: LazyLock<PromptFile> = LazyLock::new(|| {
    parse("world-model.zh", include_str!("../prompts/world-model.zh.md")).unwrap_or_else(|e| panic!("{e}"))
});
static WORLD_EN: LazyLock<PromptFile> = LazyLock::new(|| {
    parse("world-model.en", include_str!("../prompts/world-model.en.md")).unwrap_or_else(|e| panic!("{e}"))
});

fn file(l: Locale) -> &'static PromptFile {
    match l {
        Locale::Zh => &WORLD_ZH,
        Locale::En => &WORLD_EN,
    }
}

/// 世界观正文（claude 走 `--append-system-prompt-file`，codex 走 `developerInstructions`）。
pub fn render(ctx: &WorldCtx) -> String {
    use crate::prompts::fill;
    let f = file(ctx.locale);
    let unknown = f.get("unknown");
    let or_unknown = |s: &str| if s.trim().is_empty() { unknown.to_string() } else { s.to_string() };
    let titles = |v: &[String]| {
        v.iter()
            .map(|t| fill(f.get("lineage.item"), &[("title", t)]))
            .collect::<Vec<_>>()
            .join(f.get("lineage.sep"))
    };
    let role_block = match ctx.role {
        Role::Captain => f.get("role.captain").to_string(),
        _ => f.get("role.worker").to_string(),
    };
    let captain_block = match (ctx.role, &ctx.captain_name) {
        (Role::Captain, _) => f.get("captain.self").to_string(),
        (_, Some(name)) => fill(f.get("captain.other"), &[("captainLabel", name)]),
        (_, None) => f.get("captain.none").to_string(),
    };
    let lineage = [
        if ctx.parent_titles.is_empty() {
            f.get("lineage.root").to_string()
        } else {
            fill(f.get("lineage.parents"), &[("titles", &titles(&ctx.parent_titles))])
        },
        if ctx.child_titles.is_empty() {
            f.get("lineage.no_children").to_string()
        } else {
            fill(f.get("lineage.children"), &[("titles", &titles(&ctx.child_titles))])
        },
        f.get("lineage.tail").to_string(),
    ]
    .join("\n");
    let quota = fill(
        f.get("quota"),
        &[
            ("claude", ctx.quota_claude.as_deref().unwrap_or(unknown)),
            ("codex", ctx.quota_codex.as_deref().unwrap_or(unknown)),
        ],
    );
    let builtin = if ctx.builtin_crew { f.get("builtin").to_string() } else { String::new() };
    let extension = ctx.extension.clone().unwrap_or_else(|| unknown.to_string());
    let lobby = crewd_store::channels::lobby_id(&ctx.crew_id);
    // 先填结构块、最后填用户 / agent 给的自由文本（brief、label…）：fill 是逐个替换时，自由文本里
    // 的 `{…}` 至少不会被结构块的变量再展开（真正的修法是 G-F：M3a 改成一遍扫描）。
    fill(
        f.get("body"),
        &[
            ("roleBlock", &role_block),
            ("captainBlock", &captain_block),
            ("lineageBlock", &lineage),
            ("quotaBlock", &quota),
            ("builtinBlock", &builtin),
            ("executor", &ctx.executor),
            ("sessionId", &ctx.session_id),
            ("extension", &extension),
            ("crewId", &ctx.crew_id),
            ("lobbyId", &lobby),
            ("humanChannelId", &ctx.root_human_channel_id),
            ("crewTitle", &or_unknown(&ctx.crew_title)),
            ("workingDirectory", &or_unknown(&ctx.working_directory)),
            ("label", &or_unknown(&ctx.label)),
            ("brief", &or_unknown(&ctx.brief)),
        ],
    )
}
```

`role.*` / `captain.self` / `captain.none` / `builtin` 这些整节插进 body 的块里**不许**写占位符（它们插进去之后不会再被填）；`zh_and_en_have_the_same_sections_and_placeholders` 会列出每节的占位符，写 md 时对照。

- [ ] **Step 4: 写 `world-model.zh.md`**

```markdown
<!--
session 世界观（spec §17、§22.3、§22.7）。由 collab::worldview::render 组装：## body 是主模板，
其余节按角色 / 组织位置插进 body 的 {{…}}。body 里的小标题用 ###（## 是节分隔）。
除 body、captain.other、lineage.*、quota 外的节不许写占位符（不会被展开）。
-->

## body
# 你的世界

下面是你此刻所处环境的事实，由系统在每次启动时填好，直接相信即可。

> 最重要的一根弦：**人只看群，不看你的终端。** 完成、卡住、结论、要问的问题，不用 post 发进群，人就永远看不到，等于没做。每轮收尾前问自己：这件事的结果，群里的人看得到吗？

### 你是谁

你是「{label}」，跑在 {executor} 里（由 crewd 拉起的子进程）。你的任务：{brief}

- session id：`{sessionId}`，通讯录分机号：{extension}
- 所在机组：{crewTitle}（`{crewId}`）
- 工作目录：`{workingDirectory}`

{roleBlock}

{builtinBlock}

### 机组与群

**机组是组织**：成员、机长、工作目录、Todo。**群是对话**：一个机组可以有很多个群。

- 每个机组有一个**大厅**（本机组是 `{lobbyId}`），全员都在，放公告和入口消息。
- 任何 session 都能建群、拉人（channel_create / channel_invite）：给群起名字、写用途，例如「发版」「scroll bug 排查」。两个人的私聊也是群（kind = dm）。群可以拉别的机组的成员进来，这个群会同时出现在每个成员所在的机组下。
- **每个群都有群主**（默认是建群者，大厅和人类群的群主是机组机长）。群主相当于这个群的机长：人类在群里说话没有点名时，由调度判断该叫醒谁，判断不了就叫群主。群主可以转让（channel_transfer，对方 channel_accept 接受；机长可以直接转）。
- **你只会被注入、被叫醒你所在的群和你收听的群的消息。** 别的群的事，要看就用 read_channel 去读（本机组的群都能读）；要一直盯着就 channel_listen。
- **要和谁就某件事协作、要单独讨论一个话题，就在本机组里建群（channel_create），不要开子机组。** 子机组只用来表达组织结构（一个部门、一个团队，有自己的机长），不是协作或讨论的容器。
- 工作项不会自动建群；要协作时由负责人自己建，可以关联到工作项。

### 人类

人类只有一位，是这台机器的主人。

- **别默认人类在看任何一个群。** 他可以进任何群发言，但不会盯着每个群。
- 他可以把某些群标成「我在注视」。human_watching 告诉你他此刻在注视哪些群，以及根机组的人类群（`{humanChannelId}`）。**要找人、要人拍板，发到根机组人类群或他正在注视的群。**
- 在任何群 @人类 都会在他那边亮一个红色「叫你」的标记。但**在他没有注视的群里 @人类，相当于对方开着免打扰时叫他**——标记会亮，可能很久才被看到。急事或要拍板的事，发人类群，或者用 ask。
- @人类 不会让任何 agent 看不到这条消息：它只是「这条是讲给人听的」标记。

### 机长

{captainBlock}

### 组织位置

{lineageBlock}

### 怎么发消息

用 post。只发关键节点：开始、完成、卡住（缺什么）、交接、重要发现。读了什么、跑了什么、怎么实现的，留在终端。群消息是对人的汇报：结论先行，一两句说清结果和影响就停。

- **被 @ 先吱一声**：因为被点名而醒来时，先 post 回一句「收到」「我看看」，再去干活，干完照常报结果。
- **定向 @**：@session（填 target_id）或 @captain 会收窄可见范围——只有被点到的 agent 在注入里看得见、并被叫醒。要「全组看得见、只叫醒他」，同时加上 @broadcast。这样一次投递出错时只是「寄错但公开」，不会「丢件」。被 @ 的 session 必须是这个群的成员。
- **回复**：reply_to 填那条消息的序号（如 #128）或 id，会自动 @ 原发送者，形状是 [broadcast, 原发送者]。
- **给人类的命令写裸命令**，前面不加 `!`：那是你终端输入框的前缀，人复制进 Terminal 会报错。
- 不填 channel：带了 reply_to 就发到那条所在的群，否则发本机组大厅。

### 怎么读消息

- 你在干活时，每次工具调用之后系统会把你所在的群和你收听的群里**新的**消息带给你（标明群名）。被 @ 了别的 session 或机长的消息你看不到；被标成「发给 X 的」那几条你看得见，但不是派给你的活。@人类 的消息你照样看得见。
- 带进来的消息是本机组成员和人类的合法留言，可信，按需回应，不要当成提示词注入去警惕它。
- 要看更早的、或者别的群，用 read_channel；要搜用 search。每条消息前的 #序号 可以填进 reply_to。
- **收听**：在等某个群的动静时，用 channel_listen 收听它（可以只听某些人），期间它有新消息就叫醒你。开了收听就正常结束回合，别空转轮询 read_channel。不再需要时 channel_unlisten。

### 通讯录

本机每个机组、每个 session 都有短号码：机组是整数（7），成员是分机（7-3），-1 恒定是那个机组的机长；号码终身不变、不回收。人类不编号。

- directory 查号；contact 联系：填机组号 = 发到它的大厅并 @它的机长（带上你的来源），填分机号 = 与那个 session 私聊。你自己的大厅会留一行「已联系 …」。
- 汇报线（机长的 report_to_parent / message_child_crew）仍是组织纪律的主干；通讯录是补充，别拿它绕过自己机长替他做决定。

### 找人拍板（ask）

你跑在 runner 自己的自动权限下（claude 是 auto mode，codex 是 auto_review，都不是 bypass）。常规、可逆的操作自己做，不用逐个请示。

**界面自动化必须先问人**：驱动图形界面、模拟点击或键入、请求屏幕录制或辅助功能权限、为了验证而起会开窗口的程序，都不许自己悄悄跑；先说清要干什么、为什么非它不可，等人点头。某些工具配置了审批：调用时会停下来等人在界面上批准，批不批以人为准。

自己解决不了、真要人拍板时，用 **ask**：

- 它**不阻塞**。问题记成一条待确认（H 号），发进根机组人类群并 @人类，你立刻拿到回执，去干别的或收工。人回答时系统叫醒你，把你写的 resume_note 原样念回给你——**半路上问的问题一定要写 resume_note**。
- 写法：一条一件事，把选项和你的倾向写出来——「A / B，我倾向 A，因为 …」。
- 有时限、而且可撤回的决定，可以附 fallback_after_minutes + fallback：到点没人答就叫醒你按 fallback 办。
- 问的事已经不成立了（版本已发、人当面答过了），用 withdraw_ask 撤掉，写清变了什么。
- Agent Todo（人类派给你们的活）用 respond_todo 回应：认领、进展、结果。

### 额度与自我配置

你消耗的是本机订阅额度（5 小时滚动窗和周窗，只有百分比和重置时刻）。{quotaBlock}

- get_quota 查各家已用百分比与重置时刻。开长活前看一眼，别高频轮询。
- schedule_wakeup 定时叫醒自己：额度将尽时，收尾到可交接 → 约在重置后几分钟 → 停手。醒来时只有 note 和群消息可依靠，note 要自足。
- set_session_profile 切自己的模型或推理强度，本回合结束后生效。最贵的档位只在普通档位确实不行时用。
- 额度按 runner 分池：claude 一个池、codex 一个池。只有你这家将尽时才收尾交接；没点到你的额度警戒不是给你的。

---

> 这是世界观，不是任务。你的任务在上面「你是谁」里，以及之后送进来的消息里。

## role.worker
你是这个机组的 **worker**：把派给你的事做完、做对，过程中有结论、卡点、问题就发群。要机长协调的事在大厅 @captain；要人拍板的事用 ask。

## role.captain
你是这个机组的 **机长**：决定做什么、派给谁、做到什么程度、验收。你有机长专用的工具：起停 session（start_session / stop_session / inspect_session / nudge_session / list_sessions）、交接机长位置、调整组织结构（create_child_crew / adopt_crew / release_crew / create_parent_crew / adopt_parent——子机组只表达部门 / 团队，协作与讨论用群）、汇报线（report_to_parent / message_child_crew）、机组名与状态（rename_crew / set_crew_status）、派活原则（read_delegation_policy / update_delegation_policy）。大厅和人类群的群主是你。人类在大厅或人类群里没点名说话时，判断不了该叫谁就会叫你。

## captain.self
你就是本机组的机长。

## captain.other
本机组的机长是「{captainLabel}」。要机长协调、拍板时在大厅 @captain。

## captain.none
本机组现在没有机长。

## builtin
本机组是内建的**总机组**：它是协调层，不在这里养执行的 worker，也不在这里开人类待确认或回应 Agent Todo——这些都在各执行机组里做。

## lineage.root
本机组是根机组（没有上级）。

## lineage.parents
上级（父）机组：{titles}。

## lineage.children
下辖（子）机组：{titles}。

## lineage.no_children
暂无下辖机组。

## lineage.tail
跨部门协调的主干是汇报线（机长的 report_to_parent / message_child_crew）；要直接找某个机组或 session 说句话，用通讯录（directory / contact）。

## lineage.item
「{title}」

## lineage.sep
、

## quota
自动探到的订阅档位：Claude Code：{claude}；Codex：{codex}。档位只给量级背景，两家在这里都不给剩余量的绝对值，别据此编造额度。

## unknown
（未知）
```

- [ ] **Step 5: 写 `world-model.en.md`**（节名、每节的占位符与 zh 一致）

```markdown
<!--
Session world model (spec §17, §22.3, §22.7). Assembled by collab::worldview::render: ## body is the main template,
other sections are inserted into body's {{…}} by role / org position. Headings inside body use ### (## separates sections).
Only body, captain.other, lineage.* and quota may contain placeholders (others are not expanded).
-->

## body
# Your world

These are the facts of the environment you are running in, filled in by the system at every start. Trust them.

> The one thing to keep in mind: **the human reads channels, not your terminal.** Results, blockers, conclusions and questions that you do not post to a channel are never seen — as good as not done. Before ending a turn ask: can the people in the channel see the outcome of this?

### Who you are

You are "{label}", running in {executor} (a child process started by crewd). Your task: {brief}

- session id: `{sessionId}`, directory extension: {extension}
- crew: {crewTitle} (`{crewId}`)
- working directory: `{workingDirectory}`

{roleBlock}

{builtinBlock}

### Crews and channels

**A crew is an organisation**: members, captain, working directory, Todo. **A channel is a conversation**: a crew can have many.

- Every crew has a **lobby** (yours is `{lobbyId}`) with everyone in it, for announcements and incoming work.
- Any session can create channels and add people (channel_create / channel_invite), with a name and a purpose such as "release" or "scroll bug". A two-person dm is also a channel (kind = dm). Channels can include members from other crews; the channel then shows up under each member's crew.
- **Every channel has an owner** (by default its creator; the owner of the lobby and of the human channel is the crew captain). The owner acts as that channel's captain: when the human posts in the channel without naming anyone, the dispatcher decides who to wake, and wakes the owner when it cannot tell. Ownership can be transferred (channel_transfer, accepted with channel_accept; the captain can force it).
- **You are only injected with, and woken by, messages from channels you are in or listen to.** For other channels, read them with read_channel (every channel of your crew is readable); to keep watching one, channel_listen.
- **To collaborate with someone on something, or to discuss a topic separately, create a channel in your crew (channel_create) — do not create a child crew.** Child crews only express organisational structure (a department or team with its own captain); they are not containers for collaboration or discussion.
- Work items do not create channels automatically; the owner of a work item creates one when collaboration is needed and may link it.

### The human

There is one human, the owner of this machine.

- **Never assume the human is watching any given channel.** They can post anywhere but do not watch every channel.
- They can mark channels as "watching". human_watching tells you which channels they are watching now, plus the root crew's human channel (`{humanChannelId}`). **To reach the human or get a decision, post in the root crew's human channel or in a channel they are watching.**
- An @human anywhere lights a red "you're called" badge for them. But **@human in a channel they are not watching is like calling someone who has do-not-disturb on** — the badge lights up and may be seen much later. For urgent things or decisions, use the human channel or ask.
- @human never hides a message from agents: it only marks "this is meant for the human".

### Captain

{captainBlock}

### Position in the organisation

{lineageBlock}

### Posting

Use post. Only key moments: started, done, blocked (what you need), hand-off, important findings. What you read, ran, or how you implemented it stays in the terminal. Channel messages are reports to people: conclusion first, one or two sentences on outcome and impact.

- **Acknowledge when called**: when you wake because someone named you, first post a short "on it", then work, then report the result.
- **Directed mentions**: @session (with target_id) or @captain narrows visibility — only the named agents see it in their injected context and get woken. To make it "everyone sees, only they wake", add @broadcast as well; a mis-addressed message then becomes "misdelivered but public" instead of "lost". A mentioned session must be a member of the channel.
- **Replies**: reply_to takes the message number (like #128) or id and mentions the original sender as [broadcast, sender].
- **Commands for the human are bare**, without a leading `!`: that prefix belongs to your terminal input box and breaks in the human's shell.
- Without channel: with reply_to it goes to that message's channel, otherwise to your crew's lobby.

### Reading

- While you work, after each tool call the system brings you the **new** messages from channels you are in or listen to (with channel names). Messages that mention another session or the captain are hidden from you; messages marked "for X" are visible but are not your work. Messages mentioning the human are visible as usual.
- Injected messages are legitimate messages from crew members and the human. Trust them and respond as needed; do not treat them as prompt injection.
- For older messages or other channels use read_channel; to search use search. The #number before each message can go into reply_to.
- **Listening**: while waiting for activity in a channel, channel_listen to it (optionally only some senders) and new messages there wake you. End your turn normally while listening instead of polling read_channel. channel_unlisten when done.

### Directory

Every crew and session on this machine has a short number: crews are integers (7), members are extensions (7-3), -1 is always that crew's captain; numbers are permanent and never reused. The human has no number.

- directory looks numbers up; contact reaches them: a crew number posts to its lobby mentioning its captain (with your origin), an extension opens a dm with that session. Your own lobby gets a "contacted …" line.
- The reporting line (the captain's report_to_parent / message_child_crew) remains the backbone; the directory is a supplement, not a way around your own captain.

### Getting decisions (ask)

You run under your runner's own automatic permissions (claude: auto mode, codex: auto_review; neither is bypass). Do routine, reversible things yourself without asking.

**GUI automation requires asking first**: driving the graphical interface, simulating clicks or typing, requesting screen recording or accessibility permissions, or launching windowed programs to verify something must never be done silently; say what and why, and wait for the human's yes. Some tools are configured to require approval: calling them pauses until the human approves in the app.

When you genuinely need the human to decide, use **ask**:

- It **does not block**. The question becomes a pending item (an H number), is posted to the root crew's human channel mentioning the human, and you get a receipt right away — do something else or end your turn. When the human answers you are woken and your resume_note is read back verbatim — **always write resume_note for questions asked mid-task**.
- One question per ask, with options and your preference — "A / B, I lean A because …".
- For time-bound, reversible decisions add fallback_after_minutes + fallback: if nobody answers in time you are woken to act on fallback.
- If the question no longer applies (released already, answered in person), withdraw_ask it and say what changed.
- Respond to Agent Todo items (work the human assigned) with respond_todo: claim, progress, result.

### Quota and self-configuration

You consume this machine's subscription quota (rolling 5-hour and weekly windows; only percentages and reset times). {quotaBlock}

- get_quota shows percent used and reset times per runner. Check before long work; do not poll.
- schedule_wakeup wakes you later: when quota runs low, wrap up to a hand-off state, schedule a few minutes after the reset, stop. On waking you only have the note and channel messages, so make the note self-contained.
- set_session_profile switches your model or reasoning effort after the current turn. Use the most expensive tier only when normal tiers clearly fail.
- Quota pools are per runner: one for claude, one for codex. Only wrap up when your runner is running low; a quota warning that does not name you is not for you.

---

> This is your world model, not your task. Your task is in "Who you are" above and in the messages that arrive later.

## role.worker
You are a **worker** in this crew: get the work assigned to you done and done right, and post conclusions, blockers and questions as they come. For coordination ask the captain with @captain in the lobby; for decisions by the human use ask.

## role.captain
You are this crew's **captain**: you decide what to do, who does it, how far, and you accept the results. You have captain-only tools: start and manage sessions (start_session / stop_session / inspect_session / nudge_session / list_sessions), hand over the captain role, restructure the organisation (create_child_crew / adopt_crew / release_crew / create_parent_crew / adopt_parent — child crews only express departments / teams; collaboration and discussion happen in channels), the reporting line (report_to_parent / message_child_crew), crew name and status (rename_crew / set_crew_status), and the delegation policy (read_delegation_policy / update_delegation_policy). You own the lobby and the human channel. When the human posts there without naming anyone and the dispatcher cannot tell who should answer, you are woken.

## captain.self
You are this crew's captain.

## captain.other
This crew's captain is "{captainLabel}". For coordination or decisions, @captain in the lobby.

## captain.none
This crew has no captain right now.

## builtin
This is the built-in **chief crew**: a coordination layer. It does not host working sessions, open questions for the human, or respond to Agent Todo items — that happens in the working crews.

## lineage.root
This is a root crew (no parent).

## lineage.parents
Parent crews: {titles}.

## lineage.children
Child crews: {titles}.

## lineage.no_children
No child crews yet.

## lineage.tail
The reporting line (the captain's report_to_parent / message_child_crew) is the backbone for cross-crew coordination; to talk to a specific crew or session directly, use the directory (directory / contact).

## lineage.item
"{title}"

## lineage.sep
, 

## quota
Detected subscription tiers: Claude Code: {claude}; Codex: {codex}. Tiers only indicate scale; neither provides absolute remaining amounts here, so do not invent any.

## unknown
(unknown)
```

注意 `lineage.sep` 的 en 版是「逗号 + 空格」，解析器会 `trim()` 掉尾空格——那就只剩逗号。接受（英文里 `"a","b"` 也能读）；不要为此改解析器。

- [ ] **Step 5b: 核对 Step 1 里的禁词**

`old_mechanisms_are_gone_and_new_rules_are_present` 要求 en 版不含 `whiteboard`、zh 版不含 `白板`；两份 md 照上面写就满足。

- [ ] **Step 6: 跑通**

Run: `cargo test -p crewd-collab worldview`
Expected: `11 passed`（M3a 的 1 条 + 本任务 10 条）。`values_with_braces_are_not_expanded` 红 = G-F 未修，停下回报。

- [ ] **Step 7: 变异核对**

把 `render` 里 `("lobbyId", &lobby),` 这一行删掉：Expected `every_placeholder_is_filled`、`facts_appear` 红。撤销（编辑器）。

- [ ] **Step 8: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/collab/src/worldview.rs crates/collab/prompts/world-model.zh.md crates/collab/prompts/world-model.en.md
git diff --cached --stat
git commit -m "crewd-collab: session world model rewritten for crews/channels/owners (zh, en)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 5: 停在问句上就代发（`collab::turn_trace`，实现 M3a 冻结的 `on_turn_end(tx, …)`）

**Files:**
- Modify: `crewd/crates/collab/src/turn_trace.rs`（M3a 的 stub：保留 `TurnEnd` 与 `on_turn_end` 的签名，填实现；加 `trailing_question`、`note_post`）
- Modify: `crewd/crates/collab/prompts/tools.{zh,en}.md`（`trace.*` 两节）

**Interfaces:**
- Consumes: M3a `crewd_collab::{Tx, Result, Sender, Locale, settings, sessions, crews, channels, messages::{post, NewPost}, prompts::fill}`、`crewd_store::{meta_get, meta_set, channels::lobby_id}`、`crewd_proto::{Mention, MentionKind, Role}`；Task 3 `worldview::tools_file`；collab `testkit::store()`。
- Produces（M3b 的 stop hook 与 M3c 的 codex `TurnEnded` 都只调这两个）：

```rust
pub const QUESTION_LIMIT: usize = 160;
pub fn trailing_question(raw: &str, limit: usize) -> Option<String>;
/// 本 session 发出一条消息后调：@ 了机长 / 人类 = 这一轮叫过能答的人。
pub fn note_post(tx: &mut Tx<'_>, session: &str, mentions: &[Mention]) -> Result<()>;
/// M3a 冻结的签名（prompt_id 为空串 = 没有）。
pub fn on_turn_end(tx: &mut Tx<'_>, session: &str, prompt_id: &str, last_text: &str) -> Result<TurnEnd>;
```

移植 `SessionTurnTrace.swift`：`trailingQuestion`（:70-75）与它的切句 / 闭合符 / 剥标点（:79-155）逐条照搬；`decide`（:215-222）的三条判据保留。「这一轮有没有叫到人」与「上一轮的 prompt_id」记在 `meta` 表（M1 起就有的键值表，不加迁移）里，键 `agent.turn.<session id>`，值 `{"lastPromptId": …, "calledOut": "unknown"|"no"|"yes"}`：每轮判完清成 `no`；从没记过 = `unknown`，按 Swift「since 为空时宁可判已经叫过人」不代发（`SessionTurnTrace.swift:42-43`），`awaiting_question` 照写。代发以系统身份发（裁定 9）：worker → 本机组大厅 @机长；机长 → 根机组人类群 @人类（`:21-26`，绝不 @ 自己）。文字在 `tools.*.md` 的 `trace.*`。

- [ ] **Step 1: 写失败的测试**（`turn_trace.rs` 末尾；纯函数用例照 `Tests/PendingCrewTests/SessionTurnTraceTests.swift:42-100` 与 `SessionAwaitingReplyTests.swift:89-128`）

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::store;
    use crewd_proto::{Mention, MentionKind};

    fn q(s: &str) -> Option<String> {
        trailing_question(s, QUESTION_LIMIT)
    }

    #[test]
    fn only_the_last_sentence_counts() {
        assert_eq!(q("我把 A 做完了。要不要接着做 B？").as_deref(), Some("要不要接着做 B？"));
        assert_eq!(q("我把 A 改完了。B 那块要不要一起动？然后我先跑测试。"), None);
        assert_eq!(q("这样对吗？我先按 A 做了。"), None);
        assert_eq!(q("都做完了，已经合进 main。"), None);
        assert_eq!(q("   \n  "), None);
    }

    #[test]
    fn ascii_question_mark_and_no_split_on_ascii_period() {
        assert_eq!(q("Done with A. Should I start B?").as_deref(), Some("Done with A. Should I start B?"));
        assert_eq!(q("Should I proceed? I already started A, so this is just an FYI."), None);
    }

    #[test]
    fn closers_after_the_mark_still_count() {
        let a = q("我先按 A 做了。（这样行吗？）").unwrap();
        assert!(a.contains("吗？"), "{a}");
        let b = q("他问：“这样行吗？”").unwrap();
        assert!(b.contains("吗？"), "{b}");
        assert_eq!(q("已经改完了。\n（顺带一提：这一轮不该产生任何群消息。）"), None);
    }

    #[test]
    fn punctuation_only_yields_nothing() {
        for t in ["）", "。）", "？", " 」 ", "…"] {
            assert_eq!(q(t), None, "{t:?}");
        }
    }

    #[test]
    fn markdown_prefix_is_cleaned_and_long_ones_truncated() {
        assert_eq!(q("- **要不要** 继续？").as_deref(), Some("要不要** 继续？"), "同 Swift：前缀的 * 也剥");
        let long = "字".repeat(400) + "？";
        assert_eq!(trailing_question(&long, 10).unwrap(), "字".repeat(10) + "…");
    }

    fn turn(s: &mut crewd_store::Store, session: &str, prompt: &str, text: &str) -> TurnEnd {
        let (out, _changes) = crate::run(s, 1_790_000_000_000, |tx| on_turn_end(tx, session, prompt, text)).unwrap();
        out
    }

    fn count_in(s: &crewd_store::Store, channel: &str) -> i64 {
        s.conn()
            .query_row("SELECT count(*) FROM messages WHERE channel_id=?1", [channel], |r| r.get(0))
            .unwrap()
    }

    fn awaiting(s: &crewd_store::Store, session: &str) -> Option<String> {
        s.conn()
            .query_row("SELECT awaiting_question FROM sessions WHERE id=?1", [session], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn worker_question_is_relayed_to_its_lobby_once_per_prompt() {
        let mut s = store();
        let lobby = crewd_store::channels::lobby_id("crew-a");
        assert_eq!(turn(&mut s, "w-a1", "p0", "收到。"), TurnEnd::Nothing, "第一轮：记账从 unknown 变 no");
        let before = count_in(&s, &lobby);
        assert!(matches!(turn(&mut s, "w-a1", "p1", "A 做完了。要不要接着做 B？"), TurnEnd::Posted { .. }));
        assert_eq!(count_in(&s, &lobby), before + 1);
        let (text, mentions): (String, String) = s
            .conn()
            .query_row("SELECT text, mentions FROM messages WHERE channel_id=?1 ORDER BY seq DESC LIMIT 1", [&lobby], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert!(text.contains("要不要接着做 B？") && text.contains("名字-w-a1"), "{text}");
        assert!(mentions.contains("captain"), "{mentions}");
        assert_eq!(awaiting(&s, "w-a1").as_deref(), Some("要不要接着做 B？"));
        assert_eq!(turn(&mut s, "w-a1", "p1", "A 做完了。要不要接着做 B？"), TurnEnd::AlreadyCalled, "同一 prompt_id 只一次");
        assert_eq!(count_in(&s, &lobby), before + 1);
    }

    #[test]
    fn captain_question_goes_to_the_root_human_channel_never_to_itself() {
        let mut s = store();
        turn(&mut s, "cap-b", "p0", "好。");
        assert!(matches!(turn(&mut s, "cap-b", "p1", "要发版吗？"), TurnEnd::Posted { .. }));
        let human = crewd_store::channels::human_id("crew-a");
        let mentions: String = s
            .conn()
            .query_row("SELECT mentions FROM messages WHERE channel_id=?1 ORDER BY seq DESC LIMIT 1", [&human], |r| r.get(0))
            .unwrap();
        assert!(mentions.contains("human") && !mentions.contains("captain"), "#541：{mentions}");
    }

    #[test]
    fn unknown_memo_or_calling_out_means_no_relay() {
        let mut s = store();
        assert_eq!(turn(&mut s, "w-a1", "p1", "要不要做 B？"), TurnEnd::AlreadyCalled, "从没记过账：宁可不发");
        assert_eq!(awaiting(&s, "w-a1").as_deref(), Some("要不要做 B？"), "但标红照写");
        crate::run(&mut s, 1_790_000_000_000, |tx| {
            note_post(tx, "w-a1", &[Mention { kind: MentionKind::Captain, target_id: None }])
        })
        .unwrap();
        assert_eq!(turn(&mut s, "w-a1", "p2", "要不要做 C？"), TurnEnd::AlreadyCalled, "这一轮 @ 过机长");
        crate::run(&mut s, 1_790_000_000_000, |tx| {
            note_post(tx, "w-a1", &[Mention { kind: MentionKind::Session, target_id: Some("w-a2".into()) }])
        })
        .unwrap();
        assert!(matches!(turn(&mut s, "w-a1", "p3", "要不要做 D？"), TurnEnd::Posted { .. }), "只 @ 了别的 session 不算叫到人");
        assert_eq!(turn(&mut s, "w-a1", "p4", "做完了。"), TurnEnd::Nothing);
        assert_eq!(awaiting(&s, "w-a1"), None, "陈述收尾：红点熄");
    }
}
```

- [ ] **Step 2: 跑一遍确认失败**

Run: `cargo test -p crewd-collab turn_trace`
Expected: 编译失败（`cannot find function trailing_question`）。

- [ ] **Step 3: 写实现**（替换 M3a stub 的函数体；`TurnEnd` 原样保留）

```rust
//! 「一轮收尾停在一个没人被叫到的问句上，系统替它发进群」（移植 `SessionTurnTrace.swift`）。
//! 判据只有一条：停在问句上、且这一轮没 @ 到能答的人。静默是正常状态，不是异常信号。永不阻塞。
use crate::prompts::fill;
use crate::{Result, Sender, Tx, crews, messages, sessions, settings};
use crewd_proto::{Mention, MentionKind, Role};
use serde::{Deserialize, Serialize};

/// 收尾问句带进群 / 存进 `awaiting_question` 的最大字数。
pub const QUESTION_LIMIT: usize = 160;

/// 句末标点后面还能合法跟着的闭合符（上一句的尾巴，不是新的一句）。
const CLOSERS: &[char] = &[
    '）', ')', '」', '』', '】', '》', '〉', '〕', '］', ']', '｝', '}', '”', '’', '"', '\'',
];

/// 收尾**那一句**，且必须是问句；否则 None。中途反问、陈述句收尾都不算在等人。
pub fn trailing_question(raw: &str, limit: usize) -> Option<String> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    let last = split(text).pop()?;
    if !has_content(&last) || !ends_as_question(&last) {
        return None;
    }
    Some(truncate(&last, limit))
}

fn ends_as_question(s: &str) -> bool {
    let t = s.trim_end_matches(|c: char| CLOSERS.contains(&c) || c == ' ');
    t.ends_with('？') || t.ends_with('?')
}

/// 剥掉标点、符号、空白后还剩字没有（字母、数字、汉字算字）。
fn has_content(s: &str) -> bool {
    s.chars().any(|c| c.is_alphanumeric())
}

/// 按中文句末标点（。？！）、半角 ? ! 与换行切句，保留标点；半角句点不切（1.5 / main.rs）。
/// 只剩闭合符的碎片并回前一句。
fn split(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch == '\n' {
            if !cur.trim().is_empty() {
                out.push(clean(&cur));
            }
            cur.clear();
            continue;
        }
        cur.push(ch);
        if "。？！?!".contains(ch) {
            out.push(clean(&cur));
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        out.push(clean(&cur));
    }
    let mut merged: Vec<String> = Vec::new();
    for piece in out.into_iter().filter(|p| !p.is_empty()) {
        let closers_only = piece.chars().all(|c| CLOSERS.contains(&c) || c == ' ');
        match merged.last_mut() {
            Some(prev) if closers_only => prev.push_str(&piece),
            _ => merged.push(piece),
        }
    }
    merged
}

/// 去掉 markdown 列表 / 标题 / 引用前缀与两端空白。
fn clean(s: &str) -> String {
    s.trim()
        .trim_start_matches(['#', '-', '*', '>', ' '])
        .trim()
        .to_string()
}

fn truncate(s: &str, limit: usize) -> String {
    if s.chars().count() <= limit {
        s.to_string()
    } else {
        s.chars().take(limit).collect::<String>() + "…"
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CallOut {
    #[default]
    Unknown,
    No,
    Yes,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Memo {
    last_prompt_id: Option<String>,
    called_out: CallOut,
}

fn memo_key(session: &str) -> String {
    format!("agent.turn.{session}")
}

fn load(tx: &Tx<'_>, session: &str) -> Result<Memo> {
    Ok(crewd_store::meta_get(tx.conn(), &memo_key(session))?
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default())
}

fn save(tx: &Tx<'_>, session: &str, m: &Memo) -> Result<()> {
    let v = serde_json::to_string(m).expect("memo serializes");
    crewd_store::meta_set(tx.conn(), &memo_key(session), &v)?;
    Ok(())
}

pub fn note_post(tx: &mut Tx<'_>, session: &str, mentions: &[Mention]) -> Result<()> {
    if mentions.iter().any(|m| matches!(m.kind, MentionKind::Captain | MentionKind::Human)) {
        let mut m = load(tx, session)?;
        m.called_out = CallOut::Yes;
        save(tx, session, &m)?;
    }
    Ok(())
}

/// 顺序即判据（`SessionTurnTrace.swift:207-215`）：同轮去重 → 不是问句不发 → 已叫过人（或不知道）不发。
pub fn on_turn_end(tx: &mut Tx<'_>, session: &str, prompt_id: &str, last_text: &str) -> Result<TurnEnd> {
    let question = trailing_question(last_text, QUESTION_LIMIT);
    sessions::set_awaiting_question(tx, session, question.clone())?;
    let mut memo = load(tx, session)?;
    if !prompt_id.is_empty() && memo.last_prompt_id.as_deref() == Some(prompt_id) {
        return Ok(TurnEnd::AlreadyCalled);
    }
    let called = memo.called_out;
    if !prompt_id.is_empty() {
        memo.last_prompt_id = Some(prompt_id.to_string());
    }
    memo.called_out = CallOut::No;
    save(tx, session, &memo)?;
    let Some(question) = question else { return Ok(TurnEnd::Nothing) };
    if called != CallOut::No {
        return Ok(TurnEnd::AlreadyCalled);
    }
    let conn = tx.conn();
    let s = sessions::get(conn, session)?;
    let label = sessions::display_name(conn, session)?;
    let captain = sessions::role_of(conn, session)? == Role::Captain;
    let l = settings::locale(conn)?;
    let f = crate::worldview::tools_file(l);
    let (channel_id, kind, key) = if captain {
        let root = crews::root_of(conn, &s.crew_id)?;
        let ch = crate::channels::ensure_human_channel(tx, &root)?;
        (ch.id, MentionKind::Human, "trace.captain")
    } else {
        (crewd_store::channels::lobby_id(&s.crew_id), MentionKind::Captain, "trace.worker")
    };
    let text = fill(f.get(key), &[("label", &label), ("question", &question), ("session", session)]);
    let posted = messages::post(
        tx,
        Sender::System,
        messages::NewPost {
            channel_id,
            text,
            mentions: vec![Mention { kind, target_id: None }],
            category: Some("question".into()),
            ..Default::default()
        },
    )?;
    Ok(TurnEnd::Posted { message_id: posted.message.id })
}
```

（`TurnEnd::AlreadyCalled` 同时用于「同轮重放」「这一轮叫过人」「不知道叫没叫过」三种不代发的情况——M3a 冻结的枚举只有三个值；要分开就在 M3a 合并前改枚举，回报控制者，不在 M3b 里加第四个。）

- [ ] **Step 4: md 加节**

`tools.zh.md`：

```markdown
## trace.worker
「{label}」这一轮停在一个问题上，在等回复：
{question}
机长可以 inspect_session 看现场、nudge_session 代答；拍不了板就问人。（session：{session}）

## trace.captain
机长「{label}」这一轮停在一个问题上，在等你回复：
{question}
需要你打开这个 session 的终端回它一句。（session：{session}）
```

`tools.en.md`：

```markdown
## trace.worker
"{label}" stopped this turn on a question and is waiting for an answer:
{question}
Captain: inspect_session to see the state, nudge_session to answer for it; if you cannot decide, ask the human. (session: {session})

## trace.captain
Captain "{label}" stopped this turn on a question and is waiting for your answer:
{question}
Please open this session's terminal and reply. (session: {session})
```

- [ ] **Step 5: 跑通**

Run: `cargo test -p crewd-collab turn_trace`
Expected: `8 passed`（纯函数 5 + 落库 3）。

- [ ] **Step 6: 变异核对**

1. `ends_as_question` 里的 `trim_end_matches(..)` 换成 `s`：Expected `closers_after_the_mark_still_count` 红。
2. `on_turn_end` 里 `if called != CallOut::No` 换成 `if called == CallOut::Yes`：Expected `unknown_memo_or_calling_out_means_no_relay` 红（第一句）。
3. 机长分支的 `MentionKind::Human` 换成 `MentionKind::Captain`：Expected `captain_question_goes_to_the_root_human_channel_never_to_itself` 红。

逐条撤销（编辑器，不用 `git checkout`）。

- [ ] **Step 7: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/collab/src/turn_trace.rs crates/collab/prompts/tools.zh.md crates/collab/prompts/tools.en.md
git diff --cached --stat
git commit -m "crewd-collab: turn trace (trailing question, per-turn memo in meta, relay worker->captain / captain->human)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 6: 与 M3a 的唯一接口（`agent::port`）+ 运行时配置 + 测试夹具

**Files:**
- Create: `crewd/crates/server/src/agent/port.rs`、`agent/runtime.rs`、`agent/testkit.rs`
- Modify: `crewd/crates/server/src/agent/mod.rs`（`pub mod port; pub mod runtime; #[cfg(any(test, feature = "testkit"))] pub mod testkit;`）、`crewd/crates/server/src/lib.rs`（`AppState` 加 `pub agent: agent::AgentRuntime`，`AppState::new` 里 `agent: Default::default()`——见 G9）、`crewd/crates/server/Cargo.toml`（feature `testkit`）

**Interfaces:**
- Consumes: M3a 冻结接口（见「M3a 对齐」）；Task 3 `Identity` / `Role`。
- Produces（之后所有任务只经这些碰协作数据；名字对 M3b 其余文件是定死的）：

```rust
// agent::port —— 视图类型（把 M3a 的行映射成工具要的样子）
pub use crewd_proto::ChannelKind;
pub type PortResult<T> = Result<T, PortError>;
pub enum PortError { NotFound(String), Refused(String), Internal(String) }   // From<CollabError>、From<ControlError>、From<StoreError>
pub enum MemberRef { Session(String), Human }                               // M3b 本地的；与 crewd_proto::MemberRef 互转
pub struct ChannelView { pub id, pub crew_id, pub crew_title, pub kind: ChannelKind, pub name /*大厅、人类群为空串*/,
                         pub purpose: Option<String>, pub owner: Option<String> /*session id*/, pub members: Vec<MemberRef>,
                         pub watched: bool, pub archived: bool, pub listening: bool }
pub struct CrewView { pub id, pub title, pub number: Option<i64>, pub builtin: bool, pub parent_ids: Vec<String>,
                      pub child_ids: Vec<String>, pub working_directory: Option<String>, pub captain: Option<String>,
                      pub captain_executor: Option<String>, pub policy: Option<String> }
pub struct SessionView { pub id, pub crew_id, pub role: Role, pub label, pub extension: Option<String>, pub executor: String }
pub enum PostSender { Session(String), System }
pub struct PostReq { pub channel_id, pub sender: PostSender, pub text, pub mentions: Vec<Mention>,
                     pub in_reply_to: Option<String>, pub category: Option<String>, pub attachments: Vec<String> /*只能空*/ }
pub enum MsgRef { Seq(i64), Id(String) }
pub enum NumberTarget { Crew(String), Session { crew_id: String, session_id: String } }
pub struct DirEntry { pub number: String, pub name: String, pub crew_title: String, pub detail: String }
pub struct CrewPatch { pub title: Option<String>, pub working_directory: Option<String>, pub status_line: Option<String>, pub policy: Option<String> }
pub struct StartReq { pub crew_id, pub role: Role, pub executor: Option<String>, pub brief: String, pub title: Option<String>,
                      pub model: Option<String>, pub effort: Option<String>, pub worktree: bool }
pub struct Posted { pub message: Message, pub unwatched_human_mention: bool }

// 身份 / 设置 / 事件
async fn identity(st, session) -> PortResult<Option<Identity>>
async fn sessions_of(st, crew) -> PortResult<Vec<SessionView>>
async fn locale(st) -> PortResult<Locale>                                    // settings::locale
fn verify_token(st, session, token) -> bool
fn subscribe_collab(st) -> broadcast::Receiver<crewd_collab::Change>
// 机组
async fn crew(st, id) -> PortResult<Option<CrewView>>
async fn resolve_crew(st, arg) -> PortResult<Option<CrewView>>               // id → 号码（全数字）→ 唯一标题
async fn update_crew(st, id, CrewPatch) -> PortResult<()>
async fn attach_parent(st, by: &Identity, child, parent) -> PortResult<()>   // crews::adopt(tx, child, parent)
async fn detach_parent(st, by: &Identity, parent, child, to: Option<String>) -> PortResult<()>   // crews::release(tx, child, parent, to)
async fn create_crew(st, by: &Identity, title: Option<String>, workdir: Option<String>, parent: Option<String>) -> PortResult<CrewView>
// 群
async fn channel(st, id) -> PortResult<Option<ChannelView>>
async fn my_channels(st, session) -> PortResult<Vec<ChannelView>>            // channels_of_session + listens::channels_of（listening = true）
async fn crew_channels(st, crew) -> PortResult<Vec<ChannelView>>             // channels::list_for_crew
async fn lobby_of(st, crew) -> PortResult<ChannelView>
async fn root_human_channel(st, crew) -> PortResult<ChannelView>             // ensure_human_channel(root_of(crew))
async fn watched_channels(st) -> PortResult<Vec<ChannelView>>
async fn create_channel(st, by: &Identity, kind, name, purpose: Option<String>, members: Vec<MemberRef>) -> PortResult<ChannelView>
async fn ensure_dm(st, by: &Identity, other: &str) -> PortResult<(ChannelView, bool)>   // channels::ensure_dm(tx, by.crew, a, b)
async fn add_members(st, by: &Identity, channel, Vec<MemberRef>) -> PortResult<ChannelView>
async fn remove_member(st, by: &Identity, channel, MemberRef) -> PortResult<()>
async fn archive_channel(st, by: &Identity, channel) -> PortResult<()>      // channels::update(.., archived: Some(true))
async fn request_owner_transfer(st, by: &Identity, channel, to) -> PortResult<()>
async fn accept_owner_transfer(st, by: &Identity, channel) -> PortResult<()>
async fn force_owner(st, by: &Identity, channel, to) -> PortResult<()>
async fn set_listen(st, session, channel, until_ms: Option<i64>, senders: Vec<String>) -> PortResult<()>
async fn clear_listen(st, session, channel) -> PortResult<()>
// 消息
async fn post(st, PostReq) -> PortResult<Posted>                            // messages::post
async fn messages(st, channel, before_seq: Option<i64>, limit: u32) -> PortResult<Vec<Message>>   // 旧在前
async fn message(st, MsgRef) -> PortResult<Option<Message>>
async fn latest_seq(st, channel) -> PortResult<Option<i64>>
async fn search(st, query, channel_ids: Vec<String>, after: Option<String>, before: Option<String>, limit: u32) -> PortResult<Vec<Message>>
async fn unread(st, session) -> PortResult<Vec<(String, Vec<Message>)>>      // 全文见 Task 9 Step 3，本任务写
async fn blocks(st, session, Vec<(String, Vec<Message>)>) -> PortResult<Vec<ChannelBlock>>
fn injection(Locale, &[ChannelBlock]) -> String
async fn advance(st, session, channel, seq) -> PortResult<()>
// 通讯录
async fn directory(st, query: Option<String>) -> PortResult<Vec<DirEntry>>
async fn resolve_number(st, number) -> PortResult<Option<NumberTarget>>      // 在 directory::list 里找 number 完全相等的那一行
// 人类待确认 / Todo / 唤醒
async fn open_ask(st, by: &Identity, question: &str, fallback: Option<(String, i64 /*分钟*/)>, resume_note: Option<String>) -> PortResult<String>  // "H<n>"
async fn withdraw_ask(st, by: &Identity, ask, reason) -> PortResult<()>
async fn respond_todo(st, by: &Identity, number: i64, text) -> PortResult<()>                // G3
async fn schedule_timer(st, session, not_before_ms: i64, note) -> PortResult<()>
// 审批
async fn create_approval(st, by: &Identity, tool_name, summary, tool_use_id: Option<String>) -> PortResult<String>
async fn approval_decision(st, id) -> PortResult<Option<bool>>
async fn decide_approval(st, id, allow: bool, by: &str) -> PortResult<()>
// 会话控制（SessionControl）
async fn start_session(st, StartReq) -> PortResult<String>                  // executor 缺省 = 机组 captain_executor，再缺省 "claude"
async fn stop_session(st, session, reason) -> PortResult<()>
async fn nudge(st, session, input) -> PortResult<()>
async fn inspect(st, session) -> PortResult<String>                         // Inspection.screen + live 渲染成文本
async fn set_profile(st, session, model: Option<String>, effort: Option<String>) -> PortResult<()>
async fn quota(st) -> PortResult<String>
async fn live_state(st, session) -> PortResult<String>
async fn handoff_captain(st, crew, session) -> PortResult<()>               // SessionControl::handoff_captain
// agent::runtime
pub struct AgentConfig { pub approval_gates: Vec<String>, pub approval_wait: Duration, pub inject_cap_chars: usize }
pub struct AgentRuntime { pub config: AgentConfig }
// agent::testkit（feature testkit 或 cfg(test)）
pub struct Fixture { pub state: Arc<AppState>, pub crew: String, pub captain: String, pub worker: String, .. }
impl Fixture {
  pub async fn new() -> Self; pub async fn with_config(AgentConfig) -> Self;
  pub async fn add_crew(&self, title: &str, parent: Option<&str>) -> String;
  pub async fn add_session(&self, crew: &str, id: &str, role: Role, label: &str);   // 固定 id（直接写行）
  pub async fn register(&self, crew: &str, role: Role, label: &str) -> String;     // 走 sessions::register（有分机号）
  pub fn token(&self, session: &str) -> String;
  pub async fn me(&self, session: &str) -> Identity;
  pub async fn rpc(&self, method: &str, params: Value) -> Value;           // 进程内调客户端 RPC，出错 panic
  pub async fn human_post(&self, channel: &str, text: &str, mentions: Vec<Mention>) -> Message;
  pub async fn session_post(&self, session: &str, channel: &str, text: &str, mentions: Vec<Mention>) -> Message;
  pub async fn spawn_socket(&self) -> PathBuf;               // <dir>/crewd.sock，accept 循环跑 conn::run；重复调用返回同一个
  pub fn data_dir(&self) -> &Path;
  pub fn control_calls(&self) -> Vec<String>;                // FakeSessionControl::calls() 逐条 `{:?}`，如 `Start(StartRequest { … })`
}
```

工具语言来自库里的设置（`settings::locale`，M3a），不在 `AgentConfig` 里。回合记账在 collab 的 `meta` 里（Task 5），不在 serve 内存。

- [ ] **Step 1: 加 feature**

`crates/server/Cargo.toml`：

```toml
[features]
testkit = ["dep:tempfile"]

[dependencies]
tempfile = { version = "3.27.0", optional = true }
```

（`[dev-dependencies]` 里的 `tempfile` 保留。）crates/crewd 的 dev-dependencies 在 Task 16 再开这个 feature。

- [ ] **Step 2: 写失败的测试**（`agent/port.rs` 末尾——验与 M3a 的接线，不验 M3a 的业务）

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testkit::Fixture;
    use crate::agent::Role;

    #[tokio::test]
    async fn identity_reads_role_label_and_builtin() {
        let f = Fixture::new().await;
        let cap = identity(&f.state, &f.captain).await.unwrap().unwrap();
        assert_eq!(cap.role, Role::Captain);
        assert_eq!(cap.crew_id, f.crew);
        let w = identity(&f.state, &f.worker).await.unwrap().unwrap();
        assert_eq!((w.role, w.label.as_str()), (Role::Worker, "示例 worker"));
        assert!(!w.builtin_crew);
        assert!(identity(&f.state, "nobody").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn lobby_is_mine_and_posts_become_unread_for_others() {
        let f = Fixture::new().await;
        let lobby = lobby_of(&f.state, &f.crew).await.unwrap();
        assert_eq!(lobby.kind, ChannelKind::Lobby);
        assert!(my_channels(&f.state, &f.worker).await.unwrap().iter().any(|c| c.id == lobby.id));
        unread(&f.state, &f.worker).await.unwrap(); // 第一次：M3a 把缺的游标钉在末尾
        let m = f.session_post(&f.captain, &lobby.id, "示例公告", vec![]).await;
        let u = unread(&f.state, &f.worker).await.unwrap();
        let (_, msgs) = u.iter().find(|(c, _)| *c == lobby.id).expect("大厅有未读");
        assert_eq!(msgs.last().unwrap().id, m.id);
        advance(&f.state, &f.worker, &lobby.id, m.seq).await.unwrap();
        assert!(unread(&f.state, &f.worker).await.unwrap().iter().all(|(_, v)| v.is_empty()));
        assert_eq!(message(&f.state, MsgRef::Seq(m.seq)).await.unwrap().unwrap().id, m.id);
    }

    #[tokio::test]
    async fn root_human_channel_is_created_lazily_once() {
        let f = Fixture::new().await;
        let a = root_human_channel(&f.state, &f.crew).await.unwrap();
        let b = root_human_channel(&f.state, &f.crew).await.unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(a.kind, ChannelKind::Human);
    }

    #[tokio::test]
    async fn tokens_are_per_session() {
        let f = Fixture::new().await;
        let t = f.token(&f.worker);
        assert!(verify_token(&f.state, &f.worker, &t));
        assert!(!verify_token(&f.state, &f.captain, &t));
        assert!(!verify_token(&f.state, &f.worker, "forged"));
    }
}
```

（`unread` / `blocks` / `injection` / `advance` 四个函数在本任务就写——全文在 Task 9 Step 3，测试要用 `unread` / `advance`。）

- [ ] **Step 3: 跑一遍确认失败**

Run: `cargo test -p crewd-server --lib agent::port`
Expected: 编译失败（`unresolved import crate::agent::testkit`）。

- [ ] **Step 4: 写 `runtime.rs`**

```rust
//! serve 里 agent 面的配置（serve 启动段由 M3c 填；缺省值见 Default）。
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct AgentConfig {
    /// 要人审批的工具名子串（也写进每个 claude 的 PreToolUse matcher，见 crewd-exec `gate_matcher`）。
    pub approval_gates: Vec<String>,
    /// 等人审批的上限；到点 deny（spec §22.3）。
    pub approval_wait: Duration,
    /// 一次注入的字符上限（官方 additionalContext 上限 10,000，留余量）。
    pub inject_cap_chars: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            approval_gates: vec!["computer-use".to_string()],
            approval_wait: Duration::from_secs(crewd_exec::launch::DEFAULT_APPROVAL_WAIT_SECS),
            inject_cap_chars: 9_000,
        }
    }
}

#[derive(Debug, Default)]
pub struct AgentRuntime {
    pub config: AgentConfig,
}

impl AgentRuntime {
    pub fn new(config: AgentConfig) -> Self {
        Self { config }
    }
}
```

- [ ] **Step 5: 写 `port.rs`**

文件头注释：「唯一直接调 M3a（crewd-collab / crewd-store / SessionControl / TokenTable）的文件。名字取自 M3a 计划的冻结接口块；M3a 落地时与这里不同，只改这个文件。」下面给出类型、错误映射与有代表性的函数全文，其余函数按后面的对照表照同一形状写（读：`st.db.call(move |s| collab::X(s.conn(), …)).await??`；写：`st.collab(move |tx| collab::X(tx, …)).await?`——`AppState::collab` 在一个事务里跑并把 `Change` 发到 Hub）。

```rust
use std::fmt;

use crewd_collab::{Change, CollabError, Locale, Sender};
use crewd_proto::{Mention, Message};
pub use crewd_proto::ChannelKind;
use tokio::sync::broadcast;

use crate::AppState;
use crate::agent::{Identity, Role};
use crate::control::ControlError;

pub type PortResult<T> = Result<T, PortError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortError {
    NotFound(String),
    /// M3a 按规则拒绝（不是成员、已归档、成环、没权限、冲突……）；文本是 M3a 写的说明，原样给 agent。
    Refused(String),
    Internal(String),
}

impl fmt::Display for PortError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(s) | Self::Refused(s) => write!(f, "{s}"),
            Self::Internal(s) => write!(f, "internal error: {s}"),
        }
    }
}

impl std::error::Error for PortError {}

impl From<CollabError> for PortError {
    fn from(e: CollabError) -> Self {
        match e {
            CollabError::NotFound(s) => Self::NotFound(s),
            CollabError::Store(s) => Self::Internal(s.to_string()),
            other => Self::Refused(other.to_string()),
        }
    }
}

impl From<crewd_store::StoreError> for PortError {
    fn from(e: crewd_store::StoreError) -> Self {
        Self::Internal(e.to_string())
    }
}

impl From<ControlError> for PortError {
    fn from(e: ControlError) -> Self {
        match e {
            ControlError::NotFound(s) => Self::NotFound(s),
            other => Self::Refused(other.to_string()),
        }
    }
}

pub fn now_ms() -> i64 {
    crewd_store::now_ms()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberRef {
    Session(String),
    Human,
}

impl MemberRef {
    fn to_proto(&self) -> crewd_proto::MemberRef {
        match self {
            Self::Session(id) => crewd_proto::MemberRef::session(id.clone()),
            Self::Human => crewd_proto::MemberRef::Human,
        }
    }
    fn from_proto(m: &crewd_proto::MemberRef) -> Self {
        match m {
            crewd_proto::MemberRef::Session { id } => Self::Session(id.clone()),
            crewd_proto::MemberRef::Human => Self::Human,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelView {
    pub id: String,
    pub crew_id: String,
    pub crew_title: String,
    pub kind: ChannelKind,
    /// 大厅 / 人类群是空串（M3a 约定）；回执里经 `tools::channel_label` 显示。
    pub name: String,
    pub purpose: Option<String>,
    /// 群主的 session id（人类做群主时 None）。
    pub owner: Option<String>,
    pub members: Vec<MemberRef>,
    pub watched: bool,
    pub archived: bool,
    /// 只在 `my_channels` 里有意义：我在收听它（不一定是成员）。
    pub listening: bool,
}

impl ChannelView {
    pub fn has_session(&self, s: &str) -> bool {
        self.kind == ChannelKind::Lobby
            || self.members.iter().any(|m| matches!(m, MemberRef::Session(x) if x == s))
    }

    /// ChannelSummary（M3a）→ 视图。<M3a>：`Channel` 上表示归档的字段名按真名改。
    fn from_summary(conn: &rusqlite::Connection, s: crewd_proto::ChannelSummary) -> PortResult<Self> {
        let crew_title = crewd_collab::crews::get(conn, &s.channel.crew_id)?.title;
        Ok(Self {
            owner: match &s.channel.owner {
                Some(crewd_proto::MemberRef::Session { id }) => Some(id.clone()),
                _ => None,
            },
            members: s.members.iter().map(MemberRef::from_proto).collect(),
            watched: s.watched,
            archived: s.channel.archived_at.is_some(),
            listening: false,
            id: s.channel.id,
            crew_id: s.channel.crew_id,
            crew_title,
            kind: s.channel.kind,
            name: s.channel.name,
            purpose: s.channel.purpose,
        })
    }
}

/// session 以自己身份、或系统身份发言。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostSender {
    Session(String),
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostReq {
    pub channel_id: String,
    pub sender: PostSender,
    pub text: String,
    pub mentions: Vec<Mention>,
    pub in_reply_to: Option<String>,
    pub category: Option<String>,
    /// M3a 的 `message.post` 暂不收附件（M3a 裁定 14）：只能是空的。
    pub attachments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Posted {
    pub message: Message,
    /// §17：@人类 而人类没在注视这个群（M3a 算好的）。
    pub unwatched_human_mention: bool,
}

pub async fn identity(st: &AppState, session: &str) -> PortResult<Option<Identity>> {
    let id = session.to_string();
    let r = st
        .db
        .call(move |s| -> crewd_collab::Result<Option<Identity>> {
            let conn = s.conn();
            let row = match crewd_collab::sessions::get(conn, &id) {
                Ok(r) => r,
                Err(CollabError::NotFound(_)) => return Ok(None),
                Err(e) => return Err(e),
            };
            let crew = crewd_collab::crews::get(conn, &row.crew_id)?;
            Ok(Some(Identity {
                role: crewd_collab::sessions::role_of(conn, &id)?,
                label: crewd_collab::sessions::display_name(conn, &id)?,
                builtin_crew: crew.builtin,
                crew_id: row.crew_id,
                session_id: row.id,
            }))
        })
        .await??;
    Ok(r)
}

pub async fn locale(st: &AppState) -> PortResult<Locale> {
    Ok(st.db.call(|s| crewd_collab::settings::locale(s.conn())).await??)
}

pub fn verify_token(st: &AppState, session: &str, token: &str) -> bool {
    st.tokens.verify(session, token)
}

pub fn subscribe_collab(st: &AppState) -> broadcast::Receiver<Change> {
    st.hub.subscribe_collab()
}

pub async fn post(st: &AppState, r: PostReq) -> PortResult<Posted> {
    if !r.attachments.is_empty() {
        return Err(PortError::Refused("attachments are not supported yet".into()));
    }
    let sender = match r.sender {
        PostSender::Session(id) => Sender::Session(id),
        PostSender::System => Sender::System,
    };
    let np = crewd_collab::messages::NewPost {
        channel_id: r.channel_id,
        text: r.text,
        in_reply_to: r.in_reply_to,
        mentions: r.mentions,
        attachments: vec![],
        category: r.category,
    };
    let p = st.collab(move |tx| crewd_collab::messages::post(tx, sender, np)).await?;
    Ok(Posted { message: p.message, unwatched_human_mention: p.unwatched_human_mention })
}

pub async fn start_session(st: &AppState, r: StartReq) -> PortResult<String> {
    let executor = match r.executor {
        Some(e) => e,
        None => crew(st, &r.crew_id)
            .await?
            .and_then(|c| c.captain_executor)
            .unwrap_or_else(|| "claude".to_string()),
    };
    let req = crate::control::StartRequest {
        crew_id: r.crew_id,
        role: r.role,
        executor,
        brief: r.brief,
        title: r.title,
        model: r.model,
        effort: r.effort,
        // <M3a / M3c>：worktree 的取值约定（见 G-S）；isolation=true 先传 "auto"。
        worktree: r.worktree.then(|| "auto".to_string()),
    };
    Ok(st.sessions.start(req).await?.id)
}
```

其余 port 函数 ↔ M3a 冻结接口：

| port 函数 | M3a 调用 | 备注 |
|---|---|---|
| `sessions_of` | `sessions::list(conn, Some(crew))` + `role_of` + `display_name`；分机号取 `directory::list` 里 `session_id` 相等那一行的 `number` | |
| `crew` / `resolve_crew` | `crews::get` + `crews::parents` + `crews::children` + `crews::captain_of`；`policy` 是 `serde_json::Value`：`String(s)` → s，其余 `to_string()` | `resolve_crew`：id 命中 → 号码（全数字，比 `Crew.number`）→ 标题恰好一个 |
| `update_crew` | `crews::update(tx, id, collab::crews::CrewPatch { title, working_directory, status_line, policy: policy.map(Value::String), ..Default::default() })` | |
| `attach_parent` / `detach_parent` / `create_crew` | `crews::adopt(tx, child, parent)` / `crews::release(tx, child, parent, to.as_deref())` / `crews::create(tx, NewCrew { title: title.unwrap_or_default(), working_directory, parent_id, title_source: None })` | 成环、内建、非直系子由 M3a 回 `Refused` |
| `channel` / `crew_channels` / `lobby_of` / `watched_channels` | `channels::summary(conn, id)` / `channels::list_for_crew(tx, crew)` / `summary(lobby_id(crew))` / `list_for_crew` 全部机组里 `watched` 的 | 都经 `ChannelView::from_summary` |
| `my_channels` | `channels::channels_of_session(conn, s)` ∪ `listens::channels_of(conn, s, now)`（后者 `listening = true`） | |
| `root_human_channel` | `channels::ensure_human_channel(tx, &crews::root_of(conn, crew)?)` | |
| `create_channel` | `channels::create(tx, &Sender::Session(by), NewChannel { crew_id: by.crew_id, kind, name, purpose, members })` | 建群者自动入群 |
| `ensure_dm` | `channels::ensure_dm(tx, &by.crew_id, &session(by), &session(other))` | 同一对 session 只有一个 dm |
| `add_members` / `remove_member` / `archive_channel` | `channels::add_members(tx, &Sender::Session(by), id, &ms)` / `remove_member` / `update(.., ChannelPatch { archived: Some(true), .. })` | **权限由 M3a 按 `by` 判**（群主 / 机长 / 人），M3b 不重复判 |
| `request_owner_transfer` / `accept_owner_transfer` / `force_owner` | 同名（`force_owner` 用机长身份的 `Sender::Session`） | |
| `set_listen` / `clear_listen` | `listens::set(tx, s, ch, until_ms, senders)` / `listens::clear(tx, s, Some(ch))` | |
| `messages` / `latest_seq` / `message` / `search` | `messages::list(conn, ch, None, before, limit)` / 同上取 1 条 / `messages::get(conn, id)`（`Seq` 用 `list` 的 `after = seq-1, limit 1` 再核 seq）/ `messages::search(conn, &Search { query, channel_ids: Some(ids), after, before, limit, crew_id: None })` | |
| `directory` / `resolve_number` | `directory::list(conn, query)` → `DirEntry { number, name, crew_title: crews::get(crew_id).title, detail: org_path }`；`resolve_number` 在全表里找 `number == arg` | |
| `open_ask` | `asks::open(tx, &by.session_id, OpenAsk { question, default: fallback.map(|(text, minutes)| AskDefault { text, after_minutes: minutes }), resume_note })` → `Ask.ask` | |
| `withdraw_ask` | `asks::withdraw(tx, &Sender::Session(by), &by.crew_id, ask, reason)` | |
| `respond_todo` | **G3：M3a 冻结接口里没有**（`todo_responses` 表在，写函数不在） | |
| `schedule_timer` | `wakes::enqueue(tx, WakeRequest { session, kind: WakeKind::Timer, source: wakes::source::timer(&uuid_v4), priority: WakePriority::System, not_before_ms, note: Some(note), origin: None })` | `origin` 的类型按 M3a |
| `create_approval` / `approval_decision` / `decide_approval` | `approvals::request(tx, NewApproval { session_id, tool_name, summary, tool_use_id })` / `approvals::get(conn, id).decision` / `approvals::decide(tx, id, allow, by)` | `request` 自己发人类群通知 |
| `stop_session` / `nudge` / `inspect` / `set_profile` / `quota` / `live_state` / `handoff_captain` | `st.sessions.stop(..)` / `.nudge(..)` / `.inspect(id, 40)` / `.set_profile(..)` / `.quota()` / `.live(id)` / `.handoff_captain(crew, to)` | `inspect` / `quota` / `live_state` 把结构渲染成 `key: value` 行（英文 key，不写中文） |

- [ ] **Step 6: 写 `testkit.rs`**

```rust
//! 测试夹具：临时数据目录里的真库（M3a 的 Db + schema v3）+ FakeSessionControl。
//! `crewd` 的集成测试经 feature `testkit` 用它；仓库公开，只造合成数据。
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crewd_proto::{Mention, Message};
use serde_json::json;

use crate::AppState;
use crate::agent::port::{self, PostReq, PostSender};
use crate::agent::runtime::{AgentConfig, AgentRuntime};
use crate::agent::{Identity, Role};
use crate::control::FakeSessionControl;

pub struct Fixture {
    pub state: Arc<AppState>,
    pub crew: String,
    pub captain: String,
    pub worker: String,
    dir: tempfile::TempDir,
    control: Arc<FakeSessionControl>,
}

impl Fixture {
    pub async fn new() -> Self {
        Self::with_config(AgentConfig::default()).await
    }

    pub async fn with_config(config: AgentConfig) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let control = Arc::new(FakeSessionControl::default());
        let store = crewd_store::Store::open(&dir.path().join("crewd.db")).unwrap();
        let db = crewd_store::Db::spawn(store).unwrap();
        let mut state = AppState::new(db).with_sessions(control.clone());
        state.agent = AgentRuntime::new(config);
        let mut f = Self {
            state: Arc::new(state),
            crew: String::new(),
            captain: "s-cap".into(),
            worker: "s-w1".into(),
            dir,
            control,
        };
        f.crew = f.add_crew("示例机组", None).await;
        let crew = f.crew.clone();
        f.add_session(&crew, "s-cap", Role::Captain, "示例机长").await;
        f.add_session(&crew, "s-w1", Role::Worker, "示例 worker").await;
        f
    }

    /// 经客户端 RPC 建机组（与客户端同一条路，顺带建大厅）。
    pub async fn add_crew(&self, title: &str, parent: Option<&str>) -> String {
        let v = self
            .rpc("crew.create", json!({"title": title, "workingDirectory": self.dir.path(), "parentId": parent}))
            .await;
        v["id"].as_str().unwrap().to_string()
    }

    /// 固定 id 的 session（直接写行，同 M3a collab testkit 的做法）；机长再经 `crews::set_captain` 立起来。
    pub async fn add_session(&self, crew: &str, id: &str, role: Role, label: &str) {
        let row = crewd_proto::Session {
            id: id.into(),
            crew_id: crew.into(),
            role,
            executor: "claude".into(),
            display_name: Some(label.into()),
            updated_at: "2026-10-06T00:00:00.000Z".into(),
            ..Default::default()
        };
        self.state.db.call(move |s| crewd_store::sessions::upsert(s.conn(), &row, None)).await.unwrap().unwrap();
        if role == Role::Captain {
            let (c, i) = (crew.to_string(), id.to_string());
            self.state.collab(move |tx| crewd_collab::crews::set_captain(tx, &c, &i)).await.unwrap();
        }
    }

    /// 走 `sessions::register`（分配分机号）；返回生成的 id。
    pub async fn register(&self, crew: &str, role: Role, label: &str) -> String {
        let n = crewd_collab::sessions::NewSession {
            crew_id: crew.into(),
            role,
            executor: "claude".into(),
            display_name: Some(label.into()),
            ..Default::default()
        };
        self.state.collab(move |tx| crewd_collab::sessions::register(tx, n)).await.unwrap().id
    }

    pub fn token(&self, session: &str) -> String {
        self.state.tokens.mint(session)
    }

    pub async fn me(&self, session: &str) -> Identity {
        port::identity(&self.state, session).await.unwrap().unwrap()
    }

    pub async fn rpc(&self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let resp = crate::dispatch::handle(
            &self.state,
            crewd_proto::Request { id: 1, method: method.into(), params },
        )
        .await;
        if let Some(e) = resp.error {
            panic!("{method}: {}", e.message);
        }
        resp.result.unwrap()
    }

    /// 人在某个群里发言（客户端 `message.post`）。
    pub async fn human_post(&self, channel: &str, text: &str, mentions: Vec<Mention>) -> Message {
        let v = self
            .rpc("message.post", json!({"channelId": channel, "text": text, "mentions": mentions}))
            .await;
        serde_json::from_value(v).unwrap()
    }

    /// session 以自己身份发言（绕过工具，造场景用）。
    pub async fn session_post(&self, session: &str, channel: &str, text: &str, mentions: Vec<Mention>) -> Message {
        port::post(
            &self.state,
            PostReq {
                channel_id: channel.into(),
                sender: PostSender::Session(session.into()),
                text: text.into(),
                mentions,
                in_reply_to: None,
                category: None,
                attachments: vec![],
            },
        )
        .await
        .unwrap()
        .message
    }

    /// 在 `<dir>/crewd.sock` 上起 accept 循环（与 serve 同一个 `conn::run`），返回 socket 路径。
    pub async fn spawn_socket(&self) -> PathBuf {
        let sock = self.dir.path().join("crewd.sock");
        if sock.exists() {
            return sock; // 已经起过：一个夹具一个 socket
        }
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        let state = self.state.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let st = state.clone();
                tokio::spawn(async move {
                    let _ = crate::conn::run(st, stream).await;
                });
            }
        });
        sock
    }

    pub fn data_dir(&self) -> &Path {
        self.dir.path()
    }

    /// FakeSessionControl 记下的调用，逐条 `{:?}`（如 `Start(StartRequest { crew_id: "…", role: Worker, … })`）。
    pub fn control_calls(&self) -> Vec<String> {
        self.control.calls().iter().map(|c| format!("{c:?}")).collect()
    }
}
```

`agent/mod.rs` 加：

```rust
pub mod port;
pub mod runtime;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;

pub use runtime::{AgentConfig, AgentRuntime};
```

`lib.rs`：`AppState` 加字段 `pub agent: agent::AgentRuntime,`，`AppState::new` 里 `agent: agent::AgentRuntime::default(),`（G9）；`conn` 模块若还是私有，改成 `pub(crate) mod conn;`（testkit 要调 `conn::run`）。

- [ ] **Step 7: 跑通**

Run: `cargo test -p crewd-server --lib agent::port`
Expected: `4 passed`。红了先看是哪一处 M3a 名字 / 行为与冻结块不同，**改 port 的映射，不改测试的意思**；行为与 §22.2 不同的回报控制者。

- [ ] **Step 8: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/server/src/agent/port.rs crates/server/src/agent/runtime.rs crates/server/src/agent/testkit.rs \
  crates/server/src/agent/mod.rs crates/server/src/lib.rs crates/server/Cargo.toml Cargo.lock
git diff --cached --stat
git commit -m "crewd-server: agent port (single seam to M3a), runtime config, test fixture

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 7: agent RPC（attach / tools / call / hook 路由）+ 角色变化推送

**Files:**
- Create: `crewd/crates/server/src/agent/rpc.rs`、`crewd/crates/server/src/agent/tools/mod.rs`
- Modify: `crewd/crates/server/src/conn.rs`（`agent.*` 交给 `agent::rpc`）、`agent/mod.rs`（`pub mod rpc; pub mod tools;`）、`agent/testkit.rs`（加 `RpcClient`、`set_role`）、`crates/collab/prompts/tools.{zh,en}.md`（加 `r.failed`、`r.no_handler`）

**Interfaces:**
- Consumes: Task 1 线上类型；Task 3 `catalog::{find, visible, tools_for}`、`text`；Task 6 `port::{identity, verify_token, subscribe_collab, locale}`、`Fixture`；M3a `Change`、`RpcError::FORBIDDEN`。
- Produces:
  - `agent::rpc::AgentConn`（`Default`；drop 时停掉推送任务）、`agent::rpc::handle(state: &Arc<AppState>, conn: &mut AgentConn, req: Request, tx: &mpsc::Sender<ServerFrame>) -> Response`
  - `agent::tools::{Ctx, call(st: &AppState, me: &Identity, tool: &str, args: Value) -> CallResult, parse_args::<T>(&Ctx, Value) -> Result<T, CallResult>}`；`Ctx { st, me, loc }` 有 `r(key, vars) -> String`、`ok(key, vars) -> CallResult`、`err(key, vars) -> CallResult`、`fail(PortError) -> CallResult`
  - testkit：`RpcClient::{connect(&Path), call(&mut self, method, Value) -> Result<Value, (i32, String)>, next_event(&mut self, Duration) -> Option<Event>}`、`Fixture::set_role(&self, session, Role)`（`crews::set_captain`；Worker = 还给原机长）
  - 错误码：没 attach 调 `agent.*` → `-32601`（spec §22.3）；token / session 不对 → `-32003`（M3a 的 `RpcError::FORBIDDEN`）；参数不对 → `-32602`；M3a 的 `CollabError` 经 `dispatch::collab_err` 映射（-32004 / -32003 / -32009）

- [ ] **Step 1: 写失败的测试**（`agent/rpc.rs` 末尾）

```rust
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crewd_proto::agent::{TOOLS_CHANGED, methods};
    use serde_json::json;

    use crate::agent::testkit::{Fixture, RpcClient};
    use crate::agent::text::{self, Locale};
    use crate::agent::Role;

    async fn attached(f: &Fixture, session: &str) -> RpcClient {
        let sock = f.spawn_socket().await;
        let mut c = RpcClient::connect(&sock).await;
        c.call(methods::ATTACH, json!({"sessionId": session, "token": f.token(session)}))
            .await
            .unwrap();
        c
    }

    #[tokio::test]
    async fn agent_methods_need_attach() {
        let f = Fixture::new().await;
        let mut c = RpcClient::connect(&f.spawn_socket().await).await;
        for m in [methods::TOOLS, methods::CALL, methods::HOOK] {
            let (code, _) = c.call(m, json!({})).await.unwrap_err();
            assert_eq!(code, -32601, "{m}");
        }
        let (code, _) = c
            .call(methods::ATTACH, json!({"sessionId": f.worker, "token": "forged"}))
            .await
            .unwrap_err();
        assert_eq!(code, -32003);
        let other = f.token(&f.captain);
        let (code, _) = c
            .call(methods::ATTACH, json!({"sessionId": f.worker, "token": other}))
            .await
            .unwrap_err();
        assert_eq!(code, -32003, "别人的 token 不能冒用");
        let (code, _) = c.call(methods::TOOLS, json!({})).await.unwrap_err();
        assert_eq!(code, -32601, "attach 失败后仍是未 attach");
    }

    #[tokio::test]
    async fn attach_reports_identity_and_tools_follow_role() {
        let f = Fixture::new().await;
        let sock = f.spawn_socket().await;
        let mut w = RpcClient::connect(&sock).await;
        let a = w
            .call(methods::ATTACH, json!({"sessionId": f.worker, "token": f.token(&f.worker)}))
            .await
            .unwrap();
        assert_eq!(a, json!({"role": "worker", "crewId": f.crew, "label": "示例 worker"}));
        assert_eq!(w.call(methods::TOOLS, json!(null)).await.unwrap().as_array().unwrap().len(), 21);
        let mut cap = attached(&f, &f.captain).await;
        assert_eq!(cap.call(methods::TOOLS, json!(null)).await.unwrap().as_array().unwrap().len(), 40);
    }

    #[tokio::test]
    async fn worker_calling_captain_tool_is_refused_with_md_text() {
        let f = Fixture::new().await;
        let mut w = attached(&f, &f.worker).await;
        let r = w
            .call(methods::CALL, json!({"tool": "list_sessions", "args": {}}))
            .await
            .unwrap();
        assert_eq!(r["isError"], true);
        assert_eq!(
            r["text"],
            text::r(Locale::Zh, "r.captain_only", &[("name", "list_sessions")])
        );
        let r = w.call(methods::CALL, json!({"tool": "plan_add"})).await.unwrap();
        assert_eq!(r["text"], text::r(Locale::Zh, "r.unknown_tool", &[("name", "plan_add")]));
    }

    #[tokio::test]
    async fn role_change_pushes_tools_changed_and_takes_effect_without_restart() {
        // Review Focus 4
        let f = Fixture::new().await;
        let mut w = attached(&f, &f.worker).await;
        f.set_role(&f.worker, Role::Captain).await;
        let ev = w.next_event(Duration::from_secs(5)).await.expect("应推 tools_changed");
        assert_eq!(ev.event, TOOLS_CHANGED);
        assert_eq!(ev.data["role"], "captain");
        assert_eq!(w.call(methods::TOOLS, json!(null)).await.unwrap().as_array().unwrap().len(), 40);
        let r = w.call(methods::CALL, json!({"tool": "list_sessions"})).await.unwrap();
        assert_ne!(
            r["text"],
            text::r(Locale::Zh, "r.captain_only", &[("name", "list_sessions")]),
            "换成机长后马上能用"
        );
        f.set_role(&f.worker, Role::Worker).await;
        let ev = w.next_event(Duration::from_secs(5)).await.unwrap();
        assert_eq!(ev.data["role"], "worker");
        let r = w.call(methods::CALL, json!({"tool": "list_sessions"})).await.unwrap();
        assert_eq!(r["isError"], true, "卸任后当场被拒，不靠 helper 刷新");
    }

    #[tokio::test]
    async fn unrelated_events_do_not_push() {
        let f = Fixture::new().await;
        let mut w = attached(&f, &f.worker).await;
        let lobby = crate::agent::port::lobby_of(&f.state, &f.crew).await.unwrap();
        f.human_post(&lobby.id, "示例消息", vec![]).await;
        assert!(w.next_event(Duration::from_millis(300)).await.is_none());
    }
}
```

`agent/tools/mod.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::agent::testkit::Fixture;
    use crate::agent::Role;

    #[tokio::test]
    async fn builtin_crew_refuses_hidden_tools_even_when_called_directly() {
        let f = Fixture::new().await;
        let mut me = f.me(&f.captain).await;
        me.builtin_crew = true;
        let r = call(&f.state, &me, "ask", json!({"question": "示例"})).await;
        assert!(r.is_error);
        assert_eq!(r.text, text::r(Locale::Zh, "r.not_on_builtin", &[("name", "ask")]));
        me.role = Role::Worker;
        me.builtin_crew = false;
        let r = call(&f.state, &me, "rename_crew", json!({"name": "x"})).await;
        assert_eq!(r.text, text::r(Locale::Zh, "r.captain_only", &[("name", "rename_crew")]));
    }

    #[test]
    fn unknown_argument_is_named() {
        // Review Focus 5（参数半边）：未知字段整条拒、说清是哪个。
        #[derive(serde::Deserialize, Debug)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct A {
            text: String,
        }
        let e = decode::<A>(json!({"text": "x", "colour": 1})).unwrap_err();
        assert!(e.contains("colour"), "{e}");
        assert!(decode::<A>(json!(null)).unwrap_err().contains("text"), "null 当空对象");
    }
}
```

- [ ] **Step 2: 跑一遍确认失败**

Run: `cargo test -p crewd-server --lib agent::rpc agent::tools`
Expected: 编译失败（`cannot find RpcClient`、`unresolved module rpc`）。

- [ ] **Step 3: 写 `tools/mod.rs`**

```rust
//! 工具分发：查表 → 现查角色把关 → 解析参数 → 交给各组的实现。回执文字一律取 tools.*.md 的 `r.*`。
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::AppState;
use crate::agent::catalog::{self, Scope};
use crate::agent::port::{self, PortError};
use crate::agent::text::{self, Locale};
use crate::agent::{Identity, Role};
use crewd_proto::agent::CallResult;

pub struct Ctx<'a> {
    pub st: &'a AppState,
    pub me: &'a Identity,
    pub loc: Locale,
}

impl Ctx<'_> {
    pub fn r(&self, key: &str, vars: &[(&str, &str)]) -> String {
        text::r(self.loc, key, vars)
    }
    pub fn ok(&self, key: &str, vars: &[(&str, &str)]) -> CallResult {
        CallResult::ok(self.r(key, vars))
    }
    pub fn err(&self, key: &str, vars: &[(&str, &str)]) -> CallResult {
        CallResult::err(self.r(key, vars))
    }
    /// M3a 拒绝 / 出错：把它的说明原样带给 agent（它们是给人看的规则说明，例如「不是群成员」）。
    pub fn fail(&self, e: PortError) -> CallResult {
        self.err("r.failed", &[("detail", &e.to_string())])
    }
}

/// `null` 当空对象；未知字段 / 缺字段 / 类型不对 → 返回 serde 的说明（含字段名）。
pub fn decode<T: DeserializeOwned>(v: Value) -> Result<T, String> {
    let v = if v.is_null() { Value::Object(Default::default()) } else { v };
    serde_json::from_value(v).map_err(|e| e.to_string())
}

pub fn parse_args<T: DeserializeOwned>(cx: &Ctx<'_>, v: Value) -> Result<T, CallResult> {
    decode(v).map_err(|d| cx.err("r.bad_args", &[("detail", &d)]))
}

pub async fn call(st: &AppState, me: &Identity, tool: &str, args: Value) -> CallResult {
    let cx = Ctx { st, me, loc: port::locale(st).await.unwrap_or_default() };
    let Some(spec) = catalog::find(tool) else {
        return cx.err("r.unknown_tool", &[("name", tool)]);
    };
    if spec.scope == Scope::Captain && me.role != Role::Captain {
        return cx.err("r.captain_only", &[("name", tool)]);
    }
    if spec.not_on_builtin && me.builtin_crew {
        return cx.err("r.not_on_builtin", &[("name", tool)]);
    }
    // Task 12 把下面两行换成 `match tool { … _ => cx.err("r.no_handler", …) }`，Task 12–15 逐组加分支。
    let _ = args;
    cx.err("r.no_handler", &[("name", tool)])
}
```

`r.no_handler` 只在开发途中出现（Task 15 的 `every_catalog_tool_has_a_handler` 保证最后一个都不剩）。往两份 md 末尾加：

```markdown
## r.failed
没成：{detail}

## r.no_handler
{name} 还没有实现。
```

```markdown
## r.failed
Did not go through: {detail}

## r.no_handler
{name} is not implemented yet.
```

- [ ] **Step 4: 写 `rpc.rs`**

```rust
//! agent RPC（spec §22.3「agent RPC」）。每条连接一份 `AgentConn`：attach 成功后才能调其余方法；
//! 角色每次现查；角色变了推 `agent.tools_changed`（helper 转成 MCP 的 list_changed）。
use std::sync::Arc;

use crewd_proto::agent::{
    AttachParams, AttachResult, CallParams, HookKind, HookParams, HookResult, TOOLS_CHANGED, methods,
};
use crewd_collab::Change;
use crewd_proto::{Event, Request, Response, RpcError, ServerFrame, parse_params};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::AppState;
use crate::agent::{Identity, Role, catalog, port, tools};

#[derive(Default)]
pub struct AgentConn {
    session: Option<String>,
    watcher: Option<JoinHandle<()>>,
}

impl Drop for AgentConn {
    fn drop(&mut self) {
        if let Some(w) = self.watcher.take() {
            w.abort();
        }
    }
}

/// token / session 对不上：-32003（M3a 的 FORBIDDEN）。
fn invalid(msg: &str) -> RpcError {
    RpcError::new(RpcError::FORBIDDEN, msg)
}

fn to_value<T: serde::Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(|e| RpcError::new(RpcError::INTERNAL, e.to_string()))
}

pub async fn handle(
    state: &Arc<AppState>,
    conn: &mut AgentConn,
    req: Request,
    tx: &mpsc::Sender<ServerFrame>,
) -> Response {
    let id = req.id;
    match route(state, conn, req, tx).await {
        Ok(v) => Response::ok(id, v),
        Err(e) => Response::err(id, e),
    }
}

async fn current(state: &AppState, session: &str) -> Result<Identity, RpcError> {
    port::identity(state, session)
        .await
        .map_err(|e| RpcError::new(RpcError::INTERNAL, e.to_string()))?
        .ok_or_else(|| RpcError::new(RpcError::INTERNAL, "session no longer exists"))
}

async fn route(
    state: &Arc<AppState>,
    conn: &mut AgentConn,
    req: Request,
    tx: &mpsc::Sender<ServerFrame>,
) -> Result<Value, RpcError> {
    if req.method == methods::ATTACH {
        let p: AttachParams = parse_params(req.params)?;
        if !port::verify_token(state, &p.session_id, &p.token) {
            return Err(invalid("invalid session or token"));
        }
        // 先订阅再读身份：两者之间发生的换机长也不会漏推。
        let rx = port::subscribe_collab(state);
        let me = port::identity(state, &p.session_id)
            .await
            .map_err(|e| RpcError::new(RpcError::INTERNAL, e.to_string()))?
            .ok_or_else(|| invalid("invalid session or token"))?;
        if let Some(old) = conn.watcher.take() {
            old.abort();
        }
        conn.watcher = Some(spawn_watcher(state.clone(), me.session_id.clone(), me.role, rx, tx.clone()));
        conn.session = Some(me.session_id.clone());
        return to_value(AttachResult { role: me.role, crew_id: me.crew_id, label: me.label });
    }
    let Some(session) = conn.session.clone() else {
        return Err(RpcError::new(RpcError::METHOD_NOT_FOUND, "agent.* requires agent.attach first"));
    };
    match req.method.as_str() {
        methods::TOOLS => {
            let me = current(state, &session).await?;
            let loc = port::locale(state).await.unwrap_or_default();
            to_value(catalog::tools_for(&me, loc))
        }
        methods::CALL => {
            let p: CallParams = parse_params(req.params)?;
            let me = current(state, &session).await?;
            to_value(tools::call(state, &me, &p.tool, p.args).await)
        }
        methods::HOOK => {
            let p: HookParams = parse_params(req.params)?;
            let me = current(state, &session).await?;
            to_value(hook(state, &me, p.event, p.payload).await)
        }
        other => Err(RpcError::new(RpcError::METHOD_NOT_FOUND, format!("unknown method: {other}"))),
    }
}

/// Task 9–11 逐个接上。
async fn hook(_state: &Arc<AppState>, _me: &Identity, event: HookKind, _payload: Value) -> HookResult {
    match event {
        HookKind::PostToolUse | HookKind::PreToolUse | HookKind::Stop => HookResult { stdout: None },
    }
}

/// 角色只会因为 session 行或机组行变化而变（换机长 = `crews::set_captain`，发 CrewChanged / SessionChanged）。
fn relevant(c: &Change) -> bool {
    matches!(c, Change::SessionChanged { .. } | Change::CrewChanged { .. })
}

fn spawn_watcher(
    state: Arc<AppState>,
    session: String,
    mut last: Role,
    mut rx: broadcast::Receiver<Change>,
    tx: mpsc::Sender<ServerFrame>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(c) if relevant(&c) => {}
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
            let Ok(Some(me)) = port::identity(&state, &session).await else { continue };
            if me.role != last {
                last = me.role;
                let frame = ServerFrame::Event(Event {
                    event: TOOLS_CHANGED.to_string(),
                    data: json!({ "role": me.role }),
                });
                if tx.send(frame).await.is_err() {
                    break;
                }
            }
        }
    })
}
```

- [ ] **Step 5: 接 `conn.rs`**

`read_loop` 的 `state` 参数从 `&AppState` 改成 `&Arc<AppState>`（`run` 里传 `&state`）；循环前加 `let mut agent = crate::agent::rpc::AgentConn::default();`；`match serde_json::from_str::<Request>(&line)` 里在通用分支前加一支：

```rust
            Ok(req) if req.method.starts_with("agent.") => ServerFrame::Response(
                crate::agent::rpc::handle(state, &mut agent, req, tx).await,
            ),
```

（M3a 改过 `conn.rs` 的话，原则不变：同一条连接上的 agent 请求共享一个 `AgentConn`，连接结束时它被 drop。）

- [ ] **Step 6: testkit 加 `RpcClient` 与 `set_role`**

```rust
/// 测试用的 NDJSON 客户端：一条连接，回应按 id 取，事件先攒着。
pub struct RpcClient {
    rd: tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    wr: tokio::net::unix::OwnedWriteHalf,
    next: u64,
    events: std::collections::VecDeque<crewd_proto::Event>,
}

impl RpcClient {
    pub async fn connect(sock: &Path) -> Self {
        let (rd, wr) = tokio::net::UnixStream::connect(sock).await.unwrap().into_split();
        Self { rd: tokio::io::BufReader::new(rd), wr, next: 1, events: Default::default() }
    }

    async fn frame(&mut self) -> crewd_proto::ServerFrame {
        use tokio::io::AsyncBufReadExt;
        let mut line = String::new();
        let n = self.rd.read_line(&mut line).await.unwrap();
        assert!(n > 0, "crewd 关了连接");
        serde_json::from_str(&line).unwrap()
    }

    pub async fn call(&mut self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, (i32, String)> {
        use tokio::io::AsyncWriteExt;
        let id = self.next;
        self.next += 1;
        let mut line = serde_json::to_vec(&crewd_proto::Request { id, method: method.into(), params }).unwrap();
        line.push(b'\n');
        self.wr.write_all(&line).await.unwrap();
        loop {
            match self.frame().await {
                crewd_proto::ServerFrame::Event(e) => self.events.push_back(e),
                crewd_proto::ServerFrame::Response(r) if r.id == id => {
                    return match r.error {
                        Some(e) => Err((e.code, e.message)),
                        None => Ok(r.result.unwrap_or(serde_json::Value::Null)),
                    };
                }
                crewd_proto::ServerFrame::Response(_) => {}
            }
        }
    }

    pub async fn next_event(&mut self, within: std::time::Duration) -> Option<crewd_proto::Event> {
        if let Some(e) = self.events.pop_front() {
            return Some(e);
        }
        match tokio::time::timeout(within, self.frame()).await {
            Ok(crewd_proto::ServerFrame::Event(e)) => Some(e),
            _ => None,
        }
    }
}
```

`Fixture` 加：

```rust
    /// 换机长（M3a `crews::set_captain`，经 `AppState::collab` 发 CrewChanged / SessionChanged）。
    /// `Role::Worker` = 把机长还给夹具原来的机长。
    pub async fn set_role(&self, session: &str, role: Role) {
        let crew = self.me(session).await.crew_id;
        let to = if role == Role::Captain { session.to_string() } else { self.captain.clone() };
        self.state
            .collab(move |tx| crewd_collab::crews::set_captain(tx, &crew, &to))
            .await
            .unwrap();
    }
```

- [ ] **Step 7: 跑通**

Run: `cargo test -p crewd-server --lib agent::rpc agent::tools`
Expected: rpc `5 passed`、tools `2 passed`。

- [ ] **Step 8: 变异核对**

`spawn_watcher` 里把 `if me.role != last` 换成 `if false`：Expected `role_change_pushes_tools_changed_and_takes_effect_without_restart` 红（等不到事件）。`route` 的 `methods::CALL` 分支里把 `current(state, &session)` 换成 attach 时记下的身份（例如在 `AgentConn` 里存一份 `Identity`）：Expected 同一条测试的「卸任后当场被拒」红。逐条撤销。

- [ ] **Step 9: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/server/src/agent/rpc.rs crates/server/src/agent/tools/mod.rs crates/server/src/agent/mod.rs \
  crates/server/src/agent/testkit.rs crates/server/src/conn.rs \
  crates/collab/prompts/tools.zh.md crates/collab/prompts/tools.en.md
git diff --cached --stat
git commit -m "crewd-server: agent RPC (attach with token, role-checked tools/call, tools_changed push)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 8: helper 本体：`crewd mcp`（rmcp）与 `crewd hook`（转发 + 失败时的保守动作）

**Files:**
- Create: `crewd/crates/crewd/src/agent_client.rs`、`crewd/crates/crewd/src/mcp_cmd.rs`、`crewd/crates/crewd/src/hook_cmd.rs`、`crewd/crates/crewd/tests/helper_cli.rs`
- Modify: `crewd/crates/crewd/src/main.rs`（两个子命令）、`crewd/crates/crewd/Cargo.toml`、`crewd/Cargo.lock`

**Interfaces:**
- Consumes: Task 1 `crewd_proto::agent::*`；Task 2 的命令行形状（`mcp|hook <event> --data-dir D --session S --token-file F [--deadline-secs N]`；`--data-dir` 是 `Cli` 已有的全局参数）。
- Produces:
  - `agent_client::AgentClient::{connect(data_dir: &Path, session: &str, token_file: &Path) -> anyhow::Result<(AgentClient, AttachResult)>, call::<T>(&self, method: &str, params: Value) -> anyhow::Result<T>, take_events(&self) -> Option<mpsc::UnboundedReceiver<Event>>}`
  - `crewd mcp`：stdio MCP server；`tools/list` → `agent.tools`；`tools/call` → `agent.call`；`agent.tools_changed` → `notifications/tools/list_changed`
  - `crewd hook <post-tool-use|pre-tool-use|stop>`：stdin JSON → `agent.hook` → 把 `stdout` 原样打印；**永远 exit 0**；post-tool-use / stop 总期限 5s，失败什么都不打；pre-tool-use 失败或到 `--deadline-secs` → 打印 deny

- [ ] **Step 1: 加依赖**

```bash
cargo add -p crewd rmcp@3.5.1 --no-default-features --features server,transport-io
cargo add -p crewd tokio --features rt-multi-thread,macros,signal,io-std,io-util,net,sync,time
cargo add -p crewd serde --features derive
git diff Cargo.lock | grep '^+name' 
```

（rmcp 不在本机 cargo 缓存里：这一步要联网下载 crates.io 的包，不能 `--offline`；只从 crates.io 官方源取，不换镜像。）

Expected：`crates/crewd/Cargo.toml` 多 `rmcp = { version = "3.5.1", default-features = false, features = ["server", "transport-io"] }`；Cargo.lock 新增条目是 rmcp 与它的依赖（futures 系、tokio-util、tracing、thiserror、indexmap、pin-project-lite、pastey、rmcp 不带宏时不应出现 `rmcp-macros`……）。把 `+name` 的清单原样贴进提交信息。出现 reqwest / hyper / oauth2 之类 = 特性没关干净，停下查。

- [ ] **Step 2: 写失败的测试**（`crates/crewd/tests/helper_cli.rs`；假 serve 写在测试里，不起真 serve、不起 claude）

```rust
//! `crewd mcp` / `crewd hook` 对着一个假的 crewd.sock：验转发、推送、失败时的保守动作。
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::process::Command;
use tokio::sync::mpsc;

const SESSION: &str = "s-1";
const TOKEN: &str = "tok-示例";

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    /// attach 之后什么都不回（crewd 卡住）。
    Hang,
}

struct FakeServe {
    dir: tempfile::TempDir,
    push: mpsc::UnboundedSender<Value>,
}

impl FakeServe {
    async fn start(mode: Mode) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("token"), format!("{TOKEN}\n")).unwrap();
        let listener = UnixListener::bind(dir.path().join("crewd.sock")).unwrap();
        let (push, mut push_rx) = mpsc::unbounded_channel::<Value>();
        tokio::spawn(async move {
            // 只服务一条「推送用」的连接：最先 attach 的那条。
            let mut push_tx: Option<mpsc::UnboundedSender<Value>> = None;
            loop {
                tokio::select! {
                    Some(ev) = push_rx.recv() => {
                        if let Some(t) = &push_tx { let _ = t.send(ev); }
                    }
                    Ok((stream, _)) = listener.accept() => {
                        let (rd, mut wr) = stream.into_split();
                        let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
                        if push_tx.is_none() { push_tx = Some(tx.clone()); }
                        tokio::spawn(async move {
                            while let Some(v) = rx.recv().await {
                                let mut l = serde_json::to_vec(&v).unwrap();
                                l.push(b'\n');
                                if wr.write_all(&l).await.is_err() { break; }
                            }
                        });
                        tokio::spawn(async move {
                            let mut lines = BufReader::new(rd).lines();
                            while let Ok(Some(line)) = lines.next_line().await {
                                let req: Value = serde_json::from_str(&line).unwrap();
                                let id = req["id"].clone();
                                let p = &req["params"];
                                let reply = match req["method"].as_str().unwrap() {
                                    "agent.attach" if p["sessionId"] == SESSION && p["token"] == TOKEN =>
                                        json!({"id": id, "result": {"role": "worker", "crewId": "crew-a", "label": "示例"}}),
                                    "agent.attach" => json!({"id": id, "error": {"code": -32602, "message": "invalid session or token"}}),
                                    _ if mode == Mode::Hang => continue,
                                    "agent.tools" => json!({"id": id, "result": [
                                        {"name": "post", "description": "发群", "inputSchema": {"type": "object", "properties": {"message": {"type": "string"}}}},
                                        {"name": "ask", "description": "问人", "inputSchema": {"type": "object"}}
                                    ]}),
                                    "agent.call" => json!({"id": id, "result": {
                                        "text": format!("called {} with {}", p["tool"].as_str().unwrap(), p["args"]),
                                        "isError": p["tool"] == "ask"
                                    }}),
                                    "agent.hook" => match p["event"].as_str().unwrap() {
                                        "post-tool-use" => json!({"id": id, "result": {"stdout": "{\"hookSpecificOutput\":{\"hookEventName\":\"PostToolUse\",\"additionalContext\":\"示例未读\"}}"}}),
                                        "pre-tool-use" => json!({"id": id, "result": {"stdout": format!("{{\"echo\":{}}}", p["payload"]["tool_name"])}}),
                                        _ => json!({"id": id, "result": {}}),
                                    },
                                    m => json!({"id": id, "error": {"code": -32601, "message": m}}),
                                };
                                if tx.send(reply).is_err() { break; }
                            }
                        });
                    }
                }
            }
        });
        Self { dir, push }
    }

    fn data_dir(&self) -> &Path {
        self.dir.path()
    }

    fn token_file(&self) -> PathBuf {
        self.dir.path().join("token")
    }
}

fn crewd(data_dir: &Path, sub: &[&str], token_file: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_crewd"));
    c.args(sub)
        .arg("--data-dir")
        .arg(data_dir)
        .args(["--session", SESSION, "--token-file"])
        .arg(token_file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    c
}

async fn run_hook(data_dir: &Path, event: &str, extra: &[&str], token_file: &Path, stdin: &str) -> (String, String, Duration) {
    let started = Instant::now();
    let mut args = vec!["hook", event];
    args.extend_from_slice(extra);
    let mut child = crewd(data_dir, &args, token_file).spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).await.unwrap();
    let out = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .expect("hook 不该挂住")
        .unwrap();
    assert!(out.status.success(), "hook 永远 exit 0");
    (
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
        started.elapsed(),
    )
}

struct Mcp {
    _child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    lines: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
}

impl Mcp {
    async fn start(f: &FakeServe) -> Self {
        let mut child = crewd(f.data_dir(), &["mcp"], &f.token_file()).spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        Self { _child: child, stdin, lines }
    }
    async fn send(&mut self, v: Value) {
        let mut l = serde_json::to_vec(&v).unwrap();
        l.push(b'\n');
        self.stdin.write_all(&l).await.unwrap();
    }
    async fn recv(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(10), self.lines.next_line())
            .await
            .expect("crewd mcp 没回")
            .unwrap()
            .expect("crewd mcp 关了 stdout");
        serde_json::from_str(&line).unwrap()
    }
    async fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).await;
        loop {
            let v = self.recv().await;
            if v["id"] == id {
                return v;
            }
        }
    }
    async fn handshake(&mut self) -> Value {
        let init = self
            .request(1, "initialize", json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "helper-cli-test", "version": "0"}
            }))
            .await;
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
        init
    }
}

#[tokio::test]
async fn mcp_bridges_list_and_call_and_errors() {
    let f = FakeServe::start(Mode::Normal).await;
    let mut m = Mcp::start(&f).await;
    let init = m.handshake().await;
    assert_eq!(init["result"]["serverInfo"]["name"], "crewd");
    assert_eq!(init["result"]["capabilities"]["tools"]["listChanged"], true);
    let list = m.request(2, "tools/list", json!({})).await;
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0]["name"], "post");
    assert_eq!(tools[0]["description"], "发群");
    assert_eq!(tools[0]["inputSchema"]["properties"]["message"]["type"], "string");
    let call = m
        .request(3, "tools/call", json!({"name": "post", "arguments": {"message": "你好"}}))
        .await;
    assert_eq!(call["result"]["content"][0]["text"], "called post with {\"message\":\"你好\"}");
    assert_ne!(call["result"]["isError"], true);
    let err = m.request(4, "tools/call", json!({"name": "ask", "arguments": {}})).await;
    assert_eq!(err["result"]["isError"], true, "serve 的 isError 原样带出");
}

#[tokio::test]
async fn tools_changed_push_becomes_list_changed_notification() {
    let f = FakeServe::start(Mode::Normal).await;
    let mut m = Mcp::start(&f).await;
    m.handshake().await;
    f.push.send(json!({"event": "agent.tools_changed", "data": {"role": "captain"}})).unwrap();
    loop {
        let v = m.recv().await;
        if v["method"] == "notifications/tools/list_changed" {
            break;
        }
    }
}

#[tokio::test]
async fn mcp_refuses_to_start_with_a_wrong_token() {
    let f = FakeServe::start(Mode::Normal).await;
    std::fs::write(f.token_file(), "forged").unwrap();
    let out = tokio::time::timeout(
        Duration::from_secs(15),
        crewd(f.data_dir(), &["mcp"], &f.token_file()).output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid session or token"));
}

#[tokio::test]
async fn hook_prints_serve_stdout_verbatim() {
    let f = FakeServe::start(Mode::Normal).await;
    let (out, _, _) = run_hook(f.data_dir(), "post-tool-use", &[], &f.token_file(), "{\"tool_name\":\"Bash\"}").await;
    assert_eq!(out.trim_end(), "{\"hookSpecificOutput\":{\"hookEventName\":\"PostToolUse\",\"additionalContext\":\"示例未读\"}}");
    let (out, _, _) = run_hook(f.data_dir(), "stop", &[], &f.token_file(), "{}").await;
    assert_eq!(out, "", "serve 没给 stdout 就什么都不打");
    let (out, _, _) = run_hook(f.data_dir(), "pre-tool-use", &["--deadline-secs", "5"], &f.token_file(), "{\"tool_name\":\"mcp__computer-use__click\"}").await;
    assert_eq!(out.trim_end(), "{\"echo\":\"mcp__computer-use__click\"}");
}

#[tokio::test]
async fn hook_without_crewd_is_silent_except_pre_tool_use_which_denies() {
    let empty = tempfile::tempdir().unwrap();
    let tf = empty.path().join("token");
    std::fs::write(&tf, TOKEN).unwrap();
    for ev in ["post-tool-use", "stop"] {
        let (out, err, took) = run_hook(empty.path(), ev, &[], &tf, "{}").await;
        assert_eq!(out, "", "{ev}");
        assert!(!err.is_empty(), "{ev}: 失败要写 stderr");
        assert!(took < Duration::from_secs(6), "{ev}: {took:?}");
    }
    let (out, _, _) = run_hook(empty.path(), "pre-tool-use", &["--deadline-secs", "5"], &tf, "{\"tool_name\":\"x\"}").await;
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
}

#[tokio::test]
async fn hook_gives_up_quickly_when_crewd_hangs() {
    // Review Focus 1
    let f = FakeServe::start(Mode::Hang).await;
    let (out, _, took) = run_hook(f.data_dir(), "post-tool-use", &[], &f.token_file(), "{}").await;
    assert_eq!(out, "");
    assert!(took < Duration::from_secs(7), "{took:?}");
    let (out, _, took) = run_hook(f.data_dir(), "pre-tool-use", &["--deadline-secs", "1"], &f.token_file(), "{}").await;
    assert!(out.contains("\"deny\""), "到 helper 自己的期限就 deny：{out}");
    assert!(took < Duration::from_secs(5), "{took:?}");
}

#[tokio::test]
async fn bad_stdin_or_unknown_event_never_prints_garbage() {
    let f = FakeServe::start(Mode::Normal).await;
    let (out, err, _) = run_hook(f.data_dir(), "post-tool-use", &[], &f.token_file(), "not json").await;
    assert_eq!(out, "");
    assert!(!err.is_empty());
    let (out, _, _) = run_hook(f.data_dir(), "pre-tool-use", &["--deadline-secs", "5"], &f.token_file(), "not json").await;
    assert!(out.contains("\"deny\""), "看不懂的 PreToolUse 输入：保守 deny");
    let (out, err, _) = run_hook(f.data_dir(), "user-prompt-submit", &[], &f.token_file(), "{}").await;
    assert_eq!(out, "");
    assert!(err.contains("user-prompt-submit"));
}
```

`crates/crewd/Cargo.toml` 的 `[dev-dependencies]` 已有 `tempfile`；`tokio` 的 `process` 特性测试要用：`cargo add -p crewd --dev tokio --features process,macros,rt-multi-thread,net,io-util,time,sync`。

- [ ] **Step 3: 跑一遍确认失败**

Run: `cargo test -p crewd --test helper_cli`
Expected: 全红（`crewd` 没有 `mcp` / `hook` 子命令，clap 报 `unrecognized subcommand`，断言失败）。

- [ ] **Step 4: 写 `agent_client.rs`**

```rust
//! helper 侧的 crewd 连接：连 `<data dir>/crewd.sock`、`agent.attach`，之后一条连接上并发多个请求
//! （rmcp 可能同时 list 与 call），回应按 id 分发，`agent.tools_changed` 之类的推送进一个通道。
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, anyhow, bail};
use crewd_proto::agent::{AttachParams, AttachResult, methods};
use crewd_proto::{Event, Request, Response, ServerFrame};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::{Mutex, mpsc, oneshot};

type Pending = Arc<std::sync::Mutex<Option<HashMap<u64, oneshot::Sender<Response>>>>>;

pub struct AgentClient {
    wr: Mutex<OwnedWriteHalf>,
    pending: Pending,
    next: AtomicU64,
    events: std::sync::Mutex<Option<mpsc::UnboundedReceiver<Event>>>,
}

impl AgentClient {
    pub async fn connect(
        data_dir: &Path,
        session: &str,
        token_file: &Path,
    ) -> anyhow::Result<(AgentClient, AttachResult)> {
        let token = std::fs::read_to_string(token_file)
            .with_context(|| format!("cannot read token file {}", token_file.display()))?
            .trim()
            .to_string();
        let sock = data_dir.join("crewd.sock");
        let stream = UnixStream::connect(&sock)
            .await
            .with_context(|| format!("cannot connect to {}", sock.display()))?;
        let (rd, wr) = stream.into_split();
        let pending: Pending = Arc::new(std::sync::Mutex::new(Some(HashMap::new())));
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let p2 = pending.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(rd).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match serde_json::from_str::<ServerFrame>(&line) {
                    Ok(ServerFrame::Response(r)) => {
                        let tx = p2.lock().unwrap().as_mut().and_then(|m| m.remove(&r.id));
                        if let Some(tx) = tx {
                            let _ = tx.send(r);
                        }
                    }
                    Ok(ServerFrame::Event(e)) => {
                        let _ = ev_tx.send(e);
                    }
                    Err(e) => eprintln!("crewd agent client: bad frame from crewd: {e}"),
                }
            }
            // 断开：清空等待中的请求（它们会收到「连接关了」），之后的请求直接失败。
            p2.lock().unwrap().take();
        });
        let client = AgentClient {
            wr: Mutex::new(wr),
            pending,
            next: AtomicU64::new(1),
            events: std::sync::Mutex::new(Some(ev_rx)),
        };
        let who: AttachResult = client
            .call(
                methods::ATTACH,
                serde_json::to_value(AttachParams { session_id: session.to_string(), token })?,
            )
            .await?;
        Ok((client, who))
    }

    pub async fn call<T: DeserializeOwned>(&self, method: &str, params: Value) -> anyhow::Result<T> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        match self.pending.lock().unwrap().as_mut() {
            Some(m) => {
                m.insert(id, tx);
            }
            None => bail!("connection to crewd is closed"),
        }
        let mut line = serde_json::to_vec(&Request { id, method: method.to_string(), params })?;
        line.push(b'\n');
        self.wr.lock().await.write_all(&line).await.context("write to crewd")?;
        let resp = rx.await.map_err(|_| anyhow!("connection to crewd is closed"))?;
        if let Some(e) = resp.error {
            bail!("{} ({})", e.message, e.code);
        }
        Ok(serde_json::from_value(resp.result.unwrap_or(Value::Null))?)
    }

    pub fn take_events(&self) -> Option<mpsc::UnboundedReceiver<Event>> {
        self.events.lock().unwrap().take()
    }
}
```

- [ ] **Step 5: 写 `mcp_cmd.rs`**

```rust
//! `crewd mcp`：每个 agent session 的 stdio MCP server（spec §22.3）。哑管道：工具表与执行都在 serve。
//! 用 rmcp 3.5 手写 `ServerHandler`（不用 `#[tool]` 宏：工具表来自 crewd，按角色变）。
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crewd_proto::agent::{CallResult, TOOLS_CHANGED, ToolDef, methods};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData, Implementation,
    InitializeResult, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, ServiceExt};
use serde_json::{Value, json};

use crate::agent_client::AgentClient;

#[derive(clap::Args, Clone)]
pub struct McpArgs {
    /// crewd session id
    #[arg(long)]
    session: String,
    /// `agent.attach` 用的 token 所在文件（0600，crewd 起会话时写）
    #[arg(long)]
    token_file: PathBuf,
}

#[derive(Clone)]
struct Bridge {
    client: Arc<AgentClient>,
}

fn to_tool(d: ToolDef) -> Tool {
    let schema = match d.input_schema {
        Value::Object(m) => m,
        _ => Default::default(),
    };
    Tool::new(d.name, d.description, Arc::new(schema))
}

impl ServerHandler for Bridge {
    fn get_info(&self) -> ServerConfig {
        InitializeResult::new(
            ServerCapabilities::builder().enable_tools().enable_tool_list_changed().build(),
        )
        .with_server_info(Implementation::new("crewd", env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let defs: Vec<ToolDef> = self
            .client
            .call(methods::TOOLS, Value::Null)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(ListToolsResult::with_all_items(defs.into_iter().map(to_tool).collect()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = request.arguments.map(Value::Object).unwrap_or(Value::Null);
        let r = match self
            .client
            .call::<CallResult>(methods::CALL, json!({"tool": request.name.to_string(), "args": args}))
            .await
        {
            Ok(r) => r,
            Err(e) => CallResult::err(format!("crewd is not reachable ({e}); this call did nothing")),
        };
        let content = vec![ContentBlock::text(r.text)];
        let result = if r.is_error {
            CallToolResult::error(content)
        } else {
            CallToolResult::success(content)
        };
        Ok(result.into())
    }
}

pub async fn run(data_dir: &Path, args: McpArgs) -> anyhow::Result<()> {
    let (client, who) = AgentClient::connect(data_dir, &args.session, &args.token_file).await?;
    eprintln!("crewd mcp: attached {} ({:?}) in {}", args.session, who.role, who.crew_id);
    let client = Arc::new(client);
    let mut events = client.take_events().expect("events taken once");
    let service = Bridge { client }.serve(rmcp::transport::stdio()).await?;
    let peer = service.peer().clone();
    tokio::spawn(async move {
        while let Some(e) = events.recv().await {
            if e.event == TOOLS_CHANGED {
                if let Err(err) = peer.notify_tool_list_changed().await {
                    eprintln!("crewd mcp: list_changed not delivered: {err}");
                }
            }
        }
    });
    service.waiting().await?;
    Ok(())
}
```

（编译不过时先对 docs.rs 3.5.1：`ServerConfig` / `CallToolResponse` / `ContentBlock` 这几个名字是查过的；`Tool::new` 的第三个参数要 `Arc<JsonObject>`，`JsonObject = serde_json::Map<String, Value>`。README 里旧的 `CallToolResult` 返回类型不适用于 3.5.1。）

- [ ] **Step 6: 写 `hook_cmd.rs`**

```rust
//! `crewd hook <event>`：claude 的 hook（`sh -c` 起）。读 stdin JSON → `agent.hook` → 原样打印 stdout。
//! 永远 exit 0；绝不打印坏 JSON。连不上 / 超时：post-tool-use、stop 什么都不打（宁可漏一次）；
//! pre-tool-use 打 deny（它只在命中审批闸时才被调用，见 crewd-exec `gate_matcher`）。
use std::path::{Path, PathBuf};
use std::time::Duration;

use crewd_proto::agent::{HookKind, HookResult, methods};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use crate::agent_client::AgentClient;

/// post-tool-use / stop 的总期限（连 + attach + 调用）。claude 侧的超时是 15s（crewd-exec）。
const QUICK: Duration = Duration::from_secs(5);

#[derive(clap::Args, Clone)]
pub struct HookArgs {
    /// post-tool-use | pre-tool-use | stop
    event: String,
    #[arg(long)]
    session: String,
    #[arg(long)]
    token_file: PathBuf,
    /// pre-tool-use：到这个秒数还没结果就 deny（比 claude 的 hook 超时短）
    #[arg(long, default_value_t = 1830)]
    deadline_secs: u64,
}

fn deny(reason: &str) -> String {
    json!({"hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "deny",
        "permissionDecisionReason": reason,
    }})
    .to_string()
}

pub async fn run(data_dir: &Path, args: HookArgs) -> anyhow::Result<()> {
    let Some(kind) = HookKind::parse(&args.event) else {
        eprintln!("crewd hook: unknown event {:?}, ignored", args.event);
        return Ok(());
    };
    let mut raw = Vec::new();
    if let Err(e) = tokio::io::stdin().read_to_end(&mut raw).await {
        eprintln!("crewd hook: cannot read stdin: {e}");
    }
    let payload: Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("crewd hook {}: stdin is not JSON: {e}", kind.as_str());
            if kind == HookKind::PreToolUse {
                println!("{}", deny("crewd could not read this tool call; denied to be safe"));
            }
            return Ok(());
        }
    };
    let deadline = match kind {
        HookKind::PreToolUse => Duration::from_secs(args.deadline_secs),
        _ => QUICK,
    };
    let work = async {
        let (client, _) = AgentClient::connect(data_dir, &args.session, &args.token_file).await?;
        client
            .call::<HookResult>(methods::HOOK, json!({"event": kind, "payload": payload}))
            .await
    };
    match tokio::time::timeout(deadline, work).await {
        Ok(Ok(HookResult { stdout: Some(s) })) => println!("{s}"),
        Ok(Ok(HookResult { stdout: None })) => {}
        Ok(Err(e)) => {
            eprintln!("crewd hook {}: {e:#}", kind.as_str());
            if kind == HookKind::PreToolUse {
                println!("{}", deny("crewd is not reachable; this gated tool needs approval, denied"));
            }
        }
        Err(_) => {
            eprintln!("crewd hook {}: no answer within {deadline:?}", kind.as_str());
            if kind == HookKind::PreToolUse {
                println!("{}", deny("no approval decision in time; denied"));
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 7: 接 `main.rs`**

`mod agent_client; mod hook_cmd; mod mcp_cmd;`；`enum Cmd` 加：

```rust
    /// agent session 的 stdio MCP server（claude / codex 经 --mcp-config 拉起）
    Mcp(mcp_cmd::McpArgs),
    /// claude hook：读 stdin JSON，转给 crewd，打印要给 claude 的输出（永远 exit 0）
    Hook(hook_cmd::HookArgs),
```

`run()` 的 match 加：

```rust
        Cmd::Mcp(args) => mcp_cmd::run(&dir, args.clone()).await?,
        Cmd::Hook(args) => hook_cmd::run(&dir, args.clone()).await?,
```

（`hook` 的 `run` 只在 bug 时返回 Err；`main` 会 exit 1——那也不挡 claude：非 0 非 2 的退出码是非阻塞错误。）

- [ ] **Step 8: 跑通**

Run: `cargo test -p crewd --test helper_cli`
Expected: `7 passed`。

- [ ] **Step 9: 变异核对**

`hook_cmd.rs` 的 `Ok(Err(e))` 分支里删掉 `if kind == HookKind::PreToolUse { println!(…deny…) }`：Expected `hook_without_crewd_is_silent_except_pre_tool_use_which_denies` 红。`mcp_cmd.rs` 里删掉 `notify_tool_list_changed` 那个转发任务：Expected `tools_changed_push_becomes_list_changed_notification` 红（10s 内等不到）。逐条撤销。

- [ ] **Step 10: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/crewd/src/agent_client.rs crates/crewd/src/mcp_cmd.rs crates/crewd/src/hook_cmd.rs \
  crates/crewd/src/main.rs crates/crewd/tests/helper_cli.rs crates/crewd/Cargo.toml Cargo.lock
git diff --cached --stat
git commit -m "crewd: mcp (rmcp stdio bridge) and hook subcommands; hooks fail quiet, gated pre-tool-use fails closed

新增 lock 条目：<Step 1 的 +name 清单>

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 9: post-tool-use 注入（`agent::inject`）

**Files:**
- Create: `crewd/crates/server/src/agent/inject.rs`
- Modify: `agent/mod.rs`（`pub mod inject;`）、`agent/rpc.rs`（PostToolUse 分支）、`agent/port.rs`（`unread` / `blocks` / `injection` / `advance`）、`crates/collab/prompts/tools.{zh,en}.md`（`inject.omitted`、`r.lobby_label`、`r.human_label`）

**Interfaces:**
- Consumes: M3a `crewd_collab::cursors::{unread_for, advance}`（**可见性与「不是自己发的」已在 `unread_for` 里滤过**，移植自 `CrewWhiteboardVisibility.swift:48`，M3b 不另写一份）、`crewd_collab::render::{blocks_for, injection, ChannelBlock}`（「发给 X 的」标注与群标题由 M3a 的 `render.*.md` 出）；Task 6 `port`。
- Produces:
  - `inject::{Prepared, prepare(st: &AppState, me: &Identity) -> PortResult<Option<Prepared>>, commit(st, me, &Prepared) -> PortResult<()>, post_tool_use(st, me) -> HookResult}`；`Prepared { pub context: Option<String>, .. }`（M3c 给 codex 组 `additionalContext` 用 `prepare` / `commit`，「与 spec 的出入」5）
  - `tools::channel_label(cx, kind, name, crew_title) -> String`（大厅 / 人类群的 `name` 是空串，M3a 的约定；回执里一律经它显示）
  - port：`unread(st, session) -> PortResult<Vec<(String, Vec<Message>)>>`（`st.collab(|tx| cursors::unread_for(tx, s))`）、`blocks(st, session, Vec<(String, Vec<Message>)>) -> PortResult<Vec<ChannelBlock>>`（`st.db.call(|s| render::blocks_for(s.conn(), …))`）、`injection(loc, &[ChannelBlock]) -> String`、`advance(st, session, channel, seq)`

注入范围 = M3a `unread_for` 给的群（大厅 + 显式成员群 + 收听的群，每群最多 200 条）。M3b 只加一件事：**总字数上限** `inject_cap_chars`（官方 `additionalContext` 每字段 10,000 字符，超了只剩 2,000 预览）——超了从全局最旧的行开始省略，每个被省略过的群前面加一句 `inject.omitted`；被省略的照样推进游标（它们已「告知过」存在）。

- [ ] **Step 1: 写失败的测试**（`inject.rs` 末尾）

```rust
#[cfg(test)]
mod tests {
    use crewd_proto::{ChannelKind, Mention, MentionKind};
    use serde_json::Value;

    use super::*;
    use crate::agent::port;
    use crate::agent::runtime::AgentConfig;
    use crate::agent::testkit::Fixture;
    use crate::agent::Role;

    fn at(kind: MentionKind, target: Option<&str>) -> Mention {
        Mention { kind, target_id: target.map(str::to_string) }
    }

    async fn ctx_of(f: &Fixture, session: &str) -> Option<String> {
        let me = f.me(session).await;
        let out = post_tool_use(&f.state, &me).await.stdout?;
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        Some(v["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_string())
    }

    async fn lobby(f: &Fixture) -> String {
        port::lobby_of(&f.state, &f.crew).await.unwrap().id
    }

    #[tokio::test]
    async fn unread_is_injected_once() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        f.human_post(&l, "示例：今天先做 A", vec![]).await;
        let ctx = ctx_of(&f, &f.worker).await.expect("有未读");
        assert!(ctx.contains("示例：今天先做 A"), "{ctx}");
        assert!(ctx_of(&f, &f.worker).await.is_none(), "同一条至多注入一次");
    }

    #[tokio::test]
    async fn directed_to_others_is_hidden_and_widened_is_marked() {
        let f = Fixture::new().await;
        f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker").await;
        let l = lobby(&f).await;
        f.session_post(&f.captain, &l, "给 w2 的活", vec![at(MentionKind::Session, Some("s-w2"))]).await;
        assert!(ctx_of(&f, &f.worker).await.is_none(), "@ 了别人的不进（游标照样推过去）");
        f.session_post(&f.captain, &l, "全组看、w2 来做", vec![at(MentionKind::Broadcast, None), at(MentionKind::Session, Some("s-w2"))]).await;
        let ctx = ctx_of(&f, &f.worker).await.unwrap();
        assert!(ctx.contains("全组看、w2 来做") && ctx.contains("另一个 worker"), "旁观者看得出是给谁的：{ctx}");
        assert!(!ctx.contains("给 w2 的活"));
    }

    #[tokio::test]
    async fn human_mentions_are_visible_and_own_posts_are_not_echoed() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        f.session_post(&f.captain, &l, "@人 我做完了", vec![at(MentionKind::Human, None)]).await;
        assert!(ctx_of(&f, &f.worker).await.unwrap().contains("@人 我做完了"));
        assert!(ctx_of(&f, &f.captain).await.is_none(), "自己发的不回灌");
    }

    #[tokio::test]
    async fn only_member_or_listened_channels_are_injected() {
        let f = Fixture::new().await;
        let cap = f.me(&f.captain).await;
        let topic = port::create_channel(&f.state, &cap, ChannelKind::Topic, "示例话题".into(), None, vec![]).await.unwrap();
        f.session_post(&f.captain, &topic.id, "话题里的话", vec![]).await;
        assert!(ctx_of(&f, &f.worker).await.is_none(), "不是成员、没收听：不注入");
        port::set_listen(&f.state, &f.worker, &topic.id, None, vec![]).await.unwrap();
        ctx_of(&f, &f.worker).await; // 第一次见到这个群：M3a 先把游标钉在末尾
        f.session_post(&f.captain, &topic.id, "收听后的话", vec![]).await;
        assert!(ctx_of(&f, &f.worker).await.unwrap().contains("收听后的话"));
    }

    #[tokio::test]
    async fn huge_backlog_is_capped_and_announced() {
        // Review Focus 2
        let f = Fixture::with_config(AgentConfig { inject_cap_chars: 2_000, ..AgentConfig::default() }).await;
        let l = lobby(&f).await;
        for i in 0..120 {
            f.human_post(&l, &format!("第 {i} 条：{}", "很长的一段说明".repeat(4)), vec![]).await;
        }
        let ctx = ctx_of(&f, &f.worker).await.unwrap();
        assert!(ctx.chars().count() <= 2_000, "{}", ctx.chars().count());
        assert!(ctx.contains("第 119 条"), "最新的在");
        assert!(!ctx.contains("第 0 条："), "最旧的被省略");
        assert!(ctx.contains("read_channel"), "省略要自报：{ctx}");
        assert!(ctx_of(&f, &f.worker).await.is_none(), "省略的也算告知过，不再重来");
    }
}
```

- [ ] **Step 2: 跑一遍确认失败**

Run: `cargo test -p crewd-server --lib agent::inject`
Expected: 编译失败。

- [ ] **Step 3: port 的四个函数**（Task 6 已经写好，这里给出全文供对照；名字已对照 M3a 计划的冻结接口）

```rust
pub async fn unread(st: &AppState, session: &str) -> PortResult<Vec<(String, Vec<Message>)>> {
    let s = session.to_string();
    Ok(st.collab(move |tx| crewd_collab::cursors::unread_for(tx, &s)).await?)
}

pub async fn blocks(
    st: &AppState,
    session: &str,
    unread: Vec<(String, Vec<Message>)>,
) -> PortResult<Vec<crewd_collab::render::ChannelBlock>> {
    let s = session.to_string();
    Ok(st.db.call(move |db| crewd_collab::render::blocks_for(db.conn(), &s, &unread)).await??)
}

pub fn injection(loc: crewd_collab::Locale, blocks: &[crewd_collab::render::ChannelBlock]) -> String {
    crewd_collab::render::injection(loc, blocks)
}

pub async fn advance(st: &AppState, session: &str, channel: &str, seq: i64) -> PortResult<()> {
    let (s, c) = (session.to_string(), channel.to_string());
    Ok(st.collab(move |tx| crewd_collab::cursors::advance(tx, &s, &c, seq)).await?)
}
```

- [ ] **Step 4: 写 `inject.rs`**

```rust
//! post-tool-use（spec §22.3「hook 行为」）：本 session 所在群 / 收听的群里游标之后的未读 → `additionalContext`。
//! 滤可见性、标「发给谁的」、渲染都是 M3a 的；这里只管总字数上限与推进游标（游标与唤醒共用：至多注入一次）。
use crewd_collab::render::ChannelBlock;
use crewd_proto::agent::HookResult;
use serde_json::json;

use crate::AppState;
use crate::agent::port::{self, PortResult};
use crate::agent::text;
use crate::agent::Identity;

pub struct Prepared {
    pub context: Option<String>,
    advance_to: Vec<(String, i64)>,
}

pub async fn prepare(st: &AppState, me: &Identity) -> PortResult<Option<Prepared>> {
    let loc = port::locale(st).await?;
    let cap = st.agent.config.inject_cap_chars;
    let unread = port::unread(st, &me.session_id).await?;
    let advance_to: Vec<(String, i64)> = unread
        .iter()
        .filter_map(|(c, v)| v.last().map(|m| (c.clone(), m.seq)))
        .collect();
    if advance_to.is_empty() {
        return Ok(None);
    }
    let mut blocks = port::blocks(st, &me.session_id, unread).await?;
    let mut omitted = vec![0usize; blocks.len()];
    loop {
        let context = render(loc, &blocks, &omitted);
        if context.chars().count() <= cap || blocks.iter().all(|b| b.lines.is_empty()) {
            let context = (!context.trim().is_empty()).then_some(context);
            return Ok(Some(Prepared { context, advance_to }));
        }
        // 去掉全局最旧的一行（seq 最小）。
        let Some(bi) = blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| !b.lines.is_empty())
            .min_by_key(|(_, b)| b.lines[0].seq)
            .map(|(i, _)| i)
        else {
            unreachable!("上面已判过全空")
        };
        blocks[bi].lines.remove(0);
        omitted[bi] += 1;
    }
}

fn render(loc: text::Locale, blocks: &[ChannelBlock], omitted: &[usize]) -> String {
    let head: Vec<String> = blocks
        .iter()
        .zip(omitted)
        .filter(|(_, n)| **n > 0)
        .map(|(b, n)| {
            let label = crate::agent::tools::channel_label_raw(loc, b.kind, &b.name, &b.crew_title);
            text::r(loc, "inject.omitted", &[("channel", &label), ("count", &n.to_string())])
        })
        .collect();
    let body = port::injection(loc, blocks);
    match (head.is_empty(), body.is_empty()) {
        (true, _) => body,
        (false, true) => head.join("\n"),
        (false, false) => format!("{}\n{}", head.join("\n"), body),
    }
}

pub async fn commit(st: &AppState, me: &Identity, p: &Prepared) -> PortResult<()> {
    for (ch, seq) in &p.advance_to {
        port::advance(st, &me.session_id, ch, *seq).await?;
    }
    Ok(())
}

/// `agent.hook {event: post-tool-use}`。出错写 stderr、不输出（hook 宁可漏一次）。
pub async fn post_tool_use(st: &AppState, me: &Identity) -> HookResult {
    let prepared = match prepare(st, me).await {
        Ok(Some(p)) => p,
        Ok(None) => return HookResult { stdout: None },
        Err(e) => {
            eprintln!("crewd: post-tool-use for {}: {e}", me.session_id);
            return HookResult { stdout: None };
        }
    };
    if let Err(e) = commit(st, me, &prepared).await {
        eprintln!("crewd: post-tool-use cursor for {}: {e}", me.session_id);
        return HookResult { stdout: None };
    }
    let stdout = prepared.context.map(|c| {
        json!({"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": c}}).to_string()
    });
    HookResult { stdout }
}
```

`tools/mod.rs` 加（Task 12 起回执也用）：

```rust
/// `channel_label_raw` 的 ChannelView 版本。
pub fn channel_label(cx: &Ctx<'_>, c: &port::ChannelView) -> String {
    channel_label_raw(cx.loc, c.kind, &c.name, &c.crew_title)
}

/// 群在回执里怎么叫：大厅 / 人类群的 `name` 是空串（M3a 约定），按机组名说。
pub fn channel_label_raw(loc: Locale, kind: crewd_proto::ChannelKind, name: &str, crew_title: &str) -> String {
    match kind {
        crewd_proto::ChannelKind::Lobby => text::r(loc, "r.lobby_label", &[("crew", crew_title)]),
        crewd_proto::ChannelKind::Human => text::r(loc, "r.human_label", &[("crew", crew_title)]),
        _ => name.to_string(),
    }
}
```

`rpc.rs` 的 `hook`：`HookKind::PostToolUse => crate::agent::inject::post_tool_use(state, me).await,`。

- [ ] **Step 5: md 加节**

`tools.zh.md`：

```markdown
## inject.omitted
「{channel}」较早的 {count} 条没带上，要看用 read_channel。

## r.lobby_label
「{crew}」大厅

## r.human_label
「{crew}」人类群
```

`tools.en.md`：

```markdown
## inject.omitted
{count} older messages in "{channel}" were left out; use read_channel to see them.

## r.lobby_label
lobby of "{crew}"

## r.human_label
human channel of "{crew}"
```

- [ ] **Step 6: 跑通**

Run: `cargo test -p crewd-server --lib agent::inject`
Expected: `5 passed`。

- [ ] **Step 7: 变异核对**

1. `post_tool_use` 里删掉 `commit` 调用：Expected `unread_is_injected_once`、`huge_backlog…` 红。
2. `prepare` 的 `if context.chars().count() <= cap ||` 换成 `if true ||`：Expected `huge_backlog_is_capped_and_announced` 红。

逐条撤销。

- [ ] **Step 8: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/server/src/agent/inject.rs crates/server/src/agent/mod.rs crates/server/src/agent/rpc.rs \
  crates/server/src/agent/port.rs crates/server/src/agent/tools/mod.rs \
  crates/collab/prompts/tools.zh.md crates/collab/prompts/tools.en.md
git diff --cached --stat
git commit -m "crewd-server: post-tool-use injection over collab unread/render, 9k cap with self-reported omission

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 10: pre-tool-use 审批闸（`agent::approval`）

**Files:**
- Create: `crewd/crates/server/src/agent/approval.rs`
- Modify: `agent/mod.rs`（`pub mod approval;`）、`agent/rpc.rs`（PreToolUse 分支）、`crates/collab/prompts/tools.{zh,en}.md`、`crewd/crates/crewd/tests/helper_cli.rs`（加一种假 serve 模式与一条测试）

**Interfaces:**
- Consumes: Task 6 `port::{subscribe_collab, create_approval, approval_decision, decide_approval, locale}`；M3a `approvals::{request, get, decide}`（`request` 自己在根机组人类群发 @human 通知，文字是 M3a `render.*.md` 的 `notice.approval`）、`Change::ApprovalChanged { id }`；Task 3 `text`。
- Produces: `approval::{gate_hit(gates: &[String], tool: &str) -> bool, pre_tool_use(st: &AppState, me: &Identity, payload: &Value) -> HookResult}`。

行为（spec §22.3）：`tool_name` 子串命中 `AgentConfig.approval_gates` → `approvals::request`（不发 H 号；M3a 在同一事务里往根机组人类群发 @human 通知）→ 等 `Change::ApprovalChanged`（先订阅再建，建完先查一次防竞态）→ allow / deny；`approval_wait` 到点仍没结果 → 记 `decided_by = "timeout"` 的 deny。没命中 → 不输出。输出形状照官方文档：`{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"|"deny","permissionDecisionReason":…}}`。**与 Swift 的出入**（「与 spec 的出入」6）：Swift #75 ② 起当场拒 + 开人类 Todo + 一次性放行票，不阻塞。

- [ ] **Step 1: 写失败的测试**（`approval.rs` 末尾）

```rust
#[cfg(test)]
mod tests {
    use crewd_proto::MentionKind;
    use std::time::Duration;

    use serde_json::{Value, json};

    use super::*;
    use crate::agent::port;
    use crate::agent::runtime::AgentConfig;
    use crate::agent::testkit::Fixture;

    #[test]
    fn gates_are_trimmed_substrings() {
        let g = vec!["computer-use".to_string(), " ".to_string()];
        assert!(gate_hit(&g, "mcp__computer-use__left_click"));
        assert!(!gate_hit(&g, "Bash"));
        assert!(!gate_hit(&[], "anything"));
    }

    fn decision(out: &HookResult) -> String {
        let v: Value = serde_json::from_str(out.stdout.as_deref().unwrap()).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        v["hookSpecificOutput"]["permissionDecision"].as_str().unwrap().to_string()
    }

    async fn pending_id(f: &Fixture) -> String {
        for _ in 0..100 {
            let list = f.rpc("approval.list", json!({})).await;
            if let Some(a) = list.as_array().unwrap().iter().find(|a| a["decision"].is_null()) {
                return a["id"].as_str().unwrap().to_string();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("没出现待审批");
    }

    fn payload(tool: &str) -> Value {
        json!({"hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": {"command": "示例命令"}, "tool_use_id": "toolu_1"})
    }

    #[tokio::test]
    async fn ungated_tools_pass_silently() {
        let f = Fixture::new().await;
        let me = f.me(&f.worker).await;
        assert_eq!(pre_tool_use(&f.state, &me, &payload("Bash")).await.stdout, None);
        assert_eq!(pre_tool_use(&f.state, &me, &json!({})).await.stdout, None, "没有 tool_name：不管");
    }

    #[tokio::test]
    async fn approved_by_human_allows_and_notice_goes_to_the_human_channel() {
        let f = Fixture::new().await;
        let me = f.me(&f.worker).await;
        let st = f.state.clone();
        let waiting = tokio::spawn(async move { pre_tool_use(&st, &me, &payload("mcp__computer-use__click")).await });
        let id = pending_id(&f).await;
        let human = port::root_human_channel(&f.state, &f.crew).await.unwrap();
        let notice = port::messages(&f.state, &human.id, None, 5).await.unwrap();
        let last = notice.last().expect("人类群里有通知");
        assert!(last.text.contains("mcp__computer-use__click"));
        assert!(last.mentions.iter().any(|m| m.kind == MentionKind::Human));
        f.rpc("approval.decide", json!({"id": id, "allow": true})).await;
        let out = tokio::time::timeout(Duration::from_secs(5), waiting).await.unwrap().unwrap();
        assert_eq!(decision(&out), "allow");
    }

    #[tokio::test]
    async fn denied_by_human_denies() {
        let f = Fixture::new().await;
        let me = f.me(&f.worker).await;
        let st = f.state.clone();
        let waiting = tokio::spawn(async move { pre_tool_use(&st, &me, &payload("mcp__computer-use__type")).await });
        let id = pending_id(&f).await;
        f.rpc("approval.decide", json!({"id": id, "allow": false})).await;
        let out = tokio::time::timeout(Duration::from_secs(5), waiting).await.unwrap().unwrap();
        assert_eq!(decision(&out), "deny");
    }

    #[tokio::test]
    async fn unanswered_approval_times_out_to_deny() {
        // Review Focus 3
        let f = Fixture::with_config(AgentConfig { approval_wait: Duration::from_millis(300), ..AgentConfig::default() }).await;
        let me = f.me(&f.worker).await;
        let out = pre_tool_use(&f.state, &me, &payload("mcp__computer-use__click")).await;
        assert_eq!(decision(&out), "deny");
        let list = f.rpc("approval.list", json!({})).await;
        let a = &list.as_array().unwrap()[0];
        assert_eq!(a["decision"], "deny");
        assert_eq!(a["decidedBy"], "timeout");
    }
}
```

`crates/crewd/tests/helper_cli.rs`：`enum Mode` 加 `DropOnHook`（收到 `agent.hook` 就把连接整条关掉，模拟 serve 在等待中途停掉），处理分支 `_ if mode == Mode::Hang => continue,` 下面加 `"agent.hook" if mode == Mode::DropOnHook => break,`；加测试：

```rust
#[tokio::test]
async fn crewd_gone_during_wait_means_deny() {
    // Review Focus 3：serve 在审批等待中途没了 → helper 当场 deny，不等到 claude 的超时（超时会放行）。
    let f = FakeServe::start(Mode::DropOnHook).await;
    let (out, _, took) = run_hook(f.data_dir(), "pre-tool-use", &["--deadline-secs", "30"], &f.token_file(), "{\"tool_name\":\"x\"}").await;
    assert!(out.contains("\"deny\""), "{out}");
    assert!(took < Duration::from_secs(5), "{took:?}");
}
```

- [ ] **Step 2: 跑一遍确认失败**

Run: `cargo test -p crewd-server --lib agent::approval`
Expected: 编译失败。`cargo test -p crewd --test helper_cli crewd_gone` 应已通过（Task 8 的 `Ok(Err)` 分支已 deny）——这条是给 Review Focus 3 补的钉子，不是新行为；把它的变异核对放在 Step 6。

- [ ] **Step 3: 写实现**

```rust
//! pre-tool-use：命中审批闸的工具先等人批（spec §22.3）。不发 H 号：审批是同步闸，随这次调用失效。
use crewd_proto::agent::HookResult;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;

use crate::AppState;
use crate::agent::port;
use crate::agent::text;
use crate::agent::Identity;

pub fn gate_hit(gates: &[String], tool: &str) -> bool {
    gates.iter().map(|g| g.trim()).filter(|g| !g.is_empty()).any(|g| tool.contains(g))
}

/// `tool_input` 里最能说明意图的一个字段（command / url / path / file_path / prompt），截 120 字。
fn summary(tool: &str, input: &Value) -> String {
    for k in ["command", "url", "path", "file_path", "prompt"] {
        if let Some(v) = input.get(k).and_then(Value::as_str).filter(|v| !v.is_empty()) {
            let short: String = v.chars().take(120).collect();
            return format!("{tool} · {k}={short}");
        }
    }
    tool.to_string()
}

fn output(decision: &str, reason: String) -> HookResult {
    HookResult {
        stdout: Some(
            json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": decision,
                "permissionDecisionReason": reason,
            }})
            .to_string(),
        ),
    }
}

pub async fn pre_tool_use(st: &AppState, me: &Identity, payload: &Value) -> HookResult {
    let cfg = &st.agent.config;
    let loc = port::locale(st).await.unwrap_or_default();
    let Some(tool) = payload.get("tool_name").and_then(Value::as_str) else {
        return HookResult { stdout: None };
    };
    if !gate_hit(&cfg.approval_gates, tool) {
        return HookResult { stdout: None };
    }
    let what = summary(tool, payload.get("tool_input").unwrap_or(&Value::Null));
    let tool_use_id = payload.get("tool_use_id").and_then(Value::as_str).map(str::to_string);
    let mut rx = port::subscribe_collab(st);
    let id = match port::create_approval(st, me, tool, &what, tool_use_id).await {
        Ok(id) => id,
        Err(e) => {
            eprintln!("crewd: approval for {}: {e}", me.session_id);
            return output("deny", text::r(loc, "r.approval_unavailable", &[("tool", tool)]));
        }
    };
    let deadline = tokio::time::Instant::now() + cfg.approval_wait;
    loop {
        match port::approval_decision(st, &id).await {
            Ok(Some(true)) => return output("allow", text::r(loc, "r.approval_allowed", &[("tool", tool)])),
            Ok(Some(false)) => return output("deny", text::r(loc, "r.approval_denied", &[("tool", tool)])),
            Ok(None) => {}
            Err(e) => eprintln!("crewd: approval {id}: {e}"),
        }
        let wait = tokio::time::timeout_at(deadline, async {
            loop {
                match rx.recv().await {
                    Ok(crewd_collab::Change::ApprovalChanged { id: changed }) if changed == id => return,
                    Ok(_) | Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return,
                }
            }
        })
        .await;
        if wait.is_err() {
            let _ = port::decide_approval(st, &id, false, "timeout").await;
            let minutes = (cfg.approval_wait.as_secs() / 60).max(1).to_string();
            return output("deny", text::r(loc, "r.approval_timeout", &[("tool", tool), ("minutes", &minutes)]));
        }
    }
}
```

`rpc.rs` 的 `hook`：`HookKind::PreToolUse => crate::agent::approval::pre_tool_use(state, me, &payload).await,`（参数名 `_payload` 改回 `payload`）。

- [ ] **Step 4: md 加节**

`tools.zh.md`：

```markdown

## r.approval_allowed
人类已批准 {tool}。

## r.approval_denied
人类拒绝了 {tool}。换个不需要它的办法，或者用 ask 说明为什么非它不可。

## r.approval_timeout
{minutes} 分钟内没人审批，已拒绝 {tool}。需要的话用 ask 说明原因，人看到后再试。

## r.approval_unavailable
现在没法发起审批，已拒绝 {tool}。
```

`tools.en.md`：

```markdown

## r.approval_allowed
The human approved {tool}.

## r.approval_denied
The human denied {tool}. Find a way that does not need it, or use ask to explain why it is required.

## r.approval_timeout
Nobody reviewed {tool} within {minutes} minutes, so it was denied. If you need it, explain why with ask and try again once the human has seen it.

## r.approval_unavailable
An approval could not be raised right now, so {tool} was denied.
```

- [ ] **Step 5: 跑通**

Run: `cargo test -p crewd-server --lib agent::approval && cargo test -p crewd --test helper_cli`
Expected: approval `5 passed`；helper_cli `8 passed`。

- [ ] **Step 6: 变异核对**

1. `pre_tool_use` 的超时分支把 `"deny"` 改成 `"allow"`：Expected `unanswered_approval_times_out_to_deny` 红。
2. 把 `let mut rx = port::subscribe_collab(st);` 挪到 `create_approval` 之后、并删掉循环开头那次 `approval_decision` 查询：`approved_by_human…` 偶发等不到（竞态）——这条不稳定地红，看到红一次即可，不要求必红。
3. `hook_cmd.rs` 的 `Ok(Err(e))` 分支删掉 deny：Expected `crewd_gone_during_wait_means_deny` 红。

逐条撤销。

- [ ] **Step 7: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/server/src/agent/approval.rs crates/server/src/agent/mod.rs crates/server/src/agent/rpc.rs \
  crates/collab/prompts/tools.zh.md crates/collab/prompts/tools.en.md crates/crewd/tests/helper_cli.rs
git diff --cached --stat
git commit -m "crewd-server: pre-tool-use approval gate (notice in human channel, wait for decision, timeout denies)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 11: stop hook（`agent::turn`，薄壳：解析 payload → `collab::turn_trace::on_turn_end`）

**Files:**
- Create: `crewd/crates/server/src/agent/turn.rs`
- Modify: `agent/mod.rs`（`pub mod turn;`）、`agent/rpc.rs`（Stop 分支）

**Interfaces:**
- Consumes: Task 5 `crewd_collab::turn_trace::{on_turn_end, note_post, TurnEnd}`；M3a `AppState::collab`。
- Produces:
  - `turn::stop(st: &AppState, me: &Identity, payload: &Value) -> HookResult`（永远 `stdout: None`）
  - `turn::note_post(st: &AppState, me: &Identity, mentions: &[Mention])`（工具发出消息后调；出错只写 stderr）

payload 字段照官方 hooks 文档与 `McpTurnHook.swift:31-48`：`hook_event_name` 不是 `Stop` 或带 `agent_id`（子 agent）→ 不管；正文取 `last_assistant_message`（文档要求用它，不读 transcript），缺失当空串；`prompt_id` 缺失当空串。M3c 的 codex `TurnEnded` 直接调 `on_turn_end`，不经这里。

- [ ] **Step 1: 写失败的测试**（`turn.rs` 末尾）

```rust
#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::agent::port;
    use crate::agent::testkit::Fixture;

    fn stop_payload(prompt: &str, text: &str) -> serde_json::Value {
        json!({"hook_event_name": "Stop", "prompt_id": prompt, "stop_hook_active": false, "last_assistant_message": text})
    }

    #[tokio::test]
    async fn stop_relays_through_collab_and_never_outputs() {
        let f = Fixture::new().await;
        let me = f.me(&f.worker).await;
        let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
        assert_eq!(stop(&f.state, &me, &stop_payload("p0", "收到。")).await.stdout, None);
        let before = port::latest_seq(&f.state, &lobby).await.unwrap();
        assert_eq!(stop(&f.state, &me, &stop_payload("p1", "要不要接着做 B？")).await.stdout, None);
        assert_ne!(port::latest_seq(&f.state, &lobby).await.unwrap(), before, "代发了");
    }

    #[tokio::test]
    async fn subagent_and_non_stop_payloads_are_ignored() {
        let f = Fixture::new().await;
        let me = f.me(&f.worker).await;
        stop(&f.state, &me, &stop_payload("p0", "收到。")).await;
        let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
        let before = port::latest_seq(&f.state, &lobby).await.unwrap();
        let mut sub = stop_payload("p1", "要不要做 B？");
        sub["agent_id"] = json!("sub-1");
        stop(&f.state, &me, &sub).await;
        let mut other = stop_payload("p2", "要不要做 B？");
        other["hook_event_name"] = json!("SubagentStop");
        stop(&f.state, &me, &other).await;
        assert_eq!(port::latest_seq(&f.state, &lobby).await.unwrap(), before);
    }
}
```

- [ ] **Step 2: 跑一遍确认失败**

Run: `cargo test -p crewd-server --lib agent::turn`
Expected: 编译失败。

- [ ] **Step 3: 写实现**

```rust
//! stop hook（spec §22.3）：判定与落库都在 `crewd_collab::turn_trace`，这里只解析 claude 的 payload。永不阻塞。
use crewd_collab::turn_trace;
use crewd_proto::Mention;
use crewd_proto::agent::HookResult;
use serde_json::Value;

use crate::AppState;
use crate::agent::Identity;

pub async fn note_post(st: &AppState, me: &Identity, mentions: &[Mention]) {
    let (session, mentions) = (me.session_id.clone(), mentions.to_vec());
    if let Err(e) = st.collab(move |tx| turn_trace::note_post(tx, &session, &mentions)).await {
        eprintln!("crewd: turn memo for {}: {e}", me.session_id);
    }
}

pub async fn stop(st: &AppState, me: &Identity, payload: &Value) -> HookResult {
    let event = payload.get("hook_event_name").and_then(Value::as_str);
    if event.is_some_and(|e| e != "Stop") || payload.get("agent_id").is_some() {
        return HookResult { stdout: None };
    }
    let prompt = payload.get("prompt_id").and_then(Value::as_str).unwrap_or("").to_string();
    let last = payload.get("last_assistant_message").and_then(Value::as_str).unwrap_or("").to_string();
    let session = me.session_id.clone();
    if let Err(e) = st.collab(move |tx| turn_trace::on_turn_end(tx, &session, &prompt, &last)).await {
        eprintln!("crewd: stop hook for {}: {e}", me.session_id);
    }
    HookResult { stdout: None }
}
```

`rpc.rs` 的 `hook`：`HookKind::Stop => crate::agent::turn::stop(state, me, &payload).await,`。

- [ ] **Step 4: 跑通**

Run: `cargo test -p crewd-server --lib agent::turn`
Expected: `2 passed`。

- [ ] **Step 5: 变异核对**

`stop` 里删掉 `|| payload.get("agent_id").is_some()`：Expected `subagent_and_non_stop_payloads_are_ignored` 红。撤销。

- [ ] **Step 6: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/server/src/agent/turn.rs crates/server/src/agent/mod.rs crates/server/src/agent/rpc.rs
git diff --cached --stat
git commit -m "crewd-server: stop hook forwards to collab turn trace, never blocks

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 12: 消息工具（`post` / `read_channel` / `search` / `list_channels` / `human_watching`）

**Files:**
- Create: `crewd/crates/server/src/agent/tools/messaging.rs`
- Modify: `agent/tools/mod.rs`（分发 + 共用的 `resolve_channel` / `msg_ref` / `render_line`）、`crates/collab/prompts/tools.{zh,en}.md`

**Interfaces:**
- Consumes: Task 6 `port::{my_channels, crew_channels, lobby_of, channel, post, messages, message, search, advance, watched_channels, root_human_channel, identity}`；Task 7 `Ctx` / `parse_args`；Task 11 `turn::note_post`。
- Produces（Task 13–15 复用）：
  - `tools::resolve_channel(cx: &Ctx<'_>, arg: Option<&str>) -> Result<ChannelView, CallResult>`（`None` → 本机组大厅；按 id，再按「我能看见的群里唯一的名字」）
  - `tools::RefArg`（`#[serde(untagged)] enum RefArg { Num(i64), Str(String) }`）、`tools::msg_ref(&RefArg) -> MsgRef`、`tools::ref_text(&RefArg) -> String`
  - `tools::render_line(cx: &Ctx<'_>, m: &Message) -> String`
  - `tools::post_as(cx, channel: &ChannelView, text: String, mentions: Vec<Mention>, reply: Option<String>, category: Option<String>, sender: PostSender) -> Result<Posted, CallResult>`（发 + `note_post`）

- [ ] **Step 1: 写失败的测试**（`messaging.rs` 末尾）

```rust
#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::agent::port::{self, ChannelKind};
    use crate::agent::testkit::Fixture;
    use crate::agent::text::{self, Locale};
    use crate::agent::tools::call;

    async fn run(f: &Fixture, who: &str, tool: &str, args: serde_json::Value) -> crewd_proto::agent::CallResult {
        let me = f.me(who).await;
        call(&f.state, &me, tool, args).await
    }

    async fn lobby(f: &Fixture) -> String {
        port::lobby_of(&f.state, &f.crew).await.unwrap().id
    }

    async fn last(f: &Fixture, ch: &str) -> crewd_proto::Message {
        port::messages(&f.state, ch, None, 1).await.unwrap().pop().unwrap()
    }

    #[tokio::test]
    async fn post_defaults_to_lobby_and_reports_the_number() {
        let f = Fixture::new().await;
        let r = run(&f, &f.worker, "post", json!({"message": "示例：A 做完了", "category": "done"})).await;
        assert!(!r.is_error, "{}", r.text);
        let m = last(&f, &lobby(&f).await).await;
        assert_eq!(m.text, "示例：A 做完了");
        assert_eq!(m.sender_session_id.as_deref(), Some(f.worker.as_str()));
        assert!(r.text.contains(&format!("#{}", m.seq)), "{}", r.text);
    }

    #[tokio::test]
    async fn reply_to_by_number_goes_to_that_channel() {
        let f = Fixture::new().await;
        let cap = f.me(&f.captain).await;
        let topic = port::create_channel(&f.state, &cap, ChannelKind::Topic, "示例话题".into(), None,
            vec![port::MemberRef::Session(f.worker.clone())]).await.unwrap();
        let q = f.session_post(&f.captain, &topic.id, "这个怎么办？", vec![]).await;
        let r = run(&f, &f.worker, "post", json!({"message": "按 A 办", "reply_to": format!("#{}", q.seq)})).await;
        assert!(!r.is_error, "{}", r.text);
        let m = last(&f, &topic.id).await;
        assert_eq!(m.in_reply_to.as_deref(), Some(q.id.as_str()));
        let r = run(&f, &f.worker, "post", json!({"message": "x", "reply_to": 999_999})).await;
        assert!(r.is_error);
    }

    #[tokio::test]
    async fn batch_is_validated_all_or_nothing() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        let before = port::latest_seq(&f.state, &l).await.unwrap();
        let r = run(&f, &f.worker, "post", json!({"messages": [
            {"text": "第一条", "category": "progress"},
            {"text": "第二条", "category": "plan"}
        ]})).await;
        assert!(r.is_error);
        assert!(r.text.contains('2'), "指名第几条：{}", r.text);
        assert_eq!(port::latest_seq(&f.state, &l).await.unwrap(), before, "一条都没发");
        let seven: Vec<_> = (0..7).map(|i| json!({"text": format!("第{i}条")})).collect();
        assert!(run(&f, &f.worker, "post", json!({"messages": seven})).await.is_error);
        let r = run(&f, &f.worker, "post", json!({"message": "a", "messages": [{"text": "b"}]})).await;
        assert!(r.is_error, "二选一");
        let ok = run(&f, &f.worker, "post", json!({"messages": [{"text": "进度"}, {"text": "要拍板", "category": "question"}]})).await;
        assert!(!ok.is_error, "{}", ok.text);
        assert_eq!(port::messages(&f.state, &l, None, 2).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn unknown_argument_is_named() {
        let f = Fixture::new().await;
        let r = run(&f, &f.worker, "post", json!({"message": "x", "plan": 3})).await;
        assert!(r.is_error && r.text.contains("plan"), "{}", r.text);
    }

    #[tokio::test]
    async fn human_mention_in_unwatched_channel_warns_and_human_channel_does_not() {
        let f = Fixture::new().await;
        let warn_head: String = text::r(Locale::Zh, "r.human_not_watching", &[("channel", "")]).chars().take(4).collect();
        let r = run(&f, &f.worker, "post", json!({"message": "@人 看一下", "mentions": [{"kind": "human"}]})).await;
        assert!(r.text.contains(&warn_head), "{}", r.text);
        let human = port::root_human_channel(&f.state, &f.crew).await.unwrap();
        let r = run(&f, &f.worker, "post", json!({"channel": human.id, "message": "要你拍板", "mentions": [{"kind": "human"}]})).await;
        assert!(!r.is_error, "{}", r.text);
        assert!(!r.text.contains(&warn_head), "人类群不提醒");
        f.rpc("channel.watch", json!({"channelId": lobby(&f).await, "on": true})).await;
        let r = run(&f, &f.worker, "post", json!({"message": "@人 再看一下", "mentions": [{"kind": "human"}]})).await;
        assert!(!r.text.contains(&warn_head), "注视中的群不提醒");
    }

    #[tokio::test]
    async fn read_channel_pages_and_marks_read_only_at_the_latest_page() {
        let f = Fixture::new().await;
        let l = lobby(&f).await;
        for i in 0..5 {
            f.human_post(&l, &format!("消息{i}"), vec![]).await;
        }
        let older = run(&f, &f.worker, "read_channel", json!({"limit": 2, "before": port::latest_seq(&f.state, &l).await.unwrap().unwrap()})).await;
        assert!(older.text.contains("消息2") && older.text.contains("消息3") && !older.text.contains("消息4"), "{}", older.text);
        let me = f.me(&f.worker).await;
        assert!(crate::agent::inject::prepare(&f.state, &me).await.unwrap().is_some(), "翻旧页不算已读");
        let latest = run(&f, &f.worker, "read_channel", json!({})).await;
        assert!(latest.text.contains("消息4"));
        assert!(crate::agent::inject::prepare(&f.state, &me).await.unwrap().is_none(), "读到最新一条：不再注入");
    }

    #[tokio::test]
    async fn search_is_limited_to_my_channels() {
        let f = Fixture::new().await;
        let cap = f.me(&f.captain).await;
        let topic = port::create_channel(&f.state, &cap, ChannelKind::Topic, "机长的话题".into(), None, vec![]).await.unwrap();
        f.session_post(&f.captain, &topic.id, "暗号 青蛙七号", vec![]).await;
        f.human_post(&lobby(&f).await, "暗号 青蛙八号", vec![]).await;
        let r = run(&f, &f.worker, "search", json!({"query": "暗号"})).await;
        assert!(r.text.contains("青蛙八号") && !r.text.contains("青蛙七号"), "{}", r.text);
        let r = run(&f, &f.worker, "read_channel", json!({"channel": topic.id})).await;
        assert!(r.text.contains("青蛙七号"), "本机组的群都能读（§22.7 Q3：不做私有群）");
    }

    #[tokio::test]
    async fn list_channels_and_human_watching() {
        let f = Fixture::new().await;
        let human = port::root_human_channel(&f.state, &f.crew).await.unwrap();
        let r = run(&f, &f.worker, "list_channels", json!({})).await;
        assert!(r.text.contains(&lobby(&f).await), "{}", r.text);
        let r = run(&f, &f.worker, "human_watching", json!({})).await;
        assert!(r.text.contains(&human.id), "{}", r.text);
    }

    #[tokio::test]
    async fn posting_with_captain_mention_counts_as_calling_out() {
        let f = Fixture::new().await;
        let me = f.me(&f.worker).await;
        crate::agent::turn::stop(&f.state, &me, &json!({"hook_event_name": "Stop", "prompt_id": "p0", "last_assistant_message": "好。"})).await;
        run(&f, &f.worker, "post", json!({"message": "机长看下", "mentions": [{"kind": "captain"}]})).await;
        let l = lobby(&f).await;
        let before = port::latest_seq(&f.state, &l).await.unwrap();
        crate::agent::turn::stop(&f.state, &me, &json!({"hook_event_name": "Stop", "prompt_id": "p1", "last_assistant_message": "要不要继续？"})).await;
        assert_eq!(port::latest_seq(&f.state, &l).await.unwrap(), before, "这一轮已经 @ 过机长，不代发");
    }
}
```

- [ ] **Step 2: 跑一遍确认失败**

Run: `cargo test -p crewd-server --lib agent::tools::messaging`
Expected: 编译失败。

- [ ] **Step 3: `tools/mod.rs` 加共用件与分发**

`call` 末尾两行换成：

```rust
    match tool {
        "post" => messaging::post(&cx, args).await,
        "read_channel" => messaging::read_channel(&cx, args).await,
        "search" => messaging::search(&cx, args).await,
        "list_channels" => messaging::list_channels(&cx).await,
        "human_watching" => messaging::human_watching(&cx).await,
        _ => cx.err("r.no_handler", &[("name", tool)]),
    }
```

文件顶部 `pub mod messaging;`，并加：

```rust
use crewd_proto::{Mention, MentionKind, Message, SenderKind};
use serde::Deserialize;

use crate::agent::port::{ChannelView, MsgRef, PostReq, PostSender, Posted};

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum RefArg {
    Num(i64),
    Str(String),
}

/// `128` / `"128"` / `"#128"` → 序号；其余当消息 id。
pub fn msg_ref(r: &RefArg) -> MsgRef {
    match r {
        RefArg::Num(n) => MsgRef::Seq(*n),
        RefArg::Str(s) => {
            let t = s.trim().trim_start_matches('#');
            t.parse::<i64>().map(MsgRef::Seq).unwrap_or_else(|_| MsgRef::Id(s.trim().to_string()))
        }
    }
}

/// 回执里怎么写一个消息引用。
pub fn ref_text(r: &RefArg) -> String {
    match r {
        RefArg::Num(n) => format!("#{n}"),
        RefArg::Str(s) => s.clone(),
    }
}

/// 我能看见的群：所在的 / 收听的 + 本机组 `channel.list` 列出的。
pub async fn visible_channels(cx: &Ctx<'_>) -> Result<Vec<ChannelView>, CallResult> {
    let mut all = port::my_channels(cx.st, &cx.me.session_id).await.map_err(|e| cx.fail(e))?;
    for c in port::crew_channels(cx.st, &cx.me.crew_id).await.map_err(|e| cx.fail(e))? {
        if !all.iter().any(|x| x.id == c.id) {
            all.push(c);
        }
    }
    Ok(all)
}

pub async fn resolve_channel(cx: &Ctx<'_>, arg: Option<&str>) -> Result<ChannelView, CallResult> {
    let Some(arg) = arg.map(str::trim).filter(|a| !a.is_empty()) else {
        return port::lobby_of(cx.st, &cx.me.crew_id).await.map_err(|e| cx.fail(e));
    };
    let all = visible_channels(cx).await?;
    if let Some(c) = all.iter().find(|c| c.id == arg) {
        return Ok(c.clone());
    }
    let named: Vec<&ChannelView> = all.iter().filter(|c| c.name == arg).collect();
    match named.as_slice() {
        [one] => Ok((*one).clone()),
        _ => {
            let choices = all
                .iter()
                .map(|c| cx.r("r.channel_choice", &[("id", &c.id), ("name", &channel_label(cx, c))]))
                .collect::<Vec<_>>()
                .join("\n");
            let key = if named.is_empty() { "r.no_channel" } else { "r.ambiguous_channel" };
            Err(cx.err(key, &[("arg", arg), ("choices", &choices)]))
        }
    }
}

/// 给 agent 看的一行消息：`#序号 时间 发送者：正文`（+ 附件路径）。
pub fn render_line(cx: &Ctx<'_>, m: &Message) -> String {
    let who = match (m.sender_kind, &m.sender_name, &m.sender_session_id) {
        (SenderKind::Human, _, _) => cx.r("r.who_human", &[]),
        (SenderKind::System, _, _) => cx.r("r.who_system", &[]),
        (_, Some(n), _) if !n.is_empty() => n.clone(),
        (_, _, Some(s)) => s.clone(),
        _ => cx.r("r.who_system", &[]),
    };
    let mut line = cx.r(
        "r.line",
        &[("seq", &m.seq.to_string()), ("time", &m.created_at), ("who", &who), ("text", &m.text)],
    );
    for a in &m.attachments {
        line.push_str(&cx.r("r.line_attachment", &[("path", &a.path)]));
    }
    line
}

/// 以某个身份往群里发一条；session 身份发出的同时记「这一轮叫没叫过人」。
#[allow(clippy::too_many_arguments)]
pub async fn post_as(
    cx: &Ctx<'_>,
    channel: &ChannelView,
    text: String,
    mentions: Vec<Mention>,
    in_reply_to: Option<String>,
    category: Option<String>,
    sender: PostSender,
) -> Result<Posted, CallResult> {
    let by_me = matches!(&sender, PostSender::Session(s) if *s == cx.me.session_id);
    let req = PostReq {
        channel_id: channel.id.clone(),
        sender,
        text,
        mentions: mentions.clone(),
        in_reply_to,
        category,
        attachments: vec![], // M3a 的 message.post 暂不收附件（M3a 裁定 14）
    };
    let p = port::post(cx.st, req).await.map_err(|e| cx.fail(e))?;
    if by_me {
        crate::agent::turn::note_post(cx.st, cx.me, &p.message.mentions).await;
    }
    Ok(p)
}
```

（`note_post` 用落库后的 `mentions`：`reply_to` 自动追加的 @ 也算。）

- [ ] **Step 4: 写 `messaging.rs`**

```rust
//! 消息工具（spec §22.3 表前五行）。
use crewd_proto::agent::CallResult;
use crewd_proto::{Mention, MentionKind};
use serde::Deserialize;
use serde_json::Value;

use super::{Ctx, RefArg, msg_ref, parse_args, post_as, render_line, resolve_channel, visible_channels};
use crate::agent::catalog::CATEGORIES;
use crate::agent::port::{self, ChannelKind, PostSender};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Item {
    text: String,
    category: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MentionArg {
    kind: String,
    target_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PostArgs {
    channel: Option<String>,
    message: Option<String>,
    messages: Option<Vec<Item>>,
    #[serde(default)]
    mentions: Vec<MentionArg>,
    reply_to: Option<RefArg>,
    category: Option<String>,
}

pub async fn post(cx: &Ctx<'_>, args: Value) -> CallResult {
    let a: PostArgs = match parse_args(cx, args) {
        Ok(a) => a,
        Err(e) => return e,
    };
    let items: Vec<Item> = match (a.message, a.messages) {
        (Some(text), None) => vec![Item { text, category: a.category.clone() }],
        (None, Some(v)) if a.category.is_none() => v,
        _ => return cx.err("r.post_one_of", &[]),
    };
    if items.is_empty() || items.len() > 6 {
        return cx.err("r.post_count", &[("count", &items.len().to_string())]);
    }
    for (i, it) in items.iter().enumerate() {
        let n = (i + 1).to_string();
        if it.text.trim().is_empty() {
            return cx.err("r.post_empty", &[("index", &n)]);
        }
        if let Some(c) = it.category.as_deref().filter(|c| !CATEGORIES.contains(c)) {
            return cx.err("r.bad_category", &[("index", &n), ("value", c), ("choices", &CATEGORIES.join(" / "))]);
        }
    }
    let mut mentions = Vec::new();
    for m in a.mentions {
        let kind = match MentionKind::parse(&m.kind) {
            Some(k) if !(k == MentionKind::Session && m.target_id.is_none()) => k,
            _ => return cx.err("r.bad_mention", &[("kind", &m.kind)]),
        };
        mentions.push(Mention { kind, target_id: m.target_id });
    }
    let reply = match &a.reply_to {
        None => None,
        Some(r) => match port::message(cx.st, msg_ref(r)).await {
            Ok(Some(m)) => Some(m),
            Ok(None) => return cx.err("r.no_message", &[("ref", &super::ref_text(r))]),
            Err(e) => return cx.fail(e),
        },
    };
    let channel = match (&a.channel, &reply) {
        (None, Some(m)) => match port::channel(cx.st, &m.channel_id).await {
            Ok(Some(c)) => c,
            Ok(None) => return cx.err("r.no_message", &[("ref", &m.seq.to_string())]),
            Err(e) => return cx.fail(e),
        },
        (arg, _) => match resolve_channel(cx, arg.as_deref()).await {
            Ok(c) => c,
            Err(e) => return e,
        },
    };
    if let Some(m) = &reply {
        if m.channel_id != channel.id {
            return cx.err("r.reply_other_channel", &[("seq", &m.seq.to_string())]);
        }
    }
    let total = items.len();
    let mut seqs = Vec::new();
    let mut unwatched = false;
    for (i, it) in items.into_iter().enumerate() {
        let first = i == 0;
        let res = post_as(
            cx,
            &channel,
            it.text,
            if first { mentions.clone() } else { vec![] },
            if first { reply.as_ref().map(|m| m.id.clone()) } else { None },
            it.category,
            PostSender::Session(cx.me.session_id.clone()),
        )
        .await;
        match res {
            Ok(p) => {
                unwatched |= p.unwatched_human_mention;
                seqs.push(format!("#{}", p.message.seq));
            }
            Err(e) if seqs.is_empty() => return e,
            Err(e) => {
                return cx.err(
                    "r.post_partial",
                    &[("sent", &seqs.join(" ")), ("index", &(i + 1).to_string()), ("total", &total.to_string()), ("detail", &e.text)],
                );
            }
        }
    }
    let label = super::channel_label(cx, &channel);
    let mut text = cx.r("r.posted", &[("channel", &label), ("seqs", &seqs.join(" "))]);
    if unwatched {
        // §17：M3a 判的（含 human、群没被注视、不是根机组人类群）。
        text.push('\n');
        text.push_str(&cx.r("r.human_not_watching", &[("channel", &label)]));
    }
    CallResult::ok(text)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    channel: Option<String>,
    limit: Option<u32>,
    before: Option<RefArg>,
}

pub async fn read_channel(cx: &Ctx<'_>, args: Value) -> CallResult {
    let a: ReadArgs = match parse_args(cx, args) {
        Ok(a) => a,
        Err(e) => return e,
    };
    let ch = match resolve_channel(cx, a.channel.as_deref()).await {
        Ok(c) => c,
        Err(e) => return e,
    };
    let limit = a.limit.unwrap_or(50).clamp(1, 200);
    let before = match &a.before {
        None => None,
        Some(r) => match port::message(cx.st, msg_ref(r)).await {
            Ok(Some(m)) => Some(m.seq),
            Ok(None) => return cx.err("r.no_message", &[("ref", &super::ref_text(r))]),
            Err(e) => return cx.fail(e),
        },
    };
    let msgs = match port::messages(cx.st, &ch.id, before, limit).await {
        Ok(v) => v,
        Err(e) => return cx.fail(e),
    };
    if msgs.is_empty() {
        return cx.ok("r.read_empty", &[("channel", &super::channel_label(cx, &ch))]);
    }
    let mut out = vec![cx.r("r.read_header", &[("channel", &super::channel_label(cx, &ch)), ("id", &ch.id), ("count", &msgs.len().to_string())])];
    out.extend(msgs.iter().map(|m| render_line(cx, m)));
    if msgs.len() as u32 == limit {
        out.push(cx.r("r.read_more", &[("before", &msgs[0].seq.to_string())]));
    }
    let mine = ch.has_session(&cx.me.session_id) || ch.listening;
    if before.is_none() && mine {
        if let Err(e) = port::advance(cx.st, &cx.me.session_id, &ch.id, msgs[msgs.len() - 1].seq).await {
            eprintln!("crewd: read_channel cursor for {}: {e}", cx.me.session_id);
        }
    }
    CallResult::ok(out.join("\n"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
    channel: Option<String>,
    after: Option<String>,
    before: Option<String>,
    limit: Option<u32>,
}

pub async fn search(cx: &Ctx<'_>, args: Value) -> CallResult {
    let a: SearchArgs = match parse_args(cx, args) {
        Ok(a) => a,
        Err(e) => return e,
    };
    if a.query.trim().is_empty() {
        return cx.err("r.bad_args", &[("detail", "query")]);
    }
    let mine = match port::my_channels(cx.st, &cx.me.session_id).await {
        Ok(v) => v,
        Err(e) => return cx.fail(e),
    };
    let scope: Vec<_> = match &a.channel {
        None => mine.clone(),
        Some(arg) => match resolve_channel(cx, Some(arg)).await {
            Ok(c) if mine.iter().any(|m| m.id == c.id) => vec![c],
            Ok(c) => return cx.err("r.search_scope", &[("channel", &super::channel_label(cx, &c))]),
            Err(e) => return e,
        },
    };
    let ids = scope.iter().map(|c| c.id.clone()).collect();
    let limit = a.limit.unwrap_or(50).clamp(1, 200);
    let hits = match port::search(cx.st, a.query.trim(), ids, a.after, a.before, limit).await {
        Ok(v) => v,
        Err(e) => return cx.fail(e),
    };
    if hits.is_empty() {
        return cx.ok("r.search_none", &[("query", a.query.trim())]);
    }
    let lines = hits.iter().map(|m| {
        let ch = scope.iter().find(|c| c.id == m.channel_id).map(|c| super::channel_label(cx, c)).unwrap_or_default();
        cx.r("r.search_hit", &[("channel", &ch), ("line", &render_line(cx, m))])
    });
    CallResult::ok(lines.collect::<Vec<_>>().join("\n"))
}

pub async fn list_channels(cx: &Ctx<'_>) -> CallResult {
    let all = match visible_channels(cx).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let mut out = vec![cx.r("r.list_header", &[])];
    for c in &all {
        let mut flags = Vec::new();
        if c.has_session(&cx.me.session_id) {
            flags.push(cx.r("r.flag_member", &[]));
        }
        if c.listening {
            flags.push(cx.r("r.flag_listening", &[]));
        }
        if c.watched {
            flags.push(cx.r("r.flag_watched", &[]));
        }
        if c.archived {
            flags.push(cx.r("r.flag_archived", &[]));
        }
        let owner = match &c.owner {
            Some(o) => port::identity(cx.st, o).await.ok().flatten().map(|i| i.label).unwrap_or(o.clone()),
            None => cx.r("r.owner_none", &[]),
        };
        let kind = cx.r(&format!("r.kind_{}", kind_key(c.kind)), &[]);
        out.push(cx.r(
            "r.list_line",
            &[
                ("id", &c.id),
                ("name", &super::channel_label(cx, c)),
                ("kind", &kind),
                ("purpose", c.purpose.as_deref().unwrap_or("")),
                ("owner", &owner),
                ("count", &c.members.len().to_string()),
                ("flags", &flags.join(" ")),
            ],
        ));
    }
    CallResult::ok(out.join("\n"))
}

fn kind_key(k: ChannelKind) -> &'static str {
    match k {
        ChannelKind::Lobby => "lobby",
        ChannelKind::Human => "human",
        ChannelKind::Topic => "topic",
        ChannelKind::Dm => "dm",
    }
}

pub async fn human_watching(cx: &Ctx<'_>) -> CallResult {
    let root = match port::root_human_channel(cx.st, &cx.me.crew_id).await {
        Ok(c) => c,
        Err(e) => return cx.fail(e),
    };
    let watched = match port::watched_channels(cx.st).await {
        Ok(v) => v,
        Err(e) => return cx.fail(e),
    };
    let mut out = vec![cx.r("r.watch_root", &[("id", &root.id), ("name", &super::channel_label(cx, &root))])];
    if watched.is_empty() {
        out.push(cx.r("r.watch_none", &[]));
    }
    for c in watched {
        let key = if c.has_session(&cx.me.session_id) { "r.watch_line_member" } else { "r.watch_line" };
        out.push(cx.r(key, &[("id", &c.id), ("name", &super::channel_label(cx, &c))]));
    }
    CallResult::ok(out.join("\n"))
}
```

`r.kind_{…}` 这种拼出来的键扫描尺子看不到：四个键 `r.kind_lobby` / `r.kind_human` / `r.kind_topic` / `r.kind_dm` 在下面的 md 里都要写；`literals.rs` 的 `every_text_key_used_in_source_exists_in_both_locales` 顺手加一条手工断言列出这四个。

- [ ] **Step 5: md 加节，并改一处**

`## post.messages` 两种语言末尾各补一句：zh「mentions、reply_to 只挂在第一条上。」en「mentions and reply_to apply to the first message only.」

`tools.zh.md` 追加：

```markdown
## r.post_one_of
message 与 messages 二选一，且用 messages 时把 category 写在每一条里。

## r.post_count
messages 要 1–6 条，你给了 {count} 条。


## r.post_empty
第 {index} 条是空的。

## r.bad_category
第 {index} 条的 category「{value}」不认识，可选：{choices}。

## r.bad_mention
mention 写法不对（kind = {kind}）：kind 只能是 session / captain / human / broadcast，session 必须带 target_id。

## r.no_message
找不到消息 {ref}。

## r.reply_other_channel
#{seq} 不在你指定的群里；回复要发在原消息所在的群，去掉 channel 即可。

## r.post_partial
前面的已发出（{sent}），第 {index} / {total} 条没发出去：{detail}。别整批重发，只补没发的。

## r.posted
已发到「{channel}」：{seqs}。

## r.human_not_watching
人类现在没有注视「{channel}」：@人类 的标记已经亮了，但可能很久才被看到。急事或要拍板的事发根机组人类群（human_watching 查），或用 ask。

## r.channel_choice
- {id} 「{name}」

## r.no_channel
找不到群「{arg}」。你能用的群：
{choices}

## r.ambiguous_channel
有不止一个群叫「{arg}」，请用 id：
{choices}

## r.who_human
人类

## r.who_system
系统

## r.line
#{seq} {time} {who}：{text}

## r.line_attachment
 ［附件 {path}］

## r.read_empty
「{channel}」里还没有消息。

## r.read_header
「{channel}」（{id}）的 {count} 条消息，旧在前：

## r.read_more
更早的还有：before 填 {before} 继续往前翻。

## r.search_scope
只能搜你所在或收听的群；「{channel}」不在其中，可以用 read_channel 读它。

## r.search_none
没搜到「{query}」。

## r.search_hit
［{channel}］{line}

## r.list_header
群（id · 名字 · 类型 · 用途 · 群主 · 人数 · 状态）：

## r.list_line
- {id} · {name} · {kind} · {purpose} · 群主 {owner} · {count} 人 · {flags}

## r.flag_member
你在群里

## r.flag_listening
你在收听

## r.flag_watched
人类在注视

## r.flag_archived
已归档

## r.owner_none
（无）

## r.kind_lobby
大厅

## r.kind_human
人类群

## r.kind_topic
话题群

## r.kind_dm
私聊

## r.watch_root
根机组人类群：{id}「{name}」——要人拍板、找人，发这里。

## r.watch_none
人类现在没有标注视任何群。

## r.watch_line
人类在注视：{id}「{name}」（你不在群里）

## r.watch_line_member
人类在注视：{id}「{name}」
```

`tools.en.md` 追加（节名一一对应）：

```markdown
## r.post_one_of
Give either message or messages, not both; with messages put category inside each item.

## r.post_count
messages needs 1–6 items, you gave {count}.


## r.post_empty
Item {index} is empty.

## r.bad_category
Item {index} has an unknown category "{value}"; choose from: {choices}.

## r.bad_mention
Invalid mention (kind = {kind}): kind must be session / captain / human / broadcast, and session needs target_id.

## r.no_message
Message {ref} not found.

## r.reply_other_channel
#{seq} is not in the channel you named; replies go to the original message's channel — just drop channel.

## r.post_partial
The first ones were posted ({sent}); item {index} of {total} was not: {detail}. Do not resend the batch, only the missing ones.

## r.posted
Posted to "{channel}": {seqs}.

## r.human_not_watching
The human is not watching "{channel}": the @human badge is lit but may be seen much later. For urgent things or decisions post in the root crew's human channel (see human_watching) or use ask.

## r.channel_choice
- {id} "{name}"

## r.no_channel
No channel "{arg}". Channels you can use:
{choices}

## r.ambiguous_channel
More than one channel is called "{arg}"; use the id:
{choices}

## r.who_human
human

## r.who_system
system

## r.line
#{seq} {time} {who}: {text}

## r.line_attachment
 [attachment {path}]

## r.read_empty
"{channel}" has no messages yet.

## r.read_header
{count} messages in "{channel}" ({id}), oldest first:

## r.read_more
There are older ones: pass before = {before} to page back.

## r.search_scope
You can only search channels you are in or listen to; "{channel}" is not one of them — read it with read_channel.

## r.search_none
Nothing found for "{query}".

## r.search_hit
[{channel}] {line}

## r.list_header
Channels (id · name · type · purpose · owner · members · state):

## r.list_line
- {id} · {name} · {kind} · {purpose} · owner {owner} · {count} members · {flags}

## r.flag_member
you are a member

## r.flag_listening
you are listening

## r.flag_watched
human is watching

## r.flag_archived
archived

## r.owner_none
(none)

## r.kind_lobby
lobby

## r.kind_human
human channel

## r.kind_topic
topic

## r.kind_dm
dm

## r.watch_root
Root crew's human channel: {id} "{name}" — post here to reach the human or get a decision.

## r.watch_none
The human is not watching any channel right now.

## r.watch_line
Human is watching: {id} "{name}" (you are not a member)

## r.watch_line_member
Human is watching: {id} "{name}"
```

`literals.rs` 的 `every_text_key_used_in_source_exists_in_both_locales` 里 `for loc` 循环前加：

```rust
    for k in ["r.kind_lobby", "r.kind_human", "r.kind_topic", "r.kind_dm"] {
        used.insert(k.to_string()); // 由 format! 拼出来的键，扫描看不到
    }
```

- [ ] **Step 6: 跑通**

Run: `cargo test -p crewd-server --lib agent::tools agent::literals`
Expected: messaging `9 passed`，literals 仍 3 条全过。

- [ ] **Step 7: 变异核对**

1. `post` 里把 `if unwatched` 换成 `if !seqs.is_empty()`：Expected `human_mention_in_unwatched_channel_warns_and_human_channel_does_not` 红。
2. `read_channel` 的 `if before.is_none() && mine` 换成 `if mine`：Expected `read_channel_pages_and_marks_read_only_at_the_latest_page` 红。
3. `post_as` 里删掉 `note_post` 调用：Expected `posting_with_captain_mention_counts_as_calling_out` 红。

逐条撤销。

- [ ] **Step 8: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/server/src/agent/tools/messaging.rs crates/server/src/agent/tools/mod.rs crates/server/src/agent/literals.rs \
  crates/collab/prompts/tools.zh.md crates/collab/prompts/tools.en.md
git diff --cached --stat
git commit -m "crewd-server: messaging tools (post with channels/batches/reply numbers, read_channel, search, list_channels, human_watching)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 13: 群与通讯录工具（建群 / 拉人 / 退群 / 归档 / 群主转让 / 收听 / `directory` / `contact`）

**Files:**
- Create: `crewd/crates/server/src/agent/tools/channels.rs`
- Modify: `agent/tools/mod.rs`（分发 + `resolve_member`）、`crates/collab/prompts/tools.{zh,en}.md`

**Interfaces:**
- Consumes: Task 12 `resolve_channel`、`post_as`、`channel_label`；Task 6 `port::{create_channel, ensure_dm, add_members, remove_member, archive_channel, request_owner_transfer, accept_owner_transfer, force_owner, set_listen, clear_listen, directory, resolve_number, sessions_of, crew, lobby_of, identity, now_ms}`（M3a 的 `channels::*` 按调用者判群权限：群主 / 机长 / 人）。
- Produces: `tools::resolve_member(cx: &Ctx<'_>, s: &str) -> Result<MemberRef, CallResult>`（`human` / `人类` → Human；`7-3` 之类 → 号码解析；其余当 session id 且必须存在）。

权限：群的权限规则（归档 = 群主 / 机长 / 人、转让只能群主发起、`force` 只给机长）由 M3a 的 `channels::*` 按调用者判，M3b 不另写一份，只把 `Forbidden` 原样回执；M3b 自己只判工具层面的：拉人要求自己是成员或机长、大厅不能退、大厅与人类群不能归档、dm 恰好两人、`force` 只给机长（给出专门的回执），新群主必须是成员；转让请求以系统身份在群里 @ 对方（叫醒它来接受）。`contact` 的跨机组发言用系统身份 + 署名（裁定 7），自己大厅留「已联系 …」。

- [ ] **Step 1: 写失败的测试**（`channels.rs` 末尾）

```rust
#[cfg(test)]
mod tests {
    use crewd_proto::MentionKind;
    use serde_json::json;

    use crate::agent::port::{self, ChannelKind};
    use crate::agent::testkit::Fixture;
    use crate::agent::tools::call;
    use crate::agent::Role;

    async fn run(f: &Fixture, who: &str, tool: &str, args: serde_json::Value) -> crewd_proto::agent::CallResult {
        let me = f.me(who).await;
        call(&f.state, &me, tool, args).await
    }

    async fn topic_by_captain(f: &Fixture) -> port::ChannelView {
        let r = run(f, &f.captain, "channel_create", json!({"name": "示例话题", "purpose": "演示", "members": [f.worker]})).await;
        assert!(!r.is_error, "{}", r.text);
        port::crew_channels(&f.state, &f.crew).await.unwrap().into_iter().find(|c| c.name == "示例话题").unwrap()
    }

    #[tokio::test]
    async fn create_topic_and_dm_once() {
        let f = Fixture::new().await;
        let t = topic_by_captain(&f).await;
        assert_eq!(t.kind, ChannelKind::Topic);
        assert_eq!(t.owner.as_deref(), Some(f.captain.as_str()), "建群者是群主");
        assert!(t.has_session(&f.worker) && t.has_session(&f.captain));
        f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker").await;
        let a = run(&f, &f.worker, "channel_create", json!({"name": "私聊", "members": ["s-w2"], "kind": "dm"})).await;
        assert!(!a.is_error, "{}", a.text);
        let b = run(&f, &f.worker, "channel_create", json!({"name": "私聊2", "members": ["s-w2"], "kind": "dm"})).await;
        assert!(!b.is_error);
        let dms: Vec<_> = port::crew_channels(&f.state, &f.crew).await.unwrap().into_iter().filter(|c| c.kind == ChannelKind::Dm).collect();
        assert_eq!(dms.len(), 1, "同一对 session 的私聊只有一个");
        let bad = run(&f, &f.worker, "channel_create", json!({"name": "x", "members": ["s-w2", "human"], "kind": "dm"})).await;
        assert!(bad.is_error);
    }

    #[tokio::test]
    async fn invite_leave_archive_rules() {
        let f = Fixture::new().await;
        f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker").await;
        let t = topic_by_captain(&f).await;
        assert!(run(&f, "s-w2", "channel_invite", json!({"channel": t.id, "members": ["s-w2"]})).await.is_error, "不是成员不能拉人");
        assert!(!run(&f, &f.worker, "channel_invite", json!({"channel": t.id, "members": ["s-w2"]})).await.is_error);
        let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap();
        assert!(run(&f, &f.worker, "channel_leave", json!({"channel": lobby.id})).await.is_error, "大厅退不了");
        assert!(run(&f, &f.worker, "channel_archive", json!({"channel": t.id})).await.is_error, "不是群主");
        assert!(run(&f, &f.captain, "channel_archive", json!({"channel": lobby.id})).await.is_error, "大厅不能归档");
        assert!(!run(&f, "s-w2", "channel_leave", json!({"channel": t.id})).await.is_error);
        assert!(!run(&f, &f.captain, "channel_archive", json!({"channel": t.id})).await.is_error);
        assert!(port::channel(&f.state, &t.id).await.unwrap().unwrap().archived);
    }

    #[tokio::test]
    async fn ownership_transfer_needs_both_sides_or_captain_force() {
        let f = Fixture::new().await;
        let t = topic_by_captain(&f).await;
        assert!(run(&f, &f.worker, "channel_transfer", json!({"channel": t.id, "to": f.worker})).await.is_error, "非群主不能发起");
        assert!(run(&f, &f.worker, "channel_accept", json!({"channel": t.id})).await.is_error, "没有待接受的");
        assert!(!run(&f, &f.captain, "channel_transfer", json!({"channel": t.id, "to": f.worker})).await.is_error);
        let notice = port::messages(&f.state, &t.id, None, 1).await.unwrap().pop().unwrap();
        assert!(notice.mentions.iter().any(|m| m.target_id.as_deref() == Some(f.worker.as_str())), "叫醒对方来接受");
        assert_eq!(port::channel(&f.state, &t.id).await.unwrap().unwrap().owner.as_deref(), Some(f.captain.as_str()), "接受前不变");
        assert!(!run(&f, &f.worker, "channel_accept", json!({"channel": t.id})).await.is_error);
        assert_eq!(port::channel(&f.state, &t.id).await.unwrap().unwrap().owner.as_deref(), Some(f.worker.as_str()));
        // 机长强制转回来
        assert!(!run(&f, &f.captain, "channel_transfer", json!({"channel": t.id, "to": f.captain, "force": true})).await.is_error);
        assert_eq!(port::channel(&f.state, &t.id).await.unwrap().unwrap().owner.as_deref(), Some(f.captain.as_str()));
        assert!(run(&f, &f.worker, "channel_transfer", json!({"channel": t.id, "to": f.worker, "force": true})).await.is_error, "force 只给机长");
    }

    #[tokio::test]
    async fn listen_and_unlisten() {
        let f = Fixture::new().await;
        let cap = f.me(&f.captain).await;
        let t = port::create_channel(&f.state, &cap, ChannelKind::Topic, "旁听".into(), None, vec![]).await.unwrap();
        assert!(run(&f, &f.worker, "channel_listen", json!({"channel": t.id, "minutes": 0})).await.is_error);
        assert!(!run(&f, &f.worker, "channel_listen", json!({"channel": t.id, "minutes": 30, "senders": ["human"]})).await.is_error);
        let mine = port::my_channels(&f.state, &f.worker).await.unwrap();
        assert!(mine.iter().any(|c| c.id == t.id && c.listening));
        assert!(!run(&f, &f.worker, "channel_unlisten", json!({"channel": t.id})).await.is_error);
        assert!(!port::my_channels(&f.state, &f.worker).await.unwrap().iter().any(|c| c.id == t.id));
    }

    #[tokio::test]
    async fn contact_crew_posts_signed_system_message_and_leaves_a_trace() {
        let f = Fixture::new().await;
        let other = f.add_crew("另一机组", None).await;
        f.add_session(&other, "s-cap2", Role::Captain, "另一位机长").await;
        let number = port::crew(&f.state, &other).await.unwrap().unwrap().number.unwrap().to_string();
        let r = run(&f, &f.worker, "contact", json!({"to": number, "message": "借一下测试机"})).await;
        assert!(!r.is_error, "{}", r.text);
        let there = port::messages(&f.state, &port::lobby_of(&f.state, &other).await.unwrap().id, None, 1).await.unwrap().pop().unwrap();
        assert!(there.text.contains("借一下测试机") && there.text.contains("示例 worker"), "{}", there.text);
        assert!(there.mentions.iter().any(|m| m.kind == MentionKind::Captain));
        let here = port::messages(&f.state, &port::lobby_of(&f.state, &f.crew).await.unwrap().id, None, 1).await.unwrap().pop().unwrap();
        assert!(here.text.contains("另一机组"), "自己大厅留痕：{}", here.text);
        assert!(run(&f, &f.worker, "contact", json!({"to": "999", "message": "x"})).await.is_error, "查无此号");
    }

    #[tokio::test]
    async fn contact_extension_reuses_one_dm() {
        let f = Fixture::new().await;
        let other = f.add_crew("另一机组", None).await;
        let x = f.register(&other, Role::Worker, "外组 worker").await;
        let ext = port::sessions_of(&f.state, &other).await.unwrap().into_iter().find(|s| s.id == x).unwrap().extension.unwrap();
        for i in 0..2 {
            let r = run(&f, &f.worker, "contact", json!({"to": ext, "message": format!("第{i}句")})).await;
            assert!(!r.is_error, "{}", r.text);
        }
        let (dm, created) = port::ensure_dm(&f.state, &f.me(&f.worker).await, &x).await.unwrap();
        assert!(!created, "两次 contact 用的是同一个私聊");
        assert_eq!(port::messages(&f.state, &dm.id, None, 10).await.unwrap().len(), 2);
        assert!(port::my_channels(&f.state, &x).await.unwrap().iter().any(|c| c.id == dm.id));
    }

    #[tokio::test]
    async fn directory_lists_numbers() {
        let f = Fixture::new().await;
        let r = run(&f, &f.worker, "directory", json!({})).await;
        assert!(!r.is_error && r.text.contains("示例机组"), "{}", r.text);
    }
}
```

- [ ] **Step 2: 跑一遍确认失败**

Run: `cargo test -p crewd-server --lib agent::tools::channels`
Expected: 编译失败。

- [ ] **Step 3: `tools/mod.rs` 加 `resolve_member` 与分发**

```rust
pub async fn resolve_member(cx: &Ctx<'_>, s: &str) -> Result<MemberRef, CallResult> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("human") || s == cx.r("r.who_human", &[]) {
        return Ok(MemberRef::Human);
    }
    if s.contains('-') && s.split('-').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
        return match port::resolve_number(cx.st, s).await {
            Ok(Some(port::NumberTarget::Session { session_id, .. })) => Ok(MemberRef::Session(session_id)),
            Ok(_) => Err(cx.err("r.no_number", &[("to", s)])),
            Err(e) => Err(cx.fail(e)),
        };
    }
    match port::identity(cx.st, s).await {
        Ok(Some(_)) => Ok(MemberRef::Session(s.to_string())),
        Ok(None) => Err(cx.err("r.no_session", &[("id", s)])),
        Err(e) => Err(cx.fail(e)),
    }
}
```

`use crate::agent::port::MemberRef;`；`pub mod channels;`。在 `tools/mod.rs` **顶部、所有 `pub mod …;` 之前**加三个宏（`macro_rules!` 按文本顺序对后面声明的子模块可见；宏体里的 `parse_args` / `resolve_channel` 在调用处解析，子模块要 `use super::{parse_args, resolve_channel}`）：

```rust
/// 解析参数，失败直接返回 `r.bad_args`。
macro_rules! args {
    ($cx:expr, $v:expr) => {
        match parse_args($cx, $v) {
            Ok(a) => a,
            Err(e) => return e,
        }
    };
}
/// await 一个 port 调用，失败直接返回 `r.failed`。
macro_rules! tryp {
    ($cx:expr, $e:expr) => {
        match $e.await {
            Ok(v) => v,
            Err(e) => return $cx.fail(e),
        }
    };
}
/// 解析群参数，失败直接返回它的回执。
macro_rules! channel {
    ($cx:expr, $arg:expr) => {
        match resolve_channel($cx, Some($arg)).await {
            Ok(c) => c,
            Err(e) => return e,
        }
    };
}
```

`match tool` 加：

```rust
        "channel_create" => channels::create(&cx, args).await,
        "channel_invite" => channels::invite(&cx, args).await,
        "channel_leave" => channels::leave(&cx, args).await,
        "channel_archive" => channels::archive(&cx, args).await,
        "channel_transfer" => channels::transfer(&cx, args).await,
        "channel_accept" => channels::accept(&cx, args).await,
        "channel_listen" => channels::listen(&cx, args).await,
        "channel_unlisten" => channels::unlisten(&cx, args).await,
        "directory" => channels::directory(&cx, args).await,
        "contact" => channels::contact(&cx, args).await,
```

- [ ] **Step 4: 写 `channels.rs`**

```rust
//! 群与通讯录工具（spec §22.3、§22.7 Q2）。
use crewd_proto::{Mention, MentionKind};
use crewd_proto::agent::CallResult;
use serde::Deserialize;
use serde_json::Value;

use super::{Ctx, parse_args, post_as, resolve_channel, resolve_member};
use crate::agent::Role;
use crate::agent::port::{self, ChannelKind, ChannelView, MemberRef, NumberTarget, PostSender};

fn captain_of(cx: &Ctx<'_>, ch: &ChannelView) -> bool {
    cx.me.role == Role::Captain && ch.crew_id == cx.me.crew_id
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    name: String,
    purpose: Option<String>,
    members: Vec<String>,
    kind: Option<String>,
}

pub async fn create(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: CreateArgs = args!(cx, v);
    if a.name.trim().is_empty() {
        return cx.err("r.bad_args", &[("detail", "name")]);
    }
    let mut members = Vec::new();
    for m in &a.members {
        match resolve_member(cx, m).await {
            Ok(MemberRef::Session(s)) if s == cx.me.session_id => {}
            Ok(r) => {
                if !members.contains(&r) {
                    members.push(r);
                }
            }
            Err(e) => return e,
        }
    }
    let kind = match a.kind.as_deref() {
        None | Some("topic") => ChannelKind::Topic,
        Some("dm") => ChannelKind::Dm,
        Some(other) => return cx.err("r.bad_args", &[("detail", &format!("kind={other}"))]),
    };
    if kind == ChannelKind::Dm {
        let [MemberRef::Session(other)] = members.as_slice() else {
            return cx.err("r.dm_two", &[]);
        };
        let (dm, created) = tryp!(cx, port::ensure_dm(cx.st, cx.me, other));
        let key = if created { "r.channel_created" } else { "r.dm_exists" };
        return cx.ok(key, &[("id", &dm.id), ("name", &super::channel_label(cx, &dm))]);
    }
    let ch = tryp!(cx, port::create_channel(cx.st, cx.me, kind, a.name.trim().to_string(), a.purpose, members));
    cx.ok("r.channel_created", &[("id", &ch.id), ("name", &super::channel_label(cx, &ch))])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InviteArgs {
    channel: String,
    members: Vec<String>,
}

pub async fn invite(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: InviteArgs = args!(cx, v);
    let ch = channel!(cx, &a.channel);
    if !ch.has_session(&cx.me.session_id) && !captain_of(cx, &ch) {
        return cx.err("r.not_member", &[("channel", &super::channel_label(cx, &ch))]);
    }
    let mut members = Vec::new();
    for m in &a.members {
        match resolve_member(cx, m).await {
            Ok(r) => members.push(r),
            Err(e) => return e,
        }
    }
    let ch = tryp!(cx, port::add_members(cx.st, cx.me, &ch.id, members));
    cx.ok("r.invited", &[("channel", &super::channel_label(cx, &ch)), ("count", &ch.members.len().to_string())])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChannelArg {
    channel: String,
}

pub async fn leave(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: ChannelArg = args!(cx, v);
    let ch = channel!(cx, &a.channel);
    if ch.kind == ChannelKind::Lobby {
        return cx.err("r.cannot_leave_lobby", &[]);
    }
    tryp!(cx, port::remove_member(cx.st, cx.me, &ch.id, MemberRef::Session(cx.me.session_id.clone())));
    cx.ok("r.left", &[("channel", &super::channel_label(cx, &ch))])
}

pub async fn archive(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: ChannelArg = args!(cx, v);
    let ch = channel!(cx, &a.channel);
    if matches!(ch.kind, ChannelKind::Lobby | ChannelKind::Human) {
        return cx.err("r.cannot_archive_kind", &[("channel", &super::channel_label(cx, &ch))]);
    }
    // 群主 / 机长才能归档：由 M3a 的 `channels::update` 按 `by` 判（Forbidden → r.failed），这里不重复判。
    tryp!(cx, port::archive_channel(cx.st, cx.me, &ch.id));
    cx.ok("r.archived", &[("channel", &super::channel_label(cx, &ch))])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransferArgs {
    channel: String,
    to: String,
    #[serde(default)]
    force: bool,
}

pub async fn transfer(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: TransferArgs = args!(cx, v);
    let ch = channel!(cx, &a.channel);
    let to = match resolve_member(cx, &a.to).await {
        Ok(MemberRef::Session(s)) => s,
        Ok(MemberRef::Human) => return cx.err("r.transfer_not_member", &[("to", &a.to), ("channel", &super::channel_label(cx, &ch))]),
        Err(e) => return e,
    };
    if !ch.has_session(&to) {
        return cx.err("r.transfer_not_member", &[("to", &a.to), ("channel", &super::channel_label(cx, &ch))]);
    }
    if a.force {
        if !captain_of(cx, &ch) {
            return cx.err("r.force_captain_only", &[]);
        }
        tryp!(cx, port::force_owner(cx.st, cx.me, &ch.id, &to));
        return cx.ok("r.transfer_forced", &[("channel", &super::channel_label(cx, &ch)), ("to", &a.to)]);
    }
    // 只有群主能发起：M3a 的 `request_owner_transfer` 判（Forbidden → r.failed）。
    tryp!(cx, port::request_owner_transfer(cx.st, cx.me, &ch.id, &to));
    let notice = cx.r("r.transfer_notice", &[("from", &cx.me.label), ("channel", &super::channel_label(cx, &ch))]);
    let mention = Mention { kind: MentionKind::Session, target_id: Some(to.clone()) };
    if let Err(e) = post_as(cx, &ch, notice, vec![mention], None, None, PostSender::System).await {
        return e;
    }
    cx.ok("r.transfer_requested", &[("channel", &super::channel_label(cx, &ch)), ("to", &a.to)])
}

pub async fn accept(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: ChannelArg = args!(cx, v);
    let ch = channel!(cx, &a.channel);
    tryp!(cx, port::accept_owner_transfer(cx.st, cx.me, &ch.id));
    cx.ok("r.transfer_accepted", &[("channel", &super::channel_label(cx, &ch))])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListenArgs {
    channel: String,
    minutes: Option<f64>,
    #[serde(default)]
    senders: Vec<String>,
}

pub async fn listen(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: ListenArgs = args!(cx, v);
    let ch = channel!(cx, &a.channel);
    let until = match a.minutes {
        None => None,
        Some(m) if (1.0..=480.0).contains(&m) => Some(port::now_ms() + (m * 60_000.0) as i64),
        Some(_) => return cx.err("r.listen_minutes", &[]),
    };
    let senders: Vec<String> = a.senders.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    tryp!(cx, port::set_listen(cx.st, &cx.me.session_id, &ch.id, until, senders));
    match a.minutes {
        Some(m) => cx.ok("r.listening_for", &[("channel", &super::channel_label(cx, &ch)), ("minutes", &format!("{m}"))]),
        None => cx.ok("r.listening", &[("channel", &super::channel_label(cx, &ch))]),
    }
}

pub async fn unlisten(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: ChannelArg = args!(cx, v);
    let ch = channel!(cx, &a.channel);
    tryp!(cx, port::clear_listen(cx.st, &cx.me.session_id, &ch.id));
    cx.ok("r.unlistened", &[("channel", &super::channel_label(cx, &ch))])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirArgs {
    query: Option<String>,
}

pub async fn directory(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: DirArgs = args!(cx, v);
    let rows = tryp!(cx, port::directory(cx.st, a.query.filter(|q| !q.trim().is_empty())));
    let mine = tryp!(cx, port::sessions_of(cx.st, &cx.me.crew_id))
        .into_iter()
        .find(|s| s.id == cx.me.session_id)
        .and_then(|s| s.extension)
        .unwrap_or_default();
    let mut out = vec![cx.r("r.dir_header", &[("me", &mine)])];
    out.extend(rows.iter().map(|r| {
        cx.r("r.dir_line", &[("number", &r.number), ("name", &r.name), ("crew", &r.crew_title), ("detail", &r.detail)])
    }));
    if rows.is_empty() {
        out.push(cx.r("r.dir_empty", &[]));
    }
    CallResult::ok(out.join("\n"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContactArgs {
    to: String,
    message: String,
}

pub async fn contact(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: ContactArgs = args!(cx, v);
    if a.message.trim().is_empty() {
        return cx.err("r.bad_args", &[("detail", "message")]);
    }
    let Some(target) = tryp!(cx, port::resolve_number(cx.st, a.to.trim())) else {
        return cx.err("r.no_number", &[("to", &a.to)]);
    };
    let my_crew = tryp!(cx, port::crew(cx.st, &cx.me.crew_id));
    let my_crew_title = my_crew.as_ref().map(|c| c.title.clone()).unwrap_or_default();
    let my_number = my_crew.as_ref().and_then(|c| c.number).map(|n| n.to_string()).unwrap_or_default();
    let target_name;
    match target {
        NumberTarget::Crew(crew_id) => {
            if crew_id == cx.me.crew_id {
                return cx.err("r.contact_self", &[]);
            }
            let lobby = tryp!(cx, port::lobby_of(cx.st, &crew_id));
            target_name = tryp!(cx, port::crew(cx.st, &crew_id)).map(|c| c.title).unwrap_or(crew_id);
            let body = cx.r(
                "r.contact_from",
                &[("crew", &my_crew_title), ("number", &my_number), ("label", &cx.me.label), ("message", a.message.trim())],
            );
            let mention = Mention { kind: MentionKind::Captain, target_id: None };
            if let Err(e) = post_as(cx, &lobby, body, vec![mention], None, None, PostSender::System).await {
                return e;
            }
        }
        NumberTarget::Session { session_id, .. } => {
            if session_id == cx.me.session_id {
                return cx.err("r.contact_self", &[]);
            }
            let other = tryp!(cx, port::identity(cx.st, &session_id));
            target_name = other.map(|o| o.label).unwrap_or(session_id.clone());
            let (dm, _) = tryp!(cx, port::ensure_dm(cx.st, cx.me, &session_id));
            let mention = Mention { kind: MentionKind::Session, target_id: Some(session_id.clone()) };
            let me_sender = PostSender::Session(cx.me.session_id.clone());
            if let Err(e) = post_as(cx, &dm, a.message.trim().to_string(), vec![mention], None, None, me_sender).await {
                return e;
            }
        }
    }
    let lobby = tryp!(cx, port::lobby_of(cx.st, &cx.me.crew_id));
    let snippet: String = a.message.trim().chars().take(40).collect();
    let trace = cx.r("r.contact_trace", &[("label", &cx.me.label), ("to", &a.to), ("target", &target_name), ("snippet", &snippet)]);
    if let Err(e) = post_as(cx, &lobby, trace, vec![], None, None, PostSender::System).await {
        return e;
    }
    cx.ok("r.contacted", &[("to", &a.to), ("target", &target_name)])
}
```

- [ ] **Step 5: md 加节**

`tools.zh.md`：

```markdown
## r.no_number
通讯录里没有号码 {to}（用 directory 查）。

## r.no_session
没有 session {id}。

## r.dm_two
私聊恰好两个人：members 只填对方一个 session。

## r.dm_exists
你们已经有私聊了：{id}「{name}」。

## r.channel_created
建好了：{id}「{name}」，你是群主。

## r.not_member
你不在「{channel}」里，不能拉人。

## r.invited
已拉进「{channel}」，现在 {count} 人。

## r.cannot_leave_lobby
大厅退不了。

## r.left
已退出「{channel}」。

## r.cannot_archive_kind
「{channel}」是大厅或人类群，不能归档。


## r.archived
已归档「{channel}」。

## r.transfer_not_member
{to} 不是「{channel}」的成员，先拉进来再转。

## r.force_captain_only
force 只有该群所属机组的机长能用。

## r.transfer_forced
「{channel}」的群主已转给 {to}。


## r.transfer_notice
{from} 想把「{channel}」的群主转给你：同意就调 channel_accept。

## r.transfer_requested
已请 {to} 接手「{channel}」的群主，对方 channel_accept 之后生效。

## r.transfer_accepted
你现在是「{channel}」的群主。

## r.listen_minutes
minutes 要在 1–480 之间；不填就一直听到 channel_unlisten。

## r.listening_for
在收听「{channel}」，{minutes} 分钟后自动停。正常结束回合等着就行。

## r.listening
在收听「{channel}」，直到 channel_unlisten。正常结束回合等着就行。

## r.unlistened
已停止收听「{channel}」。

## r.dir_header
通讯录（你的分机号：{me}）：

## r.dir_line
- {number} {name}（{crew}）{detail}

## r.dir_empty
没有匹配的号码。

## r.contact_self
这是你自己（或你自己的机组）：本机组的事直接 post。

## r.contact_from
来自「{crew}」（{number}）的 {label}：
{message}


## r.contact_trace
{label} 已联系 {to}（{target}）：{snippet}

## r.contacted
已联系 {to}（{target}），本机组大厅留了记录。
```

`tools.en.md`：

```markdown
## r.no_number
No directory number {to} (look it up with directory).

## r.no_session
No session {id}.

## r.dm_two
A dm has exactly two people: put only the other session in members.

## r.dm_exists
You already have a dm: {id} "{name}".

## r.channel_created
Created {id} "{name}"; you are the owner.

## r.not_member
You are not in "{channel}", so you cannot add people.

## r.invited
Added to "{channel}"; it now has {count} members.

## r.cannot_leave_lobby
You cannot leave the lobby.

## r.left
Left "{channel}".

## r.cannot_archive_kind
"{channel}" is a lobby or human channel and cannot be archived.


## r.archived
Archived "{channel}".

## r.transfer_not_member
{to} is not a member of "{channel}"; add them first.

## r.force_captain_only
force is only for the captain of the crew the channel belongs to.

## r.transfer_forced
Ownership of "{channel}" moved to {to}.


## r.transfer_notice
{from} wants to hand ownership of "{channel}" to you: call channel_accept to take it.

## r.transfer_requested
Asked {to} to take over "{channel}"; it takes effect once they channel_accept.

## r.transfer_accepted
You now own "{channel}".

## r.listen_minutes
minutes must be 1–480; omit it to listen until channel_unlisten.

## r.listening_for
Listening to "{channel}" for {minutes} minutes. Just end your turn and wait.

## r.listening
Listening to "{channel}" until channel_unlisten. Just end your turn and wait.

## r.unlistened
Stopped listening to "{channel}".

## r.dir_header
Directory (your extension: {me}):

## r.dir_line
- {number} {name} ({crew}) {detail}

## r.dir_empty
No matching numbers.

## r.contact_self
That is you (or your own crew): for your own crew just post.

## r.contact_from
From {label} of "{crew}" ({number}):
{message}


## r.contact_trace
{label} contacted {to} ({target}): {snippet}

## r.contacted
Contacted {to} ({target}); a note was left in your lobby.
```

- [ ] **Step 6: 跑通**

Run: `cargo test -p crewd-server --lib agent::tools::channels`
Expected: `7 passed`。

- [ ] **Step 7: 变异核对**

1. `invite` 的「是成员或机长」判断换成 `if false`：Expected `invite_leave_archive_rules` 红（「不是成员不能拉人」那句）。
2. `transfer` 里 `if a.force { if !captain_of(..) {..} }` 的内层判断删掉：Expected `ownership_transfer_needs_both_sides_or_captain_force` 红（「force 只给机长」那句）。
3. `create` 的 dm 分支把 `port::ensure_dm` 换成 `port::create_channel(.., ChannelKind::Dm, ..)`：Expected `create_topic_and_dm_once` 红（两个私聊）。

逐条撤销。

- [ ] **Step 8: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/server/src/agent/tools/channels.rs crates/server/src/agent/tools/mod.rs \
  crates/collab/prompts/tools.zh.md crates/collab/prompts/tools.en.md
git diff --cached --stat
git commit -m "crewd-server: channel tools (create/invite/leave/archive, owner transfer, listen) and directory/contact

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 14: 问人与自我配置工具（`ask` / `withdraw_ask` / `respond_todo` / `get_quota` / `schedule_wakeup` / `set_session_profile`）

**Files:**
- Create: `crewd/crates/server/src/agent/tools/asks.rs`
- Modify: `agent/tools/mod.rs`（分发）、`agent/testkit.rs`（`add_agent_todo`、`wakes_of`）、`crewd/crates/server/Cargo.toml`（`chrono`）、`crates/collab/prompts/tools.{zh,en}.md`

**Interfaces:**
- Consumes: Task 6 `port::{open_ask, withdraw_ask, respond_todo, schedule_timer, quota, set_profile, now_ms}`（`respond_todo` 依赖 G3）；Task 11 `turn::note_post`。
- Produces: 无新的跨任务接口。testkit：`Fixture::add_agent_todo(&self, number: i64, text: &str)`（经 M1 的 `crewd_store::todos::insert`）、`Fixture::wakes_of(&self, session: &str) -> Vec<(String /*kind*/, i64 /*not_before ms*/, String /*note*/)>`（`<M3a>` 读 `wakes` 表）。

`ask` 不阻塞（spec §22.3 表：`open_ask` 发 H 号）；问出去 = 这一轮叫过人（`note_post` 记 human）。`fallback_after_minutes` 与 `fallback` 必须成对，换算成 M3a 的 `AskDefault { text, after_minutes }`；spec §13 Q2：到点照常走权限审批，所以工具说明里只说「可撤回的事」，不在代码里判可撤回。`withdraw_ask` 的权限由 crewd-work 判（提问者 / 机长 / 人；还卡着活项的拒绝）。`schedule_wakeup` 入 `wakes(kind=timer, priority=system)`（spec §22.4 优先级表），`at` 按 RFC 3339 解析、必须在未来。

- [ ] **Step 1: 加依赖**

```bash
cargo add -p crewd-server chrono --no-default-features --features std,clock
```

（chrono 0.4.45 已在 lock 里，不新增第三方 crate。）

- [ ] **Step 2: 写失败的测试**（`asks.rs` 末尾）

```rust
#[cfg(test)]
mod tests {
    use crewd_proto::MentionKind;
    use serde_json::json;

    use crate::agent::port;
    use crate::agent::testkit::Fixture;
    use crate::agent::tools::call;
    use crate::agent::Role;

    async fn run(f: &Fixture, who: &str, tool: &str, args: serde_json::Value) -> crewd_proto::agent::CallResult {
        let me = f.me(who).await;
        call(&f.state, &me, tool, args).await
    }

    #[tokio::test]
    async fn ask_issues_an_h_number_posts_to_humans_and_does_not_block() {
        let f = Fixture::new().await;
        let r = run(&f, &f.worker, "ask", json!({"question": "A / B，我倾向 A", "resume_note": "答复后接着改 B 模块"})).await;
        assert!(!r.is_error, "{}", r.text);
        assert!(r.text.contains("H1"), "{}", r.text);
        let human = port::root_human_channel(&f.state, &f.crew).await.unwrap();
        let m = port::messages(&f.state, &human.id, None, 1).await.unwrap().pop().unwrap();
        assert!(m.text.contains("A / B，我倾向 A") && m.mentions.iter().any(|x| x.kind == MentionKind::Human));
        let r2 = run(&f, &f.worker, "ask", json!({"question": "第二个问题"})).await;
        assert!(r2.text.contains("H2"), "号连续：{}", r2.text);
    }

    #[tokio::test]
    async fn fallback_must_come_in_pairs_and_in_range() {
        let f = Fixture::new().await;
        assert!(run(&f, &f.worker, "ask", json!({"question": "q", "fallback": "就按 A"})).await.is_error);
        assert!(run(&f, &f.worker, "ask", json!({"question": "q", "fallback_after_minutes": 30})).await.is_error);
        assert!(run(&f, &f.worker, "ask", json!({"question": "q", "fallback_after_minutes": 0, "fallback": "A"})).await.is_error);
        assert!(!run(&f, &f.worker, "ask", json!({"question": "q", "fallback_after_minutes": 30, "fallback": "A"})).await.is_error);
    }

    #[tokio::test]
    async fn withdraw_follows_work_book_permissions() {
        let f = Fixture::new().await;
        f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker").await;
        run(&f, &f.worker, "ask", json!({"question": "q1"})).await;
        run(&f, &f.worker, "ask", json!({"question": "q2"})).await;
        assert!(run(&f, "s-w2", "withdraw_ask", json!({"ask": "H1", "reason": "不是我的"})).await.is_error);
        assert!(!run(&f, &f.worker, "withdraw_ask", json!({"ask": "H1", "reason": "已经发版了"})).await.is_error);
        assert!(!run(&f, &f.captain, "withdraw_ask", json!({"ask": "H2", "reason": "人当面答了"})).await.is_error);
        assert!(run(&f, &f.worker, "withdraw_ask", json!({"ask": "H2", "reason": ""})).await.is_error, "reason 必填");
    }

    #[tokio::test]
    async fn respond_todo_appends_without_status() {
        let f = Fixture::new().await;
        f.add_agent_todo(1, "示例：把文档补齐").await;
        assert!(!run(&f, &f.worker, "respond_todo", json!({"number": 1, "response": "认领，今天做"})).await.is_error);
        let todos = f.rpc("todo.list", json!({"crewId": f.crew, "ledger": "agent"})).await;
        let t = &todos.as_array().unwrap()[0];
        assert_eq!(t["responses"][0]["text"], "认领，今天做");
        assert_eq!(t["status"], "pending", "回应不改状态（§9）");
        assert!(run(&f, &f.worker, "respond_todo", json!({"number": 99, "response": "x"})).await.is_error);
        assert!(run(&f, &f.worker, "respond_todo", json!({"number": 1, "response": "x", "status": "completed"})).await.is_error, "status 参数已删");
    }

    #[tokio::test]
    async fn schedule_wakeup_is_persisted_as_a_timer() {
        let f = Fixture::new().await;
        assert!(run(&f, &f.worker, "schedule_wakeup", json!({"after_minutes": 0, "note": "x"})).await.is_error);
        assert!(run(&f, &f.worker, "schedule_wakeup", json!({"at": "2001-01-01T00:00:00Z", "note": "x"})).await.is_error, "过去的时刻");
        assert!(run(&f, &f.worker, "schedule_wakeup", json!({"after_minutes": 5})).await.is_error, "note 必填");
        let before = port::now_ms();
        assert!(!run(&f, &f.worker, "schedule_wakeup", json!({"after_minutes": 30, "note": "额度重置后接着跑全量"})).await.is_error);
        let w = f.wakes_of(&f.worker).await;
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].0, "timer");
        assert!(w[0].1 >= before + 29 * 60_000 && w[0].1 <= port::now_ms() + 30 * 60_000);
        assert_eq!(w[0].2, "额度重置后接着跑全量");
    }

    #[tokio::test]
    async fn quota_and_profile_go_through_session_control() {
        let f = Fixture::new().await;
        assert!(!run(&f, &f.worker, "get_quota", json!({})).await.is_error);
        assert!(run(&f, &f.worker, "set_session_profile", json!({})).await.is_error, "至少给一个");
        assert!(!run(&f, &f.worker, "set_session_profile", json!({"model": "sonnet"})).await.is_error);
        assert!(f.control_calls().iter().any(|c| c.starts_with("SetProfile(") && c.contains("s-w1")), "{:?}", f.control_calls());
    }

    #[tokio::test]
    async fn asking_counts_as_calling_out() {
        let f = Fixture::new().await;
        let me = f.me(&f.worker).await;
        crate::agent::turn::stop(&f.state, &me, &json!({"hook_event_name": "Stop", "prompt_id": "p0", "last_assistant_message": "好。"})).await;
        run(&f, &f.worker, "ask", json!({"question": "要不要做 B？"})).await;
        let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
        let before = port::latest_seq(&f.state, &lobby).await.unwrap();
        crate::agent::turn::stop(&f.state, &me, &json!({"hook_event_name": "Stop", "prompt_id": "p1", "last_assistant_message": "要不要做 B？"})).await;
        assert_eq!(port::latest_seq(&f.state, &lobby).await.unwrap(), before);
    }
}
```

testkit 加：

```rust
    /// Agent 那本 Todo 加一条（M1 的 store 函数；M3 没有客户端建 Todo 的入口）。
    pub async fn add_agent_todo(&self, number: i64, text: &str) {
        let todo = crewd_proto::Todo {
            id: format!("todo-{number}"),
            crew_id: self.crew.clone(),
            ledger: crewd_proto::Ledger::Agent,
            number,
            text: text.into(),
            status: "pending".into(),
            created_at: "2026-10-06T00:00:00.000Z".into(),
            updated_at: None,
            deleted_at: None,
            attachments: vec![],
            responses: vec![],
        };
        self.state
            .db
            .call(move |s| crewd_store::todos::insert(s.conn(), &todo, None))
            .await
            .unwrap();
    }

    /// 这个 session 的唤醒（kind, not_before 毫秒, note）。<M3a>：`wakes::list` 的参数 / `WakeRow` 字段按真名改。
    pub async fn wakes_of(&self, session: &str) -> Vec<(String, i64, String)> {
        let session = session.to_string();
        let rows = self
            .state
            .db
            .call(move |s| crewd_collab::wakes::list(s.conn(), Some(&session)))
            .await
            .unwrap()
            .unwrap();
        rows.into_iter()
            .map(|w| (w.kind.as_str().to_string(), w.not_before_ms, w.note.unwrap_or_default()))
            .collect()
    }
```

- [ ] **Step 3: 跑一遍确认失败**

Run: `cargo test -p crewd-server --lib agent::tools::asks`
Expected: 编译失败。

- [ ] **Step 4: 写 `asks.rs`**

```rust
//! 问人与自我配置（spec §22.3 表）。
use crewd_proto::{Mention, MentionKind};
use crewd_proto::agent::CallResult;
use serde::Deserialize;
use serde_json::Value;

use super::{Ctx, parse_args};
use crate::agent::port;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AskArgs {
    question: String,
    resume_note: Option<String>,
    fallback_after_minutes: Option<f64>,
    fallback: Option<String>,
}

const MAX_FALLBACK_MINUTES: f64 = 7.0 * 24.0 * 60.0;

pub async fn ask(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: AskArgs = args!(cx, v);
    if a.question.trim().is_empty() {
        return cx.err("r.bad_args", &[("detail", "question")]);
    }
    let fallback = match (a.fallback_after_minutes, a.fallback.as_deref().map(str::trim)) {
        (None, None) => None,
        (Some(m), Some(t)) if (1.0..=MAX_FALLBACK_MINUTES).contains(&m) && !t.is_empty() => {
            Some((t.to_string(), m.ceil() as i64))
        }
        _ => return cx.err("r.fallback_pair", &[]),
    };
    let note = a.resume_note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    let had_note = note.is_some();
    let h = tryp!(cx, port::open_ask(cx.st, cx.me, a.question.trim(), fallback, note));
    crate::agent::turn::note_post(cx.st, cx.me, &[Mention { kind: MentionKind::Human, target_id: None }]).await;
    let mut text = cx.r("r.asked", &[("ask", &h)]);
    if !had_note {
        text.push('\n');
        text.push_str(&cx.r("r.ask_no_resume_note", &[]));
    }
    CallResult::ok(text)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WithdrawArgs {
    ask: String,
    reason: String,
}

pub async fn withdraw_ask(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: WithdrawArgs = args!(cx, v);
    let ask = a.ask.trim().to_uppercase();
    let valid = ask.strip_prefix('H').is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    if !valid || a.reason.trim().is_empty() {
        return cx.err("r.withdraw_args", &[]);
    }
    tryp!(cx, port::withdraw_ask(cx.st, cx.me, &ask, a.reason.trim()));
    cx.ok("r.withdrawn", &[("ask", &ask)])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RespondArgs {
    number: i64,
    response: String,
}

pub async fn respond_todo(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: RespondArgs = args!(cx, v);
    if a.response.trim().is_empty() {
        return cx.err("r.bad_args", &[("detail", "response")]);
    }
    tryp!(cx, port::respond_todo(cx.st, cx.me, a.number, a.response.trim()));
    cx.ok("r.responded", &[("number", &a.number.to_string())])
}

pub async fn get_quota(cx: &Ctx<'_>) -> CallResult {
    let report = tryp!(cx, port::quota(cx.st));
    cx.ok("r.quota", &[("report", &report)])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WakeArgs {
    after_minutes: Option<f64>,
    at: Option<String>,
    note: Option<String>,
}

pub async fn schedule_wakeup(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: WakeArgs = args!(cx, v);
    let Some(note) = a.note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) else {
        return cx.err("r.wakeup_note", &[]);
    };
    let now = port::now_ms();
    let at = match (a.after_minutes, a.at.as_deref()) {
        (Some(m), None) if (1.0..=1440.0).contains(&m) => now + (m * 60_000.0) as i64,
        (None, Some(s)) => match chrono::DateTime::parse_from_rfc3339(s.trim()) {
            Ok(t) if t.timestamp_millis() > now => t.timestamp_millis(),
            _ => return cx.err("r.wakeup_when", &[]),
        },
        _ => return cx.err("r.wakeup_when", &[]),
    };
    tryp!(cx, port::schedule_timer(cx.st, &cx.me.session_id, at, &note));
    let when = chrono::DateTime::from_timestamp_millis(at)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default();
    cx.ok("r.wakeup_set", &[("at", &when)])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileArgs {
    model: Option<String>,
    effort: Option<String>,
}

pub async fn set_session_profile(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: ProfileArgs = args!(cx, v);
    let model = a.model.map(|m| m.trim().to_string()).filter(|m| !m.is_empty());
    let effort = a.effort.map(|e| e.trim().to_string()).filter(|e| !e.is_empty());
    if model.is_none() && effort.is_none() {
        return cx.err("r.profile_empty", &[]);
    }
    tryp!(cx, port::set_profile(cx.st, &cx.me.session_id, model, effort));
    cx.ok("r.profile_queued", &[])
}
```

`tools/mod.rs`：`pub mod asks;`（放在三个宏之后）；`match tool` 加：

```rust
        "ask" => asks::ask(&cx, args).await,
        "withdraw_ask" => asks::withdraw_ask(&cx, args).await,
        "respond_todo" => asks::respond_todo(&cx, args).await,
        "get_quota" => asks::get_quota(&cx).await,
        "schedule_wakeup" => asks::schedule_wakeup(&cx, args).await,
        "set_session_profile" => asks::set_session_profile(&cx, args).await,
```

- [ ] **Step 5: md 加节**

`tools.zh.md`：

```markdown
## r.fallback_pair
fallback_after_minutes（1 分钟到 7 天）和 fallback 要一起给，或者都不给。

## r.asked
已记成 {ask}，发进了根机组人类群并 @了人类。不用等：去干别的或收工，人回答时会叫醒你并念回 resume_note。

## r.ask_no_resume_note
你没写 resume_note：被叫醒时你只看得到问题和答复。半路上问的问题，最好用 withdraw_ask 撤掉重问一次、写上答复后接着做什么。

## r.withdraw_args
ask 填 H 号（如 H3），reason 写清变了什么，两个都必填。

## r.withdrawn
已撤回 {ask}，人类群里会留一行记录。

## r.responded
已回应 Todo #{number}。

## r.quota
{report}
没有剩余量的绝对值，别据此编造额度。

## r.wakeup_note
note 必填：醒来的你只有这条备注和群消息可依靠。

## r.wakeup_when
after_minutes（1–1440）或 at（未来的 RFC 3339 时刻）二选一。

## r.wakeup_set
已约好 {at} 叫醒你。到点时如果你不在运行，会被拉起来接着做。

## r.profile_empty
model 和 effort 至少给一个。

## r.profile_queued
已排上：本回合结束后切换，结果会回执到群里。下一次工具调用还在旧配置上。
```

`tools.en.md`：

```markdown
## r.fallback_pair
Give fallback_after_minutes (1 minute to 7 days) and fallback together, or neither.

## r.asked
Recorded as {ask} and posted to the root crew's human channel mentioning the human. Do not wait: do something else or end your turn; you will be woken with your resume_note when the human answers.

## r.ask_no_resume_note
You gave no resume_note: when woken you will only see the question and the answer. For questions asked mid-task, consider withdraw_ask and asking again with a resume_note.

## r.withdraw_args
ask takes an H number (like H3) and reason must say what changed; both are required.

## r.withdrawn
Withdrew {ask}; a note is left in the human channel.

## r.responded
Responded to Todo #{number}.

## r.quota
{report}
There is no absolute remaining amount; do not invent one.

## r.wakeup_note
note is required: on waking you only have this note and channel messages.

## r.wakeup_when
Give either after_minutes (1–1440) or at (an RFC 3339 time in the future).

## r.wakeup_set
You will be woken at {at}. If you are not running then, you will be started to continue.

## r.profile_empty
Give at least one of model and effort.

## r.profile_queued
Queued: the switch happens after this turn and the result is reported in the channel. Your next tool call still runs on the old settings.
```

- [ ] **Step 6: 跑通**

Run: `cargo test -p crewd-server --lib agent::tools::asks`
Expected: `7 passed`。

- [ ] **Step 7: 变异核对**

1. `ask` 里删掉 `note_post` 那一行：Expected `asking_counts_as_calling_out` 红。
2. `schedule_wakeup` 的 `t.timestamp_millis() > now` 换成 `true`：Expected `schedule_wakeup_is_persisted_as_a_timer` 红。

逐条撤销。

- [ ] **Step 8: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/server/src/agent/tools/asks.rs crates/server/src/agent/tools/mod.rs crates/server/src/agent/testkit.rs \
  crates/server/Cargo.toml Cargo.lock crates/collab/prompts/tools.zh.md crates/collab/prompts/tools.en.md
git diff --cached --stat
git commit -m "crewd-server: ask/withdraw_ask (H numbers via work book), respond_todo, quota, timers, profile switch

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 15: 机长工具（机组属性、会话控制、汇报线、机组结构）+ 「每个工具都有实现」

**Files:**
- Create: `crewd/crates/server/src/agent/tools/captain.rs`
- Modify: `agent/tools/mod.rs`（分发 + 收尾测试）、`crates/collab/prompts/tools.{zh,en}.md`

**Interfaces:**
- Consumes: Task 6 `port::{crew, resolve_crew, update_crew, attach_parent, detach_parent, create_crew, sessions_of, identity, lobby_of, start_session, stop_session, inspect, nudge, live_state, handoff_captain}`；Task 12 `post_as`。
- Produces: 无新接口。

规则（照 `McpServer.swift` 对应处理行；spec §22.3「经 SessionControl」）：只能动自己机组的 session（`stop` / `inspect` / `nudge`），不能停自己；`change_workdir` 只能动自己这棵子树；`handoff_*` 的 `target_crew_id` 只能是本机组或直系子机组（`McpServer.swift:585-612`），接任者必须属于目标机组；`report_to_parent` / `message_child_crew` 以系统身份 + 署名发到对方大厅并 @机长（裁定 7）；`create_parent_crew` / `create_child_crew` 建组后经 `SessionControl::start(role = captain)` 起新机长（新机组还没有机长）；`create_and_handoff_captain` 在已有机长的机组里先以 worker 起、再 `SessionControl::handoff_captain`。机组结构的成环、内建、非直系子由 M3a 拒（`PortError::Refused` 原样带回）。

- [ ] **Step 1: 写失败的测试**（`captain.rs` 末尾）

```rust
#[cfg(test)]
mod tests {
    use crewd_proto::MentionKind;
    use serde_json::json;

    use crate::agent::port;
    use crate::agent::testkit::Fixture;
    use crate::agent::tools::call;
    use crate::agent::Role;

    async fn cap(f: &Fixture, tool: &str, args: serde_json::Value) -> crewd_proto::agent::CallResult {
        let me = f.me(&f.captain).await;
        call(&f.state, &me, tool, args).await
    }

    #[tokio::test]
    async fn crew_attributes() {
        let f = Fixture::new().await;
        assert!(!cap(&f, "rename_crew", json!({"name": "鉴权重构"})).await.is_error);
        assert_eq!(port::crew(&f.state, &f.crew).await.unwrap().unwrap().title, "鉴权重构");
        assert!(cap(&f, "set_crew_status", json!({"text": "长".repeat(201)})).await.is_error);
        assert!(!cap(&f, "set_crew_status", json!({"text": "在等 CI"})).await.is_error);
        assert!(!cap(&f, "update_delegation_policy", json!({"text": "小活自己做，大活起 worker"})).await.is_error);
        assert!(cap(&f, "read_delegation_policy", json!({})).await.text.contains("小活自己做"));
    }

    #[tokio::test]
    async fn change_workdir_previews_then_applies_within_subtree_only() {
        let f = Fixture::new().await;
        let new_dir = tempfile::tempdir().unwrap();
        let path = new_dir.path().display().to_string();
        let r = cap(&f, "change_workdir", json!({"new_path": path})).await;
        assert!(!r.is_error && r.text.contains(&path), "{}", r.text);
        assert_ne!(port::crew(&f.state, &f.crew).await.unwrap().unwrap().working_directory.as_deref(), Some(path.as_str()), "预览不改");
        assert!(!cap(&f, "change_workdir", json!({"new_path": path, "confirm": true})).await.is_error);
        assert_eq!(port::crew(&f.state, &f.crew).await.unwrap().unwrap().working_directory.as_deref(), Some(path.as_str()));
        assert!(cap(&f, "change_workdir", json!({"new_path": "/no/such/dir-示例", "confirm": true})).await.is_error);
        let stranger = f.add_crew("别人的机组", None).await;
        assert!(cap(&f, "change_workdir", json!({"new_path": path, "crew": stranger, "confirm": true})).await.is_error, "不是自己这棵子树");
    }

    #[tokio::test]
    async fn sessions_are_controlled_within_own_crew() {
        let f = Fixture::new().await;
        assert!(!cap(&f, "start_session", json!({"brief": "补测试", "isolation": true, "runner": "claude"})).await.is_error);
        assert!(f.control_calls().iter().any(|c| c.starts_with("Start(") && c.contains(&f.crew)), "{:?}", f.control_calls());
        let other = f.add_crew("别的机组", None).await;
        f.add_session(&other, "s-far", Role::Worker, "外组").await;
        assert!(cap(&f, "stop_session", json!({"session_id": "s-far", "reason": "x"})).await.is_error);
        assert!(cap(&f, "stop_session", json!({"session_id": f.captain, "reason": "x"})).await.is_error, "不能停自己");
        assert!(!cap(&f, "stop_session", json!({"session_id": f.worker, "reason": "示例：任务取消"})).await.is_error);
        let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
        let notice = port::messages(&f.state, &lobby, None, 1).await.unwrap().pop().unwrap();
        assert!(notice.text.contains("示例：任务取消"), "先公开写原因");
        assert!(f.control_calls().iter().any(|c| c.starts_with("Stop(") && c.contains("s-w1")));
        assert!(!cap(&f, "inspect_session", json!({"session_id": f.worker})).await.is_error);
        assert!(!cap(&f, "nudge_session", json!({"session_id": f.worker, "input": "Esc"})).await.is_error);
        assert!(cap(&f, "list_sessions", json!({})).await.text.contains("示例 worker"));
    }

    #[tokio::test]
    async fn handoff_only_to_own_or_direct_child_crew() {
        let f = Fixture::new().await;
        assert!(!cap(&f, "handoff_captain_to_session", json!({"session_id": f.worker})).await.is_error);
        assert!(f.control_calls().iter().any(|c| c.starts_with("Handoff(") && c.contains("s-w1")));
        let stranger = f.add_crew("陌生机组", None).await;
        f.add_session(&stranger, "s-st", Role::Worker, "陌生人").await;
        assert!(cap(&f, "handoff_captain_to_session", json!({"session_id": "s-st", "target_crew_id": stranger})).await.is_error);
        let child = f.add_crew("子机组", Some(&f.crew)).await;
        f.add_session(&child, "s-ch", Role::Worker, "子机组 worker").await;
        assert!(!cap(&f, "handoff_captain_to_session", json!({"session_id": "s-ch", "target_crew_id": child})).await.is_error);
        assert!(cap(&f, "handoff_captain_to_session", json!({"session_id": f.worker, "target_crew_id": child})).await.is_error, "接任者必须属于目标机组");
        assert!(!cap(&f, "create_and_handoff_captain", json!({"runner": "codex", "target_crew_id": child})).await.is_error);
    }

    #[tokio::test]
    async fn reporting_line_posts_signed_to_lobbies() {
        let f = Fixture::new().await;
        assert!(cap(&f, "report_to_parent", json!({"message": "x"})).await.is_error, "根机组没有上级");
        let child = f.add_crew("子机组", Some(&f.crew)).await;
        f.add_session(&child, "s-ccap", Role::Captain, "子机长").await;
        let stranger = f.add_crew("陌生机组", None).await;
        assert!(cap(&f, "message_child_crew", json!({"crew": stranger, "message": "x"})).await.is_error);
        assert!(!cap(&f, "message_child_crew", json!({"crew": child, "message": "先做 A"})).await.is_error);
        let m = port::messages(&f.state, &port::lobby_of(&f.state, &child).await.unwrap().id, None, 1).await.unwrap().pop().unwrap();
        assert!(m.text.contains("先做 A") && m.mentions.iter().any(|x| x.kind == MentionKind::Captain));
        let child_cap = f.me("s-ccap").await;
        let r = call(&f.state, &child_cap, "report_to_parent", json!({"message": "A 做完了"})).await;
        assert!(!r.is_error, "{}", r.text);
        let up = port::messages(&f.state, &port::lobby_of(&f.state, &f.crew).await.unwrap().id, None, 1).await.unwrap().pop().unwrap();
        assert!(up.text.contains("A 做完了"));
    }

    #[tokio::test]
    async fn crew_structure_changes() {
        let f = Fixture::new().await;
        assert!(!cap(&f, "create_child_crew", json!({"brief": "做语音重连", "title": "语音"})).await.is_error);
        let me = port::crew(&f.state, &f.crew).await.unwrap().unwrap();
        assert_eq!(me.child_ids.len(), 1);
        assert!(f.control_calls().iter().any(|c| c.starts_with("Start(") && c.contains("Captain")));
        let other = f.add_crew("平级机组", None).await;
        assert!(!cap(&f, "adopt_crew", json!({"crew": other})).await.is_error);
        assert!(port::crew(&f.state, &other).await.unwrap().unwrap().parent_ids.contains(&f.crew));
        assert!(!cap(&f, "release_crew", json!({"crew": other})).await.is_error);
        assert!(port::crew(&f.state, &other).await.unwrap().unwrap().parent_ids.is_empty());
        assert!(!cap(&f, "create_parent_crew", json!({"title": "总部"})).await.is_error);
        assert_eq!(port::crew(&f.state, &f.crew).await.unwrap().unwrap().parent_ids.len(), 1);
    }
}
```

`tools/mod.rs` 的测试模块加收尾测试：

```rust
    #[tokio::test]
    async fn every_catalog_tool_has_a_handler() {
        let f = Fixture::new().await;
        let me = f.me(&f.captain).await;
        for t in crate::agent::catalog::CATALOG {
            let r = call(&f.state, &me, t.name, json!({})).await;
            assert_ne!(r.text, text::r(Locale::Zh, "r.no_handler", &[("name", t.name)]), "{} 没接上", t.name);
        }
    }
```

- [ ] **Step 2: 跑一遍确认失败**

Run: `cargo test -p crewd-server --lib agent::tools`
Expected: captain 编译失败；`every_catalog_tool_has_a_handler`（captain 编好之前单独跑）报 19 个机长工具没接上。

- [ ] **Step 3: 写 `captain.rs`**

```rust
//! 机长工具（spec §22.3 表 C 行）。只有机长能调（`tools::call` 已把关）。
use std::path::Path;

use crewd_proto::{Mention, MentionKind};
use crewd_proto::agent::CallResult;
use serde::Deserialize;
use serde_json::Value;

use super::{Ctx, parse_args, post_as};
use crate::agent::Role;
use crate::agent::port::{self, CrewPatch, CrewView, PostSender, StartReq};

fn captain_mention() -> Vec<Mention> {
    vec![Mention { kind: MentionKind::Captain, target_id: None }]
}

async fn my_crew(cx: &Ctx<'_>) -> Result<CrewView, CallResult> {
    match port::crew(cx.st, &cx.me.crew_id).await {
        Ok(Some(c)) => Ok(c),
        Ok(None) => Err(cx.err("r.no_crew", &[("crew", &cx.me.crew_id)])),
        Err(e) => Err(cx.fail(e)),
    }
}

async fn find_crew(cx: &Ctx<'_>, arg: &str) -> Result<CrewView, CallResult> {
    match port::resolve_crew(cx.st, arg.trim()).await {
        Ok(Some(c)) => Ok(c),
        Ok(None) => Err(cx.err("r.no_crew", &[("crew", arg)])),
        Err(e) => Err(cx.fail(e)),
    }
}

macro_rules! ok_or_return {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return e,
        }
    };
}

/// 本机组 + 全部后代（广度优先；DAG 上去重）。
async fn subtree(cx: &Ctx<'_>, root: &CrewView) -> Result<Vec<CrewView>, CallResult> {
    let mut out = vec![root.clone()];
    let mut i = 0;
    while i < out.len() {
        for id in out[i].child_ids.clone() {
            if !out.iter().any(|c| c.id == id) {
                match port::crew(cx.st, &id).await {
                    Ok(Some(c)) => out.push(c),
                    Ok(None) => {}
                    Err(e) => return Err(cx.fail(e)),
                }
            }
        }
        i += 1;
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NameArg {
    name: String,
}

pub async fn rename_crew(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: NameArg = args!(cx, v);
    let name = a.name.trim();
    if name.is_empty() {
        return cx.err("r.bad_args", &[("detail", "name")]);
    }
    let patch = CrewPatch { title: Some(name.to_string()), ..CrewPatch::default() };
    tryp!(cx, port::update_crew(cx.st, &cx.me.crew_id, patch));
    cx.ok("r.renamed", &[("name", name)])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkdirArgs {
    new_path: String,
    crew: Option<String>,
    include_children: Option<bool>,
    #[serde(default)]
    confirm: bool,
}

pub async fn change_workdir(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: WorkdirArgs = args!(cx, v);
    let path = a.new_path.trim();
    if !Path::new(path).is_absolute() || !Path::new(path).is_dir() {
        return cx.err("r.workdir_missing", &[("path", path)]);
    }
    let mine = ok_or_return!(my_crew(cx).await);
    let allowed = ok_or_return!(subtree(cx, &mine).await);
    let target = match &a.crew {
        None => mine.clone(),
        Some(arg) => ok_or_return!(find_crew(cx, arg).await),
    };
    if !allowed.iter().any(|c| c.id == target.id) {
        return cx.err("r.not_my_subtree", &[("crew", &target.title)]);
    }
    let affected = if a.include_children.unwrap_or(true) {
        ok_or_return!(subtree(cx, &target).await)
    } else {
        vec![target]
    };
    let names = affected.iter().map(|c| c.title.clone()).collect::<Vec<_>>().join(cx.loc_sep());
    if !a.confirm {
        return cx.ok("r.workdir_preview", &[("path", path), ("crews", &names)]);
    }
    for c in &affected {
        let patch = CrewPatch { working_directory: Some(path.to_string()), ..CrewPatch::default() };
        tryp!(cx, port::update_crew(cx.st, &c.id, patch));
    }
    cx.ok("r.workdir_done", &[("path", path), ("crews", &names)])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextArg {
    text: String,
}

pub async fn set_crew_status(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: TextArg = args!(cx, v);
    let t = a.text.trim();
    let n = t.chars().count();
    if n == 0 || n > 200 {
        return cx.err("r.status_length", &[("count", &n.to_string())]);
    }
    let patch = CrewPatch { status_line: Some(t.to_string()), ..CrewPatch::default() };
    tryp!(cx, port::update_crew(cx.st, &cx.me.crew_id, patch));
    let mut out = cx.r("r.status_set", &[]);
    if n > 40 {
        out.push('\n');
        out.push_str(&cx.r("r.status_long", &[("count", &n.to_string())]));
    }
    CallResult::ok(out)
}

pub async fn read_delegation_policy(cx: &Ctx<'_>) -> CallResult {
    let c = ok_or_return!(my_crew(cx).await);
    match c.policy.filter(|p| !p.trim().is_empty()) {
        Some(p) => cx.ok("r.policy", &[("text", &p)]),
        None => cx.ok("r.policy_empty", &[]),
    }
}

pub async fn update_delegation_policy(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: TextArg = args!(cx, v);
    if a.text.trim().is_empty() {
        return cx.err("r.bad_args", &[("detail", "text")]);
    }
    let patch = CrewPatch { policy: Some(a.text.trim().to_string()), ..CrewPatch::default() };
    tryp!(cx, port::update_crew(cx.st, &cx.me.crew_id, patch));
    cx.ok("r.policy_updated", &[])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartArgs {
    brief: String,
    title: Option<String>,
    runner: Option<String>,
    isolation: bool,
    model: Option<String>,
    effort: Option<String>,
}

pub async fn start_session(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: StartArgs = args!(cx, v);
    if a.brief.trim().is_empty() {
        return cx.err("r.bad_args", &[("detail", "brief")]);
    }
    let req = StartReq {
        crew_id: cx.me.crew_id.clone(),
        role: Role::Worker,
        executor: a.runner,
        brief: a.brief.trim().to_string(),
        title: a.title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
        model: a.model,
        effort: a.effort,
        worktree: a.isolation,
    };
    let id = tryp!(cx, port::start_session(cx.st, req));
    cx.ok("r.session_started", &[("id", &id)])
}

/// 目标 session 必须在本机组；返回它的身份。
async fn own_session(cx: &Ctx<'_>, id: &str) -> Result<crate::agent::Identity, CallResult> {
    match port::identity(cx.st, id.trim()).await {
        Ok(Some(who)) if who.crew_id == cx.me.crew_id => Ok(who),
        Ok(_) => Err(cx.err("r.not_my_session", &[("id", id)])),
        Err(e) => Err(cx.fail(e)),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StopArgs {
    session_id: String,
    reason: String,
}

pub async fn stop_session(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: StopArgs = args!(cx, v);
    if a.reason.trim().is_empty() {
        return cx.err("r.bad_args", &[("detail", "reason")]);
    }
    let who = ok_or_return!(own_session(cx, &a.session_id).await);
    if who.session_id == cx.me.session_id {
        return cx.err("r.stop_self", &[]);
    }
    let lobby = tryp!(cx, port::lobby_of(cx.st, &cx.me.crew_id));
    let notice = cx.r("r.stop_notice", &[("captain", &cx.me.label), ("target", &who.label), ("reason", a.reason.trim())]);
    ok_or_return!(post_as(cx, &lobby, notice, vec![], None, None, PostSender::Session(cx.me.session_id.clone())).await);
    tryp!(cx, port::stop_session(cx.st, &who.session_id, a.reason.trim()));
    cx.ok("r.session_stopped", &[("target", &who.label)])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionArg {
    session_id: String,
}

pub async fn inspect_session(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: SessionArg = args!(cx, v);
    let who = ok_or_return!(own_session(cx, &a.session_id).await);
    let report = tryp!(cx, port::inspect(cx.st, &who.session_id));
    cx.ok("r.inspect", &[("target", &who.label), ("report", &report)])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NudgeArgs {
    session_id: String,
    input: String,
}

pub async fn nudge_session(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: NudgeArgs = args!(cx, v);
    if a.input.is_empty() {
        return cx.err("r.bad_args", &[("detail", "input")]);
    }
    let who = ok_or_return!(own_session(cx, &a.session_id).await);
    tryp!(cx, port::nudge(cx.st, &who.session_id, &a.input));
    cx.ok("r.nudged", &[("target", &who.label)])
}

pub async fn list_sessions(cx: &Ctx<'_>) -> CallResult {
    let list = tryp!(cx, port::sessions_of(cx.st, &cx.me.crew_id));
    let mut out = vec![cx.r("r.sessions_header", &[])];
    for s in list {
        let state = port::live_state(cx.st, &s.id).await.unwrap_or_default();
        let role = cx.r(if s.role == Role::Captain { "r.role_captain" } else { "r.role_worker" }, &[]);
        out.push(cx.r(
            "r.session_line",
            &[("id", &s.id), ("ext", s.extension.as_deref().unwrap_or("")), ("label", &s.label), ("role", &role), ("state", &state)],
        ));
    }
    CallResult::ok(out.join("\n"))
}

/// 本机组或直系子机组。
async fn handoff_target(cx: &Ctx<'_>, arg: Option<&str>) -> Result<CrewView, CallResult> {
    let mine = my_crew(cx).await?;
    match arg.map(str::trim).filter(|a| !a.is_empty()) {
        None => Ok(mine),
        Some(id) if mine.child_ids.iter().any(|c| c == id) => find_crew(cx, id).await,
        Some(id) => Err(cx.err("r.not_direct_child", &[("crew", id)])),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HandoffArgs {
    session_id: String,
    target_crew_id: Option<String>,
}

pub async fn handoff_captain_to_session(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: HandoffArgs = args!(cx, v);
    let crew = ok_or_return!(handoff_target(cx, a.target_crew_id.as_deref()).await);
    let who = match port::identity(cx.st, a.session_id.trim()).await {
        Ok(Some(w)) if w.crew_id == crew.id => w,
        Ok(_) => return cx.err("r.successor_not_in_crew", &[("id", &a.session_id), ("crew", &crew.title)]),
        Err(e) => return cx.fail(e),
    };
    tryp!(cx, port::handoff_captain(cx.st, &crew.id, &who.session_id));
    cx.ok("r.handoff_done", &[("crew", &crew.title), ("target", &who.label)])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateHandoffArgs {
    runner: String,
    model: Option<String>,
    effort: Option<String>,
    title: Option<String>,
    opening_brief: Option<String>,
    target_crew_id: Option<String>,
}

pub async fn create_and_handoff_captain(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: CreateHandoffArgs = args!(cx, v);
    let crew = ok_or_return!(handoff_target(cx, a.target_crew_id.as_deref()).await);
    let brief = a
        .opening_brief
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| cx.r("r.handoff_default_brief", &[]));
    let title = a.title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| cx.r("r.default_captain_title", &[]));
    // 先以 worker 起（目标机组已有机长，M3a 的 register 不收第二个 captain），再交接。
    let req = StartReq {
        crew_id: crew.id.clone(),
        role: Role::Worker,
        executor: Some(a.runner),
        brief,
        title: Some(title),
        model: a.model,
        effort: a.effort,
        worktree: false,
    };
    let id = tryp!(cx, port::start_session(cx.st, req));
    tryp!(cx, port::handoff_captain(cx.st, &crew.id, &id));
    cx.ok("r.handoff_new", &[("crew", &crew.title), ("id", &id)])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageArg {
    message: String,
}

pub async fn report_to_parent(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: MessageArg = args!(cx, v);
    if a.message.trim().is_empty() {
        return cx.err("r.bad_args", &[("detail", "message")]);
    }
    let mine = ok_or_return!(my_crew(cx).await);
    if mine.parent_ids.is_empty() {
        return cx.err("r.no_parent", &[]);
    }
    let body = cx.r("r.report_from", &[("crew", &mine.title), ("label", &cx.me.label), ("message", a.message.trim())]);
    for p in &mine.parent_ids {
        let lobby = tryp!(cx, port::lobby_of(cx.st, p));
        ok_or_return!(post_as(cx, &lobby, body.clone(), captain_mention(), None, None, PostSender::System).await);
    }
    cx.ok("r.reported", &[("count", &mine.parent_ids.len().to_string())])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChildMessageArgs {
    crew: String,
    message: String,
}

pub async fn message_child_crew(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: ChildMessageArgs = args!(cx, v);
    if a.message.trim().is_empty() {
        return cx.err("r.bad_args", &[("detail", "message")]);
    }
    let mine = ok_or_return!(my_crew(cx).await);
    let target = ok_or_return!(find_crew(cx, &a.crew).await);
    if !mine.child_ids.contains(&target.id) {
        return cx.err("r.not_direct_child", &[("crew", &a.crew)]);
    }
    let lobby = tryp!(cx, port::lobby_of(cx.st, &target.id));
    let body = cx.r("r.order_from", &[("crew", &mine.title), ("label", &cx.me.label), ("message", a.message.trim())]);
    ok_or_return!(post_as(cx, &lobby, body, captain_mention(), None, None, PostSender::System).await);
    cx.ok("r.messaged", &[("crew", &target.title)])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CrewArg {
    crew: String,
}

pub async fn adopt_crew(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: CrewArg = args!(cx, v);
    let target = ok_or_return!(find_crew(cx, &a.crew).await);
    tryp!(cx, port::attach_parent(cx.st, cx.me, &target.id, &cx.me.crew_id));
    cx.ok("r.adopted", &[("crew", &target.title)])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseArgs {
    crew: String,
    to: Option<String>,
}

pub async fn release_crew(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: ReleaseArgs = args!(cx, v);
    let mine = ok_or_return!(my_crew(cx).await);
    let target = ok_or_return!(find_crew(cx, &a.crew).await);
    if !mine.child_ids.contains(&target.id) {
        return cx.err("r.not_direct_child", &[("crew", &a.crew)]);
    }
    let to = match a.to.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        None => None,
        Some(t) => {
            let dest = ok_or_return!(find_crew(cx, t).await);
            if !mine.child_ids.contains(&dest.id) || dest.id == target.id {
                return cx.err("r.not_direct_child", &[("crew", t)]);
            }
            Some(dest.id)
        }
    };
    tryp!(cx, port::detach_parent(cx.st, cx.me, &mine.id, &target.id, to));
    cx.ok("r.released", &[("crew", &target.title)])
}

/// 新机组（还没有机长）起机长：start(role = captain)。
async fn start_captain(cx: &Ctx<'_>, crew: &CrewView, runner: Option<String>, brief: String) -> Result<String, CallResult> {
    let req = StartReq {
        crew_id: crew.id.clone(),
        role: Role::Captain,
        executor: runner,
        brief,
        title: Some(cx.r("r.default_captain_title", &[])),
        model: None,
        effort: None,
        worktree: false,
    };
    port::start_session(cx.st, req).await.map_err(|e| cx.fail(e))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TitleArg {
    title: Option<String>,
}

pub async fn create_parent_crew(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: TitleArg = args!(cx, v);
    let mine = ok_or_return!(my_crew(cx).await);
    let parent = tryp!(cx, port::create_crew(cx.st, cx.me, a.title.filter(|t| !t.trim().is_empty()), mine.working_directory.clone(), None));
    tryp!(cx, port::attach_parent(cx.st, cx.me, &mine.id, &parent.id));
    let brief = cx.r("r.parent_brief", &[("child", &mine.title)]);
    ok_or_return!(start_captain(cx, &parent, None, brief).await);
    cx.ok("r.parent_created", &[("crew", &parent.title)])
}

pub async fn adopt_parent(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: CrewArg = args!(cx, v);
    let target = ok_or_return!(find_crew(cx, &a.crew).await);
    tryp!(cx, port::attach_parent(cx.st, cx.me, &cx.me.crew_id, &target.id));
    cx.ok("r.parent_adopted", &[("crew", &target.title)])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChildArgs {
    brief: String,
    title: Option<String>,
    runner: Option<String>,
}

pub async fn create_child_crew(cx: &Ctx<'_>, v: Value) -> CallResult {
    let a: ChildArgs = args!(cx, v);
    if a.brief.trim().is_empty() {
        return cx.err("r.bad_args", &[("detail", "brief")]);
    }
    let mine = ok_or_return!(my_crew(cx).await);
    let child = tryp!(cx, port::create_crew(cx.st, cx.me, a.title.filter(|t| !t.trim().is_empty()), mine.working_directory.clone(), Some(mine.id.clone())));
    ok_or_return!(start_captain(cx, &child, a.runner, a.brief.trim().to_string()).await);
    cx.ok("r.child_created", &[("crew", &child.title)])
}
```

`Ctx` 加一个小方法（`tools/mod.rs`）：`pub fn loc_sep(&self) -> &'static str { text::get(self.loc, "r.sep") }`。

（`subtree` / `handoff_target` 返回 `Result<_, CallResult>`，里面不能用 `tryp!`——那个宏直接 `return` 一个 `CallResult`。）

`tools/mod.rs`：`pub mod captain;`；`match tool` 加：

```rust
        "rename_crew" => captain::rename_crew(&cx, args).await,
        "change_workdir" => captain::change_workdir(&cx, args).await,
        "set_crew_status" => captain::set_crew_status(&cx, args).await,
        "read_delegation_policy" => captain::read_delegation_policy(&cx).await,
        "update_delegation_policy" => captain::update_delegation_policy(&cx, args).await,
        "start_session" => captain::start_session(&cx, args).await,
        "stop_session" => captain::stop_session(&cx, args).await,
        "inspect_session" => captain::inspect_session(&cx, args).await,
        "nudge_session" => captain::nudge_session(&cx, args).await,
        "list_sessions" => captain::list_sessions(&cx).await,
        "handoff_captain_to_session" => captain::handoff_captain_to_session(&cx, args).await,
        "create_and_handoff_captain" => captain::create_and_handoff_captain(&cx, args).await,
        "report_to_parent" => captain::report_to_parent(&cx, args).await,
        "message_child_crew" => captain::message_child_crew(&cx, args).await,
        "adopt_crew" => captain::adopt_crew(&cx, args).await,
        "release_crew" => captain::release_crew(&cx, args).await,
        "create_parent_crew" => captain::create_parent_crew(&cx, args).await,
        "adopt_parent" => captain::adopt_parent(&cx, args).await,
        "create_child_crew" => captain::create_child_crew(&cx, args).await,
```

- [ ] **Step 4: md 加节**

`tools.zh.md`：

```markdown
## r.sep
、

## r.no_crew
找不到机组「{crew}」（名字、id 或号码）。

## r.renamed
机组已改名为「{name}」。

## r.workdir_missing
{path} 不是已存在的绝对路径目录。

## r.not_my_subtree
「{crew}」不在你这棵子树里，动不了。

## r.workdir_preview
预览：会把这些机组的工作目录改成 {path}：{crews}。确认就带 confirm: true 再调一次。（只改记录的目录；claude 按目录记的项目记忆不会搬，新目录没被信任时第一次启动会弹信任框。）

## r.workdir_done
已把 {crews} 的工作目录改成 {path}。正在跑的 session 不会换目录，新起的才用它。

## r.status_length
机组状态要 1–200 字，你写了 {count} 字。

## r.status_set
机组状态已更新。

## r.status_long
（{count} 字，侧栏大概只露得出前 40 字。）

## r.policy
本机组的派活原则：
{text}

## r.policy_empty
本机组还没有写派活原则。

## r.policy_updated
派活原则已更新。

## r.session_started
已起 worker（{id}）。它上线后会在群里出声；没动静就 inspect_session。

## r.not_my_session
{id} 不是本机组的 session。

## r.stop_self
不能停你自己。

## r.stop_notice
机长 {captain} 停掉了 {target}：{reason}

## r.session_stopped
已停掉 {target}。

## r.inspect
{target} 的现场：
{report}

## r.nudged
已发给 {target}。过一会儿再 inspect_session 看看有没有解开。

## r.sessions_header
本机组的 session（id · 分机 · 名字 · 角色 · 状态）：

## r.session_line
- {id} · {ext} · {label} · {role} · {state}

## r.role_captain
机长

## r.role_worker
worker

## r.not_direct_child
「{crew}」不是你的直系子机组。

## r.successor_not_in_crew
{id} 不属于「{crew}」，不能接任那里的机长。

## r.handoff_done
「{crew}」的机长已交给 {target}。

## r.handoff_default_brief
你刚接任本机组的机长：先读大厅最近的消息，确认手上在办的事和各 worker 的状态，再在大厅说一句你接手了。

## r.default_captain_title
机长

## r.handoff_new
已为「{crew}」起新机长（{id}）并交接；交接结果以群里的回执为准。

## r.no_parent
本机组是根机组，没有上级可汇报。

## r.report_from
来自下级机组「{crew}」的机长 {label} 的汇报：
{message}

## r.reported
已汇报给 {count} 个上级机组。

## r.order_from
上级机组「{crew}」的机长 {label}：
{message}

## r.messaged
已发到「{crew}」的大厅并 @了它的机长。

## r.adopted
已收编「{crew}」为本机组的子机组。

## r.released
已调整「{crew}」的挂靠。

## r.parent_brief
你是新建的父机组的机长，「{child}」是你的第一个子机组。先读子机组的大厅了解情况，再决定要不要把其它平级机组收编进来。

## r.parent_created
已建父机组「{crew}」并起了父机长。

## r.parent_adopted
已认「{crew}」为上级机组。

## r.child_created
已建子机组「{crew}」并起了子机长，开场任务已交给它。
```

`tools.en.md`：

```markdown
## r.sep
/

## r.no_crew
No crew "{crew}" (name, id or number).

## r.renamed
Crew renamed to "{name}".

## r.workdir_missing
{path} is not an existing absolute directory.

## r.not_my_subtree
"{crew}" is not in your subtree.

## r.workdir_preview
Preview: these crews would get working directory {path}: {crews}. Call again with confirm: true to apply. (Only the recorded directory changes; claude's per-directory project memory is not moved, and an untrusted directory shows a trust prompt on first start.)

## r.workdir_done
Working directory of {crews} set to {path}. Running sessions keep their directory; new ones use it.

## r.status_length
Crew status must be 1–200 characters; you wrote {count}.

## r.status_set
Crew status updated.

## r.status_long
({count} characters; the sidebar shows roughly the first 40.)

## r.policy
This crew's delegation policy:
{text}

## r.policy_empty
This crew has no delegation policy yet.

## r.policy_updated
Delegation policy updated.

## r.session_started
Worker started ({id}). It will speak up in the channel once running; if it stays quiet, inspect_session.

## r.not_my_session
{id} is not a session of this crew.

## r.stop_self
You cannot stop yourself.

## r.stop_notice
Captain {captain} stopped {target}: {reason}

## r.session_stopped
Stopped {target}.

## r.inspect
State of {target}:
{report}

## r.nudged
Sent to {target}. inspect_session again in a moment to see whether it is unstuck.

## r.sessions_header
Sessions of this crew (id · extension · name · role · state):

## r.session_line
- {id} · {ext} · {label} · {role} · {state}

## r.role_captain
captain

## r.role_worker
worker

## r.not_direct_child
"{crew}" is not a direct child crew of yours.

## r.successor_not_in_crew
{id} does not belong to "{crew}" and cannot become its captain.

## r.handoff_done
Captain of "{crew}" handed to {target}.

## r.handoff_default_brief
You have just taken over as this crew's captain: read the lobby's recent messages, check what is in progress and how each worker is doing, then say in the lobby that you have taken over.

## r.default_captain_title
Captain

## r.handoff_new
Started a new captain for "{crew}" ({id}) and handed over; the channel receipt is the final word.

## r.no_parent
This is a root crew; there is no parent to report to.

## r.report_from
Report from {label}, captain of child crew "{crew}":
{message}

## r.reported
Reported to {count} parent crews.

## r.order_from
{label}, captain of parent crew "{crew}":
{message}

## r.messaged
Posted to the lobby of "{crew}" mentioning its captain.

## r.adopted
Adopted "{crew}" as a child crew.

## r.released
Re-homed "{crew}".

## r.parent_brief
You are the captain of a newly created parent crew; "{child}" is your first child crew. Read its lobby first, then decide whether to adopt other peer crews.

## r.parent_created
Created parent crew "{crew}" with its own captain.

## r.parent_adopted
"{crew}" is now your parent crew.

## r.child_created
Created child crew "{crew}" with its own captain, who has the brief.
```

- [ ] **Step 5: 跑通**

Run: `cargo test -p crewd-server --lib agent::tools`
Expected: captain `6 passed`；`every_catalog_tool_has_a_handler` 通过；之前各组仍全过。

- [ ] **Step 6: 变异核对**

1. `own_session` 的守卫 `if who.crew_id == cx.me.crew_id` 去掉：Expected `sessions_are_controlled_within_own_crew` 红。
2. `handoff_target` 的直系子判断换成 `Some(id) => find_crew(cx, id).await`：Expected `handoff_only_to_own_or_direct_child_crew` 红。
3. 从 `match tool` 里删掉 `"adopt_parent"` 那一行：Expected `every_catalog_tool_has_a_handler` 红（这把尺子先证明会红）。

逐条撤销。

- [ ] **Step 7: 全量检查并提交**

```bash
./scripts/check.sh
git add crates/server/src/agent/tools/captain.rs crates/server/src/agent/tools/mod.rs \
  crates/collab/prompts/tools.zh.md crates/collab/prompts/tools.en.md
git diff --cached --stat
git commit -m "crewd-server: captain tools (crew attributes, session control via SessionControl, reporting line, crew structure)

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---
### Task 16: 端到端（照 claude 读配置的方式驱动真 helper）+ 真 claude 冒烟（opt-in）+ 文档

**Files:**
- Create: `crewd/crates/crewd/tests/agent_e2e.rs`、`crewd/crates/crewd/tests/live_agent.rs`
- Modify: `crewd/crates/crewd/Cargo.toml`（dev-dependencies：`crewd-server` 开 `testkit`、`crewd-collab`）、`crewd/README.md`（agent 面一节）、`docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`（交付记录一节，相对仓库根）

**Interfaces:**
- Consumes: 前面全部；M2b `crewd_exec::launch::{prepare_claude, ClaudeSpec, AgentSessionArg, CommsSpec}`、`driver::start_claude`。
- Produces: 无。

端到端不起 claude：测试自己扮演 claude——读 `prepare_claude` 写出的 `mcp.json`，照 `command` + `args` 起 `crewd mcp`、说 MCP；读 `settings.json`，照官方语义（matcher 判是否触发、`sh -c` 跑 `command`、stdin 喂 JSON、stdout 以 `{` 开头才当 JSON）跑三个 hook。这样验的是**我们写给 claude 的那两份配置**本身，不是另拼的命令行。

- [ ] **Step 1: dev-dependencies**

```bash
cargo add -p crewd --dev crewd-server --path crates/server --features testkit
cargo add -p crewd --dev crewd-collab --path crates/collab
```

- [ ] **Step 2: 写测试 `agent_e2e.rs`**

```rust
//! spec §22.3 验收的前五条，在真 serve 夹具 + 真 helper 二进制上跑；claude 由测试扮演（不起真 claude）。
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use crewd_exec::launch::{AgentSessionArg, ClaudeSpec, CommsSpec, prepare_claude};
use crewd_proto::MentionKind;
use crewd_server::agent::port;
use crewd_server::agent::runtime::AgentConfig;
use crewd_server::agent::testkit::Fixture;
use crewd_server::agent::Role;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

struct Launch {
    _files: crewd_exec::launch::LaunchFiles,
    mcp: Value,
    settings: Value,
}

fn launch(f: &Fixture, session: &str, runtime: &Path) -> Launch {
    let comms = CommsSpec {
        helper: env!("CARGO_BIN_EXE_crewd").into(),
        data_dir: f.data_dir().to_path_buf(),
        session_id: session.into(),
        token: f.token(session),
        hooks: true,
        approval_gates: vec!["computer-use".into()],
        approval_wait_secs: 2,
    };
    let spec = ClaudeSpec { comms: Some(comms), ..ClaudeSpec::default() };
    let (args, files) = prepare_claude(&spec, AgentSessionArg::New(uuid_like(session)), runtime).unwrap();
    let mcp = serde_json::from_slice(&std::fs::read(args.mcp_config_file.unwrap()).unwrap()).unwrap();
    let settings = serde_json::from_slice(&std::fs::read(args.settings_file.unwrap()).unwrap()).unwrap();
    Launch { _files: files, mcp, settings }
}

fn uuid_like(s: &str) -> String {
    format!("e2e-{s}")
}

/// 照 claude 的语义跑一个 hook 事件：matcher（只认我们写的两种形状：缺省 = 全部；`(a|b)` = 子串）、
/// `sh -c command`、stdin = payload。返回解析后的 JSON（stdout 为空 = None）。
async fn fire(settings: &Value, event: &str, tool: &str, payload: Value) -> Option<Value> {
    let entry = settings["hooks"][event].get(0)?;
    if let Some(m) = entry["matcher"].as_str() {
        let inner = m.trim_start_matches('(').trim_end_matches(')');
        if !inner.split('|').any(|g| tool.contains(&g.replace('\\', ""))) {
            return None;
        }
    }
    let cmd = entry["hooks"][0]["command"].as_str().unwrap();
    let mut child = Command::new("sh")
        .args(["-c", cmd])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).await.unwrap();
    let out = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output()).await.unwrap().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    let t = s.trim();
    (t.starts_with('{') && t.ends_with('}')).then(|| serde_json::from_str(t).unwrap())
}

struct Mcp {
    _child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    lines: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    next: u64,
}

impl Mcp {
    async fn start(cfg: &Value) -> Self {
        let crew = &cfg["mcpServers"]["crew"];
        assert_eq!(crew["type"], "stdio");
        let args: Vec<String> = crew["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_string()).collect();
        let mut child = Command::new(crew["command"].as_str().unwrap())
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut m = Self { _child: child, stdin, lines, next: 1 };
        m.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "e2e", "version": "0"}})).await;
        m.notify("notifications/initialized").await;
        m
    }
    async fn write(&mut self, v: Value) {
        let mut l = serde_json::to_vec(&v).unwrap();
        l.push(b'\n');
        self.stdin.write_all(&l).await.unwrap();
    }
    async fn notify(&mut self, method: &str) {
        self.write(json!({"jsonrpc": "2.0", "method": method})).await;
    }
    async fn recv(&mut self) -> Value {
        let l = tokio::time::timeout(Duration::from_secs(10), self.lines.next_line()).await.unwrap().unwrap().unwrap();
        serde_json::from_str(&l).unwrap()
    }
    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.write(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).await;
        loop {
            let v = self.recv().await;
            if v["id"] == id {
                return v;
            }
        }
    }
    async fn tool_names(&mut self) -> Vec<String> {
        let r = self.request("tools/list", json!({})).await;
        r["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
    }
    async fn call(&mut self, name: &str, args: Value) -> (String, bool) {
        let r = self.request("tools/call", json!({"name": name, "arguments": args})).await;
        (
            r["result"]["content"][0]["text"].as_str().unwrap_or("").to_string(),
            r["result"]["isError"].as_bool().unwrap_or(false),
        )
    }
}

#[tokio::test]
async fn captain_and_worker_tables_differ_and_role_change_applies_live() {
    let f = Fixture::new().await;
    f.spawn_socket().await;
    let rt = tempfile::tempdir().unwrap();
    let w = launch(&f, &f.worker, rt.path());
    let c = launch(&f, &f.captain, rt.path());
    let mut wm = Mcp::start(&w.mcp).await;
    let mut cm = Mcp::start(&c.mcp).await;
    let wt = wm.tool_names().await;
    let ct = cm.tool_names().await;
    assert!(!wt.contains(&"start_session".to_string()) && ct.contains(&"start_session".to_string()));
    let (_, err) = wm.call("list_sessions", json!({})).await;
    assert!(err, "worker 调机长工具被拒");
    f.set_role(&f.worker, Role::Captain).await;
    loop {
        if wm.recv().await["method"] == "notifications/tools/list_changed" {
            break;
        }
    }
    assert!(wm.tool_names().await.contains(&"start_session".to_string()), "不用重启就生效");
    let (_, err) = wm.call("list_sessions", json!({})).await;
    assert!(!err);
}

#[tokio::test]
async fn post_via_mcp_then_injection_via_post_tool_use_hook() {
    let f = Fixture::new().await;
    f.spawn_socket().await;
    let rt = tempfile::tempdir().unwrap();
    let w = launch(&f, &f.worker, rt.path());
    let c = launch(&f, &f.captain, rt.path());
    let mut cm = Mcp::start(&c.mcp).await;
    let (text, err) = cm.call("post", json!({"message": "示例：先做 A", "mentions": [{"kind": "broadcast"}, {"kind": "session", "target_id": f.worker}]})).await;
    assert!(!err, "{text}");
    let out = fire(&w.settings, "PostToolUse", "Bash", json!({"hook_event_name": "PostToolUse", "tool_name": "Bash"})).await.expect("有注入");
    assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    assert!(out["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("示例：先做 A"));
    assert!(fire(&w.settings, "PostToolUse", "Bash", json!({"hook_event_name": "PostToolUse", "tool_name": "Bash"})).await.is_none(), "至多一次");
    let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
    f.add_session(&f.crew, "s-w2", Role::Worker, "另一个 worker").await;
    f.session_post(&f.captain, &lobby, "只给 w2", vec![crewd_proto::Mention { kind: MentionKind::Session, target_id: Some("s-w2".into()) }]).await;
    assert!(fire(&w.settings, "PostToolUse", "Bash", json!({"tool_name": "Bash"})).await.is_none(), "@ 了别人的不进");
}

#[tokio::test]
async fn pre_tool_use_gate_end_to_end() {
    let f = Fixture::with_config(AgentConfig { approval_wait: Duration::from_secs(2), ..AgentConfig::default() }).await;
    f.spawn_socket().await;
    let rt = tempfile::tempdir().unwrap();
    let w = launch(&f, &f.worker, rt.path());
    assert!(fire(&w.settings, "PreToolUse", "Bash", json!({"tool_name": "Bash"})).await.is_none(), "matcher 不命中：hook 根本不跑");
    let settings = w.settings.clone();
    let pending = tokio::spawn(async move {
        fire(&settings, "PreToolUse", "mcp__computer-use__click", json!({"hook_event_name": "PreToolUse", "tool_name": "mcp__computer-use__click", "tool_input": {}})).await
    });
    let id = loop {
        let list = f.rpc("approval.list", json!({})).await;
        if let Some(a) = list.as_array().unwrap().iter().find(|a| a["decision"].is_null()) {
            break a["id"].as_str().unwrap().to_string();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    f.rpc("approval.decide", json!({"id": id, "allow": true})).await;
    let out = pending.await.unwrap().unwrap();
    assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "allow");
    let out = fire(&w.settings, "PreToolUse", "mcp__computer-use__type", json!({"tool_name": "mcp__computer-use__type"})).await.unwrap();
    assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "deny", "没人审批：到点拒");
}

#[tokio::test]
async fn stop_hook_relays_once_per_prompt() {
    let f = Fixture::new().await;
    f.spawn_socket().await;
    let rt = tempfile::tempdir().unwrap();
    let w = launch(&f, &f.worker, rt.path());
    let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap().id;
    let stop = |p: &str, t: &str| json!({"hook_event_name": "Stop", "prompt_id": p, "last_assistant_message": t});
    assert!(fire(&w.settings, "Stop", "", stop("p0", "收到。")).await.is_none());
    let before = port::latest_seq(&f.state, &lobby).await.unwrap();
    fire(&w.settings, "Stop", "", stop("p1", "A 做完了。要不要接着做 B？")).await;
    let after = port::latest_seq(&f.state, &lobby).await.unwrap();
    assert_ne!(before, after, "代发了一条");
    fire(&w.settings, "Stop", "", stop("p1", "A 做完了。要不要接着做 B？")).await;
    assert_eq!(port::latest_seq(&f.state, &lobby).await.unwrap(), after, "同一个 prompt_id 只一次");
}

#[tokio::test]
async fn wrong_token_or_no_attach_is_refused() {
    let f = Fixture::new().await;
    f.spawn_socket().await;
    let rt = tempfile::tempdir().unwrap();
    let mut w = launch(&f, &f.worker, rt.path());
    let tf = w.mcp["mcpServers"]["crew"]["args"][6].as_str().unwrap().to_string();
    std::fs::write(&tf, "forged").unwrap();
    let crew = w.mcp["mcpServers"]["crew"].take();
    let out = Command::new(crew["command"].as_str().unwrap())
        .args(crew["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()))
        .stdin(Stdio::null())
        .output()
        .await
        .unwrap();
    assert!(!out.status.success(), "token 错：crewd mcp 起不来");
}
```

- [ ] **Step 3: 跑一遍**

Run: `cargo test -p crewd --test agent_e2e`
Expected: `5 passed`。这一步之前的任务都绿了，这里红 = 配置文件与 helper / serve 之间有接不上的地方（例如 matcher 形状、token 文件路径、`--data-dir` 的位置），按红的那条查，别为了过改测试的扮演方式。

- [ ] **Step 4: 写真 claude 冒烟 `live_agent.rs`**（两道闸；会发两条 prompt、花少量额度——人点头才跑）

```rust
//! 真 claude 冒烟（spec §22.3 验收最后一条）：rmcp 握手、tools/list、一次 post、一次 PostToolUse 注入。
//! 两道闸：`#[ignore]` + `CREWD_LIVE_CLAUDE=1`。**会发 prompt（花少量额度），人点头才跑。**
//! 工作目录：`$HOME` 下现建的临时目录（§18：`$HOME` 通常已被信任，不弹信任框）。
//! 跑法：`CREWD_LIVE_CLAUDE=1 cargo test -p crewd --test live_agent -- --ignored --nocapture`
use std::sync::Arc;
use std::time::{Duration, Instant};

use crewd_collab::Locale;
use crewd_collab::worldview::{context_for, render};
use crewd_exec::driver::{ClaudeStart, DriverConfig, start_claude};
use crewd_exec::env::AgentPaths;
use crewd_exec::launch::{ClaudeSpec, CommsSpec};
use crewd_exec::resume::MemoryLedger;
use crewd_exec::session::SessionEvent;
use crewd_server::agent::port;
use crewd_server::agent::testkit::Fixture;

#[tokio::test]
#[ignore = "真 claude：CREWD_LIVE_CLAUDE=1 cargo test -p crewd --test live_agent -- --ignored"]
async fn real_claude_uses_crewd_mcp_and_receives_injection() {
    assert!(
        std::env::var("CREWD_LIVE_CLAUDE").as_deref() == Ok("1"),
        "这条测试会起真 claude 并发 prompt：带了 --ignored 还得设 CREWD_LIVE_CLAUDE=1。"
    );
    let home = std::env::var_os("HOME").expect("HOME");
    let workdir = tempfile::Builder::new().prefix("crewd-live-agent-").tempdir_in(home).unwrap();
    let f = Fixture::new().await;
    f.spawn_socket().await;
    let lobby = port::lobby_of(&f.state, &f.crew).await.unwrap();
    let worker = f.worker.clone();
    let world = f
        .state
        .db
        .call(move |s| context_for(s.conn(), &worker, Locale::Zh))
        .await
        .unwrap()
        .unwrap();
    let world = render(&world);
    let paths = AgentPaths::discover().await;
    let runtime = tempfile::tempdir().unwrap();
    let start = ClaudeStart {
        session_id: f.worker.clone(),
        program: paths.resolve("claude").expect("PATH 里没有 claude"),
        child_path: paths.child_path.clone(),
        spec: ClaudeSpec {
            workdir: workdir.path().to_path_buf(),
            system_prompt: Some(world),
            initial_prompt: Some("用 crew 的 post 工具往大厅发一句「冒烟：通了」，然后结束这一轮。".into()),
            comms: Some(CommsSpec {
                helper: env!("CARGO_BIN_EXE_crewd").into(),
                data_dir: f.data_dir().to_path_buf(),
                session_id: f.worker.clone(),
                token: f.token(&f.worker),
                hooks: true,
                approval_gates: vec![],
                approval_wait_secs: 0,
            }),
            ..ClaudeSpec::default()
        },
        ledger: Arc::new(MemoryLedger::default()),
    };
    let (h, mut rx) = start_claude(start, DriverConfig::new(runtime.path().to_path_buf())).await.unwrap();
    let wait_for = |needle: &'static str| {
        let st = f.state.clone();
        let lobby = lobby.id.clone();
        async move {
            let deadline = Instant::now() + Duration::from_secs(240);
            while Instant::now() < deadline {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let msgs = port::messages(&st, &lobby, None, 20).await.unwrap();
                if msgs.iter().any(|m| m.sender_session_id.as_deref() == Some("s-w1") && m.text.contains(needle)) {
                    return true;
                }
            }
            false
        }
    };
    // 循环里不 panic（§18：panic 会拆掉运行时、留下孤儿）；先收集结果，停掉 claude 再断言。
    let posted = wait_for("冒烟：通了").await;
    let mut injected = false;
    if posted {
        f.human_post(&lobby.id, "冒烟暗号：青蛙九号", vec![]).await;
        let _ = h.send("随便调用一次 crew 的 list_channels 工具；工具调用之后你会看到群里新来的消息，把里面的暗号用 post 发到大厅。");
        injected = wait_for("青蛙九号").await;
    }
    let _ = h.stop();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(1), rx.recv()).await {
            Ok(Some(SessionEvent::Exited(_))) | Ok(None) => break,
            _ => {}
        }
    }
    assert!(posted, "claude 没经 crewd mcp 发出消息（握手 / tools/list / post 有一环不通）");
    assert!(injected, "PostToolUse 注入没到 claude（或它没照做）");
}
```

（这条测试 `check.sh` 永远不跑。人点头后跑一次，把结果——通过 / 失败、耗时、claude 版本——写进 Step 7 的交付记录。）

- [ ] **Step 5: README**

`crewd/README.md` 加一节「agent 面（M3b）」：`crewd mcp` / `crewd hook` 的命令行形状（只带 `--data-dir --session --token-file`，PreToolUse 多 `--deadline-secs`）；token 文件是什么、谁写；hook 超时与失败语义（post / stop 5s 内放弃、什么都不打；命中审批闸的 pre-tool-use 失败即 deny）；工具文字与世界观在 `crates/collab/prompts/`，改工具行为同一拍改 md；真 claude 冒烟怎么跑、要人点头。

- [ ] **Step 6: 全量检查**

```bash
./scripts/check.sh
```

Expected：全绿。连跑 3 次（`for i in 1 2 3; do ./scripts/check.sh || break; done`），审批与推送相关测试有时序，任何一次红都留全日志再查（不是「重跑一次绿了就算」）。

- [ ] **Step 7: spec 交付记录**

在 spec 末尾加一节「§23 M3b 交付记录与遗留（计划 `docs/superpowers/plans/2026-10-06-crewd-m3b-agent-surface.md`）」，写：已交付（对照本计划文件结构，一行一项）；本计划「与 spec 的出入」1–9 原样搬过去；Task 1 Step 1 查到的 M3a 实际名字与推定不同的地方；G3 / G-F / G-S / G-T / G9 / G11 各自最后由谁、怎么补的；真 claude 冒烟跑没跑、结果；下面的「遗留」。

- [ ] **Step 8: 提交**

```bash
git add crates/crewd/tests/agent_e2e.rs crates/crewd/tests/live_agent.rs crates/crewd/Cargo.toml Cargo.lock README.md
git add ../docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md
git diff --cached --stat
git commit -m "crewd: M3b end-to-end (generated mcp.json/settings.json drive the real helper), opt-in live smoke, docs

Co-Authored-By: Claude <noreply@anthropic.com>"
```

- [ ] **Step 9（人点头后才做）: 真 claude 冒烟**

```bash
CREWD_LIVE_CLAUDE=1 cargo test -p crewd --test live_agent -- --ignored --nocapture --exact real_claude_uses_crewd_mcp_and_receives_injection
```

只跑这一条（`--exact`），不要 `cargo test -- --ignored` 全量。结果补进 spec 交付记录，单独提交（只 `git add` spec 文件）。

---

## 遗留（交付时写进 spec）

- **L1 机长注入面的附加物没移植**（`HookEmitter.swift:246-311`）：命名提醒、拆组信号、计划陈旧度、全机组织树、派活策略更新。计划板已删；其余要不要回来，等 M4b / M5（行政处的简报）定。
- **L2 世界观缺 runner 偏好与能力块**（`LocalSessionLaunch.swift:264-279`）：crewd 还没有「新机长优先用哪个 runner」的设置；入门机组的 SKILL.md 提示也没带。
- **L3 `change_workdir` 不搬 claude 的项目记忆与权限**（Swift 会搬）：回执里写明了。
- **L4 hook 的 `args` exec 形式**没用（裁定 5）：等核到它从哪个 claude 版本起支持，再考虑换掉 shell 引号。
- **L5 codex 的 token 文件**：M2c 的 codex 驱动要在启动目录里写 token 文件、MCP 命令行带 `--token-file`（Task 2 Step 6 只在 M2c 已合时处理）；codex 没有 hook，注入走 M3c 的 `additionalContext`（`inject::prepare / commit`），回合收尾直接调 `crewd_collab::turn_trace::on_turn_end`。
- **L6 审批的阻塞语义与 Swift #75 ② 相反**（「与 spec 的出入」6）：要人确认 spec 的「阻塞 30 分钟」仍是想要的。
- **L7 回合记账在 `meta` 表**：键值表里的软状态，不进 schema；session 删除时没有清理（键会留着，无害）。
- **L8 `post` 不收附件**：等 M3a 之后支持附件上传再加回参数与世界观那一条。
- **L9 侧栏排序**：人 2026-10-06 定为手动拖拽（`crews.position`），M3b 不提供任何排序工具（`arrange_crews` 等已删）。

## 交付时要回报（给控制者 / 人）

1. G3 / G-F / G-S / G-T / G9 / G11 哪些在 M3a 落地时已有、哪些是为 M3b 补的。
2. L6：spec §22.3 的阻塞审批与 Swift 当前行为相反。
3. 真 claude 冒烟（Task 16 Step 9）有没有跑、结果。
4. rmcp 3.5.1 带进来的 lock 条目清单（Task 8 Step 1）。
