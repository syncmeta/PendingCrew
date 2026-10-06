# crewd M2 执行器移植图（Swift → Rust）

> 2026-10-05 读 `main@d466873` 的 Swift 源码整理，供写 M2 计划用。路径相对仓库根；LR = `Sources/Mac/LocalRunner/`，CSR = `Sources/Mac/Services/CrewSessionRunner.swift`。这是读代码读到的（第三层证据），移植时每条都要拿夹具/真机验证。

## 1. claude 启动
- **找二进制**（`LR/LocalCodingAgentExecutable.swift`）：`$SHELL -lic "/usr/bin/printf '<<<PCREW_PATH:%s:PCREW_PATH>>>' \"$PATH\""`（5s 超时，SHELL 缺省 zsh），取哨兵间 PATH（:155-174），进程内缓存（:120）。childProcessPath = 登录 shell 目录 + 父 PATH + `/usr/bin:/bin:/usr/sbin:/sbin` 去重保序（:66-100）。取第一个可执行 claude/codex；codex 另有 npm 托管回退（:24）。**同一 PATH 注入子进程 env**（修 codex 的 `#!/usr/bin/env node` 找不到 node）。
- **env 白名单**（`LR/LocalCodingAgentSpec.swift:22-79`）：PATH(替换)、HOME、SHELL、LANG、TERM、USER、LOGNAME、TMPDIR、`LC_*`、调用方 additionalEnv；拒绝 `PENDINGBOT_`/`PENDINGCREW_`/`SUPABASE_` 前缀；codex 去掉 `OPENAI_API_KEY`（计费保护）；缺 TERM 补 `xterm-256color`（`AgentSessionCore.swift:199-200`）。
- **argv**（`LR/SessionConfig.swift:124-150`）：`--resume <id>` 或 `--session-id <我们生成的小写 uuid>`（CSR:2061-2079，spawn 前先落账）、`--permission-mode auto`（不用 bypass）、`--append-system-prompt-file <tmp.md>`、`--settings <file>`、`--mcp-config <file>`、`--model`、`--effort`。**首条 prompt 绝不进 argv**（ps 可见，出过事故），走 PTY 投递（§2）。
- 模型：未选时用 `SessionLaunchOptions.defaultModel`；fast 模式强制 `opus` 并写一份带 `"fastMode": true` 的 settings（`LR/LocalSessionLaunch.swift:144-156`）。
- **hooks/MCP**（`LR/LocalSessionLaunch.swift:83-140`）：helper = app 自身二进制。mcp-config：`{"mcpServers":{"crew":{"command":helper,"args":["--mcp-serve","--crew",C,"--dir",WB,"--session",S,"--agent","claude",(+"--captain"),(+"--structured-post"),(+"--label",L)]}}}`（exec args，不过 shell）。settings hooks（每个词单引号的 shell 串）：PostToolUse `helper --mcp-hook …`（注入白板未读，`Sources/Mcp/HookEmitter.swift:219`）；PreToolUse `helper --mcp-permission-hook … --gate computer-use`（`McpPermissionHook.swift:129`）；Stop `helper --mcp-turn-hook …`（读 stdin `last_assistant_message` 写回合标记，`McpTurnHook.swift:48`）。文件写在 `$TMPDIR/pendingcrew-{settings,mcp}-<sessionId>.json`。首轮白板用 `<external_crew_whiteboard>` 包在 brief 前（:13-37, CSR:2051-2056）。
- **PTY**：SwiftTerm forkpty，cwd=workdir（`AgentSessionCore.swift:201-205`），初始 80×25，scrollback 10000，像素 16×16，resize 忽略 cols<2/rows<1。

## 2. 从屏幕读 claude 状态（风险最高）
两路输入都要移植：
- **A 渲染行**（`LR/TerminalScreenText.swift:42-68`）：逐格读，`width==0`（宽字符后半）跳过，未写过的 `\0` 当空格，去行尾空格，去尾部空行。必要：claude 用绝对列定位（`\e[7G`），原始字节里没有空格。
- **B 去 ANSI 的字节尾窗**（`AnsiPlainTextTail`，`LR/SessionHealth.swift:137-230`）：状态机 normal/ESC/CSI/OSC（BEL 或 `ESC \` 结束）/2 字节 ESC（`( ) * + # %`），CR/LF→`\n`，窗口 4K 字符、到 2× 时裁剪；匹配用 ASCII 小写镜像。
- **菜单解析**（`LR/SessionPendingDecision.swift:64-247`）：两端去框线字符；光标标记 `❯ › ▸ ▶`（不含裸 `>`）。编号式（:213）：从底向上，块后最多 2 行非空尾行，连续 `N. text` 编号 1..N，≥2 个选项且至少一行带光标。光标列表式（:166，当前信任对话框）：需页脚去空白后同时含 `toconfirm` 和 `tocancel`（:116），其上 ≥2 行连续非空无编号行且恰好一行带光标。prompt = 上方 12 行内最近含 `?`/`？` 的行，否则最近非空行（:127）。排除限流菜单（:149-150）。`numbered` 是事实字段无缺省。
- **输入框**（`LR/StartupPromptDelivery.swift:267-340`）：NUL→空格；有对话框则 inputRow=nil；否则底部 15 行找去框后以 `❯`/`›`/`>` 开头且不是编号选项的行，返回其余部分。bodyTailVisible = prompt 末 64 个非空白字符出现在去空白屏幕里。
- **投递状态机**（:31-257，驱动在 `AgentSessionCore.swift:306-352`）：等就绪——对话框稳定 3s 报一次 blockedByDialog；90s 无输入行报 notReady。写正文——原样 UTF-8，**不用 bracketed paste**（虽然 claude 开了 `?2004h`）；重试前先发 Ctrl-U(0x15)。「已落」= 输入行非空且不同于基线（长输入会折叠成 `[Pasted text #1 +N lines]`；空框有 `Try "…"` 提示），判定窗口 0.5s–2.5s。提交——发 0x20、等 100ms、0x7F、等 100ms、0x0D（:337-345），原因：一次性文本+回车会被当粘贴吞掉回车。提交成功 = 输入行变化。各最多 3 次，失败报 briefUndelivered / promptUndelivered。同一时刻只有一条在途，后续排队（:789-815）。
- **节拍**：每块 PTY 输出→喂终端、scanOutput、推进投递（:287-294）；0.6s 定时器跑 recomputeWorking / pollPendingDecision / stepPromptDelivery（:221-234）。
- **待决策**（`PendingDecisionTracker.poll`，:302-327）：候选来自屏幕（优先）和尾窗，按指纹粘住已报的；稳定 3s 才 `.appeared`；两路都解析不出才清除。
- **限流菜单**（`SessionHealth.swift:386-415`）：尾窗含 `/rate-limit-options` 或 `stop and wait for limit to reset` → 30s 冷却内发 `"1"`、250ms、CR，置 `.rateLimited`（`AgentSessionCore.swift:900-912`）。
- **忙/闲**：isWorking = 运行中且距最后输出 <1.0s（:700-719）；claude 的 isBusy 恒 false（`HeadlessSessionBackend.swift:28`）。**idle 边沿** = isWorking true→false（CSR:4417-4428 → runBecameIdle CSR:490），runBecameIdle 再核 `!isBusy && !activityIsWorking`（CSR:4111）。机长另有 60s checkpoint 重触发（CSR:607-619）。
- **awaitingReply**（`Sources/Support/SessionAwaitingReply.swift:27+`）：有待决策→菜单；否则 Stop hook 记了尾随问题（`Sources/Mcp/SessionTurnTrace.swift:70`，标记 `<crew>.<session>.turn` :231）且去抖后不在打字。
- 其它屏幕驱动：切模型——等 2.5s 静默（claude 工作中每秒重绘）注入 `/model X` 再扫回显（`AgentSessionCore.swift:937-972`、`LR/SessionProfileSwitch.swift:52-64`）；resume 被拒——启动 5s 内屏幕出现 `No conversation found with session ID`+id（`LR/AgentSessionResume.swift:83-111,191`，CSR:3896-3930）；参数被拒——`SessionLaunchParameterScanner`；DECRQM 回复仅在快照探测期间且精确匹配时吞掉（`AgentSessionCore.swift:448-462`），其余终端回复都回给子进程。

## 3. 快照与 attach 协议
- 帧（`Sources/Shared/SessionProtocol.swift:9-144`）：`u32be 长度 | u8 kind | payload`，最大 64MiB。kind0 控制 JSON（含 `type`）；kind1 `u32 handle + PTY 原始字节`；kind2 `u32 handle, u32 seq, u8 flags(bit0 isLast), bytes`，64KiB 分块，空快照一帧。消息：app→daemon hello{protocolVersion,appBuild,capabilities}/listSessions/attach{sessionId,cols,rows}/detach/resize/input{handle,bytes}/control{requestId,op,arguments}/ping；daemon→app hello{…,sessionCount,pid,viewerCount,startedAt}/sessions/attached{sessionId,handle,snapshotFrames}/state{sessionId,stateSeq,delta}/data/resync{handle}/event/pong（:241-582）。未知 type 忽略，能力取交集（:697），协议版本 1。
- attach（`LR/SessionProtocolEndpoints.swift:407-476`）：cols/rows 为 0 不 resize，否则**先 resize 权威终端再取快照**。仅协商到 `terminal-bytes` 且非 codex 时发快照；codex 回放 `item/completed` 历史事件。异步链路走 `SessionAttachQueue`：实时积压上限 2MiB，溢出→全丢、发 resync、再懒取新快照（:714-756）；客户端收到 resync 丢弃半截快照（:1119-1125）。顺序规则：先喂终端再入队。输入（:320-334）：codex 的 ESC→interrupt、其它文本→门控 wake；claude PTY 含 CR/LF 的块走准入，其余直接 sendRaw。控制 op：stop/clearQuotaHealth/submitWake/applyProfileSwitch/screenText/compactCodexThread（:493-600）。能力表 `LR/SessionDaemonHost.swift:281`。
- 快照编码（`LR/TerminalSnapshotEncoder.swift:49-96`）：`ESC c`；在备用屏则主屏只出文本再 `?1049h`；所有行（scrollback+屏）SGR 每次变化从 0 重建；软换行靠「补到右边界→写触发字符→擦掉补位」还原（:114-165）；然后 DECSTBM、模式（`?1`、`?2004`、鼠标 `?9/1000/1002/1003`、`?1006` 经 DECRQM）、光标 CUP、隐藏则 `?25l`。客户端必须先 resize 再喂字节。

## 4. codex（`LR/CodexAppServer/`）
- 传输：`codex app-server` stdio，换行分隔 JSON，无 `jsonrpc` 字段（`CodexRPCMessage.swift:4-12`）；我方请求 id 与服务端 id 分表，早到的响应缓冲（`CodexRPCDispatcher.swift`）；**无单请求超时**；必须排空 stderr 否则 codex 卡住（`CodexAppServerConnection.swift:118-125`）；忽略 SIGPIPE（:63-73）。
- 生命周期（`CodexAppServerBackend.swift:230-316`）：`initialize{clientInfo, capabilities:{experimentalApi:true, requestAttestation:false, optOutNotificationMethods:["item/agentMessage/delta"]}}` → `initialized` 通知 → `thread/resume{…, excludeTurns:true}`，失败回退 `thread/start` 并公告。`thread/start`（`CodexProtocol.swift:64-87`）：`{cwd, approvalPolicy:"on-request", sandbox:"workspace-write", approvalsReviewer:"auto_review", model?, serviceTier fast|default, developerInstructions(世界观), config:{model_reasoning_effort, mcp_servers:{crew:{command,args}}}}`；取结果里 thread.id/model/reasoningEffort/serviceTier 后发首条 turn。`turn/start` = `{threadId, input:[{type:"text",text,text_elements:[]}], additionalContext:{crew_whiteboard:{value,kind:"untrusted"}}}`（:164-173）。另用 `turn/interrupt{threadId,turnId}`、`thread/settings/update`（fast 以 `thread/settings/updated` 回显确认）、`thread/compact/start`。
- **服务端请求必须逐个应答否则回合挂死**（:757-799）：`*requestApproval` → error -32000；`mcpServer/elicitation/request` → `_meta.codex_approval_kind=="mcp_tool_call"` 则 accept，否则 decline 并公告；`account/*` → 置 authRequired、回 -32601；其它 → 公告并回 -32601。
- 回合跟踪（:582-671）：`turn/started` 置 activeTurnId、isWorking；任何 `item/*` 也置 isWorking；匹配 id 的 `turn/completed` 清除（除非触发自动压缩：上下文 ≥70%，或 ≥45% 且消耗大，`CodexProtocol.swift:7-16`）。isBusy = activeTurnId≠nil || isCompacting（:120）。Stop hook 的对应物 = `notifyTurnEnded(lastAgentText)`。健康来自协议字段（`CodexProtocol.swift:202-260`）：`account/rateLimits/updated`、`codexErrorInfo` usageLimitExceeded/unauthorized、失败回合、「requires a newer version」。

## 5. 进程生命周期
- stop（`AgentSessionCore.swift:882-893`）：SIGTERM，置 exited，2s 后 `killpg(pid,SIGKILL)` 退 `kill`（`LR/AgentTerminalSession.swift:165-170`）。PTY 子进程因 forkpty 的 setsid 自成会话组长。codex 用 Foundation Process，是否独立进程组**未核实**（不是则 killpg 失败只杀父进程）。
- 启动探测（`LR/SessionLaunchProbe.swift:37-50`、`AgentSessionCore.swift:605-667`）：25s 期限、1s 轮询、`waitpid(WNOHANG)`；结论 spawnFailed / diedSilently / stalled / alive。
- 孤儿回收（`LR/SessionOrphanReaper.swift`）：`daemon.registry.json` 存 {pid, pgid, sysctl KERN_PROC_PID 的启动时间 秒+微秒, comm}；只有启动时间精确匹配才回收（pid 复用绝不动）；daemon 启动时跑（`LR/SessionDaemonHost.swift:420-470`）。daemon 调 setsid、不二次 fork（`SessionDaemonMain.swift:34`）。
- 重启后 resume：id 账本 `LocalAgentSessionStore`；`AgentSessionResume.decide` 只看是否记过 id（:67）；claude 拒绝从屏幕识别后去掉 `--resume` 重试一次；恢复提示 `LR/SessionRestoreOffer.swift:55-113`。
- **A1「升级不打断会话」为什么没达成**：原设计（`docs/internal/2026-08-19-backend-split-design.md:25,209-223,441`）daemon 比 app 活得久，靠版本/能力协商（`SessionCompatibility`，`SessionProtocol.swift:707-722`）。2026-09-11 人放弃了「会话不能断」，`BackendUpdatePlan.decide` 现在只要 build 不同就替换 daemon（`LR/BackendUpdatePlan.swift:60-63`）；daemon 持有 PTY master → 所有会话死、弹恢复提示、`--resume`/`thread/resume`。结构原因：同一二进制既是 app 又是 daemon，PTY 所有权在被替换的进程里。另：每个 session 的 `--mcp-serve` helper 继续跑旧代码（`LocalSessionLaunch.swift:45-82`）。

## 6. 各状态来源（`LR/SessionHealth.swift:88-98` 推导顺序 launchFailed → exited → health(quota→rateLimited 否则 error) → awaitingDecision → awaitingReply → working/idle）
- usageLimit：claude 尾窗含额度短语（`SessionHealth.swift:245-252`，触发一次直到重新武装）；codex 协议字段。
- rateLimited：限流菜单扫描。额度类恢复：命中后连续工作 6s（:115-131）。
- authRequired：claude 读 `~/.claude/projects/*/<id>.jsonl` 的 `isApiErrorMessage` + `error=="authentication_failed"`（:309-376），每 2s 轮询（`AgentSessionCore.swift:159-174`）；codex 来自 `account/*` 请求或 unauthorized。
- launchFailed：启动探测。brief/promptUndelivered：投递状态机。
- 退出原因 hitLimit：退出前 600s 内设过额度类 health（`LR/SessionBackend.swift:33-45`），驱动额度重置自动唤醒（CSR:1548-1569）。

## 7. 可作金样本的夹具与测试
- `Tests/Fixtures/`（`AgentTuiFixtureRecorder.swift` 在 80×25 录的原始 PTY 字节）：`tui-claude.bin`（2026-08-26，编号式信任框→横幅→输入框）、`tui-claude-trust.bin`（2026-09-06，无编号、`❯ No, exit` 在首位）、`tui-claude-ready.bin`（已信任目录，直接就绪）、`tui-shell.bin`。
- `ClaudeInputBoxFixtureTests.swift:52-340`（64 字节分块喂）：对话框先于就绪、有对话框时 inputRow=nil、空框 `Try "…"` 提示进输入行、旧框选项 trust→exit 且 numbered=true、新框 exit 在前且 numbered=false 且 prompt 提到 trust/safety、ready 夹具从不出对话框、3s 后 `.appeared`、通知文本不编造编号。
- `TerminalSnapshotFixtureTests.swift:124-215`：流中多点快照往返、claude 从不用备用屏、resize 到 60/160 列后往返、选区文本一致。
- 值得移植的纯单测：SessionPendingDecision、StartupPromptDelivery(Plan)、SessionHealth、TerminalScreenText、TerminalSnapshotEncoder、SessionAttachQueue、SessionLaunchProbe、SessionOrphanReaper、SessionConfig、CodexProtocol、CodexRPC{Message,Dispatcher}、CodexTranscript、SessionTurnTrace、SessionAwaitingReply、AgentSessionResume、SessionProtocolCodec。

## 8. Linux / Rust 会咬人的地方
- 终端仿真语义：所有启发式都假定 SwiftTerm 行为（未写格是 NUL、`width==0` 标宽字符后半、延迟换行、`isWrapped`、DECRQM 回复经 send 回来）。先选定 Rust 仿真器并用四个夹具全量复验；快照编码器与 SwiftTerm 模型绑定，要重写。
- 孤儿回收：sysctl KERN_PROC → `/proc/<pid>/stat`（第 22 字段启动时间(时钟滴答)、第 5 字段 pgid）。
- PTY：forkpty 之外要 setsid + `TIOCSCTTY` 设控制终端；PTY 子进程里把 SIGPIPE 复位成 SIG_DFL（Rust 默认忽略 SIGPIPE，手写 fork 不会复位；std Command 会）。
- socket 路径：sun_path macOS 104 / Linux 108 字节；回退 `/tmp/pendingcrew-<uid>.sock`（`SessionDaemonHost.swift:36-41`）。
- helper 路径：hook/MCP 命令里嵌可执行路径，crewd 需要稳定的 helper 路径；「helper 跑旧代码」问题依旧。
- macOS 专有：`$TMPDIR`（Linux 是 `/tmp`，settings 文件要设权限）、`~/Library/Logs`、Application Support、UserDefaults 键 `pendingcrew.defaultModel.<kind>` / `pendingcrew.sessionFastMode.<crew>.<session>`（`LR/SessionConfig.swift:6-31`）需要新的配置存储。
- 登录 shell PATH 捕获要保留（systemd user unit 也只有最小 PATH）。
- 其它保留：`CLAUDE_CONFIG_DIR` / `~/.claude/projects` 做认证探测；`HelperProcessForensics` 的 `proc_pidpath`/`proc_pidinfo` 是 macOS 专有。
- Foundation 特有坑（readability handler EOF 忙转、write 的 ObjC 异常）Rust 没有，但 EOF 和 EPIPE 仍要显式处理。

## 9. 终端仿真库选型（2026-10-06 spike 结论）

**选 `alacritty_terminal` 0.26（Apache-2.0，2026-04 发布）。** 方法：四个夹具按 64 字节分块喂给 alacritty_terminal / vt100 / avt / wezterm-term（tattoy fork），并把 SwiftTerm 本体编成 headless 对照逐步比屏幕。

- 夹具期望（`ClaudeInputBoxFixtureTests` 那 13 条）：四个候选全过。
- 与 SwiftTerm 逐步比行：alacritty/vt100/avt 在 claude/trust/ready/shell 上 117/120、18/18、42/42、160/160；那 3 处差异是 **SwiftTerm 的 bug**（64 字节边界切在 OSC 标题的 UTF-8 中间，标题残片 `个数字` 漏进网格）。所以验收标准用夹具期望，不用「与 SwiftTerm 逐字节一致」。wezterm-term 只有 30/120（越界光标不夹、末列宽字符不换行），淘汰。
- 选 alacritty 的决定因素：**它自己回答终端查询**（DA1/DA2/DSR/CPR/DECRQM/OSC 颜色/XTWINOPS，经 `Event::PtyWrite` / `ColorRequest` / `TextAreaSizeRequest` 交给我们写回 PTY）——claude 每次启动都发 DA1、XTVERSION、OSC 11；且暴露写快照所需的一切（cell 颜色/flags、WRAPLINE、宽字符 spacer、scrollback、cursor 含 pending-wrap、TermMode 位集）；resize 会 reflow。vt100 次选（自带 `state_formatted()` 快照可往返，但不答查询、缩窄截断不 reflow、2025-07 后无发布）。

已知风险与对策（写进 M2 计划）：
1. **快照编码器手写**；读不到的：滚动区域（私有字段 → 我们自己用小解析器跟踪 `CSI t;b r` / `CSI r` / RIS）、备用屏激活时的主屏（`swap_alt()` 有破坏性，别用；claude 从不用备用屏）、`?9`/1005/1015 鼠标模式。
2. **同步输出 `?2026`**：alacritty 默认缓冲 `?2026h` 后最多 150ms 的字节，快照会漏 → 用 no-op `Timeout`（`Processor<NoSync>`），已验证立即生效。
3. **与 SwiftTerm 的行为差**：DA1/DA2 回复串不同；kitty `?u` 需开 `kitty_keyboard`；SwiftTerm 在 `?1004h` 时立即发 `ESC[I`，alacritty 不发 → crewd 在 FOCUS_IN_OUT 打开时自己补发；OSC 11 背景色决定 claude 明暗主题，在回调里由我们选（SwiftTerm 报的是黑 `rgb:0000/0000/0000`）；pending-wrap 时 alacritty 报 列=cols-1 + `input_needs_wrap`，SwiftTerm 报 cols，编码器要按 flag 复现。
4. 未写过的格子在 alacritty 里是空格（不是 NUL），读屏规则相应调整。
5. alacritty 还带 Unix PTY 启动器（`alacritty_terminal::tty`，rustix-openpty + setsid/TIOCSCTTY），可能覆盖 §8 的 PTY 坑，M2 评估。

API 速记：`Term::new(Config{scrolling_history:10_000,..}, &TermSize::new(80,25), listener)`；`Processor::<NoSync>::new().advance(&mut term, &chunk)`（任意切块安全）；读行 `g=term.grid(); g[Line(r)][Column(c)]`，跳过 `Flags::WIDE_CHAR_SPACER`，`cell.c` + `cell.zerowidth()`；scrollback `g.history_size()`、`g[Line(-i)]`；软换行 `row[Column(cols-1)].flags.contains(WRAPLINE)`；光标 `g.cursor.point` / `input_needs_wrap`，可见性 `TermMode::SHOW_CURSOR`；模式 `TermMode::{ALT_SCREEN,BRACKETED_PASTE,APP_CURSOR,APP_KEYPAD,MOUSE_*,SGR_MOUSE,FOCUS_IN_OUT,LINE_WRAP,ORIGIN,INSERT}`；`term.resize(TermSize::new(c,r))`。

## 10. 业界做法对照与执行器定型（2026-10-06 调研）

- **「服务端无头仿真 + attach 时序列化快照 + 实时字节」是标准做法**：VS Code ptyHost（`@xterm/headless` + serialize addon，重连发序列化 ANSI 含 scrollback）、Superset（同构，attach 回 snapshotAnsi + modes + dims；PTY 读在 1MB 积压时暂停、250KB 恢复）、ccmanager（node-pty + headless xterm + 正则判 claude 状态，就是我们的 Node 版）、shpool（Rust，vt100 内存渲染重画）、tmux/screen（服务端持网格、对客户端重画）、mosh（服务端仿真 + 差分同步）、zellij web。
- **编排 claude 的同类**：claude-squad（Go/AGPL，tmux + capture-pane + 字符串匹配）、agent-deck（Go/MIT，tmux 控制模式 + capture-pane + Stop hook）、ccmanager（TS/MIT，同我们）、vibe-kanban（Rust/Apache，stream-json 无终端）、Sculptor、Crystal/Nimbalyst、Happy（SDK/CLI + 加密中继）。用 tmux 的那些仍然靠轮询 + 字符串匹配，tmux 没替它们解决最难的读屏。
- **不改用 tmux 做执行器**：macOS 默认没装（要打包 C 二进制 + libevent + utf8proc）；tmux 自身协议升级时新客户端连不上老服务端，只能杀服务端（「升级不断」在 tmux 自己升级时失效）；无客户端附着时不回 OSC 11（claude 主题探测会卡）；变成 tmux/读屏/SwiftTerm 三个仿真器要对齐宽度表；读屏仍要自己做。dtach/abduco 无客户端时直接丢弃 PTY 输出，不可用。
- **以后做「升级不中断」的方案**（M2 之后）：PTY 交接而非 tmux——A：旧 crewd `exec` 新二进制，PTY master fd 跨 exec 继承；B：一个极小且稳定的 pty-holder 进程经 SCM_RIGHTS 传 fd（类 VS Code 独立 ptyHost）。两者都用我们的快照编码器把仿真器状态带过去；交接空档的输出留在内核 PTY 缓冲（推论，未实测）。`--resume` 仍是崩溃兜底。
- **远程查看注意**：resize 归属要定一个策略（参考 tmux `window-size largest|smallest|latest|manual`；建议「最近交互的观看者」或固定尺寸+其他观看者留边）；alacritty / SwiftTerm / xterm.js(unicode11) 宽度表要对齐并测 CJK/emoji；重放模式字节后 TUI 被杀会留下已开启的焦点/鼠标模式（Superset 有 input-mode reclaim 文档）；背压/resync 已设计。
- **Claude 官方 Remote Control** 只同步 transcript（存 Anthropic 服务器），不同步终端，需订阅、每进程一个远程 session；可作补充，不能替代。
