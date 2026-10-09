# crewd

PendingCrew 的跨平台后端（Rust）。设计：`../docs/superpowers/specs/2026-10-05-crewd-backend-and-work-tracking-design.md`。

- 检查：`scripts/check.sh`（本机缺省 `cargo test`；`CREWD_TEST_RUNNER=nextest` 走 nextest `ci` 档，CI 就是这样调的，见下「CI」）
- 终端核心：`crates/term`（alacritty_terminal 无头终端：读屏、claude 对话框/输入框识别、待决策跟踪、快照；纯计算，不起进程、不用 tokio）
- 执行器：`crates/exec`（起 claude 会话：PTY、开场投递、健康、停止与孤儿回收、`--resume`；对外只有 `driver::SessionHandle` + 事件流）
- 前台起一个 claude 会话：`cargo run -p crewd -- claude --workdir DIR [--session SID] [--prompt-file PATH|-] [--screen]`（开场任务从文件或 stdin（`-`）读，绝不放命令行参数——`ps` 看得见；给了 `--session` 会话号就记在库里那一行上，重跑自动 `--resume`；会拿数据目录锁，不能与 serve 同时跑）
- codex 执行器：`crates/exec/src/codex`（`codex app-server` 客户端：握手、thread 落账与续跑、回合、服务端请求全答、协议健康、进程组；对外仍是 `driver::SessionHandle` + 事件流）
- 前台起一个 codex 会话：`cargo run -p crewd -- codex --workdir DIR [--session SID] [--prompt-file PATH|-] [--transcript]`（开场任务从文件或 stdin（`-`）读，绝不放命令行参数；给了 `--session` 则 thread id 记在库里那一行上，重跑自动 `thread/resume`；那一行的执行器必须是 `codex`；会拿数据目录锁，不能与 serve 同时跑）
- 工作跟进核心：`crates/work`（两层 Todo、工作项 DAG 与不变量、负责人、检查点、停下即检查、等人升级、巡检判定；纯计算，不起进程、不用 tokio、不读时钟；持久化与 MCP 接线在 M4b）
- crates/collab（crewd-collab）：协作层——群、成员、群主、消息与 @ 解析、游标与红标、机组 DAG、人类待确认（H 号经 crewd-work）、重启询问、唤醒队列与熔断台账、渲染（M3a）。
- Jev 客户端：`crates/jev`（派活 / 叫醒 / 归属 / 紧急度四个判断；发前脱敏；本机诊断日志不记 key）。给 Jev 看的文字全在 `crates/jev/prompts/*.md`
- Jev 评测（真 API、花钱、不进 check.sh）：`cargo run -p crewd-jev --bin jev-eval -- --seeds 2 --out crates/jev/eval/results/<日期>.json`；key 取 `JEV_API_KEY`，否则钥匙串 service `pendingcrew-jev` account `default`。改了 `prompts/` 的措辞或 `eval/cases.zh.json` 后重跑，读数记进 spec 的「M5a 交付记录与中文实测」一节
- 导入旧数据（只读源目录，只写新库）：`cargo run -p crewd -- import [--from DIR]`
- 运行：`cargo run -p crewd -- serve`
- 调用：`cargo run -p crewd -- rpc crew.list`
- API 契约：`schema/crewd-api.schema.json`（改了线协议类型后用 `cargo run -p crewd -- schema > schema/crewd-api.schema.json` 重新生成；测试会检查它是否过期）。契约 v3（N1，spec §26.3）：没有审批（`approval.*` 是 -32601）；`Crew.management` 标出总机组（独立的管理机组：不是任何机组的父或子，`crew.list` 排第一，号码 0），其余机组是森林；`crew.remove` = 移除（隐藏并冻结、什么都不删，返回带 `removedAt` 的 `Crew`；库里叫归档，线上一律叫 removed，archived 只用于群），`crew.restore` = 恢复，`crew.purge` = 永久删除（只收已移除的机组，真的删掉它的一切，发 `crew.purged` / `channel.purged`；先拍快照再删，快照 30 天后轮转删掉）；`crew.list` / `session.list` 默认不含已移除的机组（`includeRemoved: true` 才列）；`crew.reorder` 的 `parentId` 可省（省 = 排顶层机组；总机组不在树里：拿它当 `parentId` 是 -32602 `crew.management_not_in_tree`，把它放进 `crewIds` 是 -32602 `crew.reorder_mismatch`）
- 拒绝码：规则拒绝的 `RpcError` 带 `data.refusal`（稳定码，如 `channel.archived`、`crew.management_not_in_tree`），`message` 只是中文缺省，客户端按码本地化、不解析 `message`。码的全集是 `crates/collab/src/refusal.rs` 的 `Code`，文字在 `crates/collab/prompts/refusals.{zh,en}.md`（每码一节、两种语言占位符一致、每句说下一步怎么办，测试钉住）；collab 源码里不写给人 / agent 看的句子。`work.*` 的拒绝与参数解析错误暂时没有码
- 数据目录：`--data-dir` > `CREWD_DATA_DIR` > `~/Library/Application Support/crewd`（目录 0700；`crewd.db`、`-wal`、`-shm`、备份文件 0600）

## agent 面（M3b）

```sh
crewd mcp --data-dir D --session S --token-file F
crewd hook <post-tool-use|stop|session-start> --data-dir D --session S --token-file F
```

- `D` 必须是这次会话所属的 serve 数据目录；身份只有 `S` 和 token，角色 / 机组 / 显示名由 serve 每次现查。启动方 `TokenTable::mint` → `CommsSpec.token` → `prepare_claude` 写 `<launch dir>/agent-token`（0600，目录 0700，随 `LaunchFiles` 清理）。token 本体不进 argv / mcp.json / settings.json。
- helper 是哑管道：MCP 每次 `tools/list` / `tools/call` 都转 `agent.tools` / `agent.call`。worker / admin 17 个工具，captain 28 个；总机组无工具特例。换角色 → `agent.tools_changed` → MCP `notifications/tools/list_changed`；调用权限也现查，不依赖客户端是否刷新。
- 生成的 settings 注册 **PostToolUse、PostToolUseFailure、Stop、SessionStart**，**没有 PreToolUse / 应用审批闸**。失败的工具执行走同一个 post-tool-use helper，并输出正确的 hookEventName。SessionStart 精确 matcher 为 `resume|compact|clear|fork`，不含 `startup`：hook → RPC → `worldview::render_live` 现取事实；不重灌习惯 / 工具指南。live 刷新限 10,000 字符，超长名字 / 谱系先预算截断；未读注入缺省限 9,000 字符。
- hook 显式 timeout 15s；helper 总期限 5s（stdin + token 读取 + socket/attach/RPC）。失败 / 坏 JSON / 未知事件 / 解析错误 / panic 都是 stderr 诊断、stdout 空、exit 0，不阻塞 session。Stop 只在还没叫到人的问句上以该 session 身份代发，按 prompt 去重。
- `mcp.json` 的 crew server `alwaysLoad:true`；产品生成的 settings `permissions.deny:["AskUserQuestion"]`，人类决定用非阻塞的 `ask`。不会禁用 ToolSearch。固定配置 `spinnerTipsEnabled:false`、`prefersReducedMotion:true`、`feedbackSurveyRate:0`、`autoUpdatesChannel:"stable"`，env `CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY=1` / `CLAUDE_CODE_DISABLE_TERMINAL_TITLE=1`；只作用于生成的启动配置，不改用户 / harness 设置。**stable 是更新渠道，不是版本锁；Homebrew 忽略该字段。** 子进程还透传 SSH_AUTH_SOCK 和 HTTP(S)_PROXY / NO_PROXY（含小写）。
- 文字唯一来源 `crates/collab/prompts/{tools,world-model}.{zh,en}.md`；改工具行为同一拍改两种语言。`tools.*.md` 的 `instructions` 经 attach 进入 MCP initialize。当前 catalog / schemas / descriptions / annotations 的两种语言快照在 `crates/server/src/agent/surface.{zh,en}.json`，漂移测试必须红；审阅变更后才用 `CREWD_BLESS=1 cargo test -p crewd-server agent::surface::tests::surface_golden_snapshots -- --exact` 更新。稳定 FNV-1a/64 指纹附在 `serverInfo.version` 的 `+` 后、attach 时记 stderr；它只标识变更，不是安全凭据。旧工具名回执指出替代调用。
- 确定性接线验收 `cargo test -p crewd --test agent_e2e`：真合成 serve + 真 helper，读生成的两份配置照其命令执行，不起真实 Claude。完整入口仍是 `scripts/check.sh`。

**真 Claude 两道闸（会用额度，必须获得明确授权；永不进 check.sh）**：

```sh
CREWD_LIVE_CLAUDE=1 cargo test -p crewd --test live_agent -- --ignored --nocapture --exact real_claude_uses_crewd_mcp_and_receives_injection
CREWD_LIVE_CLAUDE=1 cargo test -p crewd --test agent_eval -- --ignored --nocapture --exact real_claude_agent_eval_seed
```

smoke 两条短 prompt；eval 三个短场景（被 @ 问题、需要人拍板、完成）。单个测试各用一个合成 serve + 一个真实 CLI process；每 prompt 最多 8 个 agentic turns / 240s，CLI 估算预算每 process 2 美元，不重试。为独立观测完整 tool-use 顺序走 print-mode stream-json，并以匹配 tool_result 和终态 result 确认覆盖，不拿 hook 的零 ToolSearch 计数当证明；不读 `~/.claude` transcripts，也不录终端 fixture。只收测试进程 stdout/stderr 到本 worktree 的忽略任务目录；权限 / 登录 / API 失败立即停，清理只用该 Child。

**2026-10-09 交付快照（代码基准 `b5bfd9b`）**：macOS 三次 check.sh 均 1382 passed / 0 failed / 8 ignored。授权 smoke 跑一次，native Claude 2.1.295、初始化成功（17 个 crew 工具 upfront，ToolSearch 可用，AskUserQuestion 不在 init）；随后 `Not logged in · Please run /login`，API 耗时 / tokens / cost 均零，没有工具执行。post / 暗号返回 / 首次 crew 前无 ToolSearch / PostToolUseFailure / live role refresh **未验证**；不重试，第二条 prompt 和三个 eval seeds 未发送。完整结果及跨分支集成 hold 见 spec §27。

**fix-r1 补充（代码基准 `1ff705b`，不是新的 live 读数）**：`68a25fa` 修正 harness 的停止/期限：版本预检 5s（stdout 上限 4KiB），prompt 的 240s 包含 stdin 写入；任何错误立即取消该 Child、拒绝后续 prompt，再最多 2s 收尸，失败报诊断并交 kill_on_drop/Tokio 后台收尸。只有成功终态允许 EOF 后最多 15s 的优雅退出，超时再取消/有界收尸。合成子进程对照验证，未再启动真实 CLI（连 --version 也未运行）。

SessionStart 与 launch 的联合入口为 `server::agent::port::world_context`：读 DB 身份/角色/locale 后，经已有 `SessionControl::quota` 取 typed report（最多 1s）；按 executor 只取订阅 plan，不从 detail/limited_until 编造余额。WorldCtx 的 Known / Unknown / Unavailable / Failed 四态分别表示已知档位、来源成功但档位未知、来源未接、读取失败/超时；错误不吞掉身份刷新。已用两种语言的 FakeControl 证明生成的 launch 世界观和四种 SessionStart source 的 known→known 更新。M3c 的实际 provider feeding / 生产 launch 调用仍需接这一个入口；这里没有读取真实额度。原 auth BLOCKED、三个 seed NOT RUN 与 relay/breaker HOLD 均不变，见 spec §27.9。

## 备份与恢复

`crewd.db` 是聊天记录的唯一一份，只在这台机器上。

- 自动：`serve` 启动时、之后每天一次，在 `<数据目录>/backups/crewd-auto-*.db` 拍一份（独立的只读连接上 `VACUUM INTO`，不挡服务；拍完的那份过 `quick_check` 才算数）。保留最近 7 天每天一份 + 更早 4 周每周一份；备份失败或在用的库没通过检查时，旧备份一份不删。每小时对在用的库跑 `quick_check`，每天拍的那次跑 `integrity_check`；出问题只在日志里报，绝不删库、换库
- 迁移前：旧版本的非空库升级前先过 `integrity_check`，再存成 `backups/crewd-v{旧}-pre-v{新}-*.db`；任何一步失败就拒绝迁移，库原样不动
- 永久删除机组前：`backups/crewd-pre-delete-{机组}-*.db`（拍不成就不删）。永久删除是真的删，这份快照只是安全网：按名字里的时间 30 天后随每天那次轮转删掉（备份失败或在用的库没通过检查时同样不删）
- 迁移前、手动的快照不参加轮转，永远保留（要清理请手动删）
- 立即备份：`cargo run -p crewd -- backup`（serve 在跑也行；写到 `backups/crewd-manual-*.db`。不提供导出到数据目录以外的选项：离开本盘的备份要加密，尚未实现）
- 检查：`cargo run -p crewd -- check [--quick]`（不改数据；只报告，不修。注意：它的连接关掉时可能把 WAL 检查点进库，这是正常落盘、不改内容）
- 恢复：先停 serve，再 `cargo run -p crewd -- restore --from <data>/backups/<文件>`。serve 在跑时会被数据目录锁拒绝。来源要是 crewd 的库、版本不比本程序新、过得了 `integrity_check`；原来的库，以及残留的 `-wal`/`-shm`（库文件不在时也一样），改名为 `crewd.db.replaced-*` 留在原地，永不覆盖。自己挪库文件时一定连同它的 `-wal`/`-shm` 一起挪，否则旧 WAL 会被回放到别的库上
- **绝不要用 `cp` 把备份盖到 `crewd.db` 上**：那是同一个文件（inode 不变），旁边残留的 `crewd.db-wal` 会被回放到盖上去的旧内容上，库就坏了，而 crewd 的身份核对看不出这种覆盖。恢复一律用 `crewd restore`
- 库丢了 / 被换了：数据目录里有 `crewd.db.id`（或 `backups/` 里有备份）却没有 `crewd.db`，或者 `crewd.db` 的 dbId 和 `crewd.db.id` 对不上时，serve 和 import 拒绝启动，不会悄悄新建空库；按提示用 `restore` 恢复

## CI

`.github/workflows/crewd.yml`（与 Xcode 那条 `ci.yml` 分开）：push main 必跑，PR 在动了 `crewd/**` 或该 workflow 时跑，也可手动触发。macOS（`macos-26`）与 Linux（`ubuntu-latest`）各一份，**都跑同一个** `scripts/check.sh`（`CREWD_TEST_RUNNER=nextest`）：`cargo fmt --check`、`clippy --all-targets -D warnings`、`cargo nextest run --profile ci`、`cargo test --doc`。工具链只认 `rust-toolchain.toml`，nextest 版本钉在 workflow 的 `NEXTEST_VERSION`。

- **抖动即红**：`.config/nextest.toml` 的 `ci` 档 `retries = 2` + `flaky-result = "fail"`——重试只为把抖动标出来（报告里 `FLAKY`、JUnit 里 `<flakyFailure>`、job 摘要里列名），不是吞掉。JUnit 作为 artifact 上传。
- **从不在 CI 跑 `--ignored`**。忽略项包括真 Claude/Codex、真钥匙串、存储量级基准和上面的 agent smoke/eval；各自双闸 / 人工跑法见 `#[ignore]` 原因与对应章节。数量不是固定基线，读取当次 test/nextest 汇总；这里只说明运行政策。
- **Linux 不排除任何可移植测试**：钥匙串（`security-framework`）与 `registry.rs` 里 macOS 的进程查询都有 `cfg(not(target_os = "macos"))` 的对应实现，Linux 上整套非 ignore 测试在容器里实测全过过一次（N0 当时 952 个；此后没在 Linux 重跑。N1 合 main 前（N1b-2）macOS 上 `scripts/check.sh` 跑 1113 个：1110 个测试 + 3 个 doctest，另 6 个 ignored；`registry::tests::reaper_kills_a_live_orphan_group_but_not_a_reused_pid` 在 Linux 上被 nextest 标 LEAK——测试故意留孤儿进程组，不影响通过）。Linux 这份的价值是抓「无意间依赖 macOS」的假设；产品的权威平台仍是 macOS 那份。
